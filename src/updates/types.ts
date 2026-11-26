import type { SessionInfo } from "../terminal/types";

export type UpdateStatus = {
  currentVersion: string;
  latestVersion: string | null;
  updateAvailable: boolean;
  downloadUrl: string | null;
  notes: string | null;
  channel: string;
  message: string;
};

export type UpdateActivity = {
  liveSessions: SessionInfo[];
  dispatchActive: boolean;
  scheduledTaskId: string | null;
  hasActiveWork: boolean;
};

export type PreparedUpdateStatus = {
  downloadUrl: string;
  latestVersion: string | null;
  ready: boolean;
};

export type InstallUpdateOutcome =
  | { status: "installing"; version: string | null }
  | {
      status: "blocked";
      activity: UpdateActivity;
      installerReady: boolean;
    };

export type RecoverableSession = {
  id: string;
  toolId: string;
  toolName: string;
  projectPath: string;
  title?: string | null;
  cliSessionId?: string | null;
};

export type UpdateRecoverySnapshot = {
  createdAtMs: number;
  targetVersion?: string | null;
  sessions: RecoverableSession[];
  hadActiveDispatch: boolean;
  interruptedScheduledTaskId?: string | null;
};
