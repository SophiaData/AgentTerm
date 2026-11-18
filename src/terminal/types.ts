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

export function sessionTitle(session: SessionInfo): string {
  const parts = session.projectPath.split("/");
  const folder = parts[parts.length - 1] || session.projectPath;

  return `${folder} · ${session.toolName}`;
}

export function newSessionId(): string {
  return `session-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
