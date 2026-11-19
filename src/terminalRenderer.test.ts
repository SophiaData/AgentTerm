// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";

const sdk = vi.hoisted(() => ({
  open: vi.fn(),
  loadAddon: vi.fn(),
  write: vi.fn(),
  focus: vi.fn(),
  disposeTerminal: vi.fn(),
  disposeInput: vi.fn(),
  fit: vi.fn(),
  propose: vi.fn((): { rows: number; cols: number } | null => ({
    rows: 24,
    cols: 80,
  })),
  onInput: null as ((data: string) => void) | null,
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    rows = 24;
    cols = 80;
    open = sdk.open;
    loadAddon = sdk.loadAddon;
    write = sdk.write;
    focus = sdk.focus;
    dispose = sdk.disposeTerminal;
    onData(callback: (data: string) => void) {
      sdk.onInput = callback;
      return { dispose: sdk.disposeInput };
    }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = sdk.fit;
    proposeDimensions = sdk.propose;
  },
}));

import { createTerminalRenderer } from "./terminalRenderer";

beforeEach(() => {
  vi.clearAllMocks();
  sdk.onInput = null;
  sdk.propose.mockReturnValue({ rows: 24, cols: 80 });
});

describe("real xterm renderer adapter SDK contract", () => {
  it("attaches xterm to its host and wires keyboard, display, focus and disposal", () => {
    const host = document.createElement("div");
    const input = vi.fn();
    const renderer = createTerminalRenderer(host, input);
    expect(sdk.open).toHaveBeenCalledWith(host);
    expect(sdk.loadAddon).toHaveBeenCalledTimes(1);
    renderer.write("ANSI \u001b[32mok\u001b[0m");
    expect(sdk.write).toHaveBeenCalledWith("ANSI \u001b[32mok\u001b[0m");
    sdk.onInput?.("pwd\r");
    expect(input).toHaveBeenCalledWith("pwd\r");
    renderer.focus();
    expect(sdk.focus).toHaveBeenCalledTimes(1);
    renderer.dispose();
    expect(sdk.disposeInput).toHaveBeenCalledTimes(1);
    expect(sdk.disposeTerminal).toHaveBeenCalledTimes(1);
  });

  it("uses fit addon dimensions only when sane and refuses 964-row blank screens", () => {
    const renderer = createTerminalRenderer(
      document.createElement("div"),
      vi.fn(),
    );
    sdk.propose.mockReturnValueOnce({ rows: 964, cols: 80 });
    expect(renderer.fit()).toBeNull();
    expect(sdk.fit).not.toHaveBeenCalled();
    sdk.propose.mockReturnValueOnce({ rows: 2, cols: 80 });
    expect(renderer.fit()).toBeNull();
    expect(sdk.fit).not.toHaveBeenCalled();
    sdk.propose.mockReturnValueOnce(null);
    expect(renderer.fit()).toBeNull();
    sdk.propose.mockReturnValueOnce({ rows: 24, cols: 80 });
    expect(renderer.fit()).toEqual({ rows: 24, cols: 80 });
    expect(sdk.fit).toHaveBeenCalledTimes(1);
  });
});
