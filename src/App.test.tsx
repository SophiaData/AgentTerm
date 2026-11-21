import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { isSupportedTarget, PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";
import * as launcherModule from "./terminal/useLauncher";
import * as historyModule from "./terminal/useSessionHistory";
import * as toolsModule from "./tools/useTools";

afterEach(() => vi.restoreAllMocks());

describe("AgentTerm application shell", () => {
  it("starts with projects and an agent session, keeping records optional", () => {
    const html = renderToStaticMarkup(<App />);

    expect(html).toContain(PRODUCT_NAME);
    expect(html).toContain("项目与会话");
    expect(html).toContain("Agent 会话工作区");
    expect(html).toContain("从一个项目开始");
    expect(html).toContain("任务记录");
    expect(html).toContain("工具状态");
    expect(html).toContain("重新检测 CLI 工具");
    expect(html).not.toContain("启用任务记录");
    expect(html).not.toContain("保存任务计划");
    expect(html).not.toContain("正在读取任务");
    expect(html).not.toContain("Tauri 2");
  });

  it("does not imply that existing external terminal sessions are attached", () => {
    const html = renderToStaticMarkup(<App />);
    expect(html).toContain("外部终端中的已有会话尚未接管");
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain('aria-label="任务记录与交付"');
  });

  it("declares exactly the macOS arm64 and Windows x64 targets", () => {
    expect(SUPPORTED_TARGETS.map((target) => target.id)).toEqual([
      "macos-arm64",
      "windows-x64",
    ]);
    expect(isSupportedTarget("macos-arm64")).toBe(true);
    expect(isSupportedTarget("macos-x64")).toBe(false);
  });

  it("keeps both terminal panels mounted and hides only the unselected session", async () => {
    const store = launcherModule.createSessionStore(
      async <T,>(_command: string, args: Record<string, unknown>) =>
        ({
          id: args.sessionId,
          toolId: args.toolId,
          toolName: "Claude Code",
          projectPath: args.projectPath,
        }) as T,
    );
    const project = { id: "project", name: "Data", path: "E:\\Data" };
    const tool = {
      id: "claude",
      name: "Claude Code",
      state: { kind: "installed" as const, path: "claude.exe", version: null },
    };
    await store.start(project, tool);
    await store.start(project, tool);
    const state = store.getSnapshot();
    const [first, second] = state.sessions;
    const mock = vi.spyOn(launcherModule, "useLauncher");
    const render = () => {
      const current = store.getSnapshot();
      mock.mockReturnValue({
        ...store,
        ...current,
        session:
          current.sessions.find((s) => s.info.id === current.activeId) ?? null,
        busy: false,
      });
      return renderToStaticMarkup(<App />);
    };
    const html = render();
    expect(html.match(/role="tabpanel"/g)).toHaveLength(2);
    expect(html).toContain(
      `id="pane-${first.info.id}" role="tabpanel" aria-labelledby="tab-${first.info.id}" hidden=""`,
    );
    expect(html).toContain(
      `id="pane-${second.info.id}" role="tabpanel" aria-labelledby="tab-${second.info.id}"`,
    );
    expect(html.match(/aria-selected="true"/g)).toHaveLength(1);
    store.showLauncher();
    const launcherHtml = render();
    expect(launcherHtml.match(/role="tabpanel"/g)).toHaveLength(2);
    expect(
      launcherHtml.match(/role="tabpanel" aria-labelledby="[^"]+" hidden=""/g),
    ).toHaveLength(2);
    expect(launcherHtml).toContain("从一个项目开始");
  });

  it("shows recent closed sessions on launcher with honest reopen controls and error recovery", async () => {
    vi.spyOn(toolsModule, "useTools").mockReturnValue({
      tools: [
        {
          id: "claude",
          name: "Claude Code",
          state: { kind: "installed", path: "claude.exe", version: "2.1.326" },
        },
        {
          id: "codex",
          name: "Codex",
          state: { kind: "missing" },
        },
      ],
      loading: false,
      error: null,
      refresh: vi.fn(),
    });
    const store = launcherModule.createSessionStore(
      async <T,>(_command: string, args: Record<string, unknown>) =>
        ({
          id: args.sessionId,
          toolId: args.toolId,
          toolName: "Claude Code",
          projectPath: args.projectPath,
        }) as T,
    );
    const project = { id: "E:\\Data", name: "Data", path: "E:\\Data" };
    const claudeTool = {
      id: "claude",
      name: "Claude Code",
      state: { kind: "installed" as const, path: "claude.exe", version: null },
    };
    await store.start(project, claudeTool);
    const activeId = store.getSnapshot().sessions[0].info.id;
    store.showLauncher();
    const current = store.getSnapshot();
    vi.spyOn(launcherModule, "useLauncher").mockReturnValue({
      ...store,
      ...current,
      session: null,
      busy: false,
    });
    vi.spyOn(historyModule, "useSessionHistory").mockReturnValue({
      records: [
        {
          id: activeId,
          toolId: "claude",
          toolName: "Claude Code",
          projectPath: "E:\\Data",
          startedAtMs: 1_763_650_000_000,
          endedAtMs: null,
          exitReason: null,
          outputBytes: null,
        },
        {
          id: "closed-ok",
          toolId: "claude",
          toolName: "Claude Code",
          projectPath: "E:\\AgentTerm",
          startedAtMs: 1_763_649_000_000,
          endedAtMs: 1_763_649_500_000,
          exitReason: "已停止",
          outputBytes: null,
        },
        {
          id: "crashed-missing",
          toolId: "codex",
          toolName: "Codex",
          projectPath: "E:\\Other",
          startedAtMs: 1_763_648_000_000,
          endedAtMs: null,
          exitReason: null,
          outputBytes: null,
        },
      ],
      loading: false,
      error: "无法读取最近会话：历史校验失败",
      reload: vi.fn(),
    });

    const html = renderToStaticMarkup(<App />);
    expect(html).toContain('aria-label="最近会话"');
    expect(html).toContain("无法读取最近会话：历史校验失败");
    expect(html).toContain("重新读取");
    expect(html).toContain("Claude Code · AgentTerm");
    expect(html).toContain("已结束 · 已停止");
    expect(html).toContain("Codex · Other");
    expect(html).toContain("上次未正常退出");
    expect(html).not.toContain("Claude Code · Data");
    expect(html).toContain("再次打开会按原项目与工具新开终端");
    expect(html).toContain('title="Codex未安装"');
  });
});
