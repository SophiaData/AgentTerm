import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import packageJson from "../package.json";
import { backend } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

beforeEach(() => {
  vi.resetAllMocks();
});

describe("Tauri command contract", () => {
  it("reads projects from Rust", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);
    expect(await backend.listProjects()).toEqual([]);
    expect(invoke).toHaveBeenCalledWith("list_projects");
  });

  it("adds a project with a typed path argument", async () => {
    const project = { id: "/repo", name: "repo", path: "/repo" };
    vi.mocked(invoke).mockResolvedValueOnce(project);
    expect(await backend.addProject("/repo")).toEqual(project);
    expect(invoke).toHaveBeenCalledWith("add_project", { path: "/repo" });
  });

  it("removes only the named project record", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({});
    await backend.removeProject("/repo");
    expect(invoke).toHaveBeenCalledWith("remove_project", { id: "/repo" });
  });

  it("checks installed tools through Rust", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);
    await backend.detectTools();
    expect(invoke).toHaveBeenCalledWith("detect_tools");
  });

  it("starts only a registered tool ID with fixed PTY dimensions and project path", async () => {
    const args = {
      sessionId: "session-1",
      toolId: "codex",
      projectPath: "/repo",
      rows: 24,
      cols: 80,
    };
    const result = {
      id: "session-1",
      toolId: "codex",
      toolName: "Codex",
      projectPath: "/repo",
    };
    vi.mocked(invoke).mockResolvedValueOnce(result);
    expect(await backend.startSession(args)).toEqual(result);
    expect(invoke).toHaveBeenCalledWith("start_session", args);
  });

  it("gets a byte-offset snapshot and sends session input and size changes", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      data: "你好",
      offset: 6,
      running: true,
    });
    expect(await backend.sessionSnapshot("a")).toEqual({
      data: "你好",
      offset: 6,
      running: true,
    });
    await backend.writeSessionInput("a", "ls\r");
    await backend.resizeSession("a", 24, 80);
    expect(invoke).toHaveBeenNthCalledWith(1, "session_snapshot", {
      sessionId: "a",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "write_session_input", {
      sessionId: "a",
      data: "ls\r",
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "resize_session", {
      sessionId: "a",
      rows: 24,
      cols: 80,
    });
  });

  it("only stops and forgets the specified managed session", async () => {
    await backend.stopSession("a");
    await backend.forgetSession("a");
    expect(invoke).toHaveBeenNthCalledWith(1, "stop_session", {
      sessionId: "a",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "forget_session", {
      sessionId: "a",
    });
    expect(invoke).not.toHaveBeenCalledWith(
      "launch_native_tool",
      expect.anything(),
    );
  });

  it("subscribes to output and exit events using SDK rather than Tauri globals", async () => {
    const unsub = vi.fn();
    vi.mocked(listen).mockResolvedValue(unsub);
    const output = vi.fn();
    const exit = vi.fn();
    expect(await backend.onSessionOutput("a", output)).toBe(unsub);
    expect(await backend.onSessionExit("a", exit)).toBe(unsub);
    expect(listen).toHaveBeenCalledWith(
      "session-output-a",
      expect.any(Function),
    );
    expect(listen).toHaveBeenCalledWith("session-exit-a", expect.any(Function));
    const onOutput = vi.mocked(listen).mock.calls[0][1];
    const onExit = vi.mocked(listen).mock.calls[1][1];
    onOutput({ payload: { data: "abc", endOffset: 3 } } as Parameters<
      typeof onOutput
    >[0]);
    onExit({ payload: { id: "a", reason: "exited" } } as Parameters<
      typeof onExit
    >[0]);
    expect(output).toHaveBeenCalledWith({ data: "abc", endOffset: 3 });
    expect(exit).toHaveBeenCalledWith({ id: "a", reason: "exited" });
  });

  it("uses a directory-only native chooser", async () => {
    vi.mocked(open).mockResolvedValueOnce("/repo");
    expect(await backend.pickDirectory()).toBe("/repo");
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "选择项目文件夹",
    });
  });

  it("does not accept a non-string dialog result as a directory", async () => {
    vi.mocked(open).mockResolvedValueOnce(null);
    expect(await backend.pickDirectory()).toBeNull();
  });
});

it("uses only vanilla DOM plus the two approved lazy xterm dependencies", () => {
  const forbidden = [
    "react",
    "react-dom",
    "svelte",
    "vue",
    "preact",
    "solid-js",
  ];
  for (const name of forbidden) {
    expect(packageJson.dependencies).not.toHaveProperty(name);
  }
  expect(packageJson.dependencies).toHaveProperty("@xterm/xterm");
  expect(packageJson.dependencies).toHaveProperty("@xterm/addon-fit");
});
