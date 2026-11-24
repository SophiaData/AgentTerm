import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ProjectFilesView } from "./ProjectFiles";
import { createFileBrowserStore, type FileBrowserState } from "./fileBrowser";

const project = { id: "project", name: "项目", path: String.raw`\\?\E:\项目` };
const store = createFileBrowserStore(
  project.id,
  async <T,>() => undefined as T,
);
function render(patch: Partial<FileBrowserState>) {
  return renderToStaticMarkup(
    <ProjectFilesView
      project={project}
      state={{ ...store.getSnapshot(), ...patch }}
      store={store}
      onClose={vi.fn()}
    />,
  );
}

describe("project file panel", () => {
  it("shows a collapsible directory and a regular path without requiring a task", () => {
    const html = render({
      directories: {
        "": {
          entries: [
            {
              name: "SQL 文档",
              path: "SQL 文档",
              kind: "directory",
              bytes: null,
            },
            { name: "README.md", path: "README.md", kind: "file", bytes: 20 },
            { name: "link", path: "link", kind: "unavailable", bytes: null },
          ],
          loaded: true,
          loading: false,
          error: null,
          truncated: false,
        },
      },
    });
    expect(html).toContain('aria-label="项目文件"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain("SQL 文档");
    expect(html).toContain("在系统资源管理器中打开");
    expect(html).toContain("点击文件查看内容");
    expect(html).toContain('title="E:\\项目"');
    expect(html).not.toContain(String.raw`\\?\E:`);
    expect(html).toContain('file-unavailable" disabled=""');
  });

  it("renders HTML and script content as plain text instead of executing it", () => {
    const html = render({
      selectedPath: "page.html",
      preview: {
        path: "page.html",
        content: '<script>alert("中文")</script>',
        bytes: 34,
      },
    });
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).toContain("只读");
    expect(html).toContain("打开所在目录");
  });

  it("explains preview errors and offers a retry without retaining stale text", () => {
    const html = render({
      selectedPath: "large.bin",
      previewError: "文件超过 1 MiB",
    });
    expect(html).toContain('role="alert"');
    expect(html).toContain("重试");
    expect(html).not.toContain("<code>");
  });
});
