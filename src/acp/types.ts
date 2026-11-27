export type AcpPhase = "connecting" | "ready" | "exited" | "error";

export type AcpMessage = {
  id: string;
  role: "user" | "assistant" | "system";
  text: string;
};

export type AcpToolCall = {
  id: string;
  title: string;
  name?: string | null;
  kind?: string | null;
  status: string;
  detail?: string | null;
};

export type AcpPermissionOption = {
  optionId: string;
  name: string;
  kind: string;
};

export type AcpPermissionRequest = {
  requestId: string;
  title: string;
  description?: string | null;
  toolCallId?: string | null;
  options: AcpPermissionOption[];
};

export type AcpSnapshot = {
  phase: AcpPhase;
  busy: boolean;
  cliSessionId?: string | null;
  error?: string | null;
  exitReason?: string | null;
  messages: AcpMessage[];
  toolCalls: AcpToolCall[];
  pendingPermission?: AcpPermissionRequest | null;
  version: number;
};

export function isAcpTool(toolId: string): boolean {
  return toolId === "dsh";
}
