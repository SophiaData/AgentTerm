import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import type { ConnectionPhase } from "../terminal/sessionTransport";
import type { SessionExit, SessionInfo } from "../terminal/types";
import type { AcpSnapshot } from "./types";

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
  const [actionError, setActionError] = useState<string | null>(null);
  const [respondingPerm, setRespondingPerm] = useState(false);

  const versionRef = useRef(0);
  const feedRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
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
    setSnapshot(initialSnapshot());
    setActionError(null);
    current.current.onConnection(session.id, "connecting", null);

    const applySnapshot = (next: AcpSnapshot, fromLiveEvent: boolean) => {
      if (disposed || next.version < versionRef.current) return;
      const changed = next.version > versionRef.current;
      versionRef.current = next.version;
      setSnapshot(next);
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

  useEffect(() => {
    const feed = feedRef.current;
    if (!feed) return;
    feed.scrollTop = feed.scrollHeight;
  }, [snapshot.messages, snapshot.toolCalls, snapshot.pendingPermission]);

  const canSend =
    running &&
    !busy &&
    !sending &&
    snapshot.phase === "ready" &&
    !snapshot.busy &&
    draft.trim().length > 0;

  async function sendPrompt() {
    const text = draft.trim();
    if (!text || !canSend) return;
    setSending(true);
    setActionError(null);
    current.current.onUserInput?.(session.id);
    current.current.onPromptLine?.(session.id, text);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("acp_send_prompt", { sessionId: session.id, text });
      setDraft("");
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

  function handleKeyDown(event: ReactKeyboardEvent<HTMLTextAreaElement>) {
    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      void sendPrompt();
    }
  }

  const statusText =
    snapshot.phase === "connecting"
      ? "正在连接 DeepSeek Harness (ACP)…"
      : snapshot.phase === "error"
        ? "ACP 连接遇到错误"
        : !running || snapshot.phase === "exited"
          ? "ACP 会话已结束，对话记录保留在下方"
          : snapshot.busy
            ? "Agent 正在思考与执行工具…"
            : "ACP 已就绪 · Enter 发送 · Shift+Enter 换行";

  return (
    <div className="acp-view" aria-label="ACP 会话面板">
      <div className="terminal-toolbar">
        <span role="status">{statusText}</span>
        <div className="terminal-actions">
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
            onClick={() => {
              if (feedRef.current) {
                feedRef.current.scrollTop = feedRef.current.scrollHeight;
              }
            }}
          >
            回到底部
          </button>
        </div>
      </div>

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

      <div className="acp-feed" ref={feedRef} aria-live="polite">
        {!snapshot.messages.length && !snapshot.toolCalls.length ? (
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

        {snapshot.messages.map((message) => (
          <article
            key={message.id}
            className={`acp-message is-${message.role}`}
          >
            <header className="acp-message-role">
              {message.role === "user"
                ? "你"
                : message.role === "assistant"
                  ? session.toolName
                  : "系统"}
            </header>
            <div className="acp-message-body">{message.text}</div>
          </article>
        ))}

        {snapshot.toolCalls.length ? (
          <section className="acp-tools" aria-label="工具调用记录">
            {snapshot.toolCalls.map((call) => (
              <details key={call.id} className="acp-tool-card">
                <summary className="acp-tool-summary">
                  <span className="acp-tool-title">{call.title}</span>
                  <span className={`acp-tool-status is-${call.status}`}>
                    {toolStatusLabel(call.status)}
                  </span>
                </summary>
                {call.detail ? (
                  <pre className="acp-tool-detail">{call.detail}</pre>
                ) : null}
              </details>
            ))}
          </section>
        ) : null}
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

      <form
        className="acp-composer"
        onSubmit={(event) => {
          event.preventDefault();
          void sendPrompt();
        }}
      >
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
                : "输入需求或指令（Enter 发送，Shift+Enter 换行）…"
          }
          value={draft}
          disabled={!running || snapshot.phase !== "ready" || busy}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={handleKeyDown}
        />
        <div className="acp-composer-actions">
          <button type="submit" className="primary-button" disabled={!canSend}>
            {sending ? "发送中…" : snapshot.busy ? "执行中…" : "发送"}
          </button>
        </div>
      </form>
    </div>
  );
}
