import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import ProjectList from "./ProjectList";
import type { Project } from "./types";

const PROJECTS: Project[] = [
  { id: "/work/agent-term", name: "agent-term", path: "/work/agent-term" },
  { id: "/work/website", name: "website", path: "/work/website" },
];

const BASE_PROPS = {
  busy: false,
  error: null,
  pendingRemoval: null,
  onAdd: () => {},
  onRequestRemove: () => {},
  onCancelRemove: () => {},
  onConfirmRemove: () => {},
};

describe("ProjectList", () => {
  it("guides the user when there is no project", () => {
    const html = renderToStaticMarkup(
      <ProjectList projects={[]} {...BASE_PROPS} />,
    );

    expect(html).toContain("添加项目");
    expect(html).toContain("选择一个本地文件夹");
  });

  it("lists every project with its path", () => {
    const html = renderToStaticMarkup(
      <ProjectList projects={PROJECTS} {...BASE_PROPS} />,
    );

    expect(html).toContain("agent-term");
    expect(html).toContain("/work/agent-term");
    expect(html).toContain("website");
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

  it("shows project errors", () => {
    const html = renderToStaticMarkup(
      <ProjectList projects={[]} {...BASE_PROPS} error="无法添加项目" />,
    );

    expect(html).toContain('role="alert"');
    expect(html).toContain("无法添加项目");
  });
});
