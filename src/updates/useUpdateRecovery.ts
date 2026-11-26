import { useCallback, useEffect, useRef, useState } from "react";
import type { UpdateRecoverySnapshot } from "./types";

type Invoke = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

async function defaultInvoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

export type UpdateRecoveryState = {
  recovery: UpdateRecoverySnapshot | null;
  loading: boolean;
  error: string | null;
  reload: () => Promise<void>;
  consumeSession: (sessionId: string) => Promise<void>;
  dismiss: () => Promise<void>;
};

export function useUpdateRecovery(
  invoke: Invoke = defaultInvoke,
): UpdateRecoveryState {
  const [recovery, setRecovery] = useState<UpdateRecoverySnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const loadVersion = useRef(0);

  const reload = useCallback(async () => {
    const version = ++loadVersion.current;
    setLoading(true);
    setError(null);
    try {
      const next = await invoke<UpdateRecoverySnapshot | null>(
        "get_update_recovery",
        {},
      );
      if (version === loadVersion.current) setRecovery(next ?? null);
    } catch (cause) {
      if (version !== loadVersion.current) return;
      const detail = cause instanceof Error ? cause.message : String(cause);
      setError(`无法读取更新恢复记录：${detail}`);
    } finally {
      if (version === loadVersion.current) setLoading(false);
    }
  }, [invoke]);

  const consumeSession = useCallback(
    async (sessionId: string) => {
      try {
        const next = await invoke<UpdateRecoverySnapshot | null>(
          "consume_update_recovery_session",
          { sessionId },
        );
        setRecovery(next ?? null);
      } catch (cause) {
        const detail = cause instanceof Error ? cause.message : String(cause);
        setError(`无法更新恢复记录：${detail}`);
      }
    },
    [invoke],
  );

  const dismiss = useCallback(async () => {
    try {
      await invoke("dismiss_update_recovery", {});
      setRecovery(null);
      setError(null);
    } catch (cause) {
      const detail = cause instanceof Error ? cause.message : String(cause);
      setError(`无法忽略更新恢复记录：${detail}`);
    }
  }, [invoke]);

  useEffect(() => {
    void reload();
  }, [reload]);

  return {
    recovery,
    loading,
    error,
    reload,
    consumeSession,
    dismiss,
  };
}
