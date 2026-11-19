import type {
  Backend,
  SessionExit,
  SessionInfo,
  SessionOutput,
  Unlisten,
} from "./api";
import { announce, button, element } from "./dom";
import { advance, byteLength, sliceChunkAfter } from "./offsetStitch";
import type { TerminalRenderer } from "./terminalRenderer";
import type { Project, ToolStatus } from "./types";

type Phase = "starting" | "running" | "finished" | "failed";

type ManagedSession = {
  id: string;
  projectName: string;
  toolName: string;
  phase: Phase;
  started: boolean;
  exited: boolean;
  exitReason: string | null;
  error: string | null;
  closing: boolean;
  busy: boolean;
  removed: boolean;
  offset: number;
  snapshotReady: boolean;
  pending: SessionOutput[];
  backpressure: boolean;
  syncing: Promise<void> | null;
  rendererInit: Promise<void> | null;
  renderer: TerminalRenderer | null;
  unlisten: Unlisten[];
  lastResize: string | null;
  pane: HTMLElement;
  host: HTMLElement;
  actions: HTMLElement;
};

const INITIAL_ROWS = 24;
const INITIAL_COLS = 80;
const MIN_HOST_WIDTH = 160;
const MIN_HOST_HEIGHT = 80;
const MAX_PENDING_CHUNKS = 128;
const MAX_PENDING_BYTES = 1024 * 1024;

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Keeps every terminal's original DOM node and xterm instance while switching tabs. */
export function createSessionDock(api: Backend) {
  let destroyed = false;
  let activeId: string | null = null;
  const sessions: ManagedSession[] = [];

  const dock = element("section", "terminal-dock");
  dock.setAttribute("aria-labelledby", "sessions-title");
  dock.hidden = true;
  const header = element("div", "terminal-dock-header");
  header.append(
    Object.assign(element("h2", "", "受管终端"), { id: "sessions-title" }),
    element("span", "terminal-hint", "多会话常驻 · 关闭窗口不会自动终止进程"),
  );
  const tabs = element("div", "terminal-tabs");
  tabs.setAttribute("role", "tablist");
  tabs.setAttribute("aria-label", "终端会话");
  const stages = element("div", "terminal-stages");
  dock.append(header, tabs, stages);

  const findSession = (id: string | undefined) =>
    sessions.find((session) => !session.removed && session.id === id);

  const currentSession = () => findSession(activeId ?? undefined);

  function isLive(session: ManagedSession) {
    return !destroyed && !session.removed;
  }

  function renderActions(session: ManagedSession) {
    if (!isLive(session)) return;
    const region = session.actions;
    region.replaceChildren();

    const state = element(
      "span",
      "session-state state-" + session.phase,
      session.phase === "starting"
        ? "启动中…"
        : session.phase === "running"
          ? "运行中"
          : session.phase === "finished"
            ? "已结束"
            : "启动失败",
    );
    state.setAttribute("role", "status");
    region.append(state);

    if (session.exitReason) {
      const detail = element("span", "session-exit-reason", session.exitReason);
      detail.title = session.exitReason;
      region.append(detail);
    }

    if (session.error) region.append(announce(session.error, true));
    if (session.renderer && !session.busy) {
      region.append(
        button("聚焦终端", "focus-session", {
          id: session.id,
          className: "button button-quiet",
        }),
      );
    }

    if (session.closing && session.phase === "running") {
      const confirm = element("div", "session-confirm");
      confirm.append(
        element("span", "", "终止运行中的终端并关闭？"),
        button(
          session.busy ? "正在终止…" : "终止并关闭",
          "confirm-close-session",
          {
            id: session.id,
            className: "button button-danger",
            disabled: session.busy,
          },
        ),
        button("取消", "cancel-close-session", {
          id: session.id,
          className: "button button-quiet",
          disabled: session.busy,
        }),
      );
      region.append(confirm);
    } else {
      if (session.started && session.renderer && session.error) {
        region.append(
          button("重新同步输出", "sync-session", {
            id: session.id,
            className: "button button-secondary",
            disabled: session.busy || !!session.syncing,
          }),
        );
      }
      if (session.started && !session.renderer && !session.busy) {
        region.append(
          button("重试终端显示", "retry-renderer", {
            id: session.id,
            className: "button button-secondary",
            disabled: !!session.rendererInit,
          }),
        );
      }
    }
  }

  function renderTabs() {
    if (destroyed) return;
    dock.hidden = sessions.length === 0;
    const focused = document.activeElement;
    const wasTabFocused =
      focused instanceof HTMLElement && focused.getAttribute("role") === "tab";
    const focusedId = wasTabFocused ? focused.dataset.id : null;
    tabs.replaceChildren();
    for (const session of sessions) {
      const selected = session.id === activeId;
      const wrapper = element(
        "div",
        "terminal-tab" + (selected ? " is-selected" : ""),
      );
      const tab = button("", "select-session", {
        id: session.id,
        className: "terminal-tab-choice",
      });
      tab.id = "session-tab-" + session.id;
      tab.setAttribute("role", "tab");
      tab.setAttribute("aria-selected", String(selected));
      tab.setAttribute("aria-controls", "session-panel-" + session.id);
      tab.tabIndex = selected ? 0 : -1;
      tab.append(
        element(
          "span",
          "terminal-tab-name",
          session.projectName + " · " + session.toolName,
        ),
        element(
          "span",
          "terminal-tab-status state-" + session.phase,
          session.phase === "running"
            ? "运行中"
            : session.phase === "starting"
              ? "启动中"
              : session.phase === "finished"
                ? "已结束"
                : "失败",
        ),
      );
      const close = button("×", "close-session", {
        id: session.id,
        className: "terminal-tab-close",
        disabled: session.busy || session.phase === "starting",
      });
      close.setAttribute(
        "aria-label",
        "关闭终端 " + session.projectName + " · " + session.toolName,
      );
      wrapper.append(tab, close);
      tabs.append(wrapper);
      session.pane.hidden = !selected;
      renderActions(session);
    }
    if (wasTabFocused && focusedId) {
      tabs
        .querySelector<HTMLButtonElement>(
          '[role="tab"][data-id="' + focusedId + '"]',
        )
        ?.focus();
    }
  }

  function selectSession(session: ManagedSession, focus = true) {
    if (!isLive(session)) return;
    activeId = session.id;
    renderTabs();
    if (focus) {
      tabs
        .querySelector<HTMLButtonElement>('[role="tab"][aria-selected="true"]')
        ?.focus();
    }
    scheduleFit();
    // Reconciliation catches output emitted while the window was minimized or
    // Tauri's event channel was temporarily unavailable.
    if (session.started && session.renderer) void syncSnapshot(session);
  }

  function addSession(project: Project, tool: ToolStatus): ManagedSession {
    const id = globalThis.crypto.randomUUID();
    const pane = element("div", "terminal-pane");
    pane.setAttribute("role", "tabpanel");
    pane.id = "session-panel-" + id;
    pane.setAttribute("aria-labelledby", "session-tab-" + id);
    const actions = element("div", "terminal-actions");
    const host = element("div", "terminal-host");
    host.setAttribute(
      "aria-label",
      "终端输出 " + project.name + " · " + tool.name,
    );
    pane.append(actions, host);
    stages.append(pane);

    const session: ManagedSession = {
      id,
      projectName: project.name,
      toolName: tool.name,
      phase: "starting",
      started: false,
      exited: false,
      exitReason: null,
      error: null,
      closing: false,
      busy: true,
      removed: false,
      offset: 0,
      snapshotReady: false,
      pending: [],
      backpressure: false,
      syncing: null,
      rendererInit: null,
      renderer: null,
      unlisten: [],
      lastResize: null,
      pane,
      host,
      actions,
    };
    sessions.push(session);
    if (resizeObserver) resizeObserver.observe(host);
    activeId = id;
    renderTabs();
    return session;
  }

  function applyPending(session: ManagedSession) {
    if (!session.snapshotReady || !session.renderer) return;
    session.pending.sort((a, b) => a.endOffset - b.endOffset);
    const remaining: SessionOutput[] = [];
    for (const chunk of session.pending) {
      if (chunk.endOffset <= session.offset) continue;
      const startOffset = chunk.endOffset - byteLength(chunk.data);
      // Never silently skip a missing range; a snapshot can fill this hole.
      if (startOffset > session.offset) {
        remaining.push(chunk);
        continue;
      }
      const next = advance(chunk, session.offset);
      if (next.text) session.renderer.write(next.text);
      session.offset = next.offset;
    }
    session.pending = remaining;
    if (remaining.length) {
      const truncated = session.error?.includes("历史已截断")
        ? "输出历史已截断，早期内容可能无法恢复。"
        : null;
      session.error = [
        truncated,
        "终端输出出现偏移缺口，请点击“重新同步输出”补齐。",
      ]
        .filter(Boolean)
        .join(" ");
      renderActions(session);
    } else if (session.error?.includes("偏移缺口")) {
      session.error = session.error.includes("历史已截断")
        ? "输出历史已截断，早期内容可能无法恢复。"
        : null;
      renderActions(session);
    }
  }

  function onOutput(session: ManagedSession, output: SessionOutput) {
    if (
      !isLive(session) ||
      typeof output.data !== "string" ||
      !Number.isFinite(output.endOffset)
    ) {
      return;
    }
    session.pending.push(output);
    if (
      session.pending.length > MAX_PENDING_CHUNKS ||
      session.pending.reduce(
        (bytes, item) => bytes + byteLength(item.data),
        0,
      ) > MAX_PENDING_BYTES
    ) {
      // Snapshot is authoritative. Bound memory while the renderer is loading or
      // the WebView is hidden; never advance the byte cursor for dropped chunks.
      session.pending = [];
      session.snapshotReady = false;
      session.backpressure = true;
      session.error = "输出积压超过缓存上限，正在从快照恢复；可点击重新同步。";
      renderActions(session);
      if (session.renderer && !session.syncing) void syncSnapshot(session);
      return;
    }
    if (session.snapshotReady && !session.syncing) applyPending(session);
  }

  function onExit(session: ManagedSession, exit: SessionExit) {
    if (!isLive(session) || exit.id !== session.id) return;
    session.exited = true;
    session.phase = "finished";
    session.exitReason = exit.reason;
    session.closing = false;
    renderTabs();
    // A final output event may be lost when Rust closes its reader. Read the
    // authoritative final snapshot after any in-flight initial reconciliation.
    void syncAfterCurrent(session);
  }

  async function syncAfterCurrent(session: ManagedSession): Promise<void> {
    if (session.syncing) await session.syncing;
    if (isLive(session) && session.started && session.renderer) {
      await syncSnapshot(session);
    }
  }

  async function syncSnapshot(session: ManagedSession): Promise<void> {
    if (!isLive(session) || !session.started || !session.renderer) return;
    if (session.syncing) return session.syncing;
    const task = (async () => {
      try {
        const snapshot = await api.sessionSnapshot(session.id);
        if (!isLive(session) || !session.renderer) return;
        if (
          typeof snapshot.data !== "string" ||
          !Number.isSafeInteger(snapshot.offset) ||
          snapshot.offset < 0
        ) {
          throw new Error("后端返回了无效的输出快照");
        }
        // Reconcile only the UTF-8 suffix that has not been displayed yet.
        if (snapshot.offset > session.offset) {
          const start = snapshot.offset - byteLength(snapshot.data);
          if (session.offset < start) {
            session.error = "输出历史已截断，早期内容可能无法恢复。";
          }
          const suffix = sliceChunkAfter(
            { data: snapshot.data, endOffset: snapshot.offset },
            session.offset,
          );
          if (suffix) session.renderer.write(suffix);
          session.offset = snapshot.offset;
        }
        session.snapshotReady = true;
        if (!session.exited) {
          session.phase = snapshot.running ? "running" : "finished";
        }
        if (!session.error?.includes("历史已截断")) session.error = null;
        applyPending(session);
      } catch (error) {
        if (isLive(session))
          session.error = "无法同步终端输出：" + message(error);
      } finally {
        if (isLive(session)) renderTabs();
      }
    })();
    session.syncing = task;
    try {
      await task;
    } finally {
      session.syncing = null;
      if (isLive(session)) renderActions(session);
      if (isLive(session) && session.backpressure && session.renderer) {
        session.backpressure = false;
        // One event-driven resync after overflow that occurred mid-snapshot.
        // No timer/polling loop; later overflows will be handled separately.
        void syncSnapshot(session);
      }
    }
  }

  function attachRenderer(session: ManagedSession): Promise<void> {
    if (!isLive(session) || session.renderer) return Promise.resolve();
    if (session.rendererInit) return session.rendererInit;
    // Reserve the initialization slot before dynamic import or xterm.open.
    // Two rapid retry clicks must never create two renderers on one host.
    const task = (async () => {
      try {
        const { createTerminalRenderer } = await import("./terminalRenderer");
        if (!isLive(session)) return;
        session.renderer = createTerminalRenderer(session.host, (data) => {
          if (!isLive(session) || session.phase !== "running") return;
          void api
            .writeSessionInput(session.id, data)
            .catch((error: unknown) => {
              if (!isLive(session)) return;
              session.error = "无法发送终端输入：" + message(error);
              renderActions(session);
            });
        });
        session.error = null;
        await syncSnapshot(session);
        scheduleFit();
      } catch (error) {
        if (isLive(session)) {
          session.error = "终端显示初始化失败：" + message(error);
          renderTabs();
        }
        throw error;
      }
    })();
    session.rendererInit = task;
    renderActions(session);
    // Observe both outcomes without making an unhandled rejecting .finally().
    void task.then(
      () => {
        if (session.rendererInit === task) session.rendererInit = null;
        if (isLive(session)) renderActions(session);
      },
      () => {
        if (session.rendererInit === task) session.rendererInit = null;
        if (isLive(session)) renderActions(session);
      },
    );
    return task;
  }

  async function launch(
    project: Project,
    tool: ToolStatus,
  ): Promise<SessionInfo> {
    if (destroyed) throw new Error("应用已关闭");
    const session = addSession(project, tool);
    try {
      // Subscribe *before* starting the process so startup bytes cannot be lost.
      const subscriptions = await Promise.allSettled([
        api.onSessionOutput(session.id, (output) => onOutput(session, output)),
        api.onSessionExit(session.id, (exit) => onExit(session, exit)),
      ]);
      for (const sub of subscriptions) {
        if (sub.status !== "fulfilled") continue;
        if (isLive(session)) session.unlisten.push(sub.value);
        else sub.value();
      }
      if (subscriptions.some((sub) => sub.status === "rejected")) {
        throw new Error("无法监听 PTY 输出或退出事件");
      }
      if (!isLive(session)) throw new Error("应用已关闭");
      const started = await api.startSession({
        sessionId: session.id,
        projectPath: project.path,
        toolId: tool.id,
        rows: INITIAL_ROWS,
        cols: INITIAL_COLS,
      });
      if (started.id !== session.id) {
        throw new Error("后端返回的会话 ID 与请求不一致");
      }
      session.started = true;
      if (!session.exited) session.phase = "running";
      renderTabs();
      let rendererWarning: string | null = null;
      try {
        await attachRenderer(session);
      } catch (error) {
        // The Rust PTY is already running: keep it managed, with a visible
        // retry action. A renderer failure must not be reported as spawn failure.
        rendererWarning =
          "终端显示暂不可用：" + message(error) + "（可在会话中重试）";
      }
      if (isLive(session)) {
        session.busy = false;
        renderTabs();
        if (session.renderer && activeId === session.id)
          session.renderer.focus();
      }
      return rendererWarning
        ? {
            ...started,
            warning: [started.warning, rendererWarning]
              .filter(Boolean)
              .join("；"),
          }
        : started;
    } catch (error) {
      if (!session.started) {
        for (const unsubscribe of session.unlisten.splice(0)) unsubscribe();
      }
      if (isLive(session)) {
        session.phase = session.started ? session.phase : "failed";
        session.error = "无法启动或接管受管终端：" + message(error);
        session.busy = false;
        renderTabs();
      }
      throw error;
    }
  }

  async function closeSession(session: ManagedSession, confirmed = false) {
    if (!isLive(session) || session.busy || session.phase === "starting")
      return;
    if (session.phase === "running" && !confirmed) {
      session.closing = true;
      activeId = session.id;
      renderTabs();
      session.actions
        .querySelector<HTMLButtonElement>(
          '[data-action="cancel-close-session"]',
        )
        ?.focus();
      return;
    }

    session.busy = true;
    session.error = null;
    renderTabs();
    try {
      if (session.phase === "running") {
        // Rust stop_session already removes the managed session record.
        // Calling forget_session afterwards would fail with "not found".
        await api.stopSession(session.id);
        removeSession(session);
        return;
      }
      if (session.started) await api.forgetSession(session.id);
      removeSession(session);
    } catch (error) {
      session.error =
        session.phase === "running"
          ? "终止终端失败，可重试：" + message(error)
          : "清理已结束会话失败，可重试：" + message(error);
      session.busy = false;
      session.closing = session.phase === "running";
      renderTabs();
      // Busy-state redraw removed the clicked button; keep the retry action
      // keyboard reachable instead of leaving focus on a detached element.
      (session.closing
        ? session.actions.querySelector<HTMLButtonElement>(
            '[data-action="confirm-close-session"]',
          )
        : tabs.querySelector<HTMLButtonElement>(
            '[role="tab"][aria-selected="true"]',
          )
      )?.focus();
    }
  }

  function removeSession(session: ManagedSession) {
    session.removed = true;
    for (const unsubscribe of session.unlisten) unsubscribe();
    if (resizeObserver) resizeObserver.unobserve(session.host);
    session.renderer?.dispose();
    session.pane.remove();
    sessions.splice(sessions.indexOf(session), 1);
    if (activeId === session.id) activeId = sessions.at(-1)?.id ?? null;
    renderTabs();
    scheduleFit();
    tabs
      .querySelector<HTMLButtonElement>('[role="tab"][aria-selected="true"]')
      ?.focus();
  }

  function fitVisible() {
    const session = currentSession();
    if (
      !session ||
      !session.renderer ||
      destroyed ||
      session.pane.hidden ||
      !session.started ||
      session.phase !== "running" ||
      session.exited
    )
      return;
    if (document.visibilityState === "hidden") return;
    const bounds = session.host.getBoundingClientRect();
    if (bounds.width < MIN_HOST_WIDTH || bounds.height < MIN_HOST_HEIGHT)
      return;
    const size = session.renderer.fit();
    if (!size) return;
    const key = size.rows + "x" + size.cols;
    if (size.rows < 3 || size.rows > 160 || size.cols < 20 || size.cols > 500)
      return;
    if (session.lastResize === key) return;
    session.lastResize = key;
    void api
      .resizeSession(session.id, size.rows, size.cols)
      .catch((error: unknown) => {
        if (!isLive(session)) return;
        session.lastResize = null;
        session.error = "无法调整终端尺寸：" + message(error);
        renderActions(session);
      });
  }

  function scheduleFit() {
    if (destroyed) return;
    if (typeof requestAnimationFrame === "function") {
      requestAnimationFrame(() => {
        if (!destroyed) fitVisible();
      });
    } else {
      fitVisible();
    }
  }

  const resizeObserver =
    typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(() => fitVisible());

  function handleTabKeys(event: KeyboardEvent) {
    const focused = event.target;
    if (
      !(focused instanceof HTMLElement) ||
      focused.getAttribute("role") !== "tab"
    )
      return;
    const index = sessions.findIndex(
      (session) => session.id === focused.dataset.id,
    );
    if (index < 0 || sessions.length === 0) return;
    let next: number;
    if (event.key === "ArrowRight") next = (index + 1) % sessions.length;
    else if (event.key === "ArrowLeft")
      next = (index - 1 + sessions.length) % sessions.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = sessions.length - 1;
    else return;
    event.preventDefault();
    selectSession(sessions[next]);
  }

  function handleClick(event: MouseEvent) {
    const target =
      event.target instanceof Element
        ? event.target.closest<HTMLButtonElement>("[data-action]")
        : null;
    if (!target || !dock.contains(target) || target.disabled) return;
    const session = findSession(target.dataset.id);
    if (!session) return;
    switch (target.dataset.action) {
      case "select-session":
        selectSession(session);
        break;
      case "close-session":
        void closeSession(session);
        break;
      case "cancel-close-session":
        session.closing = false;
        renderTabs();
        tabs
          .querySelector<HTMLButtonElement>(
            '[role="tab"][aria-selected="true"]',
          )
          ?.focus();
        break;
      case "confirm-close-session":
        void closeSession(session, true);
        break;
      case "focus-session":
        session.renderer?.focus();
        break;
      case "sync-session":
        void syncSnapshot(session);
        break;
      case "retry-renderer":
        // The session error is displayed by attachRenderer; the UI action must
        // consume the rejection instead of producing an unhandled promise.
        void attachRenderer(session).catch(() => undefined);
        break;
    }
  }

  function handleVisibilityChange() {
    scheduleFit();
    if (document.visibilityState === "hidden") return;
    // WebView/Tauri event delivery may pause while minimized. Reconcile every
    // retained session, not just the selected tab, on foreground restoration.
    for (const session of sessions) {
      if (session.started && session.renderer) void syncAfterCurrent(session);
    }
  }

  dock.addEventListener("click", handleClick);
  tabs.addEventListener("keydown", handleTabKeys);
  window.addEventListener("resize", scheduleFit);
  document.addEventListener("visibilitychange", handleVisibilityChange);

  return {
    element: dock,
    launch,
    destroy() {
      destroyed = true;
      dock.removeEventListener("click", handleClick);
      tabs.removeEventListener("keydown", handleTabKeys);
      window.removeEventListener("resize", scheduleFit);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      resizeObserver?.disconnect();
      for (const session of sessions) {
        session.removed = true;
        for (const unsubscribe of session.unlisten) unsubscribe();
        session.renderer?.dispose();
      }
      sessions.length = 0;
    },
  };
}
