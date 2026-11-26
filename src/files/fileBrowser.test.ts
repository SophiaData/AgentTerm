import { describe, expect, it, vi } from "vitest";
import {
  createFileBrowserStore,
  type DirectoryListing,
  type FilePreview,
} from "./fileBrowser";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function fixture(projectId = "project-a") {
  const invoke =
    vi.fn<
      (command: string, args: Record<string, unknown>) => Promise<unknown>
    >();
  const store = createFileBrowserStore(
    projectId,
    async <T>(command: string, args: Record<string, unknown>) =>
      (await invoke(command, args)) as T,
  );
  return { store, invoke };
}
const listing: DirectoryListing = {
  entries: [
    { name: "SQL 文档", path: "SQL 文档", kind: "directory", bytes: null },
  ],
  truncated: false,
};

describe("project file browser", () => {
  it("loads only the requested directory and never previews a file automatically", async () => {
    const { store, invoke } = fixture();
    invoke.mockResolvedValue(listing);
    await store.loadDirectory("");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("list_project_files", {
      projectId: "project-a",
      relativePath: "",
    });
    expect(store.getSnapshot().preview).toBeNull();
    expect(store.getSnapshot().selectedPath).toBeNull();
    store.toggleDirectory("SQL 文档");
    expect(invoke).toHaveBeenLastCalledWith("list_project_files", {
      projectId: "project-a",
      relativePath: "SQL 文档",
    });
    store.toggleDirectory("SQL 文档");
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("reuses the same pending request when a folder is reopened", async () => {
    const { store, invoke } = fixture();
    const pending = deferred<DirectoryListing>();
    invoke.mockReturnValueOnce(pending.promise);

    store.toggleDirectory("SQL 文档");
    store.toggleDirectory("SQL 文档");
    store.toggleDirectory("SQL 文档");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("list_project_files", {
      projectId: "project-a",
      relativePath: "SQL 文档",
    });
    pending.resolve(listing);
    await vi.waitFor(() =>
      expect(store.getSnapshot().directories["SQL 文档"].loaded).toBe(true),
    );
  });

  it("does not replace a newer selected file with a slow earlier response", async () => {
    const { store, invoke } = fixture();
    const first = deferred<FilePreview>();
    const second = deferred<FilePreview>();
    invoke
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const a = store.selectFile("a.sql");
    const b = store.selectFile("b.sql");
    second.resolve({ path: "b.sql", content: "select 2;", bytes: 9 });
    await b;
    first.resolve({ path: "a.sql", content: "select 1;", bytes: 9 });
    await a;
    expect(store.getSnapshot().selectedPath).toBe("b.sql");
    expect(store.getSnapshot().preview?.content).toBe("select 2;");
  });

  it("closing a preview invalidates pending reads and errors", async () => {
    const { store, invoke } = fixture();
    const pending = deferred<FilePreview>();
    invoke.mockReturnValue(pending.promise);
    const request = store.selectFile("a.sql");
    store.closePreview();
    pending.reject(new Error("late error"));
    await request;
    expect(store.getSnapshot()).toMatchObject({
      selectedPath: null,
      preview: null,
      previewError: null,
      previewLoading: false,
    });
  });

  it("clears old content when a deleted or binary file cannot be previewed", async () => {
    const { store, invoke } = fixture();
    invoke.mockResolvedValueOnce({
      path: "a.sql",
      content: "select 1;",
      bytes: 9,
    });
    await store.selectFile("a.sql");
    invoke.mockRejectedValueOnce(new Error("文件不存在"));
    await store.selectFile("missing.sql");
    expect(store.getSnapshot()).toMatchObject({
      selectedPath: "missing.sql",
      preview: null,
      previewLoading: false,
      previewError: "文件不存在",
    });
  });

  it("ignores stale directory replies and allows retry after a read failure", async () => {
    const { store, invoke } = fixture();
    const first = deferred<DirectoryListing>();
    invoke.mockReturnValueOnce(first.promise).mockResolvedValueOnce(listing);
    const a = store.loadDirectory("");
    await store.loadDirectory("");
    first.reject(new Error("stale"));
    await a;
    expect(store.getSnapshot().directories[""]).toMatchObject({
      ...listing,
      error: null,
    });
    invoke.mockRejectedValueOnce(new Error("访问失败"));
    await store.loadDirectory("");
    expect(store.getSnapshot().directories[""].entries).toEqual([]);
    invoke.mockResolvedValueOnce(listing);
    await store.loadDirectory("");
    expect(store.getSnapshot().directories[""].error).toBeNull();
  });

  it("refreshes visible directories and the selected preview to show CLI edits", async () => {
    const { store, invoke } = fixture();
    invoke.mockResolvedValueOnce(listing).mockResolvedValueOnce({
      path: "README.md",
      content: "before",
      bytes: 6,
    });
    await store.loadDirectory("");
    await store.selectFile("README.md");
    invoke
      .mockResolvedValueOnce({ entries: [], truncated: false })
      .mockResolvedValueOnce({ path: "README.md", content: "after", bytes: 5 });
    await store.refresh();
    expect(store.getSnapshot().preview?.content).toBe("after");
    expect(store.getSnapshot().refreshing).toBe(false);
    expect(invoke.mock.calls.slice(2).map(([command]) => command)).toEqual([
      "list_project_files",
      "preview_project_file",
    ]);
  });

  it("keeps projects isolated even when the prior project is still reading", async () => {
    const first = fixture("project-a");
    const second = fixture("project-b");
    const pending = deferred<FilePreview>();
    first.invoke.mockReturnValue(pending.promise);
    const request = first.store.selectFile("README.md");
    second.invoke.mockResolvedValue(listing);
    await second.store.loadDirectory("");
    pending.resolve({
      path: "README.md",
      content: "only project a",
      bytes: 14,
    });
    await request;
    expect(second.store.getSnapshot().preview).toBeNull();
    expect(second.invoke).toHaveBeenCalledWith("list_project_files", {
      projectId: "project-b",
      relativePath: "",
    });
  });

  it("opens the system file manager only through a project-scoped explicit request", async () => {
    const { store, invoke } = fixture();
    invoke.mockRejectedValueOnce(new Error("无法打开"));
    await store.reveal("SQL 文档/测试.sql");
    expect(invoke).toHaveBeenCalledWith("reveal_project_file", {
      projectId: "project-a",
      relativePath: "SQL 文档/测试.sql",
    });
    expect(store.getSnapshot()).toMatchObject({
      revealing: false,
      revealError: "无法打开",
    });
  });
});
