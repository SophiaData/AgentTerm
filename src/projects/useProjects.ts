import { useCallback, useEffect, useState } from "react";
import type { Project } from "./types";

function errorMessage(error: unknown, fallback: string): string {
  const detail = error instanceof Error ? error.message : String(error);

  return detail ? `${fallback}：${detail}` : fallback;
}

export function useProjects() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pendingRemovalId, setPendingRemovalId] = useState<string | null>(null);

  const pendingRemoval =
    projects.find((project) => project.id === pendingRemovalId) ?? null;

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const existing = await invoke<Project[]>("list_projects");
      setProjects(existing);
    } catch (error) {
      setError(errorMessage(error, "无法读取项目列表"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const add = useCallback(async () => {
    setBusy(true);
    setError(null);

    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        directory: true,
        multiple: false,
        title: "选择项目文件夹",
      });

      if (typeof selected !== "string") {
        return;
      }

      const { invoke } = await import("@tauri-apps/api/core");
      const project = await invoke<Project>("add_project", { path: selected });
      setProjects((previous) => [...previous, project]);
    } catch (error) {
      setError(errorMessage(error, "无法添加项目"));
    } finally {
      setBusy(false);
    }
  }, []);

  const requestRemove = useCallback((id: string) => {
    setError(null);
    setPendingRemovalId(id);
  }, []);

  const cancelRemove = useCallback(() => {
    setPendingRemovalId(null);
  }, []);

  const confirmRemove = useCallback(async () => {
    if (!pendingRemovalId) {
      return;
    }

    setBusy(true);
    setError(null);

    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke<Project>("remove_project", { id: pendingRemovalId });
      setProjects((previous) =>
        previous.filter((project) => project.id !== pendingRemovalId),
      );
      setPendingRemovalId(null);
    } catch (error) {
      setError(errorMessage(error, "无法移除项目"));
    } finally {
      setBusy(false);
    }
  }, [pendingRemovalId]);

  return {
    projects,
    busy,
    loading,
    error,
    pendingRemoval,
    add,
    requestRemove,
    cancelRemove,
    confirmRemove,
    refresh: load,
  };
}
