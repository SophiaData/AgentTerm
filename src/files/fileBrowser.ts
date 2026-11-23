export type FileEntry = {
  name: string;
  path: string;
  kind: "directory" | "file" | "unavailable";
  bytes: number | null;
};

export type DirectoryListing = { entries: FileEntry[]; truncated: boolean };
export type FilePreview = { path: string; content: string; bytes: number };
type DirectoryState = DirectoryListing & {
  loading: boolean;
  loaded: boolean;
  error: string | null;
};
export type FileBrowserState = {
  directories: Record<string, DirectoryState>;
  expanded: string[];
  selectedPath: string | null;
  preview: FilePreview | null;
  previewLoading: boolean;
  previewError: string | null;
  refreshing: boolean;
  revealing: boolean;
  revealError: string | null;
};
type Invoke = <T>(command: string, args: Record<string, unknown>) => Promise<T>;
const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

/** One store per project: responses from an old project cannot enter a new one. */
export function createFileBrowserStore(projectId: string, invoke: Invoke) {
  let state: FileBrowserState = {
    directories: {},
    expanded: [""],
    selectedPath: null,
    preview: null,
    previewLoading: false,
    previewError: null,
    refreshing: false,
    revealing: false,
    revealError: null,
  };
  let sequence = 0;
  let previewRequest = 0;
  const directoryRequests = new Map<string, number>();
  const listeners = new Set<() => void>();
  const update = (patch: Partial<FileBrowserState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) listener();
  };
  const updateDirectory = (path: string, directory: DirectoryState) =>
    update({ directories: { ...state.directories, [path]: directory } });

  const loadDirectory = async (path: string) => {
    const request = ++sequence;
    directoryRequests.set(path, request);
    const previous = state.directories[path];
    updateDirectory(path, {
      entries: previous?.entries ?? [],
      truncated: previous?.truncated ?? false,
      loaded: previous?.loaded ?? false,
      loading: true,
      error: null,
    });
    try {
      const listing = await invoke<DirectoryListing>("list_project_files", {
        projectId,
        relativePath: path,
      });
      if (directoryRequests.get(path) !== request) return;
      updateDirectory(path, {
        ...listing,
        loaded: true,
        loading: false,
        error: null,
      });
    } catch (error) {
      if (directoryRequests.get(path) !== request) return;
      updateDirectory(path, {
        entries: [],
        truncated: false,
        loaded: false,
        loading: false,
        error: message(error),
      });
    }
  };

  const selectFile = async (path: string) => {
    const request = ++previewRequest;
    update({
      selectedPath: path,
      preview: null,
      previewLoading: true,
      previewError: null,
      revealError: null,
    });
    try {
      const preview = await invoke<FilePreview>("preview_project_file", {
        projectId,
        relativePath: path,
      });
      if (request !== previewRequest) return;
      if (preview.path !== path) throw new Error("文件位置不一致，请重新选择");
      update({ preview, previewLoading: false });
    } catch (error) {
      if (request !== previewRequest) return;
      update({ previewError: message(error), previewLoading: false });
    }
  };

  return {
    getSnapshot: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    loadDirectory,
    selectFile,
    toggleDirectory(path: string) {
      if (state.expanded.includes(path)) {
        update({ expanded: state.expanded.filter((item) => item !== path) });
      } else {
        update({ expanded: [...state.expanded, path] });
        if (!state.directories[path]?.loaded) void loadDirectory(path);
      }
    },
    closePreview() {
      previewRequest += 1;
      update({
        selectedPath: null,
        preview: null,
        previewLoading: false,
        previewError: null,
      });
    },
    async refresh() {
      if (state.refreshing) return;
      update({ refreshing: true });
      try {
        for (const path of [...state.expanded]) await loadDirectory(path);
        if (state.selectedPath !== null) await selectFile(state.selectedPath);
      } finally {
        update({ refreshing: false });
      }
    },
    async reveal(path: string) {
      if (state.revealing) return;
      update({ revealing: true, revealError: null });
      try {
        await invoke<void>("reveal_project_file", {
          projectId,
          relativePath: path,
        });
      } catch (error) {
        update({ revealError: message(error) });
      } finally {
        update({ revealing: false });
      }
    },
  };
}

export type FileBrowserStore = ReturnType<typeof createFileBrowserStore>;
