export const SESSION_TITLE_MAX_CHARS = 60;
export const AUTO_TITLE_MAX_CHARS = 24;
const PROMPT_BUFFER_MAX_CHARS = 512;

export type TitleValidation =
  { ok: true; title: string } | { ok: false; error: string };

const IGNORED_SHORT_COMMANDS = new Set([
  "y",
  "n",
  "yes",
  "no",
  "ok",
  "q",
  "quit",
  "exit",
  "clear",
  "cls",
  "ls",
  "dir",
  "pwd",
  "cd",
  "hi",
  "hello",
]);

const SENSITIVE_PATTERNS = [
  /\bsk-[a-z0-9_-]{8,}/i,
  /\bgh[po]_[a-z0-9_]{8,}/i,
  /\bgithub_pat_[a-z0-9_]{8,}/i,
  /\bakia[0-9a-z]{12,}/i,
  /\bbearer\s+\S+/i,
  /\b[A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|API_?KEY)[A-Z0-9_]*=/,
  /(?:^|[^a-z0-9_])(?:password|passwd|token|cookie|api_?key|secret)\s*[:=]/i,
  /-----begin\b/i,
];

function hasControlChars(value: string): boolean {
  for (const ch of value) {
    const code = ch.codePointAt(0) ?? 0;
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

function stripControlChars(value: string): string {
  let out = "";
  for (const ch of value) {
    const code = ch.codePointAt(0) ?? 0;
    out += code < 0x20 || (code >= 0x7f && code <= 0x9f) ? " " : ch;
  }
  return out;
}

export function normalizeManualTitle(raw: string): TitleValidation {
  if (hasControlChars(raw)) {
    return { ok: false, error: "会话名称不能包含控制字符或换行" };
  }
  const normalized = raw.trim().replace(/\s+/g, " ");
  if (!normalized) {
    return { ok: false, error: "会话名称不能为空" };
  }
  const charCount = Array.from(normalized).length;
  const byteCount = new TextEncoder().encode(normalized).length;
  if (charCount > SESSION_TITLE_MAX_CHARS || byteCount > 240) {
    return {
      ok: false,
      error: `会话名称最多 ${SESSION_TITLE_MAX_CHARS} 个字符`,
    };
  }
  return { ok: true, title: normalized };
}

export function deriveAutoTitleFromPrompt(rawLine: string): string | null {
  const cleaned = stripControlChars(rawLine).trim().replace(/\s+/g, " ");
  if (!cleaned) return null;
  if (cleaned.startsWith("/")) return null;
  if (IGNORED_SHORT_COMMANDS.has(cleaned.toLowerCase())) return null;
  if (SENSITIVE_PATTERNS.some((pattern) => pattern.test(cleaned))) return null;

  const meaningful = cleaned.match(/[\p{L}\p{N}]/gu);
  if (!meaningful || meaningful.length < 2) return null;

  const chars = Array.from(cleaned);
  if (chars.length <= AUTO_TITLE_MAX_CHARS) return cleaned;
  const head = chars
    .slice(0, AUTO_TITLE_MAX_CHARS - 1)
    .join("")
    .trimEnd();
  return head ? `${head}…` : null;
}

type EscapeMode = "normal" | "esc" | "csi" | "osc" | "osc_esc" | "ss3";

/**
 * Tracks terminal input sent to a PTY and emits completed user lines while
 * stripping VT/ANSI escape sequences, arrow keys, focus reports and bracketed
 * paste markers.
 */
export function createPromptLineTracker() {
  let mode: EscapeMode = "normal";
  let buffer: string[] = [];

  const feed = (chunk: string): string[] => {
    const lines: string[] = [];
    for (const ch of chunk) {
      const code = ch.codePointAt(0) ?? 0;
      if (mode === "esc") {
        if (ch === "[") mode = "csi";
        else if (ch === "]") mode = "osc";
        else if (ch === "O") mode = "ss3";
        else mode = "normal";
        continue;
      }
      if (mode === "csi") {
        if (code >= 0x40 && code <= 0x7e) mode = "normal";
        continue;
      }
      if (mode === "osc") {
        if (code === 0x07) mode = "normal";
        else if (code === 0x1b) mode = "osc_esc";
        continue;
      }
      if (mode === "osc_esc") {
        mode = ch === "\\" ? "normal" : "osc";
        continue;
      }
      if (mode === "ss3") {
        mode = "normal";
        continue;
      }

      if (code === 0x1b) {
        mode = "esc";
      } else if (ch === "\r" || ch === "\n") {
        if (buffer.length > 0) {
          lines.push(buffer.join(""));
          buffer = [];
        }
      } else if (code === 0x7f || code === 0x08) {
        buffer.pop();
      } else if (code === 0x03 || code === 0x15) {
        buffer = [];
      } else if (ch === "\t") {
        if (buffer.length < PROMPT_BUFFER_MAX_CHARS) buffer.push(" ");
      } else if (code >= 0x20 && !(code >= 0x7f && code <= 0x9f)) {
        if (buffer.length < PROMPT_BUFFER_MAX_CHARS) buffer.push(ch);
      }
    }
    return lines;
  };

  return { feed };
}
