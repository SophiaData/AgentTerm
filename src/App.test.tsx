import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { isSupportedTarget, PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";
import * as launcherModule from "./terminal/useLauncher";

afterEach(() => vi.restoreAllMocks());

describe("AgentTerm application shell", () => {
  it("starts with projects and an agent session, keeping records optional", () => {
    const html = renderToStaticMarkup(<App />);

    expect(html).toContain(PRODUCT_NAME);
    expect(html).toContain("项目与会话");
    expect(html).toContain("Agent 会话工作区");
    expect(html).toContain("从一个项目开始");
    expect(html).toContain("任务记录");
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
});
