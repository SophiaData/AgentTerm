import { useMemo, useSyncExternalStore } from "react";
import type { Project } from "../projects/types";
import type { ToolStatus } from "../tools/types";
import { newSessionId, type SessionInfo } from "./types";
import { DEFAULT_ROWS, DEFAULT_COLS } from "./defaults";

export type ActiveSession = {
  info: SessionInfo;
  label: string;
  running: boolean;
  exitReason: string | null;
  closeError: string | null;
};

type SessionState = {
  sessions: ActiveSession[];
  activeId: string | null;
  starting: boolean;
  closingId: string | null;
  startError: string | null;
};

type Invoke = <T>(command: string, args: Record<string, unknown>) => Promise<T>;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Owns every open session; selecting a tab never starts or stops a process. */
export function createSessionStore(invoke: Invoke) {
  let state: SessionState = {
    sessions: [],
    activeId: null,
    starting: false,
    closingId: null,
    startError: null,
  };
  let sequence = 0;
  let operation = false;
  const listeners = new Set<() => void>();
  const update = (patch: Partial<SessionState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) listener();
  };
  const select = (id: string) => {
    if (state.sessions.some((s) => s.info.id === id)) {
      update({ activeId: id });
    }
  };
  const showLauncher = () => update({ activeId: null, startError: null });

  const start = async (project: Project, tool: ToolStatus) => {
    if (operation) return;
    operation = true;
    update({ starting: true, startError: null });
    const id = newSessionId();
    try {
      const info = await invoke<SessionInfo>("start_session", {
        sessionId: id,
        toolId: tool.id,
        projectPath: project.path,
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
      });
      const next: ActiveSession = {
        info,
        label: `${tool.name} · ${++sequence}`,
        running: true,
        exitReason: null,
        closeError: null,
      };
      update({ sessions: [...state.sessions, next], activeId: info.id });
    } catch (error) {
      update({ startError: `无法启动会话：${errorMessage(error)}` });
    } finally {
      operation = false;
      update({ starting: false });
    }
  };

  const handleExit = (id: string, reason: string) => {
    if (!state.sessions.some((s) => s.info.id === id)) return;
    update({
      sessions: state.sessions.map((s) =>
        s.info.id === id ? { ...s, running: false, exitReason: reason } : s,
      ),
    });
  };

  const close = async (id: string) => {
    const previous = state.sessions.find((s) => s.info.id === id);
    if (!previous || operation) return;
    operation = true;
    update({
      closingId: id,
      sessions: state.sessions.map((s) =>
        s.info.id === id ? { ...s, closeError: null } : s,
      ),
    });
    try {
      if (previous.running) {
        try {
          await invoke("stop_session", { sessionId: id });
        } catch (error) {
          const snapshot = await invoke<{ running: boolean }>(
            "session_snapshot",
            { sessionId: id },
          );
          if (snapshot.running) throw error;
        }
      }
      // Release archived output as well; the native command rejects live PTYs.
      await invoke("forget_session", { sessionId: id });
      const index = state.sessions.findIndex((s) => s.info.id === id);
      const remaining = state.sessions.filter((s) => s.info.id !== id);
      update({
        sessions: remaining,
        activeId:
          state.activeId === id
            ? (remaining[Math.min(index, remaining.length - 1)]?.info.id ??
              null)
            : state.activeId,
      });
    } catch (error) {
      update({
        sessions: state.sessions.map((s) =>
          s.info.id === id
            ? {
                ...s,
                closeError: `无法关闭会话：${errorMessage(error)}。请重试。`,
              }
            : s,
        ),
      });
    } finally {
      operation = false;
      update({ closingId: null });
    }
  };

  return {
    getSnapshot: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    select,
    showLauncher,
    start,
    close,
    handleExit,
  };
}

export function useLauncher() {
  const store = useMemo(
    () =>
      createSessionStore(
        async <T>(command: string, args: Record<string, unknown>) => {
          const { invoke } = await import("@tauri-apps/api/core");
          return invoke<T>(command, args);
        },
      ),
    [],
  );
  const state = useSyncExternalStore(
    store.subscribe,
    store.getSnapshot,
    store.getSnapshot,
  );
  return {
    ...state,
    session: state.sessions.find((s) => s.info.id === state.activeId) ?? null,
    busy: state.starting || state.closingId !== null,
    select: store.select,
    showLauncher: store.showLauncher,
    start: store.start,
    close: store.close,
    handleExit: store.handleExit,
  };
}
