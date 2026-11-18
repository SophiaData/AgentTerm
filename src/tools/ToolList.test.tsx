import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import ToolList from "./ToolList";
import { badgeLabel, detailText, type ToolStatus } from "./types";

const SAMPLE_TOOLS: ToolStatus[] = [
  {
    id: "claude",
    name: "Claude Code",
    state: {
      kind: "installed",
      path: "/opt/homebrew/bin/claude",
      version: "2.0.14",
    },
  },
  { id: "codex", name: "Codex", state: { kind: "missing" } },
  {
    id: "agy",
    name: "Agy",
    state: { kind: "notExecutable", reason: "已找到 /tmp/agy，但没有执行权限" },
  },
];

describe("tool status labels", () => {
  it("names all three states", () => {
    expect(
      badgeLabel({ kind: "installed", path: "/bin/claude", version: "1.0.0" }),
    ).toBe("已安装");
    expect(badgeLabel({ kind: "missing" })).toBe("未安装");
    expect(badgeLabel({ kind: "notExecutable", reason: "没有执行权限" })).toBe(
      "无法执行",
    );
  });

  it("explains each state", () => {
    expect(
      detailText({ kind: "installed", path: "/bin/claude", version: "2.0.14" }),
    ).toBe("版本 2.0.14");
    expect(
      detailText({ kind: "installed", path: "/bin/claude", version: null }),
    ).toBe("已找到：/bin/claude");
    expect(detailText({ kind: "missing" })).toContain("未在 PATH");
    expect(detailText({ kind: "notExecutable", reason: "权限不足" })).toBe(
      "权限不足",
    );
  });
});

describe("ToolList", () => {
  it("renders the three tools with their states", () => {
    const html = renderToStaticMarkup(
      <ToolList
        tools={SAMPLE_TOOLS}
        loading={false}
        error={null}
        onRefresh={() => {}}
      />,
    );

    expect(html).toContain("Claude Code");
    expect(html).toContain("版本 2.0.14");
    expect(html).toContain("未安装");
    expect(html).toContain("无法执行");
    expect(html).toContain("重新检测");
  });

  it("shows progress while the first detection runs", () => {
    const html = renderToStaticMarkup(
      <ToolList tools={[]} loading error={null} onRefresh={() => {}} />,
    );

    expect(html).toContain("正在检测");
    expect(html).toContain("检测中…");
    expect(html).toContain("disabled");
  });

  it("surfaces detection errors", () => {
    const html = renderToStaticMarkup(
      <ToolList
        tools={[]}
        loading={false}
        error="无法读取工具状态"
        onRefresh={() => {}}
      />,
    );

    expect(html).toContain('role="alert"');
    expect(html).toContain("无法读取工具状态");
  });
});
