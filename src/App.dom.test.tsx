// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import * as launcherModule from "./terminal/useLauncher";
import * as historyModule from "./terminal/useSessionHistory";
import * as toolsModule from "./tools/useTools";
import * as projectsModule from "./projects/useProjects";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./terminal/SessionWorkspace", () => ({ default: () => null }));
vi.mock("./files/ProjectFiles", () => ({ default: () => null }));

const session = {
  info: {
    id: "session-1",
    toolId: "claude",
    toolName: "Claude Code",
    projectPath: "E:\\AgentTerm",
  },
  label: "Claude Code · AgentTerm",
  titleSource: "default" as const,
  running: true,
  statusUnknown: false,
  exitReason: null,
  closeError: null,
};

beforeEach(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: () => ({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }),
  });
  vi.spyOn(projectsModule, "useProjects").mockReturnValue({
    projects: [{ id: "project-1", name: "AgentTerm", path: "E:\\AgentTerm" }],
    busy: false,
    loading: false,
    error: null,
    pendingRemoval: null,
    add: vi.fn(),
    requestRemove: vi.fn(),
    cancelRemove: vi.fn(),
    confirmRemove: vi.fn(),
    refresh: vi.fn(),
  });
  vi.spyOn(toolsModule, "useTools").mockReturnValue({
    tools: [
      {
        id: "claude",
        name: "Claude Code",
        state: { kind: "installed", path: "claude.exe", version: "2.1.336" },
      },
    ],
    loading: false,
    error: null,
    refresh: vi.fn(),
  });
  vi.spyOn(historyModule, "useSessionHistory").mockReturnValue({
    records: [],
    loading: false,
    error: null,
    reload: vi.fn(),
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  invoke.mockReset();
});

function mockLauncher(close = vi.fn()) {
  vi.spyOn(launcherModule, "useLauncher").mockReturnValue({
    sessions: [session],
    activeId: session.info.id,
    paneIds: [session.info.id],
    focusRequest: 0,
    starting: false,
    closingId: null,
    startError: null,
    busy: false,
    session,
    select: vi.fn(),
    focus: vi.fn(),
    split: vi.fn(),
    single: vi.fn(),
    showLauncher: vi.fn(),
    start: vi.fn(),
    close,
    handleExit: vi.fn(),
    handleConnection: vi.fn(),
    reconcile: vi.fn(async () => true),
    rename: vi.fn(),
    autoName: vi.fn(),
  });
  return close;
}

describe("App interactions", () => {
  it("requires confirmation before closing a running tab and supports Escape", async () => {
    const close = mockLauncher();
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: /关闭 Claude Code/ }));
    expect(screen.getByRole("alertdialog").textContent).toContain("仍在运行");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(close).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /关闭 Claude Code/ }));
    await user.click(screen.getByRole("button", { name: "确认结束" }));
    expect(close).toHaveBeenCalledWith(session.info.id);
  });

  it("shows the authoritative update result", async () => {
    mockLauncher();
    invoke.mockResolvedValueOnce({
      currentVersion: "0.2.0-rc.8",
      latestVersion: "0.2.0-rc.8",
      updateAvailable: false,
      downloadUrl: null,
      notes: null,
      channel: "prerelease",
      message: "当前已是 0.2.0-rc.8",
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("check_for_update"),
    );
    expect(await screen.findByText("当前已是 0.2.0-rc.8")).toBeTruthy();
  });
});
