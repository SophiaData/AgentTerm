import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import App from "./App";
import { isSupportedTarget, PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";

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
});
