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
  it("connects the copy/paste buttons and xterm key handler to native clipboard", async () => {
    render(<TerminalView {...props} />);
    await waitFor(() =>
      expect(mocks.terminal.attachCustomKeyEventHandler).toHaveBeenCalledOnce(),
    );
    expect(mocks.readText).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    await waitFor(() =>
      expect(mocks.writeText).toHaveBeenCalledExactlyOnceWith("合成选中文字"),
    );
    fireEvent.click(screen.getByRole("button", { name: "粘贴" }));
    await waitFor(() =>
      expect(mocks.terminal.paste).toHaveBeenCalledExactlyOnceWith(
        "合成粘贴文本\n第二行",
      ),
    );
    const handler = mocks.terminal.attachCustomKeyEventHandler.mock
      .calls[0][0] as (event: KeyboardEvent) => boolean;
    expect(
      handler(
        new KeyboardEvent("keydown", {
          key: "v",
          ctrlKey: true,
          cancelable: true,
        }),
      ),
    ).toBe(false);
    await waitFor(() => expect(mocks.terminal.paste).toHaveBeenCalledTimes(2));
  });

  it("keeps copy available after exit while disabling paste", async () => {
    vi.mocked(connectSession).mockImplementation((_id, _bridge, callbacks) => {
      callbacks.phase("exited", null);
      return { send: vi.fn(), dispose: vi.fn(), ready: Promise.resolve() };
    });
    render(<TerminalView {...props} running={false} />);
    await waitFor(() =>
      expect(mocks.terminal.attachCustomKeyEventHandler).toHaveBeenCalledOnce(),
    );
    expect(
      (screen.getByRole("button", { name: "粘贴" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    await waitFor(() => expect(mocks.writeText).toHaveBeenCalledOnce());
    expect(mocks.readText).not.toHaveBeenCalled();
  });

  it("allows dismissing a paste failure alert manually", async () => {
    mocks.readText.mockRejectedValueOnce(new Error("no text"));
    render(<TerminalView {...props} />);
    await waitFor(() =>
      expect(mocks.terminal.attachCustomKeyEventHandler).toHaveBeenCalledOnce(),
    );
    fireEvent.click(screen.getByRole("button", { name: "粘贴" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("粘贴失败");
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("clears a paste failure alert when typing into the terminal", async () => {
    mocks.readText.mockRejectedValueOnce(new Error("no text"));
    render(<TerminalView {...props} />);
    await waitFor(() => expect(mocks.terminal.onData).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "粘贴" }));
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
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "粘贴" }));
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
});
