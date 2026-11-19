import type { Project, ToolStatus } from "./types";

export type StartSessionArgs = {
  sessionId: string;
  toolId: string;
  projectPath: string;
  rows: number;
  cols: number;
};

export type SessionInfo = {
  id: string;
  toolId: string;
  toolName: string;
  projectPath: string;
  /** Starting succeeded; remembering the preferred tool may still fail. */
  warning?: string;
};

export type SessionSnapshot = {
  data: string;
  /** UTF-8 byte offset following all data in the snapshot. */
  offset: number;
  running: boolean;
};

export type SessionOutput = {
  data: string;
  endOffset: number;
};

export type SessionExit = {
  id: string;
  reason: string;
};

export type Unlisten = () => void;

export interface Backend {
  listProjects(): Promise<Project[]>;
  addProject(path: string): Promise<Project>;
  removeProject(id: string): Promise<Project>;
  detectTools(): Promise<ToolStatus[]>;
  pickDirectory(): Promise<string | null>;
  startSession(args: StartSessionArgs): Promise<SessionInfo>;
  sessionSnapshot(sessionId: string): Promise<SessionSnapshot>;
  writeSessionInput(sessionId: string, data: string): Promise<void>;
  resizeSession(sessionId: string, rows: number, cols: number): Promise<void>;
  stopSession(sessionId: string): Promise<void>;
  forgetSession(sessionId: string): Promise<void>;
  onSessionOutput(
    sessionId: string,
    listener: (output: SessionOutput) => void,
  ): Promise<Unlisten>;
  onSessionExit(
    sessionId: string,
    listener: (exit: SessionExit) => void,
  ): Promise<Unlisten>;
}

export const backend: Backend = {
  async listProjects() {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Project[]>("list_projects");
  },
  async addProject(path) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Project>("add_project", { path });
  },
  async removeProject(id) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<Project>("remove_project", { id });
  },
  async detectTools() {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<ToolStatus[]>("detect_tools");
  },
  async pickDirectory() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择项目文件夹",
    });
    return typeof selected === "string" ? selected : null;
  },
  async startSession(args) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<SessionInfo>("start_session", args);
  },
  async sessionSnapshot(sessionId) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<SessionSnapshot>("session_snapshot", { sessionId });
  },
  async writeSessionInput(sessionId, data) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("write_session_input", { sessionId, data });
  },
  async resizeSession(sessionId, rows, cols) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("resize_session", { sessionId, rows, cols });
  },
  async stopSession(sessionId) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("stop_session", { sessionId });
  },
  async forgetSession(sessionId) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("forget_session", { sessionId });
  },
  async onSessionOutput(sessionId, listener) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<SessionOutput>(`session-output-${sessionId}`, (event) => {
      listener(event.payload);
    });
  },
  async onSessionExit(sessionId, listener) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<SessionExit>(`session-exit-${sessionId}`, (event) => {
      listener(event.payload);
    });
  },
};
