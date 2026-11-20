export type SessionInfo = {
  id: string;
  toolId: string;
  toolName: string;
  projectPath: string;
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

export function newSessionId(): string {
  return `session-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
