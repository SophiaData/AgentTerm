export type ScheduledRun = {
  startedAtMs: number;
  finishedAtMs: number;
  status: string;
  exitCode: number | null;
  output: string;
};

export type ScheduledTask = {
  id: string;
  name: string;
  projectPath: string;
  toolId: string;
  model: string | null;
  prompt: string;
  intervalMinutes: number;
  timeoutMinutes: number;
  enabled: boolean;
  nextRunAtMs: number;
  runs: ScheduledRun[];
};

export type ScheduledTaskDraft = {
  name: string;
  projectPath: string;
  toolId: string;
  model: string;
  prompt: string;
  intervalMinutes: number;
  timeoutMinutes: number;
  enabled: boolean;
};

export const EMPTY_SCHEDULE_DRAFT: ScheduledTaskDraft = {
  name: "",
  projectPath: "",
  toolId: "claude",
  model: "",
  prompt: "",
  intervalMinutes: 60,
  timeoutMinutes: 20,
  enabled: true,
};
