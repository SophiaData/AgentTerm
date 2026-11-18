import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import packageJson from "../package.json";
import { backend } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

beforeEach(() => {
  vi.resetAllMocks();
});

describe("Tauri command contract", () => {
  it("reads projects from Rust", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);
    expect(await backend.listProjects()).toEqual([]);
    expect(invoke).toHaveBeenCalledWith("list_projects");
  });

  it("adds a project with a typed path argument", async () => {
    const project = { id: "/repo", name: "repo", path: "/repo" };
    vi.mocked(invoke).mockResolvedValueOnce(project);
    expect(await backend.addProject("/repo")).toEqual(project);
    expect(invoke).toHaveBeenCalledWith("add_project", { path: "/repo" });
  });

  it("removes only the named project record", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({});
    await backend.removeProject("/repo");
    expect(invoke).toHaveBeenCalledWith("remove_project", { id: "/repo" });
  });

  it("checks installed tools through Rust", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);
    await backend.detectTools();
    expect(invoke).toHaveBeenCalledWith("detect_tools");
  });

  it("passes only tool ID and project path to native launch", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ launched: true });
    expect(await backend.launchNativeTool("codex", "/repo")).toEqual({
      launched: true,
    });
    expect(invoke).toHaveBeenCalledWith("launch_native_tool", {
      toolId: "codex",
      projectPath: "/repo",
    });
  });

  it("uses a directory-only native chooser", async () => {
    vi.mocked(open).mockResolvedValueOnce("/repo");
    expect(await backend.pickDirectory()).toBe("/repo");
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "选择项目文件夹",
    });
  });

  it("does not accept a non-string dialog result as a directory", async () => {
    vi.mocked(open).mockResolvedValueOnce(null);
    expect(await backend.pickDirectory()).toBeNull();
  });
});

it("has no React, xterm or substitute UI framework production dependency", () => {
  const forbidden = [
    "react",
    "react-dom",
    "@xterm/xterm",
    "@xterm/addon-fit",
    "svelte",
    "vue",
    "preact",
    "solid-js",
  ];
  for (const name of forbidden) {
    expect(packageJson.dependencies).not.toHaveProperty(name);
  }
});
