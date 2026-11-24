import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import SessionWorkspace, { clampSplitRatio } from "./SessionWorkspace";
import type { ActiveSession } from "./useLauncher";

const sessions: ActiveSession[] = ["a", "b", "c"].map((id) => ({
  info: {
    id,
    toolId: "claude",
    toolName: "Claude Code",
    projectPath: "E:\\Data",
  },
  label: `CLI ${id}`,
  titleSource: "default",
  running: true,
  exitReason: null,
  closeError: null,
}));

function render(paneIds: string[], activeId: string | null) {
  return renderToStaticMarkup(
    <SessionWorkspace
      sessions={sessions}
      paneIds={paneIds}
      activeId={activeId}
      focusRequest={0}
      closingId={null}
      onFocus={vi.fn()}
      onClose={vi.fn()}
      onExit={vi.fn()}
      onRename={vi.fn(async (title) => ({ ok: true as const, title }))}
      onPromptLine={vi.fn()}
    />,
  );
}

describe("terminal split presentation", () => {
  it("keeps every terminal mounted, displaying the pair with exactly one input target", () => {
    const html = render(["a", "b"], "b");
    expect(html.match(/role="tabpanel"/g)).toHaveLength(3);
    expect(html).toContain(
      'id="pane-c" role="tabpanel" aria-labelledby="tab-c" hidden=""',
    );
    expect(html).not.toContain(
      'id="pane-a" role="tabpanel" aria-labelledby="tab-a" hidden',
    );
    expect(html).not.toContain(
      'id="pane-b" role="tabpanel" aria-labelledby="tab-b" hidden',
    );
    expect(html.match(/data-focused="true"/g)).toHaveLength(1);
    expect(html).toContain('aria-label="调整终端分屏"');
    expect(html).toContain('aria-valuenow="50"');
    expect(html).toContain('aria-orientation="vertical"');
    expect(html).toContain("点击修改名称");
  });

  it("single view removes the divider but retains the other mounted terminals", () => {
    const html = render(["b"], "b");
    expect(html.match(/role="tabpanel"/g)).toHaveLength(3);
    expect(
      html.match(/role="tabpanel" aria-labelledby="[^"]+" hidden/g),
    ).toHaveLength(2);
    expect(html).not.toContain('role="separator"');
  });

  it("the launcher hides both panes without discarding their terminal components", () => {
    const html = render(["a", "b"], null);
    expect(html).toContain('hidden="" aria-label="双终端工作区"');
    expect(
      html.match(/role="tabpanel" aria-labelledby="[^"]+" hidden/g),
    ).toHaveLength(3);
    expect(html).not.toContain('data-focused="true"');
    expect(html.match(/aria-label="CLI 输入与输出"/g)).toHaveLength(3);
  });

  it("keeps both panes usable at the drag limits and ignores invalid measurements", () => {
    expect(clampSplitRatio(-100)).toBe(30);
    expect(clampSplitRatio(500)).toBe(70);
    expect(clampSplitRatio(48)).toBe(48);
    expect(clampSplitRatio(Number.NaN)).toBe(50);
    expect(clampSplitRatio(Number.POSITIVE_INFINITY)).toBe(50);
  });
});
