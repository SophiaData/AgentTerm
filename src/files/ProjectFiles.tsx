import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { displayPath } from "../projects/displayPath";
import type { Project } from "../projects/types";
import {
  createFileBrowserStore,
  type FileBrowserState,
  type FileBrowserStore,
} from "./fileBrowser";

function Directory({
  path,
  state,
  store,
  searchActive,
}: {
  path: string;
  state: FileBrowserState;
  store: FileBrowserStore;
  searchActive: boolean;
}) {
  const directory = state.directories[path];
  if (!directory) return <p className="file-message">正在读取目录…</p>;
  return (
    <>
      {directory.loading ? (
        <p className="file-message" role="status">
          正在读取目录…
        </p>
      ) : null}
      {directory.error ? (
        <div className="file-message file-error" role="alert">
          <p>{directory.error}</p>
          <button
            type="button"
            className="ghost-button"
            onClick={() => void store.loadDirectory(path)}
          >
            重试
          </button>
        </div>
      ) : null}
      <ul className="file-tree-list">
        {directory.entries.map((entry) => (
          <li key={entry.path}>
            <button
              type="button"
              className={`file-tree-entry file-${entry.kind}`}
              disabled={entry.kind === "unavailable"}
              aria-expanded={
                entry.kind === "directory"
                  ? state.expanded.includes(entry.path)
                  : undefined
              }
              aria-current={
                state.selectedPath === entry.path ? "true" : undefined
              }
              title={
                entry.kind === "unavailable"
                  ? `${entry.name}：链接或不可访问的文件，请在系统资源管理器中查看`
                  : entry.path
              }
              onClick={() => {
                if (entry.kind === "directory")
                  store.toggleDirectory(entry.path);
                else if (entry.kind === "file")
                  void store.selectFile(entry.path);
              }}
            >
              <span className="file-entry-icon" aria-hidden="true">
                {entry.kind === "directory"
                  ? state.expanded.includes(entry.path)
                    ? "▾"
                    : "▸"
                  : "·"}
              </span>
              <span className="file-entry-name">{entry.name}</span>
            </button>
            {entry.kind === "directory" &&
            state.expanded.includes(entry.path) ? (
              <div className="file-subdirectory">
                <Directory
                  path={entry.path}
                  state={state}
                  store={store}
                  searchActive={searchActive}
                />
              </div>
            ) : null}
          </li>
        ))}
      </ul>
      {!directory.loading && !directory.error && !directory.entries.length ? (
        <p className="file-message">
          {searchActive ? "已加载范围无匹配" : "空目录"}
        </p>
      ) : null}
      {directory.truncated ? (
        <p className="file-message">
          此目录仅显示前 1000 项，可用系统资源管理器查看完整内容。
        </p>
      ) : null}
    </>
  );
}

export function ProjectFilesView({
  project,
  state,
  store,
  onClose,
}: {
  project: Project;
  state: FileBrowserState;
  store: FileBrowserStore;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const matchingPaths = new Set(
    Object.values(state.directories)
      .flatMap((directory) => directory.entries)
      .filter((entry) =>
        entry.name.toLocaleLowerCase().includes(normalizedQuery),
      )
      .map((entry) => entry.path),
  );
  const visiblePaths = new Set(matchingPaths);
  for (const path of matchingPaths) {
    const parts = path.split("/");
    for (let index = 1; index < parts.length; index += 1) {
      visiblePaths.add(parts.slice(0, index).join("/"));
    }
  }
  const filtered = {
    ...state,
    directories: Object.fromEntries(
      Object.entries(state.directories).map(([path, directory]) => [
        path,
        {
          ...directory,
          entries:
            normalizedQuery.length === 0
              ? directory.entries
              : directory.entries.filter((entry) =>
                  visiblePaths.has(entry.path),
                ),
        },
      ]),
    ),
  };
  return (
    <aside className="workspace-files" id="project-files" aria-label="项目文件">
      <div className="file-panel-heading">
        <strong>项目文件</strong>
        <div className="file-panel-actions">
          <button
            type="button"
            className="ghost-button"
            disabled={state.refreshing}
            onClick={() => void store.refresh()}
          >
            {state.refreshing ? "刷新中…" : "刷新"}
          </button>
          <button
            type="button"
            className="sidebar-icon-button"
            aria-label="收起文件面板"
            onClick={onClose}
          >
            ×
          </button>
        </div>
      </div>
      <input
        className="file-search"
        aria-label="搜索文件名"
        placeholder="搜索文件名"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <div className="file-project-heading">
        <strong>{project.name}</strong>
        <small title={displayPath(project.path)}>
          {displayPath(project.path)}
        </small>
        <button
          type="button"
          className="ghost-button"
          disabled={state.revealing}
          onClick={() => void store.reveal("")}
        >
          在系统资源管理器中打开
        </button>
      </div>
      {state.revealError ? (
        <p className="file-message file-error" role="alert">
          {state.revealError}
        </p>
      ) : null}
      <nav className="file-tree" aria-label="项目目录">
        <Directory
          path=""
          state={filtered}
          store={store}
          searchActive={normalizedQuery.length > 0}
        />
      </nav>
      {normalizedQuery ? (
        <p className="file-message" role="status">
          搜索仅覆盖已加载目录；展开更多目录可获得更多匹配。
        </p>
      ) : null}
      <section className="file-preview" aria-label="文件预览">
        {state.selectedPath === null ? (
          <p className="file-preview-empty">点击文件查看内容</p>
        ) : (
          <>
            <div className="file-preview-heading">
              <strong title={state.selectedPath}>{state.selectedPath}</strong>
              <button
                type="button"
                className="sidebar-icon-button"
                aria-label="关闭文件预览"
                onClick={store.closePreview}
              >
                ×
              </button>
            </div>
            <div className="file-preview-actions">
              <small>
                {state.preview
                  ? `${state.preview.bytes} 字节 · 只读`
                  : "只读预览"}
              </small>
              <button
                type="button"
                className="ghost-button"
                disabled={state.revealing}
                onClick={() => void store.reveal(state.selectedPath!)}
              >
                打开所在目录
              </button>
            </div>
            {state.previewLoading ? (
              <p className="file-message" role="status">
                正在读取文件…
              </p>
            ) : state.previewError ? (
              <div className="file-message file-error" role="alert">
                <p>{state.previewError}</p>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => void store.selectFile(state.selectedPath!)}
                >
                  重试
                </button>
              </div>
            ) : state.preview ? (
              <pre tabIndex={0} aria-label={state.selectedPath}>
                <code>{state.preview.content || "（空文件）"}</code>
              </pre>
            ) : null}
          </>
        )}
      </section>
    </aside>
  );
}

export default function ProjectFiles({
  project,
  onClose,
}: {
  project: Project;
  onClose: () => void;
}) {
  const store = useMemo(
    () =>
      createFileBrowserStore(
        project.id,
        async <T,>(command: string, args: Record<string, unknown>) => {
          const { invoke } = await import("@tauri-apps/api/core");
          return invoke<T>(command, args);
        },
      ),
    [project.id],
  );
  const state = useSyncExternalStore(
    store.subscribe,
    store.getSnapshot,
    store.getSnapshot,
  );
  useEffect(() => {
    void store.loadDirectory("");
  }, [store]);
  return (
    <ProjectFilesView
      project={project}
      state={state}
      store={store}
      onClose={onClose}
    />
  );
}
