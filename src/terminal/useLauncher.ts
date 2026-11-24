import { useMemo, useSyncExternalStore } from "react";
import type { Project } from "../projects/types";
import type { ToolStatus } from "../tools/types";
import {
  deriveAutoTitleFromPrompt,
  normalizeManualTitle,
} from "./sessionNaming";
import { newSessionId, type SessionInfo } from "./types";
import { DEFAULT_ROWS, DEFAULT_COLS } from "./defaults";

export type TitleSource = "default" | "auto" | "manual";

export type RenameResult =
  { ok: true; title: string } | { ok: false; error: string };

export type ActiveSession = {
  info: SessionInfo;
  label: string;
  titleSource: TitleSource;
  running: boolean;
  exitReason: string | null;
  closeError: string | null;
};

type SessionState = {
  sessions: ActiveSession[];
  activeId: string | null;
  paneIds: string[];
  focusRequest: number;
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
    paneIds: [],
    focusRequest: 0,
    starting: false,
    closingId: null,
    startError: null,
  };
  let sequence = 0;
  let operation = false;
  const renameEpochs = new Map<string, number>();
  const autoNaming = new Set<string>();
  const listeners = new Set<() => void>();
  const update = (patch: Partial<SessionState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) listener();
  };
  const select = (id: string) => {
    if (state.sessions.some((s) => s.info.id === id)) {
      const paneIds = state.paneIds.includes(id)
        ? state.paneIds
        : state.paneIds.length === 2
          ? state.paneIds.map((pane, index) =>
              index ===
              Math.max(
                0,
                state.paneIds.indexOf(state.activeId ?? state.paneIds[1]),
              )
                ? id
                : pane,
            )
          : [id];
      update({
        activeId: id,
        paneIds,
        focusRequest: state.focusRequest + 1,
      });
    }
  };
  // Focusing a visible pane must not steal focus from its buttons or selection.
  const focus = (id: string) => {
    if (
      state.activeId !== null &&
      state.activeId !== id &&
      state.paneIds.includes(id)
    )
      update({ activeId: id });
  };
  const split = (partnerId?: string) => {
    const active = state.sessions.find((s) => s.info.id === state.activeId);
    if (!active) return;
    const candidates = state.sessions.filter(
      (s) => s.info.id !== active.info.id,
    );
    const partner = partnerId
      ? candidates.find((s) => s.info.id === partnerId)
      : (candidates
          .filter((s) => s.info.projectPath === active.info.projectPath)
          .slice(-1)[0] ?? candidates[candidates.length - 1]);
    if (!partner) return;
    update({
      paneIds: [active.info.id, partner.info.id],
      focusRequest: state.focusRequest + 1,
    });
  };
  const single = () => {
    if (state.activeId)
      update({
        paneIds: [state.activeId],
        focusRequest: state.focusRequest + 1,
      });
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
        label: info.title?.trim() || `${tool.name} · ${++sequence}`,
        titleSource: info.title?.trim() ? "manual" : "default",
        running: true,
        exitReason: null,
        closeError: null,
      };
      update({ sessions: [...state.sessions, next] });
      select(info.id);
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

  const rename = async (
    id: string,
    rawTitle: string,
  ): Promise<RenameResult> => {
    if (!state.sessions.some((s) => s.info.id === id)) {
      return { ok: false, error: "会话不存在或已关闭" };
    }
    const checked = normalizeManualTitle(rawTitle);
    if (!checked.ok) return checked;
    const epoch = (renameEpochs.get(id) ?? 0) + 1;
    renameEpochs.set(id, epoch);
    try {
      const saved = await invoke<string>("rename_session", {
        sessionId: id,
        title: checked.title,
      });
      if (renameEpochs.get(id) === epoch) {
        update({
          sessions: state.sessions.map((s) =>
            s.info.id === id
              ? {
                  ...s,
                  label: saved,
                  titleSource: "manual",
                  info: { ...s.info, title: saved },
                }
              : s,
          ),
        });
      }
      return { ok: true, title: saved };
    } catch (error) {
      return { ok: false, error: `无法修改会话名称：${errorMessage(error)}` };
    }
  };

  const autoName = async (id: string, promptLine: string): Promise<void> => {
    const target = state.sessions.find((s) => s.info.id === id);
    if (!target || target.titleSource !== "default" || autoNaming.has(id)) {
      return;
    }
    const candidate = deriveAutoTitleFromPrompt(promptLine);
    if (!candidate) return;
    const epoch = renameEpochs.get(id) ?? 0;
    autoNaming.add(id);
    try {
      const saved = await invoke<string>("rename_session", {
        sessionId: id,
        title: candidate,
      });
      if ((renameEpochs.get(id) ?? 0) !== epoch) return;
      update({
        sessions: state.sessions.map((s) =>
          s.info.id === id && s.titleSource === "default"
            ? {
                ...s,
                label: saved,
                titleSource: "auto",
                info: { ...s.info, title: saved },
              }
            : s,
        ),
      });
    } catch {
      // Best-effort automatic naming must never interrupt terminal input.
    } finally {
      autoNaming.delete(id);
    }
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
      renameEpochs.delete(id);
      autoNaming.delete(id);
      const index = state.sessions.findIndex((s) => s.info.id === id);
      const remaining = state.sessions.filter((s) => s.info.id !== id);
      const panes = state.paneIds.filter((pane) => pane !== id);
      const fallback =
        panes[0] ??
        remaining[Math.min(index, remaining.length - 1)]?.info.id ??
        null;
      const activeId = state.activeId === id ? fallback : state.activeId;
      update({
        sessions: remaining,
        paneIds: panes.length ? panes : fallback ? [fallback] : [],
        activeId,
        focusRequest: state.focusRequest + (state.activeId === id ? 1 : 0),
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
    focus,
    split,
    single,
    showLauncher,
    start,
    close,
    handleExit,
    rename,
    autoName,
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
    focus: store.focus,
    split: store.split,
    single: store.single,
    showLauncher: store.showLauncher,
    start: store.start,
    close: store.close,
    handleExit: store.handleExit,
    rename: store.rename,
    autoName: store.autoName,
  };
}
