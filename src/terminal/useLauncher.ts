import { useCallback, useRef, useState } from "react";
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

export function useLauncher() {
  const [session, setSession] = useState<ActiveSession | null>(null);
  const [busy, setBusy] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [closeError, setCloseError] = useState<string | null>(null);
  const currentSession = useRef<ActiveSession | null>(null);
  const operation = useRef(false);

  const start = useCallback(async (project: Project, tool: ToolStatus) => {
    if (operation.current || currentSession.current) return;
    operation.current = true;
    setBusy(true);
    setStartError(null);
    setCloseError(null);

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
      const next = { info, running: true, exitReason: null };
      currentSession.current = next;
      setSession(next);
    } catch (error) {
      setStartError(`无法启动会话：${errorMessage(error)}`);
    } finally {
      operation.current = false;
      setBusy(false);
    }
  }, []);

  const handleExit = useCallback((id: string, reason: string) => {
    const previous = currentSession.current;
    if (!previous || previous.info.id !== id) return;
    const next = { ...previous, running: false, exitReason: reason };
    currentSession.current = next;
    setSession(next);
  }, []);

  const close = useCallback(async () => {
    const previous = currentSession.current;
    if (!previous || operation.current) {
      return;
    }

    operation.current = true;
    setBusy(true);
    setCloseError(null);
    try {
      if (previous.running) {
        const { invoke } = await import("@tauri-apps/api/core");
        try {
          await invoke("stop_session", { sessionId: previous.info.id });
        } catch (error) {
          // Natural exit can race the stop request. Require a backend snapshot
          // before treating a failed stop as an already-ended session.
          const snapshot = await invoke<{ running: boolean }>(
            "session_snapshot",
            {
              sessionId: previous.info.id,
            },
          );
          if (snapshot.running) throw error;
        }
      }
      // Only leave the view once the backend actually released the session.
      currentSession.current = null;
      setSession(null);
    } catch (error) {
      // Keep the session open so the user can see the failure and retry, rather
      // than silently dropping a session whose process may still be running (#17).
      setCloseError(`无法关闭会话：${errorMessage(error)}。请重试。`);
    } finally {
      operation.current = false;
      setBusy(false);
    }
  }, []);

  return {
    session,
    busy,
    startError,
    closeError,
    start,
    close,
    handleExit,
  };
}
