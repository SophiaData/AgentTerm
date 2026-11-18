import { useEffect, useRef } from "react";
import { advance } from "./offsetStitch";
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

      terminal.onData((data) => {
        void invoke("write_session_input", {
          sessionId: session.id,
          data,
        }).catch(() => {});
      });

      // Only push a size to the PTY once the container has a real layout.
      // A premature fit() on an unlaid-out element yields absurd row counts
      // (e.g. 964 rows) and makes full-screen TUIs like Claude Code render blank.
      const proposeResize = () => {
        if (disposed) {
          return;
        }
        const host = hostRef.current;
        if (!host || host.clientHeight < 120 || host.clientWidth < 200) {
          return;
        }
        fit.fit();
        const rows = terminal.rows;
        const cols = terminal.cols;
        if (!Number.isFinite(rows) || !Number.isFinite(cols)) {
          return;
        }
        if (rows < 4 || rows > 300 || cols < 20 || cols > 500) {
          return;
        }
        void invoke("resize_session", {
          sessionId: session.id,
          rows,
          cols,
        }).catch(() => {});
      };

      // Wait two animation frames so the flex layout has settled before the
      // first measurement.
      requestAnimationFrame(() => {
        requestAnimationFrame(proposeResize);
      });

      let resizeTimer: ReturnType<typeof setTimeout> | null = null;
      resizeObserver = new ResizeObserver(() => {
        if (resizeTimer) {
          clearTimeout(resizeTimer);
        }
        resizeTimer = setTimeout(proposeResize, 60);
      });
      resizeObserver.observe(hostRef.current);

      // Output stitching via a shared byte offset:
      //  - The backend tags every live chunk with `endOffset` (bytes emitted so
      //    far) and the snapshot with the offset of its last byte.
      //  - We write the snapshot, remember how many bytes it covered, then write
      //    each live chunk only for the bytes beyond that point. Overlap is
      //    dropped; anything after the snapshot is written in full and in order.
      // This replaces the old "buffer then flush" approach, which could not tell
      // whether a buffered chunk was already inside the snapshot (#8).

      // Guards against reporting the exit twice (e.g. the exit event arrives and
      // the snapshot also says the process already ended).
      let exitReported = false;
      const reportExit = (id: string, reason: string) => {
        if (exitReported) {
          return;
        }
        exitReported = true;
        onExitRef.current(id, reason);
      };

      unlistenExit = await listen<{ id: string; reason: string }>(
        `session-exit-${session.id}`,
        (event) => {
          terminal.write(
            `\r\n\x1b[33m[会话结束：${event.payload.reason}]\x1b[0m\r\n`,
          );
          reportExit(event.payload.id, event.payload.reason);
        },
      );

      // Bytes already handed to the terminal. Starts at -1 meaning "unknown yet"
      // (before the snapshot arrives); we only compare offsets after that.
      let writtenOffset = -1;
      // Live chunks that arrived before the snapshot, held until we know the
      // snapshot's end offset so we can trim precisely.
      const pendingBeforeSnapshot: { data: string; endOffset: number }[] = [];

      const handleChunk = (payload: { data: string; endOffset: number }) => {
        if (writtenOffset < 0) {
          pendingBeforeSnapshot.push(payload);
          return;
        }
        const result = advance(payload, writtenOffset);
        if (result.text) {
          terminal.write(result.text);
        }
        writtenOffset = result.offset;
      };

      unlistenOutput = await listen<{ data: string; endOffset: number }>(
        `session-output-${session.id}`,
        (event) => handleChunk(event.payload),
      );

      // Replay everything the PTY produced before this view mounted. Without
      // this, the first screen of a full-screen TUI (Claude Code, Codex) is
      // lost and the terminal looks blank even though the process is running.
      try {
        const snapshot = await invoke<{
          data: string;
          offset: number;
          running: boolean;
        }>("session_snapshot", { sessionId: session.id });
        if (snapshot.data) {
          terminal.write(snapshot.data);
        }
        writtenOffset = snapshot.offset;
        // Now reconcile chunks that raced in before the snapshot: write only
        // their bytes beyond the snapshot's end.
        for (const chunk of pendingBeforeSnapshot) {
          handleChunk(chunk);
        }
        pendingBeforeSnapshot.length = 0;
        if (!snapshot.running) {
          // The process already exited before this view appeared; show the final
          // state instead of a stuck "running" indicator.
          terminal.write("\r\n\x1b[33m[会话已结束]\x1b[0m\r\n");
          reportExit(session.id, "进程已结束");
        }
      } catch {
        // The session may have already exited; the exit listener handles that.
        // Let any buffered chunks through now that we know there is no snapshot.
        writtenOffset = 0;
        for (const chunk of pendingBeforeSnapshot) {
          handleChunk(chunk);
        }
        pendingBeforeSnapshot.length = 0;
      }

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
        if (resizeTimer) {
          clearTimeout(resizeTimer);
        }
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
