// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import * as launcherModule from "./terminal/useLauncher";
import * as historyModule from "./terminal/useSessionHistory";
import * as toolsModule from "./tools/useTools";
import * as projectsModule from "./projects/useProjects";
import * as updateRecoveryModule from "./updates/useUpdateRecovery";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const swProps = vi.hoisted(() => ({
  latest: null as null | {
    activities?: Record<string, string>;
    themeId?: string;
    onOutputActivity?: (id: string, text?: string) => void;
    onUserInput?: (id: string) => void;
  },
}));
vi.mock("./terminal/SessionWorkspace", () => ({
  default: (props: {
    activities?: Record<string, string>;
    themeId?: string;
    onOutputActivity?: (id: string, text?: string) => void;
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
  vi.spyOn(updateRecoveryModule, "useUpdateRecovery").mockReturnValue({
    recovery: null,
    loading: false,
    error: null,
    reload: vi.fn(),
    consumeSession: vi.fn(),
    dismiss: vi.fn(),
  });
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  delete document.documentElement.dataset.theme;
  vi.restoreAllMocks();
  invoke.mockReset();
});

function mockLauncher(
  close = vi.fn(),
  sessions = [session],
  start = vi.fn(async () => true),
) {
  const active = sessions[0] ?? null;
  vi.spyOn(launcherModule, "useLauncher").mockReturnValue({
    sessions,
    activeId: active?.info.id ?? null,
    paneIds: active ? [active.info.id] : [],
    focusRequest: 0,
    starting: false,
    closingId: null,
    startError: null,
    busy: false,
    session: active,
    select: vi.fn(),
    focus: vi.fn(),
    split: vi.fn(),
    single: vi.fn(),
    showLauncher: vi.fn(),
    start,
    close,
    handleExit: vi.fn(),
    handleConnection: vi.fn(),
    reconcile: vi.fn(async () => true),
    rename: vi.fn(),
    autoName: vi.fn(),
  });
  return { close, start };
}

describe("App interactions", () => {
  it("requires confirmation before closing a running tab and supports Escape", async () => {
    const { close } = mockLauncher();
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
    mockLauncher(vi.fn(), []);
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
      .mockResolvedValueOnce({
        liveSessions: [],
        dispatchActive: false,
        scheduledTaskId: null,
        hasActiveWork: false,
      })
      .mockResolvedValueOnce({
        status: "installing",
        version: "0.2.0-rc.15",
      });
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText("发现新版本 0.2.0-rc.15")).toBeTruthy();
    expect(screen.getByText("修复剪贴板与体验优化")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "立即更新" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("install_update", {
        downloadUrl: "https://example.com/setup.exe",
        latestVersion: "0.2.0-rc.15",
        allowStopActive: false,
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

  it("shows streaming output badge on session tabs and settles to running without claiming completed", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      mockLauncher();
      render(<App />);
      expect(swProps.latest?.onOutputActivity).toBeTruthy();
      swProps.latest?.onOutputActivity?.(session.info.id, "正在输出…");
      expect(await screen.findAllByText("输出中")).toHaveLength(2);
      await waitFor(() => {}, { timeout: 10 });
      vi.advanceTimersByTime(4000);
      await waitFor(() => expect(screen.queryByText("输出中")).toBeNull());
      expect(screen.queryByText("已完成")).toBeNull();
      expect(screen.getByText("运行中")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("detects running sessions before updating and allows download-only without exiting", async () => {
    mockLauncher();
    invoke
      .mockResolvedValueOnce({
        currentVersion: "0.2.0-rc.14",
        latestVersion: "0.2.0-rc.15",
        updateAvailable: true,
        downloadUrl:
          "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0-rc.15/AgentTerm-0.2.0-rc.15-windows-x64-setup.exe",
        notes: "修复恢复问题",
        channel: "pre-release",
        message: "发现测试更新 0.2.0-rc.15",
      })
      .mockResolvedValueOnce({
        liveSessions: [session.info],
        dispatchActive: false,
        scheduledTaskId: null,
        hasActiveWork: true,
      })
      .mockResolvedValueOnce({
        downloadUrl:
          "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0-rc.15/AgentTerm-0.2.0-rc.15-windows-x64-setup.exe",
        latestVersion: "0.2.0-rc.15",
        ready: true,
      });

    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));

    const card = await screen.findByRole("status", { name: "版本更新" });
    expect(card.textContent).toContain("运行中的终端会话（1 个）");

    await user.click(
      screen.getByRole("button", { name: "仅下载安装包（稍后安装）" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("download_update", {
        downloadUrl:
          "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0-rc.15/AgentTerm-0.2.0-rc.15-windows-x64-setup.exe",
        latestVersion: "0.2.0-rc.15",
      }),
    );
    expect(invoke).not.toHaveBeenCalledWith(
      "install_update",
      expect.anything(),
    );
    expect(
      await screen.findByRole("button", { name: "安装 0.2.0-rc.15" }),
    ).toBeTruthy();
  });

  it("blocks exit when active work appears during download", async () => {
    mockLauncher(vi.fn(), []);
    invoke
      .mockResolvedValueOnce({
        currentVersion: "0.2.0-rc.14",
        latestVersion: "0.2.0-rc.15",
        updateAvailable: true,
        downloadUrl:
          "https://github.com/SophiaData/AgentTerm/releases/download/v0.2.0-rc.15/AgentTerm-0.2.0-rc.15-windows-x64-setup.exe",
        notes: "修复恢复问题",
        channel: "pre-release",
        message: "发现测试更新 0.2.0-rc.15",
      })
      .mockResolvedValueOnce({
        liveSessions: [],
        dispatchActive: false,
        scheduledTaskId: null,
        hasActiveWork: false,
      })
      .mockResolvedValueOnce({
        status: "blocked",
        installerReady: true,
        activity: {
          liveSessions: [],
          dispatchActive: true,
          scheduledTaskId: "nightly-job",
          hasActiveWork: true,
        },
      });

    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    await user.click(await screen.findByRole("button", { name: "立即更新" }));

    const card = await screen.findByRole("status", { name: "版本更新" });
    expect(card.textContent).toContain("已就绪，当前仍有任务运行");
    expect(card.textContent).toContain("任务工作台中有正在执行的受控任务");
    expect(card.textContent).toContain("定时任务（nightly-job）正在执行");
    expect(
      screen.getByRole("button", { name: "保存会话并立即更新" }),
    ).toBeTruthy();
  });

  it("recovers interrupted sessions and surfaces task notices after an update", async () => {
    const { start } = mockLauncher(vi.fn(), []);
    const consumeSession = vi.fn(async () => {});
    const dismiss = vi.fn(async () => {});
    vi.spyOn(updateRecoveryModule, "useUpdateRecovery").mockReturnValue({
      recovery: {
        createdAtMs: 1_763_650_000_000,
        targetVersion: "0.2.0-rc.15",
        sessions: [
          {
            id: "old-s1",
            toolId: "claude",
            toolName: "Claude Code",
            projectPath: "E:\\AgentTerm",
            title: "修复更新恢复",
            cliSessionId: "01234567-89ab-4cde-8123-456789abcdef",
          },
        ],
        hadActiveDispatch: true,
        interruptedScheduledTaskId: "daily-check",
      },
      loading: false,
      error: null,
      reload: vi.fn(),
      consumeSession,
      dismiss,
    });

    const user = userEvent.setup();
    render(<App />);

    const banner = screen.getByRole("region", { name: "更新后恢复" });
    expect(banner.textContent).toContain("应用已更新至 0.2.0-rc.15");
    expect(banner.textContent).toContain("修复更新恢复");
    expect(banner.textContent).toContain("受控任务正在执行");
    expect(banner.textContent).toContain("daily-check");

    await user.click(screen.getByRole("button", { name: "全部恢复并继续" }));
    expect(start).toHaveBeenCalledWith(
      { id: "project-1", name: "AgentTerm", path: "E:\\AgentTerm" },
      expect.objectContaining({ id: "claude" }),
      {
        resume: true,
        cliSessionId: "01234567-89ab-4cde-8123-456789abcdef",
        title: "修复更新恢复",
      },
    );
    expect(consumeSession).toHaveBeenCalledWith("old-s1");

    await user.click(screen.getByRole("button", { name: "暂不恢复" }));
    expect(dismiss).toHaveBeenCalledOnce();
  });

  it("switches interface skins, forwards themeId to sessions, and persists choice", async () => {
    mockLauncher();
    const user = userEvent.setup();
    const firstRender = render(<App />);

    expect(document.documentElement.dataset.theme).toBe("dark");
    await user.click(screen.getByRole("button", { name: /切换皮肤/ }));
    await user.click(screen.getByRole("menuitemradio", { name: /清爽浅白/ }));

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(localStorage.getItem("agentterm-theme")).toBe("light");
    expect(swProps.latest?.themeId).toBe("light");

    firstRender.unmount();
    render(<App />);
    await waitFor(() =>
      expect(document.documentElement.dataset.theme).toBe("light"),
    );
    expect(swProps.latest?.themeId).toBe("light");
  });

  it("shows DeepSeek Harness ACP action and gates resume on cliSessionId", async () => {
    const { start } = mockLauncher(vi.fn(), []);
    vi.spyOn(toolsModule, "useTools").mockReturnValue({
      tools: [
        {
          id: "dsh",
          name: "DeepSeek Harness",
          state: { kind: "installed", path: "dsh.cmd", version: "0.2.2-rc.5" },
        },
      ],
      loading: false,
      error: null,
      refresh: vi.fn(),
    });
    vi.spyOn(historyModule, "useSessionHistory").mockReturnValue({
      records: [
        {
          id: "h-dsh-ok",
          toolId: "dsh",
          toolName: "DeepSeek Harness",
          projectPath: "E:\\AgentTerm",
          title: "ACP 历史会话",
          cliSessionId: "sess_abc123",
          startedAtMs: 1000,
          endedAtMs: 2000,
          exitReason: "已停止",
          outputBytes: 120,
        },
        {
          id: "h-dsh-no-id",
          toolId: "dsh",
          toolName: "DeepSeek Harness",
          projectPath: "E:\\AgentTerm",
          title: "未绑定会话",
          cliSessionId: null,
          startedAtMs: 900,
          endedAtMs: 950,
          exitReason: "异常退出",
          outputBytes: 0,
        },
      ],
      loading: false,
      error: null,
      reload: vi.fn(),
    });

    const user = userEvent.setup();
    render(<App />);

    expect(
      screen.getByRole("button", { name: /DeepSeek Harness.*打开 ACP 对话/ }),
    ).toBeTruthy();

    const resumeButtons = screen.getAllByRole("button", { name: "接续对话" });
    expect((resumeButtons[0] as HTMLButtonElement).disabled).toBe(false);
    expect((resumeButtons[1] as HTMLButtonElement).disabled).toBe(true);

    await user.click(resumeButtons[0]);
    expect(start).toHaveBeenCalledWith(
      { id: "project-1", name: "AgentTerm", path: "E:\\AgentTerm" },
      expect.objectContaining({ id: "dsh" }),
      {
        resume: true,
        cliSessionId: "sess_abc123",
        title: "ACP 历史会话",
      },
    );
  });

  it("shows up to 20 history items by default, allows expanding, and gates resume on cliSessionId for all tools", async () => {
    const { start } = mockLauncher(vi.fn(), []);
    vi.spyOn(toolsModule, "useTools").mockReturnValue({
      tools: [
        {
          id: "claude",
          name: "Claude Code",
          state: { kind: "installed", path: "claude.exe", version: "2.1.336" },
        },
        {
          id: "codex",
          name: "Codex",
          state: { kind: "installed", path: "codex.cmd", version: "0.172.0" },
        },
      ],
      loading: false,
      error: null,
      refresh: vi.fn(),
    });
    const records = Array.from({ length: 22 }, (_, index) => ({
      id: `h-${index}`,
      toolId: index === 1 ? "codex" : "claude",
      toolName: index === 1 ? "Codex" : "Claude Code",
      projectPath: "E:\\AgentTerm",
      title: `历史会话 ${index}`,
      cliSessionId:
        index === 2
          ? null
          : `01234567-89ab-4cde-8123-${String(index).padStart(12, "0")}`,
      startedAtMs: 10_000 - index * 100,
      endedAtMs: 10_050 - index * 100,
      exitReason: "已停止",
      outputBytes: 100,
    }));
    vi.spyOn(historyModule, "useSessionHistory").mockReturnValue({
      records,
      loading: false,
      error: null,
      reload: vi.fn(),
    });

    const user = userEvent.setup();
    render(<App />);

    expect(screen.getAllByRole("button", { name: "接续对话" })).toHaveLength(
      20,
    );
    const resumeButtons = screen.getAllByRole("button", {
      name: "接续对话",
    }) as HTMLButtonElement[];
    expect(resumeButtons[0].disabled).toBe(false);
    expect(resumeButtons[1].disabled).toBe(false);
    expect(resumeButtons[2].disabled).toBe(true);

    await user.click(resumeButtons[1]);
    expect(start).toHaveBeenCalledWith(
      { id: "project-1", name: "AgentTerm", path: "E:\\AgentTerm" },
      expect.objectContaining({ id: "codex" }),
      {
        resume: true,
        cliSessionId: "01234567-89ab-4cde-8123-000000000001",
        title: "历史会话 1",
      },
    );

    await user.click(screen.getByRole("button", { name: "展开全部（22）" }));
    expect(screen.getAllByRole("button", { name: "接续对话" })).toHaveLength(
      22,
    );
    const historyList = document.querySelector(".workspace-history-list");
    expect(historyList).toBeTruthy();
    const historyTags = Array.from(
      historyList!.querySelectorAll(".session-agent-tag"),
    ).map((el) => el.textContent);
    expect(historyTags).toContain("Codex");
    expect(historyTags).toContain("Claude");

    await user.click(screen.getByRole("button", { name: "收起" }));
    expect(screen.getAllByRole("button", { name: "接续对话" })).toHaveLength(
      20,
    );
    expect(screen.getByRole("button", { name: "展开全部（22）" })).toBeTruthy();
  });

  it("renders agent tags on active sidebar links and workspace tabs", () => {
    mockLauncher(vi.fn(), [
      {
        ...session,
        info: {
          ...session.info,
          id: "s-dsh",
          toolId: "dsh",
          toolName: "DeepSeek Harness",
        },
        label: "自定义标题",
      },
    ]);
    render(<App />);
    const sidebarTag = document.querySelector(
      ".workspace-session-link .session-agent-tag",
    );
    const tabTag = document.querySelector(".workspace-tab .session-agent-tag");
    expect(sidebarTag?.textContent).toBe("DSH");
    expect(tabTag?.textContent).toBe("DSH");
    expect(screen.getAllByText("自定义标题").length).toBeGreaterThanOrEqual(2);
  });
});
