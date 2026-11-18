import { useCallback, useState } from "react";
import type { Project } from "../projects/types";
import type { ToolStatus } from "../tools/types";
import { newSessionId, type SessionInfo } from "./types";
import { DEFAULT_ROWS, DEFAULT_COLS } from "./defaults";

export type ActiveSession = {
  info: SessionInfo;
  running: boolean;
  exitReason: string | null;
};

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function useSessions() {
  const [sessions, setSessions] = useState<ActiveSession[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [pendingCloseId, setPendingCloseId] = useState<string | null>(null);

  const activeSession =
    sessions.find((session) => session.info.id === activeId) ?? null;

  const start = useCallback(async (project: Project, tool: ToolStatus) => {
    setBusy(true);
    setStartError(null);

    const id = newSessionId();
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const info = await invoke<SessionInfo>("start_session", {
        sessionId: id,
        toolId: tool.id,
        projectPath: project.path,
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
      });
      setSessions((previous) => [
        ...previous,
        { info, running: true, exitReason: null },
      ]);
      setActiveId(info.id);
    } catch (error) {
      setStartError(`无法启动会话：${errorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  }, []);

  const closeNow = useCallback(async (id: string) => {
    setBusy(true);
    setPendingCloseId(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("stop_session", { sessionId: id });
    } catch {
      // The session may already have exited on its own; closing the view is
      // still the correct outcome either way.
    } finally {
      setSessions((previous) => {
        const remaining = previous.filter((session) => session.info.id !== id);
        setActiveId((currentActive) =>
          currentActive === id
            ? (remaining[remaining.length - 1]?.info.id ?? null)
            : currentActive,
        );
        return remaining;
      });
      setBusy(false);
    }
  }, []);

  const requestClose = useCallback(
    (id: string, running: boolean) => {
      if (running) {
        setPendingCloseId(id);
      } else {
        void closeNow(id);
      }
    },
    [closeNow],
  );

  const cancelClose = useCallback(() => {
    setPendingCloseId(null);
  }, []);

  const confirmClose = useCallback(() => {
    if (pendingCloseId) {
      void closeNow(pendingCloseId);
    }
  }, [pendingCloseId, closeNow]);

  const handleExit = useCallback((id: string, reason: string) => {
    setSessions((previous) =>
      previous.map((session) =>
        session.info.id === id
          ? { ...session, running: false, exitReason: reason }
          : session,
      ),
    );
  }, []);

  return {
    sessions,
    activeSession,
    activeId,
    busy,
    startError,
    pendingCloseId,
    start,
    requestClose,
    cancelClose,
    confirmClose,
    handleExit,
  };
}
