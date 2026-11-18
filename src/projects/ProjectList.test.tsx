import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import ProjectList from "./ProjectList";
import type { Project } from "./types";
import type { ToolStatus } from "../tools/types";

const PROJECTS: Project[] = [
  { id: "/work/agent-term", name: "agent-term", path: "/work/agent-term" },
  { id: "/work/website", name: "website", path: "/work/website" },
];

const INSTALLED: ToolStatus[] = [
  {
    id: "claude",
    name: "Claude Code",
    state: { kind: "installed", path: "/bin/claude", version: "2.0.0" },
  },
  { id: "codex", name: "Codex", state: { kind: "missing" } },
];

const BASE_PROPS = {
  tools: INSTALLED,
  busy: false,
  error: null,
  startError: null,
  pendingRemoval: null,
  onAdd: () => {},
  onRequestRemove: () => {},
  onCancelRemove: () => {},
  onConfirmRemove: () => {},
  onStart: () => {},
};

describe("ProjectList", () => {
  it("guides the user when there is no project", () => {
    const html = renderToStaticMarkup(
      <ProjectList projects={[]} {...BASE_PROPS} />,
    );

    expect(html).toContain("添加项目");
    expect(html).toContain("选择一个本地文件夹");
  });

  it("lists every project with a start button", () => {
    const html = renderToStaticMarkup(
      <ProjectList projects={PROJECTS} {...BASE_PROPS} />,
    );

    expect(html).toContain("agent-term");
    expect(html).toContain("/work/agent-term");
    expect(html).toContain("启动");
    expect(html).toContain("移除");
  });

  it("asks for confirmation without deleting files", () => {
    const html = renderToStaticMarkup(
      <ProjectList
        projects={PROJECTS}
        {...BASE_PROPS}
        pendingRemoval={PROJECTS[0]}
      />,
    );

    expect(html).toContain("只移除记录，不删除文件夹");
    expect(html).toContain("确认移除");
    expect(html).toContain("取消");
  });

  it("shows start errors", () => {
    const html = renderToStaticMarkup(
      <ProjectList
        projects={PROJECTS}
        {...BASE_PROPS}
        startError="无法启动会话"
      />,
    );

    expect(html).toContain('role="alert"');
    expect(html).toContain("无法启动会话");
  });
});
