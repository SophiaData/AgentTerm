import {
  useEffect,
  useRef,
  useState,
  type ClipboardEvent as ReactClipboardEvent,
  type DragEvent as ReactDragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import type { ConnectionPhase } from "../terminal/sessionTransport";
import type { SessionExit, SessionInfo } from "../terminal/types";
import AcpRichText, {
  AcpCopyButton,
  AcpDiffBlock,
  copyToClipboard,
  extractToolDiff,
} from "./AcpRichText";
import type {
  AcpCommandSpec,
  AcpImageAttachment,
  AcpMessage,
  AcpSnapshot,
  AcpToolCall,
  AcpUsage,
} from "./types";

type Props = {
  session: SessionInfo;
  visible: boolean;
  focused: boolean;
  focusRequest: number;
  running: boolean;
  busy: boolean;
  onExit: (id: string, reason: string) => void;
  onConnection: (
    id: string,
    phase: ConnectionPhase,
    error: string | null,
  ) => void;
  onPromptLine?: (id: string, line: string) => void;
  onOutputActivity?: (id: string, text?: string) => void;
  onUserInput?: (id: string) => void;
};

function toolStatusLabel(status: string): string {
  switch (status) {
    case "pending":
      return "等待中";
    case "in_progress":
    case "running":
      return "执行中";
    case "completed":
      return "已完成";
    case "failed":
    case "error":
      return "失败";
    default:
      return status;
  }
}

function formatTokens(count: number): string {
  if (count >= 1000) {
    return `${(count / 1000).toFixed(count >= 100_000 ? 0 : 1)}k`;
  }
  return String(count);
}

function formatUsageLabel(usage?: AcpUsage | null): string | null {
  if (!usage || usage.size <= 0) return null;
  const pct = Math.min(100, Math.round((usage.used / usage.size) * 100));
  return `上下文 ${formatTokens(usage.used)} / ${formatTokens(usage.size)} (${pct}%)`;
}

type FeedEntry =
  { kind: "message"; item: AcpMessage } | { kind: "tool"; item: AcpToolCall };

function resolveFeedEntries(snapshot: AcpSnapshot): FeedEntry[] {
  if (snapshot.timeline && snapshot.timeline.length > 0) {
    const messageMap = new Map(snapshot.messages.map((m) => [m.id, m]));
    const toolMap = new Map(snapshot.toolCalls.map((t) => [t.id, t]));
    const seenMessages = new Set<string>();
    const seenTools = new Set<string>();
    const entries: FeedEntry[] = [];
    for (const ref of snapshot.timeline) {
      if (ref.kind === "message") {
        const msg = messageMap.get(ref.id);
        if (msg && !seenMessages.has(msg.id)) {
          seenMessages.add(msg.id);
          entries.push({ kind: "message", item: msg });
        }
      } else {
        const call = toolMap.get(ref.id);
        if (call && !seenTools.has(call.id)) {
          seenTools.add(call.id);
          entries.push({ kind: "tool", item: call });
        }
      }
    }
    for (const msg of snapshot.messages) {
      if (!seenMessages.has(msg.id))
        entries.push({ kind: "message", item: msg });
    }
    for (const call of snapshot.toolCalls) {
      if (!seenTools.has(call.id)) entries.push({ kind: "tool", item: call });
    }
    return entries;
  }
  return [
    ...snapshot.messages.map((item): FeedEntry => ({ kind: "message", item })),
    ...snapshot.toolCalls.map((item): FeedEntry => ({ kind: "tool", item })),
  ];
}

const ALLOWED_IMAGE_MIMES = new Set([
  "image/png",
  "image/jpeg",
  "image/webp",
  "image/gif",
]);
const MAX_PENDING_IMAGES = 4;
const MAX_IMAGE_BYTES = 6 * 1024 * 1024;

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

function initialSnapshot(): AcpSnapshot {
  return {
    phase: "connecting",
    busy: false,
    messages: [],
    toolCalls: [],
    version: 0,
  };
}

export default function AcpSessionView({
  session,
  visible,
  focused,
  focusRequest,
  running,
  busy,
  onExit,
  onConnection,
  onPromptLine,
  onOutputActivity,
  onUserInput,
}: Props) {
  const [snapshot, setSnapshot] = useState<AcpSnapshot>(initialSnapshot);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [pendingConfigValues, setPendingConfigValues] = useState<
    Record<string, string>
  >({});
  const [actionError, setActionError] = useState<string | null>(null);
  const [respondingPerm, setRespondingPerm] = useState(false);
  const [pendingImages, setPendingImages] = useState<PendingImage[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [caretPos, setCaretPos] = useState(0);
  const [fileMatches, setFileMatches] = useState<string[]>([]);
  const [popupIndex, setPopupIndex] = useState(0);
  const [popupDismissedKey, setPopupDismissedKey] = useState<string | null>(
    null,
  );

  const versionRef = useRef(0);
  const feedRef = useRef<HTMLDivElement | null>(null);
  const followBottomRef = useRef(true);
  const lastScrollTopRef = useRef(0);
  const scrollFrameRef = useRef(0);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const firstConfigSelectRef = useRef<HTMLSelectElement | null>(null);
  const promptHistoryRef = useRef<string[]>([]);
  const historyIndexRef = useRef(-1);
  const savedDraftRef = useRef("");
  const current = useRef({
    onExit,
    onConnection,
    onPromptLine,
    onOutputActivity,
    onUserInput,
  });
  current.current = {
    onExit,
    onConnection,
    onPromptLine,
    onOutputActivity,
    onUserInput,
  };

  useEffect(() => {
    let disposed = false;
    versionRef.current = 0;
    followBottomRef.current = true;
    lastScrollTopRef.current = 0;
    promptHistoryRef.current = [];
    historyIndexRef.current = -1;
    savedDraftRef.current = "";
    setDraft("");
    setCaretPos(0);
    setPendingImages([]);
    setFileMatches([]);
    setPopupDismissedKey(null);
    setSending(false);
    setPendingConfigValues({});
    setSnapshot(initialSnapshot());
    setActionError(null);
    current.current.onConnection(session.id, "connecting", null);

    const applySnapshot = (next: AcpSnapshot, fromLiveEvent: boolean) => {
      if (disposed || next.version < versionRef.current) return;
      const changed = next.version > versionRef.current;
      versionRef.current = next.version;
      setSnapshot(next);
      if (changed) {
        setPendingConfigValues((prev) => {
          const keys = Object.keys(prev);
          if (keys.length === 0) return prev;
          if (next.error) return {};
          const remaining: Record<string, string> = {};
          for (const key of keys) {
            const matched = next.configOptions?.find((opt) => opt.id === key);
            if (matched && matched.currentValue !== prev[key]) {
              remaining[key] = prev[key];
            }
          }
          return Object.keys(remaining).length === keys.length
            ? prev
            : remaining;
        });
      }
      current.current.onConnection(session.id, next.phase, next.error ?? null);
      if (fromLiveEvent && changed && (next.busy || next.messages.length > 0)) {
        current.current.onOutputActivity?.(session.id, "acp-update");
      }
    };

    let unlistenState: (() => void) | undefined;
    let unlistenExit: (() => void) | undefined;

    void Promise.all([
      import("@tauri-apps/api/core"),
      import("@tauri-apps/api/event"),
    ])
      .then(async ([{ invoke }, { listen }]) => {
        if (disposed) return;
        const stopState = await listen<AcpSnapshot>(
          `acp-state-${session.id}`,
          (event) => {
            applySnapshot(event.payload, true);
          },
        );
        if (disposed) {
          stopState();
          return;
        }
        unlistenState = stopState;

        const stopExit = await listen<SessionExit>(
          `session-exit-${session.id}`,
          (event) => {
            if (!disposed) {
              current.current.onExit(session.id, event.payload.reason);
            }
          },
        );
        if (disposed) {
          stopExit();
          return;
        }
        unlistenExit = stopExit;

        const initial = await invoke<AcpSnapshot>("acp_snapshot", {
          sessionId: session.id,
        });
        applySnapshot(initial, false);
      })
      .catch((error: unknown) => {
        if (disposed) return;
        const message =
          error instanceof Error ? error.message : String(error ?? "加载失败");
        setActionError(message);
        current.current.onConnection(session.id, "error", message);
      });

    return () => {
      disposed = true;
      unlistenState?.();
      unlistenExit?.();
    };
  }, [session.id]);

  useEffect(() => {
    if (visible && focused && snapshot.phase === "ready") {
      inputRef.current?.focus();
    }
  }, [visible, focused, focusRequest, snapshot.phase]);

  const scrollToBottom = (forceFollow = false) => {
    if (forceFollow) {
      followBottomRef.current = true;
    }
    const snapFeedToBottom = () => {
      const feed = feedRef.current;
      if (!feed) return;
      feed.scrollTop = feed.scrollHeight;
      lastScrollTopRef.current = feed.scrollTop;
    };
    snapFeedToBottom();
    cancelAnimationFrame(scrollFrameRef.current);
    scrollFrameRef.current = requestAnimationFrame(() => {
      scrollFrameRef.current = 0;
      if (feedRef.current && followBottomRef.current) {
        snapFeedToBottom();
      }
    });
  };

  useEffect(() => {
    return () => cancelAnimationFrame(scrollFrameRef.current);
  }, []);

  useEffect(() => {
    const feed = feedRef.current;
    if (!feed || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (visible && followBottomRef.current) {
        scrollToBottom(false);
      }
    });
    observer.observe(feed);
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (visible && followBottomRef.current) {
      scrollToBottom(false);
    }
  }, [
    visible,
    snapshot.busy,
    snapshot.messages,
    snapshot.toolCalls,
    snapshot.timeline,
    snapshot.pendingPermission,
    snapshot.configOptions,
    snapshot.error,
    actionError,
    snapshot.phase,
  ]);

  const slashMatch = draft.match(/^\/([^\s]*)$/);
  const atMatch = slashMatch ? null : detectAtMention(draft, caretPos);

  const slashItems: SlashItem[] = (() => {
    if (!slashMatch) return [];
    const query = slashMatch[1].toLowerCase();
    const merged: SlashItem[] = [...BUILTIN_SLASH_COMMANDS];
    const seen = new Set(merged.map((c) => c.name));
    for (const cmd of (snapshot.availableCommands ?? []) as AcpCommandSpec[]) {
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
    : atMatch
      ? `at:${atMatch.start}:${atMatch.query}`
      : null;

  useEffect(() => {
    if (
      !atMatch ||
      !running ||
      snapshot.phase !== "ready" ||
      popupDismissedKey === `at:${atMatch.start}:${atMatch.query}`
    ) {
      setFileMatches([]);
      return;
    }
    let cancelled = false;
    void import("@tauri-apps/api/core")
      .then(({ invoke }) =>
        invoke<string[] | undefined>("acp_search_project_files", {
          sessionId: session.id,
          query: atMatch.query,
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
    return () => {
      cancelled = true;
    };
  }, [
    session.id,
    running,
    snapshot.phase,
    atMatch?.start,
    atMatch?.query,
    popupDismissedKey,
  ]);

  const showSlashPopup =
    Boolean(slashMatch) &&
    slashItems.length > 0 &&
    popupDismissedKey !== activePopupKey;
  const showAtPopup =
    Boolean(atMatch) &&
    fileMatches.length > 0 &&
    popupDismissedKey !== activePopupKey;

  const canSend =
    running &&
    !busy &&
    !sending &&
    snapshot.phase === "ready" &&
    !snapshot.busy &&
    (draft.trim().length > 0 || pendingImages.length > 0);

  async function addImageFiles(files: FileList | File[]) {
    const incoming = Array.from(files);
    if (incoming.length === 0) return;
    setActionError(null);
    const added: PendingImage[] = [];
    for (const file of incoming) {
      if (!ALLOWED_IMAGE_MIMES.has(file.type)) {
        setActionError("仅支持 PNG、JPEG、WebP 或 GIF 图片附件");
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        setActionError("单张图片大小不能超过 6 MB");
        continue;
      }
      if (pendingImages.length + added.length >= MAX_PENDING_IMAGES) {
        setActionError(`单次最多附带 ${MAX_PENDING_IMAGES} 张图片`);
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
        setActionError(error instanceof Error ? error.message : String(error));
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
      setActionError(null);
      return;
    }
    if (item.local === "stop") {
      setDraft("");
      setCaretPos(0);
      if (snapshot.busy && running) {
        void cancelTurn();
      }
      return;
    }
    if (item.local === "model") {
      setDraft("");
      setCaretPos(0);
      firstConfigSelectRef.current?.focus();
      return;
    }
    if (item.local === "copy") {
      setDraft("");
      setCaretPos(0);
      const lastAssistant = [...snapshot.messages]
        .reverse()
        .find((m) => m.role === "assistant");
      if (lastAssistant) {
        void copyToClipboard(lastAssistant.text);
      } else {
        setActionError("当前会话还没有可复制的助手回复");
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

  async function sendPrompt() {
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
    followBottomRef.current = true;
    const imagesToSend = pendingImages.map(({ mimeType, data, name }) => ({
      mimeType,
      data,
      name,
    }));
    setSending(true);
    setActionError(null);
    current.current.onUserInput?.(session.id);
    if (text) {
      current.current.onPromptLine?.(session.id, text);
    }
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      if (imagesToSend.length > 0) {
        await invoke("acp_send_prompt", {
          sessionId: session.id,
          text,
          images: imagesToSend,
        });
      } else {
        await invoke("acp_send_prompt", { sessionId: session.id, text });
      }
      setDraft("");
      setCaretPos(0);
      setPendingImages([]);
      setFileMatches([]);
      setPopupDismissedKey(null);
      scrollToBottom(true);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setSending(false);
    }
  }

  async function cancelTurn() {
    setActionError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("acp_cancel_turn", { sessionId: session.id });
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    }
  }

  async function respondPermission(optionId: string | null) {
    const perm = snapshot.pendingPermission;
    if (!perm || respondingPerm) return;
    setRespondingPerm(true);
    setActionError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("acp_respond_permission", {
        sessionId: session.id,
        requestId: perm.requestId,
        optionId,
      });
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setRespondingPerm(false);
    }
  }

  const switchingConfig = Object.keys(pendingConfigValues).length > 0;

  async function changeConfigOption(configId: string, value: string) {
    if (pendingConfigValues[configId] !== undefined) return;
    setPendingConfigValues((prev) => ({ ...prev, [configId]: value }));
    setActionError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("acp_set_config_option", {
        sessionId: session.id,
        configId,
        value,
      });
    } catch (error) {
      setPendingConfigValues((prev) => {
        const next = { ...prev };
        delete next[configId];
        return next;
      });
      setActionError(error instanceof Error ? error.message : String(error));
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
    if (!running || snapshot.phase !== "ready" || busy) return;
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
    if (event.key === "Escape" && snapshot.busy && running) {
      event.preventDefault();
      void cancelTurn();
      return;
    }
    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      void sendPrompt();
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

  const usageLabel = formatUsageLabel(snapshot.usage);
  const feedEntries = resolveFeedEntries(snapshot);

  const statusText =
    snapshot.phase === "connecting"
      ? "正在连接 DeepSeek Harness (ACP)…"
      : snapshot.phase === "error"
        ? "ACP 连接遇到错误"
        : !running || snapshot.phase === "exited"
          ? "ACP 会话已结束，对话记录保留在下方"
          : snapshot.busy
            ? "Agent 正在思考与执行工具…（Esc 停止）"
            : "ACP 已就绪 · Enter 发送 · Shift+Enter 换行 · ↑↓ 历史指令";

  return (
    <div
      className="acp-view"
      aria-label="ACP 会话面板"
      onKeyDown={(event) => {
        if (
          event.key === "Escape" &&
          snapshot.busy &&
          running &&
          !(event.target instanceof HTMLSelectElement)
        ) {
          event.preventDefault();
          void cancelTurn();
        }
      }}
    >
      <div className="terminal-toolbar">
        <span role="status">{statusText}</span>
        <div className="terminal-actions">
          {usageLabel ? (
            <span className="acp-usage-badge" aria-label="上下文占用">
              {usageLabel}
            </span>
          ) : null}
          {snapshot.busy && running ? (
            <button
              type="button"
              className="danger-button"
              onClick={() => void cancelTurn()}
            >
              停止本轮
            </button>
          ) : null}
          <button
            type="button"
            className="ghost-button"
            onClick={() => scrollToBottom(true)}
          >
            回到底部
          </button>
        </div>
      </div>

      {snapshot.configOptions?.length ? (
        <div className="acp-config-bar" role="group" aria-label="ACP 会话配置">
          {snapshot.configOptions.map((option, optIdx) => {
            const activeValue =
              pendingConfigValues[option.id] ?? option.currentValue;
            const hasActiveChoice = option.choices.some(
              (choice) => choice.value === activeValue,
            );
            const ungrouped = option.choices.filter((c) => !c.group);
            const groupOrder: string[] = [];
            const byGroup = new Map<string, typeof option.choices>();
            for (const choice of option.choices) {
              if (!choice.group) continue;
              if (!byGroup.has(choice.group)) {
                groupOrder.push(choice.group);
                byGroup.set(choice.group, []);
              }
              byGroup.get(choice.group)!.push(choice);
            }
            return (
              <label key={option.id} className="acp-config-item">
                <span>
                  {option.category === "model"
                    ? "模型"
                    : option.category === "thought_level"
                      ? "思考强度"
                      : option.name}
                </span>
                <select
                  ref={optIdx === 0 ? firstConfigSelectRef : undefined}
                  className="acp-config-select"
                  aria-label={option.name}
                  value={activeValue}
                  disabled={
                    !running ||
                    busy ||
                    switchingConfig ||
                    snapshot.phase !== "ready" ||
                    snapshot.busy
                  }
                  onChange={(event) =>
                    void changeConfigOption(option.id, event.target.value)
                  }
                >
                  {!hasActiveChoice && activeValue ? (
                    <option key={`cur-${activeValue}`} value={activeValue}>
                      {activeValue}
                    </option>
                  ) : null}
                  {ungrouped.map((choice) => (
                    <option
                      key={`opt-${choice.value}`}
                      value={choice.value}
                      title={choice.description ?? undefined}
                    >
                      {choice.name}
                    </option>
                  ))}
                  {groupOrder.map((group) => (
                    <optgroup key={`grp-${group}`} label={group}>
                      {byGroup.get(group)!.map((choice) => (
                        <option
                          key={`opt-${choice.value}`}
                          value={choice.value}
                          title={choice.description ?? undefined}
                        >
                          {choice.name}
                        </option>
                      ))}
                    </optgroup>
                  ))}
                </select>
              </label>
            );
          })}
        </div>
      ) : null}

      {snapshot.error || actionError ? (
        <div className="terminal-problem" role="alert">
          <span>{actionError ?? snapshot.error}</span>
          <button
            type="button"
            className="ghost-button"
            onClick={() => setActionError(null)}
          >
            关闭
          </button>
        </div>
      ) : null}

      <div
        className="acp-feed"
        ref={feedRef}
        aria-live="polite"
        onScroll={(event) => {
          const el = event.currentTarget;
          const nearBottom =
            el.scrollHeight - el.scrollTop - el.clientHeight <= 24;
          if (nearBottom) {
            followBottomRef.current = true;
          } else if (
            el.scrollTop < lastScrollTopRef.current - 1 ||
            (lastScrollTopRef.current === 0 && el.scrollTop > 0)
          ) {
            followBottomRef.current = false;
          }
          lastScrollTopRef.current = el.scrollTop;
        }}
      >
        {!feedEntries.length ? (
          <div className="acp-empty">
            {snapshot.phase === "connecting" ? (
              <p>正在初始化 DeepSeek Harness ACP 会话…</p>
            ) : (
              <p>
                已通过原生 ACP 协议连接 {session.toolName}
                ，直接输入任务即可开始协作。
              </p>
            )}
          </div>
        ) : null}

        {feedEntries.map((entry, entryIdx) => {
          if (entry.kind === "tool") {
            const call = entry.item;
            const toolDiff = extractToolDiff(
              call.title,
              call.input,
              call.detail,
            );
            return (
              <details
                key={`tool-${call.id}`}
                className="acp-tool-card"
                open={
                  call.status === "in_progress" ||
                  call.status === "running" ||
                  call.status === "failed" ||
                  Boolean(toolDiff)
                }
              >
                <summary className="acp-tool-summary">
                  <span className="acp-tool-title">{call.title}</span>
                  <span className={`acp-tool-status is-${call.status}`}>
                    {toolStatusLabel(call.status)}
                  </span>
                </summary>
                {toolDiff || call.input || call.detail ? (
                  <div className="acp-tool-body">
                    {toolDiff ? (
                      <div className="acp-tool-section">
                        <div className="acp-tool-section-label">
                          {toolDiff.path
                            ? `变更 Diff · ${toolDiff.path}`
                            : "变更 Diff"}
                        </div>
                        <AcpDiffBlock lines={toolDiff.lines} />
                      </div>
                    ) : null}
                    {call.input ? (
                      <div className="acp-tool-section">
                        <div className="acp-tool-section-label">输入</div>
                        <pre className="acp-tool-detail">{call.input}</pre>
                      </div>
                    ) : null}
                    {call.detail ? (
                      <div className="acp-tool-section">
                        <div className="acp-tool-section-label">输出</div>
                        <pre className="acp-tool-detail">{call.detail}</pre>
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </details>
            );
          }

          const message = entry.item;
          if (message.role === "thought") {
            const isActiveThought =
              snapshot.busy && entryIdx === feedEntries.length - 1;
            return (
              <details
                key={`msg-${message.id}`}
                className="acp-thought-card"
                open={isActiveThought}
              >
                <summary className="acp-thought-summary">
                  <span>{isActiveThought ? "正在思考…" : "思考过程"}</span>
                </summary>
                <div className="acp-thought-body">
                  <AcpRichText text={message.text} />
                </div>
              </details>
            );
          }

          return (
            <article
              key={`msg-${message.id}`}
              className={`acp-message is-${message.role}`}
            >
              <header className="acp-message-header">
                <span className="acp-message-role">
                  {message.role === "user"
                    ? "你"
                    : message.role === "assistant"
                      ? session.toolName
                      : "系统"}
                </span>
                {message.role === "assistant" ? (
                  <AcpCopyButton text={message.text} />
                ) : null}
              </header>
              {message.role === "assistant" ? (
                <div className="acp-message-body is-rich">
                  <AcpRichText text={message.text} />
                </div>
              ) : (
                <div className="acp-message-body">{message.text}</div>
              )}
            </article>
          );
        })}
      </div>

      {snapshot.pendingPermission ? (
        <section
          className="acp-permission-bar"
          role="region"
          aria-label="权限确认"
        >
          <div className="acp-permission-copy">
            <strong>{snapshot.pendingPermission.title}</strong>
            {snapshot.pendingPermission.description ? (
              <p>{snapshot.pendingPermission.description}</p>
            ) : null}
          </div>
          <div className="acp-permission-actions">
            {snapshot.pendingPermission.options.length ? (
              snapshot.pendingPermission.options.map((option) => (
                <button
                  key={option.optionId}
                  type="button"
                  className={
                    option.kind.startsWith("reject")
                      ? "danger-button"
                      : "primary-button"
                  }
                  disabled={respondingPerm}
                  onClick={() => void respondPermission(option.optionId)}
                >
                  {option.name}
                </button>
              ))
            ) : (
              <button
                type="button"
                className="danger-button"
                disabled={respondingPerm}
                onClick={() => void respondPermission(null)}
              >
                拒绝
              </button>
            )}
          </div>
        </section>
      ) : null}

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
                <span className="acp-attachment-name">
                  {img.name ?? "图片"}
                </span>
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
            if (running && snapshot.phase === "ready" && !busy) {
              setDragOver(true);
            }
          }}
          onDragLeave={() => setDragOver(false)}
          onDrop={handleDrop}
          onSubmit={(event) => {
            event.preventDefault();
            void sendPrompt();
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
              snapshot.phase === "connecting"
                ? "等待 ACP 就绪…"
                : !running
                  ? "会话已结束"
                  : "输入需求（/ 命令 · @ 引用文件 · 粘贴/拖入图片 · Enter 发送）…"
            }
            value={draft}
            disabled={!running || snapshot.phase !== "ready" || busy}
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
              disabled={!running || snapshot.phase !== "ready" || busy}
              title="上传图片（也可直接粘贴截图或拖入图片）"
              onClick={() => fileInputRef.current?.click()}
            >
              + 图片
            </button>
            <button
              type="submit"
              className="primary-button"
              disabled={!canSend}
            >
              {sending ? "发送中…" : snapshot.busy ? "执行中…" : "发送"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
