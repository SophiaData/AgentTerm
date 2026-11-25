import { describe, expect, it, vi } from "vitest";
import { createTerminalClipboard } from "./terminalClipboard";

function fixture() {
  const terminal = {
    getSelection: vi.fn(() => "选中文字 🧪"),
    paste: vi.fn(),
    focus: vi.fn(),
  };
  const readText = vi.fn(async () => "第一行\r\n第二行 🧪\n");
  const writeText = vi.fn<(text: string) => Promise<void>>(async () => {});
  const canPaste = vi.fn(() => true);
  const problem = vi.fn();
  const clipboard = createTerminalClipboard({
    terminal,
    readText,
    writeText,
    canPaste,
    problem,
  });
  return { terminal, readText, writeText, canPaste, problem, clipboard };
}

function keyboard(key: string, options: Partial<KeyboardEvent> = {}) {
  return {
    key,
    type: "keydown",
    ctrlKey: true,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    repeat: false,
    isComposing: false,
    preventDefault: vi.fn(),
    stopPropagation: vi.fn(),
    ...options,
  } as unknown as KeyboardEvent;
}

function deferredText() {
  let resolve!: (text: string) => void;
  const promise = new Promise<string>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("terminal clipboard", () => {
  it("copies the xterm selection and consumes Ctrl+C without interrupting", async () => {
    const { clipboard, writeText, terminal } = fixture();
    const event = keyboard("c");
    expect(clipboard.key(event)).toBe(false);
    await Promise.resolve();
    expect(event.preventDefault).toHaveBeenCalledOnce();
    expect(event.stopPropagation).toHaveBeenCalledOnce();
    expect(writeText).toHaveBeenCalledExactlyOnceWith("选中文字 🧪");
    expect(terminal.paste).not.toHaveBeenCalled();
  });

  it("preserves Ctrl+C as a CLI interrupt when there is no selection", () => {
    const { clipboard, terminal, writeText } = fixture();
    terminal.getSelection.mockReturnValue("");
    const event = keyboard("c");
    expect(clipboard.key(event)).toBe(true);
    expect(event.preventDefault).not.toHaveBeenCalled();
    expect(writeText).not.toHaveBeenCalled();
  });

  it.each([
    ["C", { shiftKey: true }],
    ["c", { ctrlKey: false, metaKey: true }],
    ["Insert", {}],
  ])("handles explicit copy %s with %j", async (key, modifiers) => {
    const { clipboard, writeText } = fixture();
    expect(clipboard.key(keyboard(key, modifiers))).toBe(false);
    await Promise.resolve();
    expect(writeText).toHaveBeenCalledOnce();
  });

  it.each([
    ["v", {}],
    ["V", { shiftKey: true }],
    ["v", { ctrlKey: false, metaKey: true }],
    ["Insert", { ctrlKey: false, shiftKey: true }],
  ])("pastes %s through xterm with %j", async (key, modifiers) => {
    const { clipboard, terminal, readText } = fixture();
    const event = keyboard(key, modifiers);
    expect(clipboard.key(event)).toBe(false);
    await Promise.resolve();
    expect(readText).toHaveBeenCalledOnce();
    expect(terminal.paste).toHaveBeenCalledExactlyOnceWith(
      "第一行\r\n第二行 🧪\n",
    );
    expect(terminal.focus).toHaveBeenCalledOnce();
    expect(event.preventDefault).toHaveBeenCalledOnce();
  });

  it("does not replace the clipboard or send an interrupt for explicit empty copy", async () => {
    const { clipboard, terminal, writeText, problem } = fixture();
    terminal.getSelection.mockReturnValue("");
    expect(clipboard.key(keyboard("C", { shiftKey: true }))).toBe(false);
    await clipboard.copy();
    expect(writeText).not.toHaveBeenCalled();
    expect(problem).toHaveBeenCalledWith(expect.stringContaining("先拖动选中"));
  });

  it("does not consume AltGr, composition, or ordinary input", () => {
    const { clipboard, readText, writeText } = fixture();
    for (const event of [
      keyboard("v", { altKey: true }),
      keyboard("v", { isComposing: true }),
      keyboard("v", { ctrlKey: false }),
      keyboard("x"),
    ]) {
      expect(clipboard.key(event)).toBe(true);
      expect(event.preventDefault).not.toHaveBeenCalled();
    }
    expect(readText).not.toHaveBeenCalled();
    expect(writeText).not.toHaveBeenCalled();
  });

  it("does not paste twice on key repeat, keypress, or keyup", async () => {
    const { clipboard, terminal, readText } = fixture();
    clipboard.key(keyboard("v"));
    await Promise.resolve();
    for (const options of [
      { repeat: true },
      { type: "keypress" },
      { type: "keyup" },
    ]) {
      expect(clipboard.key(keyboard("v", options))).toBe(false);
    }
    expect(readText).toHaveBeenCalledOnce();
    expect(terminal.paste).toHaveBeenCalledOnce();
  });

  it("does not access the clipboard when input is blocked", async () => {
    const { clipboard, canPaste, readText, terminal } = fixture();
    canPaste.mockReturnValue(false);
    await clipboard.paste();
    expect(readText).not.toHaveBeenCalled();
    expect(terminal.paste).not.toHaveBeenCalled();
  });

  it.each(["blocked", "switched", "disposed"])(
    "discards pending clipboard data when %s",
    async (transition) => {
      const { clipboard, canPaste, readText, terminal, problem } = fixture();
      const pending = deferredText();
      readText.mockReturnValue(pending.promise);
      const request = clipboard.paste();
      if (transition === "blocked") canPaste.mockReturnValue(false);
      if (transition === "switched") clipboard.invalidate();
      if (transition === "disposed") clipboard.dispose();
      pending.resolve("不应输入的测试文本");
      await request;
      expect(terminal.paste).not.toHaveBeenCalled();
      expect(terminal.focus).not.toHaveBeenCalled();
      expect(problem).not.toHaveBeenCalled();
    },
  );

  it("serializes reads so overlapping paste gestures cannot reorder text", async () => {
    const { clipboard, readText, terminal } = fixture();
    const pending = deferredText();
    readText.mockReturnValueOnce(pending.promise);
    const first = clipboard.paste();
    await clipboard.paste();
    expect(readText).toHaveBeenCalledOnce();
    pending.resolve("第一段");
    await first;
    await clipboard.paste();
    expect(terminal.paste.mock.calls).toEqual([
      ["第一段"],
      ["第一行\r\n第二行 🧪\n"],
    ]);
  });

  it("reports clipboard failures without exposing contents and allows retry", async () => {
    const { clipboard, readText, writeText, problem, terminal } = fixture();
    readText.mockRejectedValueOnce(new Error("synthetic-private-detail"));
    writeText.mockRejectedValueOnce(new Error("synthetic-private-detail"));
    await clipboard.paste();
    await clipboard.copy();
    expect(problem.mock.calls.flat().join()).not.toContain(
      "synthetic-private-detail",
    );
    expect(problem).toHaveBeenCalledWith(expect.stringContaining("粘贴失败"));
    expect(problem).toHaveBeenCalledWith(expect.stringContaining("复制失败"));
    await clipboard.paste();
    expect(terminal.paste).toHaveBeenCalledOnce();
  });

  it("does not send empty clipboard text to the CLI", async () => {
    const { clipboard, readText, terminal } = fixture();
    readText.mockResolvedValue("");
    await clipboard.paste();
    expect(terminal.paste).not.toHaveBeenCalled();
    expect(terminal.focus).toHaveBeenCalledOnce();
  });
});
