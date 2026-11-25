import { useEffect, useRef, useState } from "react";
import { createPromptLineTracker } from "./sessionNaming";
import { connectSession, type ConnectionPhase } from "./sessionTransport";
import { createTerminalClipboard } from "./terminalClipboard";
import type { SessionInfo } from "./types";

type Props = {
  session: SessionInfo;
  visible: boolean;
  focused: boolean;
  focusRequest: number;
  running: boolean;
  busy: boolean;
  onExit: (id: string, reason: string) => void;
  onConnection: (
    id: string,
    phase: ConnectionPhase,
    error: string | null,
  ) => void;
  onPromptLine?: (id: string, line: string) => void;
};

export default function TerminalView({
  session,
  visible,
  focused,
  focusRequest,
  running,
  busy,
  onExit,
  onConnection,
  onPromptLine,
}: Props) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const current = useRef({
    visible,
    focused,
    running,
    busy,
    onExit,
    onConnection,
    onPromptLine,
  });
  current.current = {
    visible,
    focused,
    running,
    busy,
    onExit,
    onConnection,
    onPromptLine,
  };
  const controls = useRef<{
    focus: () => void;
    scroll: () => void;
    layout: () => void;
    input: () => void;
    copy: () => Promise<void>;
    paste: () => Promise<void>;
    invalidatePaste: () => void;
  } | null>(null);
  const [phase, setPhase] = useState<ConnectionPhase>("connecting");
  const [problem, setProblem] = useState<string | null>(null);
  const [clipboardProblem, setClipboardProblem] = useState<string | null>(null);
  const [connection, setConnection] = useState(0);
  const [compact, setCompact] = useState(false);

  useEffect(() => {
    let disposed = false;
    let localPhase: ConnectionPhase = "connecting";
    const cleanups: (() => void)[] = [];
    const frames: number[] = [];
    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    setPhase("connecting");
    setProblem(null);
    setClipboardProblem(null);

    async function mount() {
      const [{ Terminal }, { FitAddon }, { listen }, { invoke }, clipboardApi] =
        await Promise.all([
          import("@xterm/xterm"),
          import("@xterm/addon-fit"),
          import("@tauri-apps/api/event"),
          import("@tauri-apps/api/core"),
          import("@tauri-apps/plugin-clipboard-manager"),
        ]);
      if (disposed || !hostRef.current) return;

      const terminal = new Terminal({
        fontFamily:
          "'Cascadia Mono', 'Cascadia Code', 'SFMono-Regular', Consolas, 'Microsoft YaHei', 'Noto Sans SC', monospace",
        fontSize: 14,
        windowsPty:
          typeof navigator !== "undefined" &&
          /Windows/i.test(navigator.userAgent)
            ? { backend: "conpty", buildNumber: 22621 }
            : undefined,
        scrollback: 5000,
        cursorBlink: true,
        disableStdin: true,
        theme: {
          background: "#0d1117",
          foreground: "#e6edf3",
          cursor: "#8cdec2",
          selectionBackground: "#26404f",
        },
      });
      cleanups.push(() => terminal.dispose());
      const fit = new FitAddon();
      terminal.loadAddon(fit);
      terminal.open(hostRef.current);
      const input = () => {
        terminal.options.disableStdin =
          localPhase !== "ready" ||
          !current.current.visible ||
          !current.current.running ||
          current.current.busy;
      };
      const focus = () => {
        if (current.current.visible && localPhase === "ready") terminal.focus();
      };
      let lastSize = "";
      const resize = () => {
        const host = hostRef.current;
        if (
          disposed ||
          !current.current.visible ||
          !host ||
          host.clientHeight < 80 ||
          host.clientWidth < 160
        )
          return;
        fit.fit();
        const { rows, cols } = terminal;
        if (
          !Number.isFinite(rows) ||
          !Number.isFinite(cols) ||
          rows < 3 ||
          rows > 300 ||
          cols < 10 ||
          cols > 500 ||
          localPhase !== "ready" ||
          !current.current.running
        )
          return;
        const size = `${rows}:${cols}`;
        if (size === lastSize) return;
        lastSize = size;
        void invoke("resize_session", {
          sessionId: session.id,
          rows,
          cols,
        }).catch(() => {
          if (!disposed && localPhase === "ready" && current.current.running) {
            lastSize = "";
            setProblem("终端尺寸同步失败。请重新连接显示后再继续操作。");
          }
        });
      };
      const layout = () => {
        clearTimeout(resizeTimer);
        resizeTimer = setTimeout(resize, 60);
      };
      const clipboard = createTerminalClipboard({
        terminal,
        readText: clipboardApi.readText,
        writeText: clipboardApi.writeText,
        canPaste: () =>
          !disposed &&
          localPhase === "ready" &&
          current.current.visible &&
          current.current.focused &&
          current.current.running &&
          !current.current.busy,
        problem: setClipboardProblem,
      });
      terminal.attachCustomKeyEventHandler(clipboard.key);
      cleanups.push(clipboard.dispose);
      controls.current = {
        focus,
        scroll: () => terminal.scrollToBottom(),
        layout,
        input,
        copy: clipboard.copy,
        paste: clipboard.paste,
        invalidatePaste: clipboard.invalidate,
      };
      const observer = new ResizeObserver(layout);
      observer.observe(hostRef.current);
      cleanups.push(() => observer.disconnect());

      const transport = connectSession(
        session.id,
        { listen, invoke },
        {
          output: (text) => {
            if (!disposed) terminal.write(text);
          },
          phase: (next, error) => {
            if (disposed) return;
            localPhase = next;
            clipboard.invalidate();
            setPhase(next);
            setProblem(error);
            current.current.onConnection(session.id, next, error);
            input();
            if (next === "ready") {
              frames.push(
                requestAnimationFrame(() => {
                  if (disposed) return;
                  resize();
                  if (current.current.focused) focus();
                }),
              );
            }
          },
          exit: (id, reason) => {
            if (!disposed) current.current.onExit(id, reason);
          },
          problem: (message) => {
            // Non-fatal: the connection stays ready; only that input was
            // refused or dropped.
            if (!disposed) setProblem(message);
          },
        },
      );
      cleanups.push(transport.dispose);
      const tracker = createPromptLineTracker();
      const data = terminal.onData((text) => {
        if (
          current.current.visible &&
          current.current.running &&
          !current.current.busy
        ) {
          transport.send(text);
          for (const line of tracker.feed(text)) {
            current.current.onPromptLine?.(session.id, line);
          }
        }
      });
      cleanups.push(() => data.dispose());
      frames.push(
        requestAnimationFrame(() => {
          if (!disposed) frames.push(requestAnimationFrame(resize));
        }),
      );
      await transport.ready;
    }

    void mount().catch(() => {
      if (!disposed) {
        setPhase("error");
        setProblem("终端界面未能加载。请重新连接显示；不会重新启动 CLI。");
      }
    });
    return () => {
      disposed = true;
      controls.current = null;
      clearTimeout(resizeTimer);
      for (const frame of frames) cancelAnimationFrame(frame);
      for (const cleanup of cleanups.reverse()) cleanup();
    };
  }, [session.id, connection]);

  useEffect(() => {
    controls.current?.invalidatePaste();
    controls.current?.input();
    if (visible) {
      controls.current?.layout();
    }
  }, [visible, focused, running, busy]);

  useEffect(() => {
    if (current.current.visible && current.current.focused)
      controls.current?.focus();
  }, [focusRequest]);

  return (
    <div className={`terminal-view${compact ? " is-compact" : ""}`}>
      <div className="terminal-toolbar">
        <span role="status">
          {compact
            ? "简洁模式已隐藏终端辅助栏"
            : phase === "connecting"
              ? "正在连接终端…"
              : phase === "error"
                ? "终端连接需要处理"
                : !running || phase === "exited"
                  ? "进程已结束，输出保留在下方"
                  : "选中文字后 Ctrl+C 复制 · Ctrl+V 粘贴 · 未选择时 Ctrl+C 中断"}
        </span>
        <div className="terminal-actions">
          <button
            type="button"
            className="ghost-button"
            onClick={() => setCompact((value) => !value)}
          >
            {compact ? "完整界面" : "简洁模式"}
          </button>
          <button
            type="button"
            className="ghost-button terminal-clipboard-action"
            title="复制选中文字 · Ctrl+Shift+C / ⌘C"
            onClick={() => void controls.current?.copy()}
          >
            复制
          </button>
          <button
            type="button"
            className="ghost-button terminal-clipboard-action"
            title="粘贴文本 · Ctrl+V / Ctrl+Shift+V / ⌘V"
            disabled={phase !== "ready" || !running || busy}
            onClick={() => void controls.current?.paste()}
          >
            粘贴
          </button>
          <button
            type="button"
            className="ghost-button"
            disabled={phase !== "ready" || !running || busy}
            onClick={() => controls.current?.focus()}
          >
            输入终端
          </button>
          <button
            type="button"
            className="ghost-button"
            onClick={() => controls.current?.scroll()}
          >
            回到底部
          </button>
        </div>
      </div>
      {clipboardProblem ? (
        <div className="terminal-problem" role="alert">
          <span>{clipboardProblem}</span>
        </div>
      ) : null}
      {problem ? (
        <div className="terminal-problem" role="alert">
          <span>{problem}</span>
          <button
            type="button"
            className="ghost-button"
            disabled={busy}
            onClick={() => setConnection((value) => value + 1)}
          >
            重新连接显示
          </button>
        </div>
      ) : null}
      <div
        className="terminal-host"
        ref={hostRef}
        aria-label="CLI 输入与输出"
      />
    </div>
  );
}
