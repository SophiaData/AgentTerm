import type { Terminal } from "@xterm/xterm";

type Options = {
  terminal: Pick<Terminal, "getSelection" | "paste" | "focus">;
  readText: () => Promise<string>;
  writeText: (text: string) => Promise<void>;
  canPaste: () => boolean;
  problem: (message: string | null) => void;
};

export function createTerminalClipboard({
  terminal,
  readText,
  writeText,
  canPaste,
  problem,
}: Options) {
  let active = true;
  let pasting = false;
  let revision = 0;

  const copy = async () => {
    if (!active) return;
    const selection = terminal.getSelection();
    if (!selection) {
      problem(
        "请先拖动选中要复制的终端文字。CLI 占用鼠标时可按住 Shift 选择。",
      );
      return;
    }
    try {
      await writeText(selection);
      if (active) problem(null);
    } catch {
      if (active) problem("复制失败，系统剪贴板暂不可用，请重试。");
    }
  };

  const paste = async () => {
    if (!active || pasting || !canPaste()) return;
    pasting = true;
    const requestedRevision = revision;
    try {
      const text = await readText();
      if (!active || revision !== requestedRevision || !canPaste()) return;
      problem(null);
      if (text) terminal.paste(text);
      terminal.focus();
    } catch {
      if (active && revision === requestedRevision && canPaste()) {
        problem("粘贴失败，剪贴板中可能没有文本或暂不可用，请重试。");
      }
    } finally {
      pasting = false;
    }
  };

  const key = (event: KeyboardEvent) => {
    if (!active || event.altKey || event.isComposing) return true;
    const pressed = event.key.toLowerCase();
    const control = event.ctrlKey && !event.metaKey;
    const command = event.metaKey && !event.ctrlKey;
    const copying =
      (pressed === "c" &&
        (command ||
          (control && (event.shiftKey || !!terminal.getSelection())))) ||
      (pressed === "insert" && control && !event.shiftKey);
    const pastingKey =
      (pressed === "v" && (control || command)) ||
      (pressed === "insert" &&
        event.shiftKey &&
        !event.ctrlKey &&
        !event.metaKey);
    if (!copying && !pastingKey) return true;
    event.preventDefault();
    event.stopPropagation();
    if (event.type === "keydown" && !event.repeat) {
      if (copying) void copy();
      else void paste();
    }
    return false;
  };

  return {
    copy,
    paste,
    key,
    invalidate: () => {
      revision += 1;
    },
    dispose: () => {
      active = false;
    },
  };
}
