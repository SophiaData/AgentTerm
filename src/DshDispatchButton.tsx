import { useState } from "react";

export default function DshDispatchButton() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function open() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("open_dispatch_dashboard");
    } catch (reason) {
      setError(
        typeof reason === "string"
          ? reason
          : "无法打开协调面板，请确认本机任务服务已启动。",
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <span>
      <button
        type="button"
        className="ghost-button"
        disabled={busy}
        onClick={() => void open()}
      >
        DSH 协调
      </button>
      {error && <span role="alert">{error}</span>}
    </span>
  );
}
