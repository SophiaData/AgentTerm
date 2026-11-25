export type ScheduledRun = {
  startedAtMs: number;
  finishedAtMs: number;
  status: string;
  exitCode: number | null;
  output: string;
};

export type ScheduleSpec =
  | { kind: "interval"; minutes: number }
  | { kind: "daily"; hour: number; minute: number };

export type ScheduledTask = {
  id: string;
  name: string;
  projectPath: string;
  toolId: string;
  model: string | null;
  prompt: string;
  intervalMinutes?: number;
  schedule?: ScheduleSpec | null;
  timeoutMinutes: number;
  enabled: boolean;
  approvalPending?: boolean;
  nextRunAtMs: number;
  runs: ScheduledRun[];
  sourceSessionId?: string | null;
};
