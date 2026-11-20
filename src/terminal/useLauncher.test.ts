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
});
