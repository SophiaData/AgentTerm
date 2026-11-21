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
  sessionLink?: SessionLink | null;
};
export type SessionLink = {
  id: string;
  toolId: string;
  toolName: string;
  projectPath: string;
};
export type WorkbenchView = {
  ledger: { revision: number; generation: number; active: MainTask | null };
  document: TaskDocument | null;
  recovery?: RecoveryView | null;
};
export type RecoveryAttempt = {
  attemptId: string;
  stepId: string;
  observation: "launchUncertain" | "exited" | "alive" | "reused" | "unknown";
  safeToResume: boolean;
};
export type RecoveryView = {
  attempts: RecoveryAttempt[];
  allExited: boolean;
};
export type PlanInput = {
  projectId: string;
  goal: string;
  owner: string;
  scope: string;
  outputDir: string;
};
export type Preview = { id: string; sha256: string; content: string };

export function emptyPlan(): PlanInput {
  return {
    projectId: "",
    goal: "",
    owner: "主 Agent",
    scope: "",
    outputDir: ".",
  };
}

/** Only auto-select when there is exactly one choice and no prior selection. */
export function selectedProject(
  draft: PlanInput,
  projects: { id: string }[],
  activeProject?: string,
): string {
  if (activeProject) return activeProject;
  if (draft.projectId)
    return projects.some((project) => project.id === draft.projectId)
      ? draft.projectId
      : "";
  return projects.length === 1 ? projects[0].id : "";
}

export function planProblem(input: PlanInput): string | null {
  if (!input.projectId) return "请先选择或添加一个项目。";
  if (!input.goal.trim()) return "请填写这次希望完成的任务。";
  if (!input.scope.trim()) return "请说明本次允许处理的范围。";
  if (!input.owner.trim()) return "请填写负责人。";
  if (!input.outputDir.trim()) return "请选择一个已有的交付目录。";
  return null;
}

/** A rejected action is retryable after a successful state read, not before. */
export function actionsBlocked(
  busy: boolean,
  loading: boolean,
  readError: string | null,
): boolean {
  return busy || loading || readError !== null;
}

export const phaseLabels: Record<string, string> = {
  reserved: "计划已保存 · 未开始执行",
  recoveredReserved: "已接回任务 · 未开始执行",
  running: "执行中",
  verifying: "独立验收中",
  waitingInput: "等待确认",
  waitingApproval: "等待授权",
  pausing: "正在暂停",
  recoveryRequired: "待核实",
};

export const phaseMessages: Record<string, string> = {
  running: "执行 Agent 正在工作，任务仍占用执行位置。",
  verifying: "执行 Agent 已交回，验收 Agent 正在独立核对。",
  waitingInput: "本轮接力已结束，结果已写入记录，等待你核对结果或补充信息。",
  waitingApproval: "任务正在等待授权，当前占用不会自动释放。",
  pausing: "正在确认停止和外部残留，确认前不能切换任务。",
  recoveryRequired: "执行状态无法直接确认，系统保留占用，需先核实旧尝试。",
};

export function isReservation(phase: string): boolean {
  return phase === "reserved" || phase === "recoveredReserved";
}
export const attemptLabels: Record<AttemptEvent["state"], string> = {
  running: "执行意图已登记",
  reported: "结果已提交",
  blocked: "等待补充",
  failed: "此步骤未通过",
  unknown: "结果待核实",
};

export const recoveryObservationLabels: Record<
  RecoveryAttempt["observation"],
  string
> = {
  launchUncertain: "启动边界未确认",
  exited: "已确认退出",
  alive: "仍在运行",
  reused: "PID 已复用",
  unknown: "无法确认",
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
