import { useCallback, useEffect, useState } from "react";
import type { SessionHistoryRecord } from "./types";

type Invoke = <T>(command: string, args: Record<string, unknown>) => Promise<T>;

async function defaultInvoke<T>(
  command: string,
  args: Record<string, unknown>,
): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

export type SessionHistoryState = {
  records: SessionHistoryRecord[];
  loading: boolean;
  error: string | null;
  reload: () => Promise<void>;
};

export function useSessionHistory(
  invoke: Invoke = defaultInvoke,
): SessionHistoryState {
  const [records, setRecords] = useState<SessionHistoryRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setRecords(await invoke<SessionHistoryRecord[]>("session_history", {}));
    } catch (cause) {
      const detail = cause instanceof Error ? cause.message : String(cause);
      setError(`无法读取最近会话：${detail}`);
    } finally {
      setLoading(false);
    }
  }, [invoke]);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { records, loading, error, reload };
}
