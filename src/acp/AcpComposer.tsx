import {
  memo,
  useEffect,
  useRef,
  useState,
  type ClipboardEvent as ReactClipboardEvent,
  type DragEvent as ReactDragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { copyToClipboard } from "./AcpRichText";
import type { AcpCommandSpec, AcpImageAttachment, AcpPhase } from "./types";

const ALLOWED_IMAGE_MIMES = new Set([
  "image/png",
  "image/jpeg",
  "image/webp",
  "image/gif",
]);
const MAX_PENDING_IMAGES = 4;
const MAX_IMAGE_BYTES = 6 * 1024 * 1024;
const AT_SEARCH_DEBOUNCE_MS = 100;

type PendingImage = AcpImageAttachment & {
  id: string;
  previewUrl: string;
};

type AtMentionMatch = {
  query: string;
  start: number;
  end: number;
};

type SlashItem = {
  name: string;
  description: string;
  inputHint?: string | null;
  local?: "model" | "stop" | "copy" | "clear";
};

const BUILTIN_SLASH_COMMANDS: SlashItem[] = [
  { name: "model", description: "切换模型或思考强度", local: "model" },
  { name: "stop", description: "停止当前正在执行的轮次", local: "stop" },
  { name: "copy", description: "复制最近一条助手回复", local: "copy" },
  { name: "clear", description: "清空输入框、附件与提示", local: "clear" },
];

function detectAtMention(text: string, caret: number): AtMentionMatch | null {
  const before = text.slice(0, caret);
  const match = before.match(/(?:^|\s)@([^\s@]{0,80})$/);
  if (!match) return null;
  const query = match[1] ?? "";
  const atIndex = before.lastIndexOf("@");
  if (atIndex < 0) return null;
  return { query, start: atIndex, end: caret };
}

function readImageDataUrl(file: File): Promise<{
  mimeType: string;
  data: string;
  previewUrl: string;
}> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = typeof reader.result === "string" ? reader.result : "";
      const comma = result.indexOf(",");
      if (comma < 0) {
        reject(new Error("无法读取图片内容"));
        return;
      }
      const data = result.slice(comma + 1).replace(/\s+/g, "");
      resolve({ mimeType: file.type, data, previewUrl: result });
    };
    reader.onerror = () => reject(new Error("读取图片文件失败"));
    reader.readAsDataURL(file);
  });
}

type Props = {
  sessionId: string;
  phase: AcpPhase;
  running: boolean;
  busy: boolean;
  modelBusy: boolean;
  visible: boolean;
  focused: boolean;
  focusRequest: number;
  availableCommands: AcpCommandSpec[];
  lastAssistantText: string | null;
  onSendPrompt: (
    text: string,
    images: AcpImageAttachment[],
  ) => Promise<boolean>;
  onCancelTurn: () => Promise<void>;
  onFocusConfig: () => void;
  onActionError: (message: string | null) => void;
};

const AcpComposer = memo(function AcpComposer({
  sessionId,
  phase,
  running,
  busy,
  modelBusy,
  visible,
  focused,
  focusRequest,
  availableCommands,
  lastAssistantText,
  onSendPrompt,
  onCancelTurn,
  onFocusConfig,
  onActionError,
}: Props) {
  const [draft, setDraft] = useState("");
  const [caretPos, setCaretPos] = useState(0);
  const [sending, setSending] = useState(false);
  const [pendingImages, setPendingImages] = useState<PendingImage[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [fileMatches, setFileMatches] = useState<string[]>([]);
  const [popupIndex, setPopupIndex] = useState(0);
  const [popupDismissedKey, setPopupDismissedKey] = useState<string | null>(
    null,
  );

  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const promptHistoryRef = useRef<string[]>([]);
  const historyIndexRef = useRef(-1);
  const savedDraftRef = useRef("");

  useEffect(() => {
    promptHistoryRef.current = [];
    historyIndexRef.current = -1;
    savedDraftRef.current = "";
    setDraft("");
    setCaretPos(0);
    setPendingImages([]);
    setFileMatches([]);
    setPopupDismissedKey(null);
    setSending(false);
  }, [sessionId]);

  useEffect(() => {
    if (visible && focused && phase === "ready") {
      inputRef.current?.focus();
    }
  }, [visible, focused, focusRequest, phase]);

  const slashMatch = draft.match(/^\/([^\s]*)$/);
  const atMatch = slashMatch ? null : detectAtMention(draft, caretPos);
  const atStart = atMatch?.start ?? -1;
  const atQuery = atMatch?.query ?? null;

  const slashItems: SlashItem[] = (() => {
    if (!slashMatch) return [];
    const query = slashMatch[1].toLowerCase();
    const merged: SlashItem[] = [...BUILTIN_SLASH_COMMANDS];
    const seen = new Set(merged.map((c) => c.name));
    for (const cmd of availableCommands) {
      if (!seen.has(cmd.name)) {
        seen.add(cmd.name);
        merged.push({
          name: cmd.name,
          description: cmd.description,
          inputHint: cmd.inputHint,
        });
      }
    }
    return merged.filter((item) => item.name.toLowerCase().includes(query));
  })();

  const activePopupKey = slashMatch
    ? `slash:${draft}`
    : atQuery !== null
      ? `at:${atStart}:${atQuery}`
      : null;

  useEffect(() => {
    if (
      atQuery === null ||
      !running ||
      phase !== "ready" ||
      popupDismissedKey === `at:${atStart}:${atQuery}`
    ) {
      setFileMatches([]);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      void import("@tauri-apps/api/core")
        .then(({ invoke }) =>
          invoke<string[] | undefined>("acp_search_project_files", {
            sessionId,
            query: atQuery,
          }),
        )
        .then((results) => {
          if (!cancelled) {
            setFileMatches(Array.isArray(results) ? results : []);
            setPopupIndex(0);
          }
        })
        .catch(() => {
          if (!cancelled) setFileMatches([]);
        });
    }, AT_SEARCH_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [sessionId, running, phase, atStart, atQuery, popupDismissedKey]);

  const showSlashPopup =
    Boolean(slashMatch) &&
    slashItems.length > 0 &&
    popupDismissedKey !== activePopupKey;
  const showAtPopup =
    atQuery !== null &&
    fileMatches.length > 0 &&
    popupDismissedKey !== activePopupKey;

  const canSend =
    running &&
    !busy &&
    !sending &&
    phase === "ready" &&
    !modelBusy &&
    (draft.trim().length > 0 || pendingImages.length > 0);

  async function addImageFiles(files: FileList | File[]) {
    const incoming = Array.from(files);
    if (incoming.length === 0) return;
    onActionError(null);
    const added: PendingImage[] = [];
    for (const file of incoming) {
      if (!ALLOWED_IMAGE_MIMES.has(file.type)) {
        onActionError("仅支持 PNG、JPEG、WebP 或 GIF 图片附件");
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        onActionError("单张图片大小不能超过 6 MB");
        continue;
      }
      if (pendingImages.length + added.length >= MAX_PENDING_IMAGES) {
        onActionError(`单次最多附带 ${MAX_PENDING_IMAGES} 张图片`);
        break;
      }
      try {
        const encoded = await readImageDataUrl(file);
        added.push({
          id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          name: file.name || "pasted-image",
          mimeType: encoded.mimeType,
          data: encoded.data,
          previewUrl: encoded.previewUrl,
        });
      } catch (error) {
        onActionError(error instanceof Error ? error.message : String(error));
      }
    }
    if (added.length > 0) {
      setPendingImages((prev) =>
        [...prev, ...added].slice(0, MAX_PENDING_IMAGES),
      );
    }
  }

  function applySlashItem(item: SlashItem) {
    setPopupDismissedKey(null);
    if (item.local === "clear") {
      setDraft("");
      setCaretPos(0);
      setPendingImages([]);
      onActionError(null);
      return;
    }
    if (item.local === "stop") {
      setDraft("");
      setCaretPos(0);
      if (modelBusy && running) {
        void onCancelTurn();
      }
      return;
    }
    if (item.local === "model") {
      setDraft("");
      setCaretPos(0);
      onFocusConfig();
      return;
    }
    if (item.local === "copy") {
      setDraft("");
      setCaretPos(0);
      if (lastAssistantText) {
        void copyToClipboard(lastAssistantText);
      } else {
        onActionError("当前会话还没有可复制的助手回复");
      }
      return;
    }
    const filled = `/${item.name} `;
    setDraft(filled);
    setCaretPos(filled.length);
    inputRef.current?.focus();
  }

  function applyAtFile(filePath: string) {
    if (!atMatch) return;
    const before = draft.slice(0, atMatch.start);
    const after = draft.slice(atMatch.end);
    const inserted = `@${filePath} `;
    const nextDraft = `${before}${inserted}${after}`;
    const nextCaret = before.length + inserted.length;
    setDraft(nextDraft);
    setCaretPos(nextCaret);
    setFileMatches([]);
    setPopupDismissedKey(null);
    requestAnimationFrame(() => {
      const el = inputRef.current;
      if (el) {
        el.focus();
        el.setSelectionRange(nextCaret, nextCaret);
      }
    });
  }

  async function submitPrompt() {
    const text = draft.trim();
    if ((!text && pendingImages.length === 0) || !canSend) return;
    if (text) {
      const history = promptHistoryRef.current;
      if (history[history.length - 1] !== text) {
        history.push(text);
      }
    }
    historyIndexRef.current = -1;
    savedDraftRef.current = "";
    const imagesToSend = pendingImages.map(({ mimeType, data, name }) => ({
      mimeType,
      data,
      name,
    }));
    setSending(true);
    try {
      const ok = await onSendPrompt(text, imagesToSend);
      if (ok) {
        setDraft("");
        setCaretPos(0);
        setPendingImages([]);
        setFileMatches([]);
        setPopupDismissedKey(null);
      }
    } finally {
      setSending(false);
    }
  }

  function handlePaste(event: ReactClipboardEvent<HTMLTextAreaElement>) {
    const items = event.clipboardData?.items;
    if (!items) return;
    const imageFiles: File[] = [];
    for (const item of Array.from(items)) {
      if (item.kind === "file" && ALLOWED_IMAGE_MIMES.has(item.type)) {
        const file = item.getAsFile();
        if (file) imageFiles.push(file);
      }
    }
    if (imageFiles.length > 0) {
      event.preventDefault();
      void addImageFiles(imageFiles);
    }
  }

  function handleDrop(event: ReactDragEvent<HTMLFormElement>) {
    event.preventDefault();
    setDragOver(false);
    if (!running || phase !== "ready" || busy) return;
    if (event.dataTransfer?.files?.length) {
      void addImageFiles(event.dataTransfer.files);
    }
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLTextAreaElement>) {
    if (showSlashPopup || showAtPopup) {
      const count = showSlashPopup ? slashItems.length : fileMatches.length;
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setPopupIndex((idx) => (idx + 1) % count);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setPopupIndex((idx) => (idx - 1 + count) % count);
        return;
      }
      if (
        (event.key === "Enter" || event.key === "Tab") &&
        !event.shiftKey &&
        !event.nativeEvent.isComposing
      ) {
        event.preventDefault();
        const safeIdx = Math.min(popupIndex, count - 1);
        if (showSlashPopup) {
          applySlashItem(slashItems[safeIdx]);
        } else {
          applyAtFile(fileMatches[safeIdx]);
        }
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setPopupDismissedKey(activePopupKey);
        return;
      }
    }
    if (event.key === "Escape" && modelBusy && running) {
      event.preventDefault();
      void onCancelTurn();
      return;
    }
    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      void submitPrompt();
      return;
    }
    const history = promptHistoryRef.current;
    if (history.length === 0 || event.nativeEvent.isComposing) return;
    const textarea = event.currentTarget;
    if (
      event.key === "ArrowUp" &&
      !event.shiftKey &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      textarea.selectionStart === 0 &&
      textarea.selectionEnd === 0
    ) {
      event.preventDefault();
      if (historyIndexRef.current === -1) {
        savedDraftRef.current = draft;
        historyIndexRef.current = history.length - 1;
      } else if (historyIndexRef.current > 0) {
        historyIndexRef.current -= 1;
      }
      setDraft(history[historyIndexRef.current] ?? "");
    } else if (
      event.key === "ArrowDown" &&
      !event.shiftKey &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      historyIndexRef.current >= 0 &&
      textarea.selectionStart === draft.length &&
      textarea.selectionEnd === draft.length
    ) {
      event.preventDefault();
      if (historyIndexRef.current < history.length - 1) {
        historyIndexRef.current += 1;
        setDraft(history[historyIndexRef.current] ?? "");
      } else {
        historyIndexRef.current = -1;
        setDraft(savedDraftRef.current);
      }
    }
  }

  return (
    <div className="acp-composer-wrap">
      {showSlashPopup ? (
        <div className="acp-popup" role="listbox" aria-label="斜杠命令补全">
          {slashItems.map((item, index) => (
            <button
              key={item.name}
              type="button"
              role="option"
              aria-selected={
                index === Math.min(popupIndex, slashItems.length - 1)
              }
              className={`acp-popup-item${index === Math.min(popupIndex, slashItems.length - 1) ? " is-active" : ""}`}
              onMouseDown={(event) => {
                event.preventDefault();
                applySlashItem(item);
              }}
            >
              <span className="acp-popup-name">/{item.name}</span>
              <span className="acp-popup-desc">{item.description}</span>
            </button>
          ))}
        </div>
      ) : null}

      {showAtPopup ? (
        <div className="acp-popup" role="listbox" aria-label="引用项目文件">
          {fileMatches.map((filePath, index) => (
            <button
              key={filePath}
              type="button"
              role="option"
              aria-selected={
                index === Math.min(popupIndex, fileMatches.length - 1)
              }
              className={`acp-popup-item${index === Math.min(popupIndex, fileMatches.length - 1) ? " is-active" : ""}`}
              onMouseDown={(event) => {
                event.preventDefault();
                applyAtFile(filePath);
              }}
            >
              <span className="acp-popup-name">@{filePath}</span>
            </button>
          ))}
        </div>
      ) : null}

      {pendingImages.length > 0 ? (
        <div className="acp-attachment-strip" aria-label="待发送图片">
          {pendingImages.map((img) => (
            <div key={img.id} className="acp-attachment-chip">
              <img
                src={img.previewUrl}
                alt={img.name ?? "附件图片"}
                className="acp-attachment-thumb"
              />
              <span className="acp-attachment-name">{img.name ?? "图片"}</span>
              <button
                type="button"
                className="acp-attachment-remove"
                aria-label={`移除 ${img.name ?? "图片"}`}
                onClick={() =>
                  setPendingImages((prev) =>
                    prev.filter((item) => item.id !== img.id),
                  )
                }
              >
                ×
              </button>
            </div>
          ))}
        </div>
      ) : null}

      <form
        className={`acp-composer${dragOver ? " is-drag-over" : ""}`}
        onDragOver={(event) => {
          event.preventDefault();
          if (running && phase === "ready" && !busy) {
            setDragOver(true);
          }
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={handleDrop}
        onSubmit={(event) => {
          event.preventDefault();
          void submitPrompt();
        }}
      >
        <input
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          multiple
          hidden
          aria-label="上传图片附件"
          onChange={(event) => {
            if (event.target.files?.length) {
              void addImageFiles(event.target.files);
            }
            event.target.value = "";
          }}
        />
        <textarea
          ref={inputRef}
          className="acp-input"
          rows={3}
          aria-label="发送消息给 DeepSeek Harness"
          placeholder={
            phase === "connecting"
              ? "等待 ACP 就绪…"
              : !running
                ? "会话已结束"
                : "输入需求（/ 命令 · @ 引用文件 · 粘贴/拖入图片 · Enter 发送）…"
          }
          value={draft}
          disabled={!running || phase !== "ready" || busy}
          onChange={(event) => {
            const nextVal = event.target.value;
            setDraft(nextVal);
            setCaretPos(event.target.selectionStart ?? nextVal.length);
            setPopupIndex(0);
            setPopupDismissedKey(null);
          }}
          onClick={(event) =>
            setCaretPos(event.currentTarget.selectionStart ?? draft.length)
          }
          onKeyUp={(event) =>
            setCaretPos(event.currentTarget.selectionStart ?? draft.length)
          }
          onPaste={handlePaste}
          onKeyDown={handleKeyDown}
        />
        <div className="acp-composer-actions">
          <button
            type="button"
            className="ghost-button acp-attach-btn"
            disabled={!running || phase !== "ready" || busy}
            title="上传图片（也可直接粘贴截图或拖入图片）"
            onClick={() => fileInputRef.current?.click()}
          >
            + 图片
          </button>
          <button type="submit" className="primary-button" disabled={!canSend}>
            {sending ? "发送中…" : modelBusy ? "执行中…" : "发送"}
          </button>
        </div>
      </form>
    </div>
  );
});

export default AcpComposer;
