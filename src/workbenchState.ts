export type MainTask = {
  taskId: string;
  projectId: string;
  phase: string;
  recoveryRequired: boolean;
};
export type Step = { id: string; title: string; assignee: string };
export type AttemptEvent = {
  id: string;
  sequence: number;
  attemptId: string;
  stepId: string;
  state: "running" | "reported" | "blocked" | "failed" | "unknown";
  summary: string;
};
export type Artifact = {
  id: string;
  relativePath: string;
  sha256: string;
  bytes: number;
  version: number;
  label: string;
};
export type TaskDocument = {
  revision: number;
  plan: {
    taskId: string;
    generation: number;
    projectId: string;
    goal: string;
    owner: string;
    scope: string;
    outputDir: string;
    steps: Step[];
  };
  events: AttemptEvent[];
  artifacts: Artifact[];
};
export type WorkbenchView = {
  ledger: { revision: number; generation: number; active: MainTask | null };
  document: TaskDocument | null;
};
export type PlanInput = {
  projectId: string;
  goal: string;
  owner: string;
  scope: string;
  outputDir: string;
};
export type Preview = { id: string; sha256: string; content: string };

export const phaseLabels: Record<string, string> = {
  reserved: "已预留 · 尚未派发",
  running: "执行中",
  verifying: "独立验收中",
  waitingInput: "等待确认",
  waitingApproval: "等待授权",
  pausing: "正在暂停",
  recoveryRequired: "待核实",
};
export const attemptLabels: Record<AttemptEvent["state"], string> = {
  running: "执行意图已登记",
  reported: "结果已提交",
  blocked: "等待补充",
  failed: "验收未通过",
  unknown: "结果待核实",
};

/** Derived display only; never advances the main task or grants approval. */
export function latestAttempts(events: AttemptEvent[]): AttemptEvent[] {
  const latest = new Map<string, AttemptEvent>();
  for (const event of events) {
    const before = latest.get(event.attemptId);
    if (!before || event.sequence > before.sequence) {
      latest.set(event.attemptId, event);
    }
  }
  return [...latest.values()].sort((a, b) => a.sequence - b.sequence);
}

/** A late poll or old mutation response cannot replace newer task state. */
export function acceptSnapshot(
  previous: WorkbenchView | null,
  incoming: WorkbenchView,
): WorkbenchView {
  if (!previous) return incoming;
  if (incoming.ledger.revision < previous.ledger.revision) return previous;
  if (
    incoming.ledger.revision === previous.ledger.revision &&
    incoming.ledger.generation === previous.ledger.generation &&
    (incoming.document?.revision ?? 0) < (previous.document?.revision ?? 0)
  )
    return previous;
  return incoming;
}

/** Shared request generation protects both success and failure completions. */
export function requestOrder() {
  let sequence = 0;
  return {
    next: () => ++sequence,
    current: (request: number) => request === sequence,
  };
}
