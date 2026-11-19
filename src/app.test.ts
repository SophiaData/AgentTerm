// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { createApp } from "./app";
import type { Backend, SessionExit, SessionInfo, SessionOutput } from "./api";

const renderers: Array<{
  host: HTMLElement;
  input: (data: string) => void;
  writes: string[];
  focus: ReturnType<typeof vi.fn>;
  fit: ReturnType<typeof vi.fn>;
  dispose: ReturnType<typeof vi.fn>;
}> = [];

// The real SDK renderer has its own tests; DOM tests exercise actual state,
// Tauri mocks, persistent host nodes, input and byte stream integration.
vi.mock("./terminalRenderer", () => ({
  createTerminalRenderer: vi.fn(
    (host: HTMLElement, input: (data: string) => void) => {
      const record = {
        host,
        input,
        writes: [] as string[],
        focus: vi.fn(),
        fit: vi.fn(() => ({ rows: 24, cols: 80 })),
        dispose: vi.fn(),
      };
      renderers.push(record);
      return {
        write: (text: string) => record.writes.push(text),
        focus: record.focus,
        fit: record.fit,
        dispose: record.dispose,
      };
    },
  ),
}));
import type { Project, ToolStatus } from "./types";
import { createTerminalRenderer } from "./terminalRenderer";

const apps: Array<ReturnType<typeof createApp>> = [];

const projectA: Project = {
  id: "/work/alpha",
  name: "alpha",
  path: "/work/alpha",
};
const projectB: Project = {
  id: "/work/beta",
  name: "beta",
  path: "/work/beta",
};
const tools: ToolStatus[] = [
  {
    id: "claude",
    name: "Claude Code",
    state: { kind: "installed", path: "/bin/claude", version: "2.1.0" },
  },
  {
    id: "codex",
    name: "Codex",
    state: { kind: "installed", path: "/bin/codex", version: null },
  },
  { id: "agy", name: "Agy", state: { kind: "missing" } },
];

function fixture(
  projects: Project[] = [projectA],
  statuses: ToolStatus[] = tools,
) {
  const outputs = new Map<string, (chunk: SessionOutput) => void>();
  const exits = new Map<string, (exit: SessionExit) => void>();
  const api: Backend = {
    listProjects: vi.fn().mockResolvedValue(projects),
    addProject: vi.fn().mockImplementation(async (path: string) => ({
      id: path,
      path,
      name: path.split("/").pop() || path,
    })),
    removeProject: vi.fn().mockResolvedValue(projectA),
    detectTools: vi.fn().mockResolvedValue(statuses),
    pickDirectory: vi.fn().mockResolvedValue("/work/new"),
    startSession: vi
      .fn()
      .mockImplementation(
        async (args: {
          sessionId: string;
          toolId: string;
          projectPath: string;
        }) => ({
          id: args.sessionId,
          toolId: args.toolId,
          toolName:
            statuses.find((tool) => tool.id === args.toolId)?.name ??
            args.toolId,
          projectPath: args.projectPath,
        }),
      ),
    sessionSnapshot: vi
      .fn()
      .mockResolvedValue({ data: "", offset: 0, running: true }),
    writeSessionInput: vi.fn().mockResolvedValue(undefined),
    resizeSession: vi.fn().mockResolvedValue(undefined),
    stopSession: vi.fn().mockResolvedValue(undefined),
    forgetSession: vi.fn().mockResolvedValue(undefined),
    onSessionOutput: vi
      .fn()
      .mockImplementation(
        async (id: string, handler: (chunk: SessionOutput) => void) => {
          outputs.set(id, handler);
          return () => outputs.delete(id);
        },
      ),
    onSessionExit: vi
      .fn()
      .mockImplementation(
        async (id: string, handler: (exit: SessionExit) => void) => {
          exits.set(id, handler);
          return () => exits.delete(id);
        },
      ),
  };
  const root = document.createElement("div");
  document.body.append(root);
  const app = createApp(root, api);
  apps.push(app);
  return { root, app, api, outputs, exits };
}

function control(
  root: HTMLElement,
  action: string,
  id?: string,
): HTMLButtonElement {
  const candidates = root.querySelectorAll<HTMLButtonElement>("[data-action]");
  const result = Array.from(candidates).find(
    (element) =>
      element.dataset.action === action &&
      (id === undefined || element.dataset.id === id),
  );
  if (!result) throw new Error("Missing control: " + action + " " + id);
  return result;
}

async function settled() {
  await vi.waitFor(() =>
    expect(document.querySelector(".app-shell")).not.toBeNull(),
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
}

afterEach(() => {
  for (const app of apps.splice(0)) app.destroy();
  document.body.replaceChildren();
  renderers.length = 0;
  vi.restoreAllMocks();
});

describe("vanilla desktop launcher", () => {
  it("starts with project and tool loading feedback", async () => {
    const { root, app } = fixture();
    expect(root.textContent).toContain("正在读取项目");
    await app.ready;
    expect(root.textContent).toContain("alpha");
    expect(root.textContent).toContain("Claude Code");
    app.destroy();
  });

  it("shows an actionable empty state", async () => {
    const { root, app } = fixture([]);
    await app.ready;
    expect(root.textContent).toContain("还没有项目");
    expect(control(root, "launch").disabled).toBe(true);
  });

  it("renders installed, missing and non-executable states with text", async () => {
    const statuses = [
      ...tools,
      {
        id: "broken",
        name: "Broken",
        state: { kind: "notExecutable" as const, reason: "权限不足" },
      },
    ];
    const { root, app } = fixture([projectA], statuses);
    await app.ready;
    expect(root.textContent).toContain("已安装 · 2.1.0");
    expect(root.textContent).toContain("未安装");
    expect(root.textContent).toContain("不可执行");
    expect(control(root, "select-tool", "agy").disabled).toBe(true);
    expect(control(root, "select-tool", "broken").disabled).toBe(true);
  });

  it("launches the selected tool for the selected project", async () => {
    const { root, app, api } = fixture([projectA, projectB]);
    await app.ready;
    control(root, "select-project", projectB.id).click();
    control(root, "select-tool", "codex").click();
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(api.startSession).toHaveBeenCalledWith(
        expect.objectContaining({
          toolId: "codex",
          projectPath: projectB.path,
          rows: 24,
          cols: 80,
        }),
      ),
    );
    await vi.waitFor(() =>
      expect(root.textContent).toContain("已在内嵌终端启动 Codex"),
    );
  });

  it("uses the remembered tool for a project", async () => {
    const { root, app } = fixture([{ ...projectA, lastTool: "codex" }]);
    await app.ready;
    expect(
      control(root, "select-tool", "codex").getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("falls back to an installed tool if remembered tool is unavailable", async () => {
    const { root, app } = fixture([{ ...projectA, lastTool: "agy" }]);
    await app.ready;
    expect(
      control(root, "select-tool", "claude").getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("adds a chosen project folder", async () => {
    const { root, app, api } = fixture([]);
    await app.ready;
    control(root, "add-project").click();
    await vi.waitFor(() => expect(root.textContent).toContain("new"));
    expect(api.addProject).toHaveBeenCalledWith("/work/new");
    expect(control(root, "launch").disabled).toBe(false);
  });

  it("does not add a project when folder picker is canceled", async () => {
    const { root, app, api } = fixture([]);
    vi.mocked(api.pickDirectory).mockResolvedValueOnce(null);
    await app.ready;
    control(root, "add-project").click();
    await settled();
    expect(api.addProject).not.toHaveBeenCalled();
    expect(root.textContent).toContain("还没有项目");
  });

  it("reports add failure without losing existing project", async () => {
    const { root, app, api } = fixture();
    vi.mocked(api.addProject).mockRejectedValueOnce(new Error("重复项目"));
    await app.ready;
    control(root, "add-project").click();
    await vi.waitFor(() => expect(root.textContent).toContain("重复项目"));
    expect(root.textContent).toContain("alpha");
    expect(root.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("requires confirmation before removing a project", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "request-remove", projectA.id).click();
    expect(root.textContent).toContain("只移除记录，不删除文件夹");
    expect(api.removeProject).not.toHaveBeenCalled();
    control(root, "cancel-remove").click();
    expect(api.removeProject).not.toHaveBeenCalled();
    control(root, "request-remove", projectA.id).click();
    control(root, "confirm-remove").click();
    await vi.waitFor(() =>
      expect(api.removeProject).toHaveBeenCalledWith(projectA.id),
    );
    expect(root.textContent).toContain("还没有项目");
  });

  it("retains the project and confirmation if remove fails", async () => {
    const { root, app, api } = fixture();
    vi.mocked(api.removeProject).mockRejectedValueOnce(new Error("保存失败"));
    await app.ready;
    control(root, "request-remove", projectA.id).click();
    control(root, "confirm-remove").click();
    await vi.waitFor(() => expect(root.textContent).toContain("保存失败"));
    expect(root.textContent).toContain("alpha");
    expect(control(root, "confirm-remove")).toBeDefined();
  });

  it("reports launch failure without pretending the tool started", async () => {
    const { root, app, api } = fixture();
    vi.mocked(api.startSession).mockRejectedValueOnce(new Error("PTY 不可用"));
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(root.textContent).toContain("PTY 不可用"));
    expect(root.textContent).not.toContain("已在内嵌终端启动");
    expect(control(root, "launch").disabled).toBe(false);
  });

  it("supports manually refreshing tool states", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.detectTools).mockResolvedValueOnce(
      [...tools].filter((tool) => tool.id !== "agy"),
    );
    control(root, "refresh-tools").click();
    await vi.waitFor(() => expect(api.detectTools).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(root.textContent).not.toContain("Agy"));
  });

  it("shows backend errors as accessible alerts", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.detectTools).mockRejectedValueOnce(new Error("访问受限"));
    control(root, "refresh-tools").click();
    await vi.waitFor(() => expect(root.textContent).toContain("访问受限"));
    expect(root.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("treats project names and paths as text, never HTML", async () => {
    const hostile = {
      id: "<svg/onload=x>",
      name: "<img src=x onerror=alert(1)>",
      path: "<script>x</script>",
    };
    const { root, app } = fixture([hostile]);
    await app.ready;
    expect(root.textContent).toContain(hostile.name);
    expect(root.querySelector("img, svg, script")).toBeNull();
  });

  it("keeps managed terminal launch disabled when nothing is installed", async () => {
    const { root, app } = fixture([projectA], [tools[2]]);
    await app.ready;
    expect(control(root, "launch").disabled).toBe(true);
    expect(root.textContent).toContain("未安装");
  });

  it("selects a usable tool when the first one is missing", async () => {
    const { root, app } = fixture([projectA], [tools[2], tools[1]]);
    await app.ready;
    expect(
      control(root, "select-tool", "codex").getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("changes the active project without launching a process", async () => {
    const { root, app, api } = fixture([projectA, projectB]);
    await app.ready;
    control(root, "select-project", projectB.id).click();
    expect(
      control(root, "select-project", projectB.id).getAttribute("aria-pressed"),
    ).toBe("true");
    expect(api.startSession).not.toHaveBeenCalled();
  });

  it("retains keyboard focus after choosing a project", async () => {
    const { root, app } = fixture([projectA, projectB]);
    await app.ready;
    const choice = control(root, "select-project", projectB.id);
    choice.focus();
    choice.click();
    expect(document.activeElement).toBe(
      control(root, "select-project", projectB.id),
    );
  });

  it("selects the next project after removing the active one", async () => {
    const { root, app } = fixture([projectA, projectB]);
    await app.ready;
    control(root, "request-remove", projectA.id).click();
    control(root, "confirm-remove").click();
    await vi.waitFor(() =>
      expect(
        control(root, "select-project", projectB.id).getAttribute(
          "aria-pressed",
        ),
      ).toBe("true"),
    );
  });

  it("keeps a usable selection when an unavailable tool is clicked", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "select-tool", "agy").click();
    control(root, "launch").click();
    await settled();
    expect(api.startSession).toHaveBeenCalledWith(
      expect.objectContaining({ toolId: "claude", projectPath: projectA.path }),
    );
    expect(api.startSession).toHaveBeenCalledTimes(1);
  });

  it("retains usable tool choices when refresh fails", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.detectTools).mockRejectedValueOnce(new Error("检测失败"));
    control(root, "refresh-tools").click();
    await vi.waitFor(() => expect(root.textContent).toContain("检测失败"));
    expect(control(root, "select-tool", "claude").disabled).toBe(false);
  });

  it("ignores user launch during in-flight managed PTY startup", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    let finish!: (result: SessionInfo) => void;
    let requested!: { sessionId: string; toolId: string; projectPath: string };
    vi.mocked(api.startSession).mockImplementationOnce((args) => {
      requested = args;
      return new Promise<SessionInfo>((resolve) => {
        finish = resolve;
      });
    });
    control(root, "launch").click();
    expect(control(root, "launch").disabled).toBe(true);
    control(root, "launch").click();
    await vi.waitFor(() => expect(api.startSession).toHaveBeenCalledTimes(1));
    finish({
      id: requested.sessionId,
      toolId: requested.toolId,
      toolName: "Claude Code",
      projectPath: requested.projectPath,
    });
    await vi.waitFor(() =>
      expect(control(root, "launch").disabled).toBe(false),
    );
  });

  it("keeps manual tool selection after refreshing the registry", async () => {
    const { root, app, api } = fixture([{ ...projectA, lastTool: "claude" }]);
    await app.ready;
    control(root, "select-tool", "codex").click();
    control(root, "refresh-tools").click();
    await vi.waitFor(() => expect(api.detectTools).toHaveBeenCalledTimes(2));
    await vi.waitFor(() =>
      expect(
        control(root, "select-tool", "codex").getAttribute("aria-pressed"),
      ).toBe("true"),
    );
  });

  it("links both panels to their heading IDs", async () => {
    const { root, app } = fixture();
    await app.ready;
    expect(
      root.querySelector('[aria-labelledby="projects-title"] #projects-title')
        ?.textContent,
    ).toBe("项目");
    expect(
      root.querySelector('[aria-labelledby="tools-title"] #tools-title')
        ?.textContent,
    ).toBe("AI 工具");
  });

  it("moves focus to cancel when removal confirmation replaces the old button", async () => {
    const { root, app } = fixture();
    await app.ready;
    const remove = control(root, "request-remove", projectA.id);
    remove.focus();
    remove.click();
    expect(document.activeElement).toBe(control(root, "cancel-remove"));
  });

  it("restores keyboard focus after an async refresh", async () => {
    const { root, app } = fixture();
    await app.ready;
    const refresh = control(root, "refresh-tools");
    refresh.focus();
    refresh.click();
    await vi.waitFor(() =>
      expect(control(root, "refresh-tools").disabled).toBe(false),
    );
    expect(document.activeElement).toBe(control(root, "refresh-tools"));
  });

  it("shows a non-blocking preference warning after successful PTY startup", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.startSession).mockImplementationOnce(async (args) => ({
      id: args.sessionId,
      toolId: args.toolId,
      toolName: "Claude Code",
      projectPath: args.projectPath,
      warning: "最近工具未保存",
    }));
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("最近工具未保存"),
    );
    expect(root.textContent).not.toContain("无法启动终端");
  });

  it("never remounts UI after destroy during an unfinished launch", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    let resolve!: (value: SessionInfo) => void;
    let requested!: { sessionId: string; toolId: string; projectPath: string };
    vi.mocked(api.startSession).mockImplementationOnce((args) => {
      requested = args;
      return new Promise<SessionInfo>((done) => {
        resolve = done;
      });
    });
    control(root, "launch").click();
    await vi.waitFor(() => expect(api.startSession).toHaveBeenCalledTimes(1));
    app.destroy();
    resolve({
      id: requested.sessionId,
      toolId: requested.toolId,
      toolName: "Claude Code",
      projectPath: requested.projectPath,
    });
    await new Promise((done) => setTimeout(done, 0));
    expect(root.childElementCount).toBe(0);
  });

  it("removes the listener on destroy", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    const launch = control(root, "launch");
    app.destroy();
    launch.click();
    expect(api.startSession).not.toHaveBeenCalled();
    expect(root.childElementCount).toBe(0);
  });

  it("keeps four simultaneous PTY instances and their DOM nodes mounted across tabs", async () => {
    const { root, app, api } = fixture([projectA, projectB]);
    await app.ready;
    for (let index = 0; index < 4; index++) {
      if (index % 2 === 1) control(root, "select-project", projectB.id).click();
      else control(root, "select-project", projectA.id).click();
      control(root, "select-tool", index % 2 ? "codex" : "claude").click();
      control(root, "launch").click();
      await vi.waitFor(() => expect(renderers.length).toBe(index + 1));
    }
    const hosts = renderers.map((item) => item.host);
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(4);
    expect(api.startSession).toHaveBeenCalledTimes(4);
    const ids = vi
      .mocked(api.startSession)
      .mock.calls.map(([args]) => args.sessionId);
    control(root, "select-session", ids[0]).click();
    expect(
      control(root, "select-session", ids[0]).getAttribute("aria-selected"),
    ).toBe("true");
    expect(root.textContent).toContain("alpha · Claude Code");
    expect(root.textContent).toContain("beta · Codex");
    control(root, "refresh-tools").click();
    await vi.waitFor(() => expect(api.detectTools).toHaveBeenCalledTimes(2));
    expect(renderers.map((item) => item.host)).toEqual(hosts);
    for (const host of hosts) expect(host.isConnected).toBe(true);
    expect(api.startSession).toHaveBeenCalledTimes(4);
  });

  it("keeps terminal output while inactive and reactivates without duplicating bytes", async () => {
    const { root, app, api, outputs } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(2));
    const ids = vi
      .mocked(api.startSession)
      .mock.calls.map(([args]) => args.sessionId);
    outputs.get(ids[0])?.({ data: "后台输出", endOffset: 12 });
    expect(renderers[0].writes.join("")).toBe("后台输出");
    control(root, "select-session", ids[0]).click();
    await vi.waitFor(() =>
      expect(
        control(root, "select-session", ids[0]).getAttribute("aria-selected"),
      ).toBe("true"),
    );
    expect(renderers[0].writes.join("")).toBe("后台输出");
    expect(renderers[0].host.isConnected).toBe(true);
  });

  it("stitches UTF-8 events received before the first snapshot without duplication", async () => {
    const { root, app, api, outputs } = fixture();
    await app.ready;
    let completeSnapshot!: (snapshot: {
      data: string;
      offset: number;
      running: boolean;
    }) => void;
    vi.mocked(api.sessionSnapshot).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          completeSnapshot = resolve;
        }),
    );
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(1),
    );
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    outputs.get(id)?.({ data: "你好", endOffset: 6 });
    outputs.get(id)?.({ data: "!", endOffset: 7 });
    completeSnapshot({ data: "你", offset: 3, running: true });
    await vi.waitFor(() => expect(renderers[0].writes.join("")).toBe("你好!"));
    outputs.get(id)?.({ data: "你好", endOffset: 6 });
    expect(renderers[0].writes.join("")).toBe("你好!");
  });

  it("drops already-snapshotted live events and appends later bytes once", async () => {
    const { root, app, api, outputs } = fixture();
    await app.ready;
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "abc",
      offset: 3,
      running: true,
    });
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers[0]?.writes.join("")).toBe("abc"));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    outputs.get(id)?.({ data: "abc", endOffset: 3 });
    outputs.get(id)?.({ data: "def", endOffset: 6 });
    outputs.get(id)?.({ data: "def", endOffset: 6 });
    expect(renderers[0].writes.join("")).toBe("abcdef");
  });

  it("keeps finished state when exit precedes the snapshot response", async () => {
    const { root, app, api, exits } = fixture();
    await app.ready;
    let completeSnapshot!: (snapshot: {
      data: string;
      offset: number;
      running: boolean;
    }) => void;
    vi.mocked(api.sessionSnapshot).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          completeSnapshot = resolve;
        }),
    );
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(1),
    );
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    exits.get(id)?.({ id, reason: "exit code 0" });
    completeSnapshot({ data: "ok", offset: 2, running: true });
    await vi.waitFor(() => expect(renderers[0].writes.join("")).toBe("ok"));
    expect(root.textContent).toContain("已结束");
    expect(root.textContent).toContain("exit code 0");
  });

  it("reorders briefly out-of-order output without skipping bytes", async () => {
    const { root, app, api, outputs } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    outputs.get(id)?.({ data: "c", endOffset: 3 });
    expect(renderers[0].writes.join("")).toBe("");
    outputs.get(id)?.({ data: "ab", endOffset: 2 });
    expect(renderers[0].writes.join("")).toBe("abc");
  });

  it("reports byte gaps and repairs them through explicit snapshot reconciliation", async () => {
    const { root, app, api, outputs } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    outputs.get(id)?.({ data: "e", endOffset: 5 });
    expect(root.textContent).toContain("偏移缺口");
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "abcd",
      offset: 4,
      running: true,
    });
    control(root, "sync-session", id).click();
    await vi.waitFor(() => expect(renderers[0].writes.join("")).toBe("abcde"));
    expect(root.textContent).not.toContain("偏移缺口");
  });

  it("routes xterm keyboard input into the selected PTY and reports failures", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    renderers[0].input("pwd\r");
    await vi.waitFor(() =>
      expect(api.writeSessionInput).toHaveBeenCalledWith(id, "pwd\r"),
    );
    vi.mocked(api.writeSessionInput).mockRejectedValueOnce(
      new Error("stream closed"),
    );
    renderers[0].input("a");
    await vi.waitFor(() => expect(root.textContent).toContain("stream closed"));
    expect(root.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("requires confirmation to close a running session and supports cancel", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    control(root, "close-session", id).click();
    expect(root.textContent).toContain("终止运行中的终端并关闭");
    expect(api.stopSession).not.toHaveBeenCalled();
    control(root, "cancel-close-session", id).click();
    expect(api.stopSession).not.toHaveBeenCalled();
    control(root, "close-session", id).click();
    control(root, "confirm-close-session", id).click();
    await vi.waitFor(() => expect(api.stopSession).toHaveBeenCalledWith(id));
    expect(api.forgetSession).not.toHaveBeenCalled();
    await vi.waitFor(() =>
      expect(root.querySelector('[role="tab"]')).toBeNull(),
    );
    expect(renderers[0].dispose).toHaveBeenCalledTimes(1);
  });

  it("preserves a running session and retry controls when stop fails", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    vi.mocked(api.stopSession).mockRejectedValueOnce(
      new Error("signal refused"),
    );
    control(root, "close-session", id).click();
    control(root, "confirm-close-session", id).click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("signal refused"),
    );
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(api.forgetSession).not.toHaveBeenCalled();
    control(root, "confirm-close-session", id).click();
    await vi.waitFor(() => expect(api.stopSession).toHaveBeenCalledTimes(2));
    await vi.waitFor(() =>
      expect(root.querySelectorAll('[role="tab"]')).toHaveLength(0),
    );
    expect(api.forgetSession).not.toHaveBeenCalled();
  });

  it("forgets a finished session without sending stop and keeps it if forget fails", async () => {
    const { root, app, api, exits } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    exits.get(id)?.({ id, reason: "code 0" });
    vi.mocked(api.forgetSession).mockRejectedValueOnce(
      new Error("cleanup refused"),
    );
    control(root, "close-session", id).click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("cleanup refused"),
    );
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(api.stopSession).not.toHaveBeenCalled();
    control(root, "close-session", id).click();
    await vi.waitFor(() =>
      expect(root.querySelectorAll('[role="tab"]')).toHaveLength(0),
    );
    expect(api.forgetSession).toHaveBeenCalledTimes(2);
  });

  it("supports accessible arrow-key tab navigation and focus", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(2));
    const ids = vi
      .mocked(api.startSession)
      .mock.calls.map(([args]) => args.sessionId);
    const second = control(root, "select-session", ids[1]);
    second.focus();
    second.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }),
    );
    expect(
      control(root, "select-session", ids[0]).getAttribute("aria-selected"),
    ).toBe("true");
    expect(document.activeElement).toBe(
      control(root, "select-session", ids[0]),
    );
  });

  it("fits only visible measured panes and blocks invalid 964-row dimensions", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const host = renderers[0].host;
    vi.spyOn(host, "getBoundingClientRect").mockReturnValue(
      new DOMRect(0, 0, 700, 300),
    );
    window.dispatchEvent(new Event("resize"));
    await vi.waitFor(() => expect(api.resizeSession).toHaveBeenCalledTimes(1));
    expect(api.resizeSession).toHaveBeenCalledWith(
      vi.mocked(api.startSession).mock.calls[0][0].sessionId,
      24,
      80,
    );
    renderers[0].fit.mockReturnValue({ rows: 964, cols: 80 });
    window.dispatchEvent(new Event("resize"));
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(api.resizeSession).toHaveBeenCalledTimes(1);
    expect(host.isConnected).toBe(true);
  });

  it("does not stop an active PTY when its window/app view is destroyed", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    app.destroy();
    expect(api.stopSession).not.toHaveBeenCalled();
    expect(api.forgetSession).not.toHaveBeenCalled();
    expect(renderers[0].dispose).toHaveBeenCalledTimes(1);
  });

  it("reports output listener errors without starting an unobservable process", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.onSessionOutput).mockRejectedValueOnce(
      new Error("listen denied"),
    );
    control(root, "launch").click();
    await vi.waitFor(() => expect(root.textContent).toContain("无法监听 PTY"));
    expect(api.startSession).not.toHaveBeenCalled();
    expect(root.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("reports renderer failure as a recoverable warning without mislabeling PTY spawn", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(createTerminalRenderer).mockImplementationOnce(() => {
      throw new Error("canvas unavailable");
    });
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("canvas unavailable"),
    );
    expect(api.startSession).toHaveBeenCalledTimes(1);
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(1);
    expect(root.textContent).not.toContain("无法启动终端");
    expect(root.textContent).toContain("已在内嵌终端启动");
    expect(control(root, "retry-renderer")).toBeDefined();
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    control(root, "retry-renderer", id).click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    expect(renderers[0].host.isConnected).toBe(true);
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(1);
  });

  it("bounds pending output memory while snapshot waits, then restores from snapshot", async () => {
    const { root, app, api, outputs } = fixture();
    await app.ready;
    let resolveSnapshot!: (snapshot: {
      data: string;
      offset: number;
      running: boolean;
    }) => void;
    vi.mocked(api.sessionSnapshot).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveSnapshot = resolve;
        }),
    );
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(1),
    );
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    for (let offset = 1; offset <= 129; offset++) {
      outputs.get(id)?.({ data: "x", endOffset: offset });
    }
    expect(root.textContent).toContain("输出积压超过缓存上限");
    resolveSnapshot({ data: "x".repeat(129), offset: 129, running: true });
    await vi.waitFor(() =>
      expect(renderers[0].writes.join("")).toBe("x".repeat(129)),
    );
    expect(root.textContent).not.toContain("输出积压超过缓存上限");
    expect(api.startSession).toHaveBeenCalledTimes(1);
  });

  it("unsubscribes late listeners if app closes before subscription resolves", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    const unsubscribe = vi.fn();
    let finishSubscription!: (remove: () => void) => void;
    vi.mocked(api.onSessionOutput).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishSubscription = resolve;
        }),
    );
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(api.onSessionOutput).toHaveBeenCalledTimes(1),
    );
    app.destroy();
    finishSubscription(unsubscribe);
    await vi.waitFor(() => expect(unsubscribe).toHaveBeenCalledTimes(1));
    expect(api.startSession).not.toHaveBeenCalled();
    expect(root.childElementCount).toBe(0);
  });

  it("recovers from failed snapshots without dropping the live session", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.sessionSnapshot).mockRejectedValueOnce(
      new Error("snapshot refused"),
    );
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("snapshot refused"),
    );
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(1);
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "recover",
      offset: 7,
      running: true,
    });
    control(root, "sync-session", id).click();
    await vi.waitFor(() =>
      expect(renderers[0].writes.join("")).toBe("recover"),
    );
    expect(root.textContent).not.toContain("snapshot refused");
  });

  it("provides an explicit keyboard focus control for embedded terminals", async () => {
    const { root, app } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    await vi.waitFor(() =>
      expect(control(root, "focus-session")).toBeDefined(),
    );
    const previousCount = renderers[0].focus.mock.calls.length;
    control(root, "focus-session").click();
    expect(renderers[0].focus).toHaveBeenCalledTimes(previousCount + 1);
  });

  it("fetches final PTY bytes on exit even if the last output event was lost", async () => {
    const { root, app, api, exits } = fixture();
    await app.ready;
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "abc",
      offset: 3,
      running: true,
    });
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers[0]?.writes.join("")).toBe("abc"));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "abcdef",
      offset: 6,
      running: false,
    });
    exits.get(id)?.({ id, reason: "code 0" });
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(2),
    );
    await vi.waitFor(() => expect(renderers[0].writes.join("")).toBe("abcdef"));
    expect(root.textContent).toContain("已结束");
    expect(api.stopSession).not.toHaveBeenCalled();
  });

  it("refreshes again after an in-flight initial snapshot when exit arrives early", async () => {
    const { root, app, api, exits } = fixture();
    await app.ready;
    let finishInitial!: (result: {
      data: string;
      offset: number;
      running: boolean;
    }) => void;
    vi.mocked(api.sessionSnapshot)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishInitial = resolve;
          }),
      )
      .mockResolvedValueOnce({ data: "complete", offset: 8, running: false });
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(1),
    );
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    exits.get(id)?.({ id, reason: "code 0" });
    finishInitial({ data: "comp", offset: 4, running: true });
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(2),
    );
    await vi.waitFor(() =>
      expect(renderers[0].writes.join("")).toBe("complete"),
    );
    expect(root.textContent).toContain("已结束");
  });

  it("reconciles every retained PTY on returning from a hidden window", async () => {
    const { root, app, api } = fixture([projectA, projectB]);
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    control(root, "select-project", projectB.id).click();
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(2));
    const ids = vi
      .mocked(api.startSession)
      .mock.calls.map(([args]) => args.sessionId);
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(2),
    );
    const visibility = vi.spyOn(document, "visibilityState", "get");
    visibility.mockReturnValue("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    expect(api.sessionSnapshot).toHaveBeenCalledTimes(2);
    // No events are dispatched while hidden: snapshots are the only recovery.
    vi.mocked(api.sessionSnapshot).mockImplementation(async (id: string) =>
      id === ids[0]
        ? { data: "missing A", offset: 9, running: true }
        : { data: "missing B", offset: 9, running: true },
    );
    visibility.mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.waitFor(() =>
      expect(api.sessionSnapshot).toHaveBeenCalledTimes(4),
    );
    await vi.waitFor(() => {
      expect(renderers[0].writes.join("")).toBe("missing A");
      expect(renderers[1].writes.join("")).toBe("missing B");
    });
    expect(
      control(root, "select-session", ids[1]).getAttribute("aria-selected"),
    ).toBe("true");
    expect(renderers[0].host.isConnected).toBe(true);
    expect(renderers[1].host.isConnected).toBe(true);
  });

  it("reports truncated history even if backpressure reset snapshotReady", async () => {
    const { root, app, api, outputs } = fixture();
    await app.ready;
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "seed",
      offset: 4,
      running: true,
    });
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers[0]?.writes.join("")).toBe("seed"));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "tail",
      offset: 20,
      running: true,
    });
    for (let offset = 1; offset <= 129; offset++) {
      outputs.get(id)?.({ data: "x", endOffset: 20 + offset });
    }
    await vi.waitFor(() =>
      expect(renderers[0].writes.join("")).toBe("seedtail"),
    );
    expect(root.textContent).toContain("输出历史已截断");
    expect(root.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("warns when the first snapshot begins after an unavailable history prefix", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.sessionSnapshot).mockResolvedValueOnce({
      data: "tail",
      offset: 10,
      running: true,
    });
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers[0]?.writes.join("")).toBe("tail"));
    expect(root.textContent).toContain("输出历史已截断");
  });

  it("keeps renderer retries single-flight after a prior renderer failure", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(createTerminalRenderer).mockImplementationOnce(() => {
      throw new Error("first renderer failed");
    });
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("first renderer failed"),
    );
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    const retry = control(root, "retry-renderer", id);
    retry.click();
    retry.click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    expect(createTerminalRenderer).toHaveBeenCalledTimes(2);
    expect(api.startSession).toHaveBeenCalledTimes(1);
    expect(renderers[0].host.isConnected).toBe(true);
  });

  it("catches rejected renderer retries and keeps the retry control usable", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(createTerminalRenderer).mockImplementationOnce(() => {
      throw new Error("open failed");
    });
    control(root, "launch").click();
    await vi.waitFor(() => expect(root.textContent).toContain("open failed"));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    vi.mocked(createTerminalRenderer).mockImplementationOnce(() => {
      throw new Error("retry failed");
    });
    control(root, "retry-renderer", id).click();
    await vi.waitFor(() => expect(root.textContent).toContain("retry failed"));
    await vi.waitFor(() =>
      expect(control(root, "retry-renderer", id).disabled).toBe(false),
    );
    control(root, "retry-renderer", id).click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    expect(api.startSession).toHaveBeenCalledTimes(1);
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(1);
  });

  it("never resizes a PTY after the session has exited", async () => {
    const { root, app, api, exits } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    vi.spyOn(renderers[0].host, "getBoundingClientRect").mockReturnValue(
      new DOMRect(0, 0, 700, 300),
    );
    window.dispatchEvent(new Event("resize"));
    await vi.waitFor(() => expect(api.resizeSession).toHaveBeenCalledTimes(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    exits.get(id)?.({ id, reason: "code 0" });
    await vi.waitFor(() => expect(root.textContent).toContain("已结束"));
    window.dispatchEvent(new Event("resize"));
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(api.resizeSession).toHaveBeenCalledTimes(1);
  });

  it("restores focus to the actionable confirmation control after stop failure", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() => expect(renderers).toHaveLength(1));
    const id = vi.mocked(api.startSession).mock.calls[0][0].sessionId;
    vi.mocked(api.stopSession).mockRejectedValueOnce(new Error("stop refused"));
    control(root, "close-session", id).click();
    const confirm = control(root, "confirm-close-session", id);
    confirm.focus();
    confirm.click();
    await vi.waitFor(() => expect(root.textContent).toContain("stop refused"));
    expect(document.activeElement).toBe(
      control(root, "confirm-close-session", id),
    );
    expect(control(root, "confirm-close-session", id).disabled).toBe(false);
    expect(root.querySelectorAll('[role="tab"]')).toHaveLength(1);
  });
});
