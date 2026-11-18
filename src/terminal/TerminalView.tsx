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

      // Ordering to avoid duplicate/missing output:
      //  1. Register the exit listener FIRST, so a short-lived process that dies
      //     before we take the snapshot still reports its final state.
      //  2. Register the live-output listener, but buffer events instead of
      //     writing them yet — they may overlap with the snapshot.
      //  3. Take the snapshot, write it once.
      //  4. Flush buffered live events that arrived after the snapshot was
      //     requested, then switch to writing live events directly.
      // This guarantees old snapshots and new output never double-write and
      // never leave a gap.

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

      let replaying = true;
      const pendingDuringReplay: string[] = [];

      unlistenOutput = await listen<string>(
        `session-output-${session.id}`,
        (event) => {
          if (replaying) {
            pendingDuringReplay.push(event.payload);
            return;
          }
          terminal.write(event.payload);
        },
      );

      // Replay everything the PTY produced before this view mounted. Without
      // this, the first screen of a full-screen TUI (Claude Code, Codex) is
      // lost and the terminal looks blank even though the process is running.
      try {
        const snapshot = await invoke<{ data: string; running: boolean }>(
          "session_snapshot",
          { sessionId: session.id },
        );
        if (snapshot.data) {
          terminal.write(snapshot.data);
        }
        if (!snapshot.running) {
          // The process already exited before this view appeared; show the final
          // state instead of a stuck "running" indicator.
          terminal.write("\r\n\x1b[33m[会话已结束]\x1b[0m\r\n");
          reportExit(session.id, "进程已结束");
        }
      } catch {
        // The session may have already exited; the exit listener handles that.
      } finally {
        // Snapshot is on screen; now drain anything that arrived meanwhile, then
        // let future events write straight through.
        replaying = false;
        for (const chunk of pendingDuringReplay) {
          terminal.write(chunk);
        }
        pendingDuringReplay.length = 0;
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
