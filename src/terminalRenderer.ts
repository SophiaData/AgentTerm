import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

/** Narrow renderer boundary: DOM tests can mock this without loading canvas. */
export interface TerminalRenderer {
  write(text: string): void;
  focus(): void;
  /** Never mutate rows when the viewport is implausibly sized. */
  fit(): { rows: number; cols: number } | null;
  dispose(): void;
}

export function createTerminalRenderer(
  host: HTMLElement,
  onInput: (data: string) => void,
): TerminalRenderer {
  const terminal = new Terminal({
    cols: 80,
    rows: 24,
    cursorBlink: true,
    scrollback: 3000,
    fontSize: 13,
    fontFamily: '"SFMono-Regular", Menlo, Consolas, monospace',
    theme: { background: "#101827", foreground: "#e8eef8", cursor: "#22c55e" },
  });
  const fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.open(host);
  const inputSubscription = terminal.onData(onInput);
  return {
    write(text) {
      terminal.write(text);
    },
    focus() {
      terminal.focus();
    },
    fit() {
      const dimensions = fitAddon.proposeDimensions();
      if (
        !dimensions ||
        !Number.isInteger(dimensions.rows) ||
        !Number.isInteger(dimensions.cols) ||
        dimensions.rows < 3 ||
        dimensions.rows > 160 ||
        dimensions.cols < 20 ||
        dimensions.cols > 500
      ) {
        return null;
      }
      fitAddon.fit();
      return { rows: terminal.rows, cols: terminal.cols };
    },
    dispose() {
      inputSubscription.dispose();
      terminal.dispose();
    },
  };
}
