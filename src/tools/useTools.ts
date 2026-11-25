import { useCallback, useEffect, useRef, useState } from "react";
import type { ToolStatus } from "./types";

type ToolsState = {
  tools: ToolStatus[];
  loading: boolean;
  error: string | null;
};

const INITIAL_STATE: ToolsState = {
  tools: [],
  loading: true,
  error: null,
};

export function useTools() {
  const [state, setState] = useState<ToolsState>(INITIAL_STATE);
  const loadVersion = useRef(0);

  const refresh = useCallback(async () => {
    const version = ++loadVersion.current;
    setState((previous) => ({ ...previous, loading: true, error: null }));

    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const tools = await invoke<ToolStatus[]>("detect_tools");
      if (version !== loadVersion.current) return;
      setState({ tools, loading: false, error: null });
    } catch (error) {
      if (version !== loadVersion.current) return;
      const message = error instanceof Error ? error.message : String(error);
      setState({
        tools: [],
        loading: false,
        error: `无法读取工具状态：${message}`,
      });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { ...state, refresh };
}
