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

export function useLauncher() {
  const [session, setSession] = useState<ActiveSession | null>(null);
  const [busy, setBusy] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [closeError, setCloseError] = useState<string | null>(null);

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
      setSession({ info, running: true, exitReason: null });
    } catch (error) {
      setStartError(`无法启动会话：${errorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  }, []);

  const handleExit = useCallback((id: string, reason: string) => {
    setSession((previous) =>
      previous && previous.info.id === id
        ? { ...previous, running: false, exitReason: reason }
        : previous,
    );
  }, []);

  const close = useCallback(async () => {
    if (!session) {
      return;
    }

    setBusy(true);
    setCloseError(null);
    try {
      if (session.running) {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("stop_session", { sessionId: session.info.id });
      }
      // Only leave the view once the backend actually released the session.
      setSession(null);
    } catch (error) {
      // Keep the session open so the user can see the failure and retry, rather
      // than silently dropping a session whose process may still be running (#17).
      setCloseError(`无法关闭会话：${errorMessage(error)}。请重试。`);
    } finally {
      setBusy(false);
    }
  }, [session]);

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
