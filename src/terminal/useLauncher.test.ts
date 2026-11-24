import { describe, expect, it, vi } from "vitest";
import { createSessionStore } from "./useLauncher";
import type { Project } from "../projects/types";
import type { ToolStatus } from "../tools/types";

const project: Project = { id: "a", name: "Data", path: "E:\\Data" };
const other: Project = { id: "b", name: "Reports", path: "E:\\Reports" };
const tool: ToolStatus = {
  id: "claude",
  name: "Claude Code",
  state: { kind: "installed", path: "claude.exe", version: null },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function fixture() {
  const invoke = vi.fn<
    (command: string, args: Record<string, unknown>) => Promise<unknown>
  >(async (command, args) => {
    if (command === "start_session") {
      return {
        id: args.sessionId,
        toolId: args.toolId,
        toolName: tool.name,
        projectPath: args.projectPath,
      };
    }
    if (command === "rename_session") {
      return String(args.title).trim().replace(/\s+/g, " ");
    }
    return undefined;
  });
  const store = createSessionStore(
    async <T>(command: string, args: Record<string, unknown>) =>
      (await invoke(command, args)) as T,
  );
  return { store, invoke };
}

describe("independent CLI sessions", () => {
  it("opens two sessions and changes selection without restarting either", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    const first = store.getSnapshot().sessions[0];
    await store.start(other, tool);
    const second = store.getSnapshot().sessions[1];
    expect(first.info.id).not.toBe(second.info.id);
    expect(first.info.projectPath).toBe(project.path);
    expect(second.info.projectPath).toBe(other.path);
    expect(first.label).not.toBe(second.label);
    store.select(first.info.id);
    store.showLauncher();
    expect(store.getSnapshot().activeId).toBeNull();
    expect(store.getSnapshot().sessions).toHaveLength(2);
    store.select(second.info.id);
    expect(store.getSnapshot().activeId).toBe(second.info.id);
    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "start_session",
      "start_session",
    ]);
  });

  it("a double click starts only one process while opening is pending", async () => {
    const { store, invoke } = fixture();
    const pending = deferred<unknown>();
    invoke.mockImplementationOnce((_command, args) =>
      pending.promise.then(() => ({
        id: args.sessionId,
        toolId: tool.id,
        toolName: tool.name,
        projectPath: project.path,
      })),
    );
    const first = store.start(project, tool);
    await store.start(project, tool);
    expect(invoke).toHaveBeenCalledOnce();
    pending.resolve(undefined);
    await first;
    expect(store.getSnapshot().sessions).toHaveLength(1);
  });

  it("a background exit updates only its own tab and preserves selection", async () => {
    const { store } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    const [first, second] = store.getSnapshot().sessions;
    store.handleExit(first.info.id, "exit 0");
    expect(store.getSnapshot().activeId).toBe(second.info.id);
    expect(store.getSnapshot().sessions[0].running).toBe(false);
    expect(store.getSnapshot().sessions[1].running).toBe(true);
    store.handleExit("old-session", "late exit");
    expect(store.getSnapshot().sessions).toHaveLength(2);
  });

  it("marks a failed transport as unknown instead of claiming the CLI is running", async () => {
    const { store } = fixture();
    await store.start(project, tool);
    const id = store.getSnapshot().sessions[0].info.id;
    store.handleConnection(id, "error");
    expect(store.getSnapshot().sessions[0].running).toBe(true);
    expect(store.getSnapshot().sessions[0].statusUnknown).toBe(true);
    store.handleConnection(id, "ready");
    expect(store.getSnapshot().sessions[0].statusUnknown).toBe(false);
  });

  it("reconciles a live backend PTY after a WebView reload", async () => {
    const { store, invoke } = fixture();
    invoke.mockResolvedValueOnce([
      {
        id: "recovered-pty",
        toolId: "claude",
        toolName: "Claude Code",
        projectPath: project.path,
        title: "重载后恢复",
      },
    ]);
    await expect(store.reconcile()).resolves.toBe(true);
    const state = store.getSnapshot();
    expect(state.sessions).toHaveLength(1);
    expect(state.sessions[0].info.id).toBe("recovered-pty");
    expect(state.sessions[0].label).toBe("重载后恢复");
    expect(state.sessions[0].running).toBe(true);
    store.select("recovered-pty");
    expect(store.getSnapshot().activeId).toBe("recovered-pty");
  });

  it("stops showing running when backend reconciliation no longer lists the PTY", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    invoke.mockResolvedValueOnce([]);
    await expect(store.reconcile()).resolves.toBe(true);
    const session = store.getSnapshot().sessions[0];
    expect(session.running).toBe(false);
    expect(session.exitReason).toBe("后端会话已结束");
    expect(session.statusUnknown).toBe(false);
  });

  it("closing one session leaves the other and ignores late events", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    await store.start(other, tool);
    const [first, second] = store.getSnapshot().sessions;
    await store.close(second.info.id);
    expect(store.getSnapshot().sessions).toEqual([first]);
    expect(store.getSnapshot().activeId).toBe(first.info.id);
    expect(invoke).toHaveBeenCalledWith("stop_session", {
      sessionId: second.info.id,
    });
    expect(invoke).toHaveBeenCalledWith("forget_session", {
      sessionId: second.info.id,
    });
    store.handleExit(second.info.id, "delayed");
    expect(store.getSnapshot().sessions).toEqual([first]);
  });

  it("failed stop retains that tab with an error and does not forget it", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    await store.start(other, tool);
    const [first, second] = store.getSnapshot().sessions;
    invoke.mockRejectedValueOnce(new Error("denied"));
    invoke.mockResolvedValueOnce({ running: true });
    await store.close(first.info.id);
    const state = store.getSnapshot();
    expect(state.sessions).toHaveLength(2);
    expect(state.sessions[0].closeError).toContain("denied");
    expect(state.sessions[1]).toBe(second);
    expect(state.activeId).toBe(second.info.id);
    expect(
      invoke.mock.calls.some(([command]) => command === "forget_session"),
    ).toBe(false);
  });

  it("releases an ended tab without killing a process and keeps the launcher open", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    const id = store.getSnapshot().sessions[0].info.id;
    store.handleExit(id, "exit 0");
    store.showLauncher();
    await store.close(id);
    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "start_session",
      "forget_session",
    ]);
    expect(store.getSnapshot().sessions).toHaveLength(0);
    expect(store.getSnapshot().activeId).toBeNull();
  });

  it("accepts a stop/natural-exit race only after a stopped snapshot", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    const id = store.getSnapshot().sessions[0].info.id;
    invoke.mockRejectedValueOnce(new Error("already exited"));
    invoke.mockResolvedValueOnce({ running: false });
    await store.close(id);
    expect(invoke).toHaveBeenCalledWith("session_snapshot", { sessionId: id });
    expect(invoke).toHaveBeenCalledWith("forget_session", { sessionId: id });
    expect(store.getSnapshot().sessions).toHaveLength(0);
  });

  it("removes a status-unknown tab when the backend no longer has the PTY", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    const id = store.getSnapshot().sessions[0].info.id;
    store.handleConnection(id, "error");
    invoke.mockRejectedValueOnce(new Error("stop failed"));
    invoke.mockRejectedValueOnce(new Error("snapshot missing"));
    invoke.mockResolvedValueOnce(undefined);
    await store.close(id);
    expect(store.getSnapshot().sessions).toHaveLength(0);
    expect(invoke.mock.calls.slice(1).map(([command]) => command)).toEqual([
      "stop_session",
      "session_snapshot",
      "forget_session",
    ]);
  });

  it("does not replace a tab selected while another is closing", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    await store.start(other, tool);
    const [first, second] = store.getSnapshot().sessions;
    const pending = deferred<unknown>();
    invoke.mockImplementationOnce(() => pending.promise);
    const closing = store.close(second.info.id);
    store.showLauncher();
    pending.resolve(undefined);
    await closing;
    expect(store.getSnapshot().sessions).toEqual([first]);
    expect(store.getSnapshot().activeId).toBeNull();
  });

  it("a failed launch leaves existing sessions intact and can be retried", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    const original = store.getSnapshot().sessions;
    invoke.mockRejectedValueOnce(new Error("missing executable"));
    store.showLauncher();
    await store.start(other, tool);
    expect(store.getSnapshot().sessions).toBe(original);
    expect(store.getSnapshot().startError).toContain("missing executable");
    expect(store.getSnapshot().starting).toBe(false);
    await store.start(other, tool);
    expect(store.getSnapshot().sessions).toHaveLength(2);
    expect(store.getSnapshot().startError).toBeNull();
  });

  it("splits two existing terminals without launching another process or duplicating panes", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    const [a, b] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    expect(store.getSnapshot().activeId).toBe(a);
    store.split(a);
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("focusing the other pane keeps their positions and does not request terminal focus again", async () => {
    const { store } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    const [a, b] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    const revision = store.getSnapshot().focusRequest;
    store.focus(b);
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    expect(store.getSnapshot().activeId).toBe(b);
    expect(store.getSnapshot().focusRequest).toBe(revision);
    store.select(a);
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    expect(store.getSnapshot().focusRequest).toBe(revision + 1);
  });

  it("selecting a hidden third session replaces only the focused pane", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    await store.start(other, tool);
    const [a, b, c] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    store.focus(b);
    store.select(c);
    expect(store.getSnapshot().paneIds).toEqual([a, c]);
    expect(store.getSnapshot().activeId).toBe(c);
    expect(store.getSnapshot().sessions).toHaveLength(3);
    expect(invoke).toHaveBeenCalledTimes(3);
  });

  it("single view keeps the focused terminal and leaves its partner open", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    const [a, b] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    store.focus(b);
    store.single();
    expect(store.getSnapshot().paneIds).toEqual([b]);
    expect(store.getSnapshot().sessions).toHaveLength(2);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("new-session view preserves the split and a new session replaces only its right side", async () => {
    const { store } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    const [a, b] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    store.showLauncher();
    store.focus(b);
    expect(store.getSnapshot().activeId).toBeNull();
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    await store.start(project, tool);
    const c = store.getSnapshot().sessions[2].info.id;
    expect(store.getSnapshot().paneIds).toEqual([a, c]);
    expect(store.getSnapshot().activeId).toBe(c);
  });

  it("closing either split pane keeps its partner in place and returns to single view", async () => {
    const { store } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    await store.start(other, tool);
    const [a, b, c] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    store.focus(b);
    await store.close(b);
    expect(store.getSnapshot().paneIds).toEqual([a]);
    expect(store.getSnapshot().activeId).toBe(a);
    store.split(c);
    await store.close(c);
    expect(store.getSnapshot().paneIds).toEqual([a]);
    expect(store.getSnapshot().activeId).toBe(a);
  });

  it("closing a hidden session or a failed close leaves the visible split intact", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    await store.start(other, tool);
    const [a, b, c] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    await store.close(c);
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    invoke.mockRejectedValueOnce(new Error("denied"));
    invoke.mockResolvedValueOnce({ running: true });
    await store.close(a);
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    expect(store.getSnapshot().activeId).toBe(a);
  });

  it("a background natural exit preserves its last screen beside the active terminal", async () => {
    const { store } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    const [a, b] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split(b);
    store.handleExit(b, "exit 1");
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
    expect(store.getSnapshot().activeId).toBe(a);
    expect(store.getSnapshot().sessions[1].exitReason).toBe("exit 1");
  });

  it("splitting a lone session is a no-op and unknown focus never changes selection", async () => {
    const { store } = fixture();
    store.split();
    await store.start(project, tool);
    const id = store.getSnapshot().activeId;
    store.split();
    store.focus("unknown");
    store.select("unknown");
    expect(store.getSnapshot().paneIds).toEqual([id]);
    expect(store.getSnapshot().activeId).toBe(id);
  });

  it("prefers another terminal in the same project as the split partner", async () => {
    const { store } = fixture();
    await store.start(project, tool);
    await store.start(project, tool);
    await store.start(other, tool);
    const [a, b] = store.getSnapshot().sessions.map((s) => s.info.id);
    store.select(a);
    store.split();
    expect(store.getSnapshot().paneIds).toEqual([a, b]);
  });

  it("auto-names once from the first meaningful prompt and never overwrites manual renames", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    const id = store.getSnapshot().sessions[0].info.id;

    await store.autoName(id, "/help");
    await store.autoName(id, "yes");
    expect(store.getSnapshot().sessions[0].titleSource).toBe("default");

    await store.autoName(id, "排查广告表主键重复问题");
    expect(store.getSnapshot().sessions[0].label).toBe(
      "排查广告表主键重复问题",
    );
    expect(store.getSnapshot().sessions[0].titleSource).toBe("auto");
    expect(invoke).toHaveBeenCalledWith("rename_session", {
      sessionId: id,
      title: "排查广告表主键重复问题",
    });

    // Subsequent prompts must not overwrite an already auto-named session.
    await store.autoName(id, "第二个问题不应覆盖标题");
    expect(store.getSnapshot().sessions[0].label).toBe(
      "排查广告表主键重复问题",
    );

    // Manual rename overrides the auto title and persists through backend.
    const renamed = await store.rename(id, "  核心广告表修复  ");
    expect(renamed).toEqual({ ok: true, title: "核心广告表修复" });
    expect(store.getSnapshot().sessions[0].label).toBe("核心广告表修复");
    expect(store.getSnapshot().sessions[0].titleSource).toBe("manual");
    expect(store.getSnapshot().sessions[0].info.title).toBe("核心广告表修复");

    // Invalid manual rename fails closed without touching the current title.
    const bad = await store.rename(id, "   ");
    expect(bad.ok).toBe(false);
    expect(store.getSnapshot().sessions[0].label).toBe("核心广告表修复");
  });

  it("manual rename wins over an in-flight automatic naming request", async () => {
    const { store, invoke } = fixture();
    await store.start(project, tool);
    const id = store.getSnapshot().sessions[0].info.id;

    const pendingAuto = deferred<string>();
    invoke.mockImplementationOnce(() => pendingAuto.promise);
    const autoTask = store.autoName(id, "自动命名的第一条指令");

    const manual = await store.rename(id, "用户手动命名");
    expect(manual).toEqual({ ok: true, title: "用户手动命名" });

    pendingAuto.resolve("自动命名的第一条指令");
    await autoTask;

    expect(store.getSnapshot().sessions[0].label).toBe("用户手动命名");
    expect(store.getSnapshot().sessions[0].titleSource).toBe("manual");
  });
});
