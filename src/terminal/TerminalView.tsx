import { useEffect, useRef } from "react";
import type { SessionInfo } from "./types";

type Props = {
  session: SessionInfo;
  onExit: (id: string, reason: string) => void;
};

export default function TerminalView({ session, onExit }: Props) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const onExitRef = useRef(onExit);
  onExitRef.current = onExit;

  useEffect(() => {
    let disposed = false;
    let unlistenOutput: (() => void) | null = null;
    let unlistenExit: (() => void) | null = null;
    let resizeObserver: ResizeObserver | null = null;

    async function mount() {
      const [{ Terminal }, { FitAddon }, { listen }, { invoke }] =
        await Promise.all([
          import("@xterm/xterm"),
          import("@xterm/addon-fit"),
          import("@tauri-apps/api/event"),
          import("@tauri-apps/api/core"),
        ]);

      if (disposed || !hostRef.current) {
        return;
      }

      const terminal = new Terminal({
        fontFamily: "'SFMono-Regular', Consolas, 'Liberation Mono', monospace",
        fontSize: 13,
        theme: {
          background: "#0d1117",
          foreground: "#e6edf3",
          cursor: "#00ff41",
          selectionBackground: "#26404f",
        },
        allowProposedApi: true,
      });
      const fit = new FitAddon();
      terminal.loadAddon(fit);
      terminal.open(hostRef.current);
      fit.fit();

      terminal.onData((data) => {
        void invoke("write_session_input", {
          sessionId: session.id,
          data,
        }).catch(() => {});
      });

      const proposeResize = () => {
        if (disposed) {
          return;
        }
        fit.fit();
        void invoke("resize_session", {
          sessionId: session.id,
          rows: terminal.rows,
          cols: terminal.cols,
        }).catch(() => {});
      };

      resizeObserver = new ResizeObserver(proposeResize);
      resizeObserver.observe(hostRef.current);
      proposeResize();

      unlistenOutput = await listen<string>(
        `session-output-${session.id}`,
        (event) => {
          terminal.write(event.payload);
        },
      );

      unlistenExit = await listen<{ id: string; reason: string }>(
        `session-exit-${session.id}`,
        (event) => {
          terminal.write(
            `\r\n\x1b[33m[会话结束：${event.payload.reason}]\x1b[0m\r\n`,
          );
          onExitRef.current(event.payload.id, event.payload.reason);
        },
      );

      if (disposed) {
        unlistenOutput();
        unlistenExit();
        resizeObserver?.disconnect();
        terminal.dispose();
        return;
      }

      cleanup = () => {
        unlistenOutput?.();
        unlistenExit?.();
        resizeObserver?.disconnect();
        terminal.dispose();
      };
    }

    let cleanup: (() => void) | null = null;
    void mount();

    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [session.id]);

  return <div className="terminal-host" ref={hostRef} />;
}
