export type SessionInfo = {
  id: string;
  toolId: string;
  toolName: string;
  projectPath: string;
  title?: string | null;
  cliSessionId?: string | null;
};

export type SessionExit = {
  id: string;
  reason: string;
};

export type SessionHistoryRecord = {
  id: string;
  toolId: string;
  toolName: string;
  projectPath: string;
  title?: string | null;
  cliSessionId?: string | null;
  startedAtMs: number;
  endedAtMs: number | null;
  exitReason: string | null;
  outputBytes: number | null;
};

export function sessionTitle(session: SessionInfo): string {
  const parts = session.projectPath.split(/[\\/]/).filter(Boolean);
  const folder = parts[parts.length - 1] || session.projectPath;

  return `${folder} · ${session.toolName}`;
}

export function agentBadgeLabel(toolId: string, toolName?: string): string {
  switch (toolId) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "agy":
      return "Agy";
    case "dsh":
      return "DSH";
    default:
      return toolName?.trim() || toolId;
  }
}

export function newSessionId(): string {
  const crypto = globalThis.crypto;
  if (typeof crypto?.randomUUID === "function") {
    return `session-${crypto.randomUUID()}`;
  }
  if (!crypto) throw new Error("安全随机数不可用");
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return `session-${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}
