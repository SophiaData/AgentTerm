import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ConnectionPhase } from "../terminal/sessionTransport";
import type { SessionExit, SessionInfo } from "../terminal/types";
import AcpComposer from "./AcpComposer";
import AcpRichText, {
  AcpCopyButton,
  AcpDiffBlock,
  extractToolDiff,
} from "./AcpRichText";
import type {
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

const AcpToolCard = memo(function AcpToolCard({ call }: { call: AcpToolCall }) {
  const toolDiff = useMemo(
    () => extractToolDiff(call.title, call.input, call.detail),
    [call.title, call.input, call.detail],
  );
  return (
    <details
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
                {toolDiff.path ? `变更 Diff · ${toolDiff.path}` : "变更 Diff"}
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
});

const AcpMessageRow = memo(function AcpMessageRow({
  message,
  toolName,
  isActiveThought,
}: {
  message: AcpMessage;
  toolName: string;
  isActiveThought: boolean;
}) {
  if (message.role === "thought") {
    return (
      <details className="acp-thought-card" open={isActiveThought}>
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
    <article className={`acp-message is-${message.role}`}>
      <header className="acp-message-header">
        <span className="acp-message-role">
          {message.role === "user"
            ? "你"
            : message.role === "assistant"
              ? toolName
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
});

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
  const [pendingConfigValues, setPendingConfigValues] = useState<
    Record<string, string>
  >({});
  const [actionError, setActionError] = useState<string | null>(null);
  const [respondingPerm, setRespondingPerm] = useState(false);
  const [atBottom, setAtBottom] = useState(true);

  const versionRef = useRef(0);
  const feedRef = useRef<HTMLDivElement | null>(null);
  const followBottomRef = useRef(true);
  const lastScrollTopRef = useRef(0);
  const scrollFrameRef = useRef(0);
  const firstConfigSelectRef = useRef<HTMLSelectElement | null>(null);
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
    setAtBottom(true);
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

  const scrollToBottom = useCallback((forceFollow = false) => {
    if (forceFollow) {
      followBottomRef.current = true;
      setAtBottom(true);
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
  }, []);

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
  }, [visible, scrollToBottom]);

  useEffect(() => {
    if (visible && followBottomRef.current) {
      scrollToBottom(false);
    }
  }, [
    visible,
    scrollToBottom,
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

  const handleSendPrompt = useCallback(
    async (text: string, images: AcpImageAttachment[]): Promise<boolean> => {
      followBottomRef.current = true;
      setAtBottom(true);
      setActionError(null);
      current.current.onUserInput?.(session.id);
      if (text) {
        current.current.onPromptLine?.(session.id, text);
      }
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        if (images.length > 0) {
          await invoke("acp_send_prompt", {
            sessionId: session.id,
            text,
            images,
          });
        } else {
          await invoke("acp_send_prompt", { sessionId: session.id, text });
        }
        scrollToBottom(true);
        return true;
      } catch (error) {
        setActionError(error instanceof Error ? error.message : String(error));
        return false;
      }
    },
    [session.id, scrollToBottom],
  );

  const handleCancelTurn = useCallback(async () => {
    setActionError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("acp_cancel_turn", { sessionId: session.id });
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    }
  }, [session.id]);

  const handleFocusConfig = useCallback(() => {
    firstConfigSelectRef.current?.focus();
  }, []);

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

  const usageLabel = formatUsageLabel(snapshot.usage);
  const feedEntries = useMemo(() => resolveFeedEntries(snapshot), [snapshot]);
  const lastAssistantText = useMemo(() => {
    for (let i = snapshot.messages.length - 1; i >= 0; i -= 1) {
      if (snapshot.messages[i].role === "assistant") {
        return snapshot.messages[i].text;
      }
    }
    return null;
  }, [snapshot.messages]);

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
          void handleCancelTurn();
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
              onClick={() => void handleCancelTurn()}
            >
              停止本轮
            </button>
          ) : null}
        </div>
      </div>

      {snapshot.configOptions?.length ? (
        <div className="acp-config-bar" role="group" aria-label="ACP 会话配置">
          {snapshot.configOptions.map((option, optIdx) => {
            const activeValue =
              pendingConfigValues[option.id] ?? option.currentValue;
            const activeChoice = option.choices.find(
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
                  title={activeChoice?.name ?? activeValue}
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
                  {!activeChoice && activeValue ? (
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

      <div className="acp-feed-wrap">
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
              setAtBottom(true);
            } else if (
              el.scrollTop < lastScrollTopRef.current - 1 ||
              (lastScrollTopRef.current === 0 && el.scrollTop > 0)
            ) {
              followBottomRef.current = false;
              setAtBottom(false);
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

          {feedEntries.map((entry, entryIdx) =>
            entry.kind === "tool" ? (
              <AcpToolCard key={`tool-${entry.item.id}`} call={entry.item} />
            ) : (
              <AcpMessageRow
                key={`msg-${entry.item.id}`}
                message={entry.item}
                toolName={session.toolName}
                isActiveThought={
                  entry.item.role === "thought" &&
                  snapshot.busy &&
                  entryIdx === feedEntries.length - 1
                }
              />
            ),
          )}
        </div>

        {!atBottom ? (
          <button
            type="button"
            className="acp-float-scroll"
            aria-label="回到底部"
            onClick={() => scrollToBottom(true)}
          >
            ↓ 回到底部
          </button>
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

      <AcpComposer
        sessionId={session.id}
        phase={snapshot.phase}
        running={running}
        busy={busy}
        modelBusy={snapshot.busy}
        visible={visible}
        focused={focused}
        focusRequest={focusRequest}
        availableCommands={snapshot.availableCommands ?? []}
        lastAssistantText={lastAssistantText}
        onSendPrompt={handleSendPrompt}
        onCancelTurn={handleCancelTurn}
        onFocusConfig={handleFocusConfig}
        onActionError={setActionError}
      />
    </div>
  );
}
