// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ProjectFilesView } from "./ProjectFiles";
import { createFileBrowserStore, type FileBrowserState } from "./fileBrowser";

afterEach(cleanup);

describe("project file search", () => {
  it("distinguishes no match from a genuinely empty loaded scope", async () => {
    const project = { id: "p", name: "项目", path: "E:\\项目" };
    const store = createFileBrowserStore(
      project.id,
      async <T,>() => undefined as T,
    );
    const state: Partial<FileBrowserState> = {
      directories: {
        "": {
          entries: [
            { name: "src", path: "src", kind: "directory", bytes: null },
            { name: "README.md", path: "README.md", kind: "file", bytes: 10 },
          ],
          loaded: true,
          loading: false,
          error: null,
          truncated: false,
        },
      },
    };
    render(
      <ProjectFilesView
        project={project}
        state={{ ...store.getSnapshot(), ...state }}
        store={store}
        onClose={vi.fn()}
      />,
    );
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("搜索文件名"), "not-found");
    expect(screen.getByText("已加载范围无匹配")).toBeTruthy();
    expect(screen.queryByText("空目录")).toBeNull();
    expect(screen.getByText(/搜索仅覆盖已加载目录/)).toBeTruthy();
  });
});
