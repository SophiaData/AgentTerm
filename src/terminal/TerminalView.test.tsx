// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getThemePreset } from "../theme";
import TerminalView from "./TerminalView";
import { connectSession } from "./sessionTransport";

const mocks = vi.hoisted(() => ({
  readText: vi.fn(async () => "合成粘贴文本\n第二行"),
  writeText: vi.fn<(text: string) => Promise<void>>(async () => {}),
  initOptions: null as null | Record<string, unknown>,
  terminal: {
    options: { disableStdin: true } as {
      disableStdin: boolean;
      theme?: unknown;
    },
    rows: 24,
    cols: 80,
    getSelection: vi.fn(() => "合成选中文字"),
    paste: vi.fn(),
    focus: vi.fn(),
    write: vi.fn(),
    open: vi.fn(),
    loadAddon: vi.fn(),
    dispose: vi.fn(),
    scrollToBottom: vi.fn(),
    scrollLines: vi.fn(),
    refresh: vi.fn(),
    buffer: { active: { viewportY: 12, baseY: 20 } },
    onScroll: vi.fn<(cb: () => void) => { dispose: () => void }>(() => ({
      dispose: vi.fn(),
    })),
    onData: vi.fn<(cb: (text: string) => void) => { dispose: () => void }>(
      () => ({ dispose: vi.fn() }),
    ),
    attachCustomKeyEventHandler: vi.fn(),
  },
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    constructor(opts?: Record<string, unknown>) {
      mocks.initOptions = opts ?? null;
      return mocks.terminal;
    }
  },
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  readText: mocks.readText,
  writeText: mocks.writeText,
}));
vi.mock("./sessionTransport", () => ({ connectSession: vi.fn() }));

const props = {
  session: {
    id: "test",
    toolId: "codex",
    toolName: "Codex",
    projectPath: "test-project",
  },
  visible: true,
  focused: true,
  focusRequest: 0,
  running: true,
  busy: false,
  onExit: vi.fn(),
  onConnection: vi.fn(),
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.initOptions = null;
  mocks.terminal.options = { disableStdin: true };
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.stubGlobal(
    "requestAnimationFrame",
    vi.fn(() => 0),
  );
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  vi.mocked(connectSession).mockImplementation((_id, _bridge, callbacks) => {
    callbacks.phase("ready", null);
    return { send: vi.fn(), dispose: vi.fn(), ready: Promise.resolve() };
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("terminal clipboard wiring", () => {
  const sendKey = (
    handler: (event: KeyboardEvent) => boolean,
    key: string,
    init: KeyboardEventInit = {},
  ) =>
    handler(
      new KeyboardEvent("keydown", {
        key,
        cancelable: true,
        ...init,
      }),
    );

  it("connects the xterm key handler to native clipboard without redundant toolbar buttons", async () => {
    render(<TerminalView {...props} />);
    await waitFor(() =>
      expect(mocks.terminal.attachCustomKeyEventHandler).toHaveBeenCalledOnce(),
    );
    expect(screen.queryByRole("button", { name: "简洁模式" })).toBeNull();
    expect(screen.queryByRole("button", { name: "复制" })).toBeNull();
    expect(screen.queryByRole("button", { name: "粘贴" })).toBeNull();
    expect(screen.queryByRole("button", { name: "输入终端" })).toBeNull();
    expect(screen.getByRole("button", { name: "回到底部" })).toBeTruthy();

    expect(mocks.readText).not.toHaveBeenCalled();
    const handler = mocks.terminal.attachCustomKeyEventHandler.mock
      .calls[0][0] as (event: KeyboardEvent) => boolean;
    expect(sendKey(handler, "c", { ctrlKey: true })).toBe(false);
    await waitFor(() =>
      expect(mocks.writeText).toHaveBeenCalledExactlyOnceWith("合成选中文字"),
    );
    expect(sendKey(handler, "v", { ctrlKey: true })).toBe(false);
    await waitFor(() =>
      expect(mocks.terminal.paste).toHaveBeenCalledExactlyOnceWith(
        "合成粘贴文本\n第二行",
      ),
    );
  });

  it("keeps copy available after exit while ignoring paste", async () => {
    vi.mocked(connectSession).mockImplementation((_id, _bridge, callbacks) => {
      callbacks.phase("exited", null);
      return { send: vi.fn(), dispose: vi.fn(), ready: Promise.resolve() };
    });
    render(<TerminalView {...props} running={false} />);
    await waitFor(() =>
      expect(mocks.terminal.attachCustomKeyEventHandler).toHaveBeenCalledOnce(),
    );
    const handler = mocks.terminal.attachCustomKeyEventHandler.mock
      .calls[0][0] as (event: KeyboardEvent) => boolean;
    expect(sendKey(handler, "v", { ctrlKey: true })).toBe(false);
    expect(sendKey(handler, "c", { ctrlKey: true })).toBe(false);
    await waitFor(() => expect(mocks.writeText).toHaveBeenCalledOnce());
    expect(mocks.readText).not.toHaveBeenCalled();
  });

  it("allows dismissing a paste failure alert manually", async () => {
    mocks.readText.mockRejectedValueOnce(new Error("no text"));
    render(<TerminalView {...props} />);
    await waitFor(() =>
      expect(mocks.terminal.attachCustomKeyEventHandler).toHaveBeenCalledOnce(),
    );
    const handler = mocks.terminal.attachCustomKeyEventHandler.mock
      .calls[0][0] as (event: KeyboardEvent) => boolean;
    sendKey(handler, "v", { ctrlKey: true });
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("粘贴失败");
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("clears a paste failure alert when typing into the terminal", async () => {
    mocks.readText.mockRejectedValueOnce(new Error("no text"));
    render(<TerminalView {...props} />);
    await waitFor(() => expect(mocks.terminal.onData).toHaveBeenCalledOnce());
    const handler = mocks.terminal.attachCustomKeyEventHandler.mock
      .calls[0][0] as (event: KeyboardEvent) => boolean;
    sendKey(handler, "v", { ctrlKey: true });
    await screen.findByRole("alert");
    const onDataHandler = mocks.terminal.onData.mock.calls[0][0] as (
      text: string,
    ) => void;
    act(() => {
      onDataHandler("ls\r");
    });
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("auto-dismisses a clipboard failure alert after 6 seconds", async () => {
    vi.useFakeTimers();
    try {
      mocks.readText.mockRejectedValueOnce(new Error("no text"));
      render(<TerminalView {...props} />);
      await act(async () => {
        await Promise.resolve();
      });
      expect(mocks.terminal.attachCustomKeyEventHandler).toHaveBeenCalledOnce();
      const handler = mocks.terminal.attachCustomKeyEventHandler.mock
        .calls[0][0] as (event: KeyboardEvent) => boolean;
      await act(async () => {
        sendKey(handler, "v", { ctrlKey: true });
        await Promise.resolve();
      });
      expect(screen.getByRole("alert").textContent).toContain("粘贴失败");
      act(() => {
        vi.advanceTimersByTime(6000);
      });
      expect(screen.queryByRole("alert")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("applies initial theme and updates terminal options live without reconnecting", async () => {
    const { rerender } = render(<TerminalView {...props} themeId="nord" />);
    await waitFor(() => expect(connectSession).toHaveBeenCalledOnce());
    expect(mocks.initOptions?.theme).toEqual(getThemePreset("nord").terminal);

    rerender(<TerminalView {...props} themeId="light" />);
    await waitFor(() =>
      expect(mocks.terminal.options.theme).toEqual(
        getThemePreset("light").terminal,
      ),
    );
    expect(connectSession).toHaveBeenCalledOnce();
  });

  it("forces buffer, DOM viewport, and refresh to bottom on button click and tab reveal", async () => {
    const rafQueue: FrameRequestCallback[] = [];
    vi.stubGlobal(
      "requestAnimationFrame",
      vi.fn((cb: FrameRequestCallback) => {
        rafQueue.push(cb);
        return rafQueue.length;
      }),
    );
    const flushRaf = () => {
      let guard = 0;
      while (rafQueue.length > 0 && guard < 10) {
        guard += 1;
        const batch = rafQueue.splice(0, rafQueue.length);
        for (const cb of batch) cb(0);
      }
    };

    mocks.terminal.buffer = { active: { viewportY: 5, baseY: 18 } };
    const { container, rerender } = render(
      <TerminalView {...props} visible={false} />,
    );
    await waitFor(() => expect(mocks.terminal.onScroll).toHaveBeenCalledOnce());
    flushRaf();

    const host = container.querySelector(".terminal-host")!;
    const viewport = document.createElement("div");
    viewport.className = "xterm-viewport";
    Object.defineProperty(viewport, "scrollHeight", {
      value: 480,
      configurable: true,
    });
    viewport.scrollTop = 40;
    host.appendChild(viewport);

    fireEvent.click(screen.getByRole("button", { name: "回到底部" }));
    expect(mocks.terminal.scrollLines).toHaveBeenCalledWith(13);
    expect(mocks.terminal.scrollToBottom).toHaveBeenCalled();
    expect(viewport.scrollTop).toBe(480);

    mocks.terminal.refresh.mockClear();
    mocks.terminal.scrollLines.mockClear();
    mocks.terminal.scrollToBottom.mockClear();
    mocks.terminal.buffer = { active: { viewportY: 5, baseY: 18 } };
    viewport.scrollTop = 10;

    rerender(<TerminalView {...props} visible={true} />);
    await waitFor(() => expect(rafQueue.length).toBeGreaterThan(0));
    flushRaf();

    expect(mocks.terminal.scrollLines).toHaveBeenCalledWith(13);
    expect(mocks.terminal.scrollToBottom).toHaveBeenCalled();
    expect(mocks.terminal.refresh).toHaveBeenCalled();
    expect(viewport.scrollTop).toBe(480);
  });
});
