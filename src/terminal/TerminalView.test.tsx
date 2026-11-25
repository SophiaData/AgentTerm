// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalView from "./TerminalView";
import { connectSession } from "./sessionTransport";

const mocks = vi.hoisted(() => ({
  readText: vi.fn(async () => "合成粘贴文本\n第二行"),
  writeText: vi.fn<(text: string) => Promise<void>>(async () => {}),
  terminal: {
    options: { disableStdin: true },
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
    onData: vi.fn(() => ({ dispose: vi.fn() })),
    attachCustomKeyEventHandler: vi.fn(),
  },
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    constructor() {
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
});
