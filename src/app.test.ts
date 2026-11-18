// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { createApp } from "./app";
import type { Backend } from "./api";
import type { Project, ToolStatus } from "./types";

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
    launchNativeTool: vi.fn().mockResolvedValue({ launched: true }),
  };
  const root = document.createElement("div");
  document.body.append(root);
  const app = createApp(root, api);
  return { root, app, api };
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
  document.body.replaceChildren();
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
      expect(api.launchNativeTool).toHaveBeenCalledWith("codex", projectB.path),
    );
    expect(root.textContent).toContain("已请求在系统终端启动 Codex");
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
    vi.mocked(api.launchNativeTool).mockRejectedValueOnce(
      new Error("系统终端不可用"),
    );
    await app.ready;
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("系统终端不可用"),
    );
    expect(root.textContent).not.toContain("已请求在系统终端启动");
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

  it("keeps native terminal launch disabled when nothing is installed", async () => {
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
    expect(api.launchNativeTool).not.toHaveBeenCalled();
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
    expect(api.launchNativeTool).toHaveBeenCalledWith("claude", projectA.path);
    expect(api.launchNativeTool).toHaveBeenCalledTimes(1);
  });

  it("retains usable tool choices when refresh fails", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.detectTools).mockRejectedValueOnce(new Error("检测失败"));
    control(root, "refresh-tools").click();
    await vi.waitFor(() => expect(root.textContent).toContain("检测失败"));
    expect(control(root, "select-tool", "claude").disabled).toBe(false);
  });

  it("ignores user launch during an in-flight native startup", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    let finish!: (result: { launched: boolean }) => void;
    vi.mocked(api.launchNativeTool).mockImplementationOnce(
      () =>
        new Promise<{ launched: boolean }>((resolve) => {
          finish = resolve;
        }),
    );
    control(root, "launch").click();
    expect(control(root, "launch").disabled).toBe(true);
    control(root, "launch").click();
    expect(api.launchNativeTool).toHaveBeenCalledTimes(1);
    finish({ launched: true });
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

  it("shows a non-blocking warning after a successful native launch", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    vi.mocked(api.launchNativeTool).mockResolvedValueOnce({
      launched: true,
      warning: "最近工具未保存",
    });
    control(root, "launch").click();
    await vi.waitFor(() =>
      expect(root.textContent).toContain("最近工具未保存"),
    );
    expect(root.textContent).not.toContain("无法启动终端");
  });

  it("never remounts UI after destroy during an unfinished launch", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    let resolve!: (value: { launched: boolean }) => void;
    vi.mocked(api.launchNativeTool).mockImplementationOnce(
      () =>
        new Promise<{ launched: boolean }>((done) => {
          resolve = done;
        }),
    );
    control(root, "launch").click();
    app.destroy();
    resolve({ launched: true });
    await new Promise((done) => setTimeout(done, 0));
    expect(root.childElementCount).toBe(0);
  });

  it("removes the listener on destroy", async () => {
    const { root, app, api } = fixture();
    await app.ready;
    const launch = control(root, "launch");
    app.destroy();
    launch.click();
    expect(api.launchNativeTool).not.toHaveBeenCalled();
    expect(root.childElementCount).toBe(0);
  });
});
