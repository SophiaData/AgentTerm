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
const swProps = vi.hoisted(() => ({
  latest: null as null | {
    activities?: Record<string, string>;
    onOutputActivity?: (id: string) => void;
    onUserInput?: (id: string) => void;
  },
}));
vi.mock("./terminal/SessionWorkspace", () => ({
  default: (props: {
    activities?: Record<string, string>;
    onOutputActivity?: (id: string) => void;
    onUserInput?: (id: string) => void;
  }) => {
    swProps.latest = props;
    return null;
  },
}));
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
      currentVersion: "0.2.0-rc.14",
      latestVersion: "0.2.0-rc.14",
      updateAvailable: false,
      downloadUrl: null,
      notes: null,
      channel: "prerelease",
      message: "当前已是 0.2.0-rc.14",
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("check_for_update"),
    );
    expect(await screen.findByText("当前已是 0.2.0-rc.14")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.queryByText("当前已是 0.2.0-rc.14")).toBeNull();
  });

  it("renders an inline update card when a new version is available", async () => {
    mockLauncher();
    invoke
      .mockResolvedValueOnce({
        currentVersion: "0.2.0-rc.14",
        latestVersion: "0.2.0-rc.15",
        updateAvailable: true,
        downloadUrl: "https://example.com/setup.exe",
        notes: "修复剪贴板与体验优化",
        channel: "prerelease",
        message: "发现新版本 0.2.0-rc.15",
      })
      .mockResolvedValueOnce(undefined);
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText("发现新版本 0.2.0-rc.15")).toBeTruthy();
    expect(screen.getByText("修复剪贴板与体验优化")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "立即更新" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("install_update", {
        downloadUrl: "https://example.com/setup.exe",
      }),
    );
  });

  it("lists all added projects in the bottom-left project management drawer", async () => {
    const requestRemove = vi.fn();
    vi.spyOn(projectsModule, "useProjects").mockReturnValue({
      projects: [
        { id: "project-1", name: "AgentTerm", path: "E:\\AgentTerm" },
        { id: "project-2", name: "DataWarehouse", path: "E:\\dw" },
      ],
      busy: false,
      loading: false,
      error: null,
      pendingRemoval: null,
      add: vi.fn(),
      requestRemove,
      cancelRemove: vi.fn(),
      confirmRemove: vi.fn(),
      refresh: vi.fn(),
    });
    mockLauncher();
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByText("项目管理（2）"));
    expect(screen.getByText("E:\\AgentTerm")).toBeTruthy();
    expect(screen.getByText("E:\\dw")).toBeTruthy();
    const removeButtons = screen.getAllByRole("button", {
      name: "移除项目记录",
    }) as HTMLButtonElement[];
    expect(removeButtons).toHaveLength(2);
    expect(removeButtons[0].disabled).toBe(true);
    expect(removeButtons[1].disabled).toBe(false);
    await user.click(removeButtons[1]);
    expect(requestRemove).toHaveBeenCalledWith("project-2");
  });

  it("shows working and completed badges on session tabs", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      mockLauncher();
      render(<App />);
      expect(swProps.latest?.onOutputActivity).toBeTruthy();
      swProps.latest?.onOutputActivity?.(session.info.id);
      expect(await screen.findAllByText("执行中")).toHaveLength(2);
      await waitFor(() => {}, { timeout: 10 });
      vi.advanceTimersByTime(2500);
      expect(await screen.findAllByText("已完成")).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
