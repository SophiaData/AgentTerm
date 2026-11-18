import type { Project, ToolStatus } from "./types";

export type LaunchNativeResult = { launched: boolean; warning?: string };

export interface Backend {
  listProjects(): Promise<Project[]>;
  addProject(path: string): Promise<Project>;
  removeProject(id: string): Promise<Project>;
  detectTools(): Promise<ToolStatus[]>;
  pickDirectory(): Promise<string | null>;
  launchNativeTool(
    toolId: string,
    projectPath: string,
  ): Promise<LaunchNativeResult>;
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
  async launchNativeTool(toolId, projectPath) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<LaunchNativeResult>("launch_native_tool", {
      toolId,
      projectPath,
    });
  },
};
