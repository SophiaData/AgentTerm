import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { PRODUCT_NAME } from "./meta";
import { loadStoredLayout, saveStoredLayout } from "./layout";
import {
  THEMES,
  applyDocumentTheme,
  getThemePreset,
  loadStoredTheme,
  saveStoredTheme,
  type ThemeId,
} from "./theme";
import TaskWorkbench from "./TaskWorkbench";
import DshDispatchButton from "./DshDispatchButton";
import ProjectFiles from "./files/ProjectFiles";
import ScheduledTasks from "./schedule/ScheduledTasks";
import { displayPath } from "./projects/displayPath";
import { useProjects } from "./projects/useProjects";
import SessionWorkspace from "./terminal/SessionWorkspace";
import {
  historyDisplayTitle,
  historySubtitleText,
  installedToolForRecord,
  projectFromRecord,
  recentClosedHistory,
} from "./terminal/sessionHistory";
import { useLauncher, type ActiveSession } from "./terminal/useLauncher";
import { agentBadgeLabel } from "./terminal/types";
import {
  activityLabel,
  useSessionActivity,
  type SessionActivity,
} from "./terminal/useSessionActivity";
import { useSessionHistory } from "./terminal/useSessionHistory";
import { badgeLabel, detailText } from "./tools/types";
import { useTools } from "./tools/useTools";
import type {
  InstallUpdateOutcome,
  PreparedUpdateStatus,
  RecoverableSession,
  UpdateActivity,
  UpdateStatus,
} from "./updates/types";
import {
  describeUpdateActivity,
  mergeUpdateActivity,
  projectForRecovery,
} from "./updates/updateHelpers";
import { useUpdateRecovery } from "./updates/useUpdateRecovery";

function toolStatusIcon(kind: string): string {
  switch (kind) {
    case "installed":
      return "✓";
    case "missing":
      return "–";
    default:
      return "!";
  }
}

function resizeWithPointer(
  event: ReactPointerEvent<HTMLDivElement>,
  start: number,
  setWidth: (width: number) => void,
  invert: boolean,
) {
  const target = event.currentTarget;
  const startX = event.clientX;
  target.setPointerCapture(event.pointerId);
  const move = (next: PointerEvent) => {
    if (!target.hasPointerCapture(event.pointerId)) return;
    const delta = next.clientX - startX;
    const width = start + (invert ? -delta : delta);
    setWidth(Math.min(invert ? 720 : 420, Math.max(invert ? 280 : 160, width)));
  };
  const stop = () => {
    target.removeEventListener("pointermove", move);
    target.removeEventListener("pointerup", stop);
    target.removeEventListener("pointercancel", stop);
    target.removeEventListener("lostpointercapture", stop);
    if (target.hasPointerCapture(event.pointerId)) {
      target.releasePointerCapture(event.pointerId);
    }
  };
  target.addEventListener("pointermove", move);
  target.addEventListener("pointerup", stop);
  target.addEventListener("pointercancel", stop);
  target.addEventListener("lostpointercapture", stop);
}

function resizeWithKeyboard(
  event: ReactKeyboardEvent<HTMLDivElement>,
  current: number,
  setWidth: (width: number) => void,
  invert: boolean,
) {
  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
  event.preventDefault();
  const direction = event.key === "ArrowRight" ? 1 : -1;
  const delta = direction * (invert ? -16 : 16);
  setWidth(
    Math.min(invert ? 720 : 420, Math.max(invert ? 280 : 160, current + delta)),
  );
}
import { emptyPlan } from "./workbenchState";

export default function App() {
  const tools = useTools();
  const projects = useProjects();
  const launcher = useLauncher();
  const history = useSessionHistory();
  const updateRecovery = useUpdateRecovery();
  const [projectId, setProjectId] = useState<string | null>(null);
  const [toolId, setToolId] = useState<string | null>(null);
  const [recordsOpen, setRecordsOpen] = useState(false);
  const [scheduleOpen, setScheduleOpen] = useState(false);
  const [filesOpen, setFilesOpen] = useState(true);
  const [sidebarWidth, setSidebarWidth] = useState(204);
  const [sideWidth, setSideWidth] = useState(390);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [narrowLayout, setNarrowLayout] = useState(false);
  const [themeId, setThemeId] = useState<ThemeId>("dark");
  const [themeMenuOpen, setThemeMenuOpen] = useState(false);
  const themePickerRef = useRef<HTMLDivElement | null>(null);
  const [closeConfirmation, setCloseConfirmation] =
    useState<ActiveSession | null>(null);
  type UpdateNotice =
    | { kind: "status"; text: string; isError?: boolean }
    | {
        kind: "available";
        status: UpdateStatus;
        activity: UpdateActivity;
        blockedAfterDownload?: boolean;
      }
    | { kind: "downloading"; version: string };
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateNotice, setUpdateNotice] = useState<UpdateNotice | null>(null);
  const [preparedUpdate, setPreparedUpdate] =
    useState<PreparedUpdateStatus | null>(null);
  const [restoringUpdates, setRestoringUpdates] = useState(false);
  const [taskNotice, setTaskNotice] = useState<string | null>(null);
  const [historyExpanded, setHistoryExpanded] = useState(false);
  const activity = useSessionActivity(launcher.sessions);
  const [draft, setDraft] = useState(emptyPlan);
  const selectedProject =
    projects.projects.find((p) => p.id === projectId) ?? projects.projects[0];
  const selectedTool =
    tools.tools.find((t) => t.id === toolId && t.state.kind === "installed") ??
    tools.tools.find((t) => t.state.kind === "installed");
  const currentSession = launcher.session;
  const fileProject = currentSession
    ? projects.projects.find((p) => p.path === currentSession.info.projectPath)
    : selectedProject;
  const projectBusy = projects.loading || projects.busy || launcher.busy;
  const activeSessionIds = useMemo(
    () => new Set(launcher.sessions.map((s) => s.info.id)),
    [launcher.sessions],
  );
  const closedHistory = useMemo(
    () =>
      recentClosedHistory(
        history.records,
        activeSessionIds,
        history.records.length,
      ),
    [history.records, activeSessionIds],
  );
  const recentHistory = useMemo(
    () => (historyExpanded ? closedHistory : closedHistory.slice(0, 20)),
    [historyExpanded, closedHistory],
  );
  const runningCount = useMemo(
    () => launcher.sessions.filter((s) => s.running && !s.statusUnknown).length,
    [launcher.sessions],
  );
  const unknownCount = useMemo(
    () => launcher.sessions.filter((s) => s.statusUnknown).length,
    [launcher.sessions],
  );
  const hasSidePanel =
    recordsOpen || scheduleOpen || (filesOpen && !!fileProject);
  const historyReloaded = useRef(false);
  const closeTriggerFocus = useRef<HTMLElement | null>(null);
  const reloadHistory = history.reload;
  useEffect(() => {
    const storedTheme = loadStoredTheme();
    setThemeId(storedTheme);
    applyDocumentTheme(storedTheme);
    const layout = loadStoredLayout();
    if (!layout) return;
    setSidebarWidth(layout.sidebarWidth);
    setSideWidth(layout.sideWidth);
    setFilesOpen(layout.filesOpen);
    if (layout.projectId) setProjectId(layout.projectId);
  }, []);
  useEffect(() => {
    applyDocumentTheme(themeId);
  }, [themeId]);
  useEffect(() => {
    if (!themeMenuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (
        themePickerRef.current &&
        event.target instanceof Node &&
        !themePickerRef.current.contains(event.target)
      ) {
        setThemeMenuOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setThemeMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [themeMenuOpen]);
  function selectTheme(nextTheme: ThemeId) {
    setThemeId(nextTheme);
    saveStoredTheme(nextTheme);
    applyDocumentTheme(nextTheme);
    setThemeMenuOpen(false);
  }
  useEffect(() => {
    const media = window.matchMedia("(max-width: 900px)");
    const update = () => {
      setNarrowLayout(media.matches);
      if (!media.matches) setSidebarOpen(false);
    };
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  useEffect(() => {
    if (!closeConfirmation) return;
    const previous = closeTriggerFocus.current;
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setCloseConfirmation(null);
    };
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("keydown", escape);
      previous?.focus();
    };
  }, [closeConfirmation]);
  useEffect(() => {
    if (!closeConfirmation) return;
    const current = launcher.sessions.find(
      (session) => session.info.id === closeConfirmation.info.id,
    );
    if (!current?.running) setCloseConfirmation(null);
  }, [closeConfirmation, launcher.sessions]);
  useEffect(() => {
    if (updateNotice?.kind !== "available") return;
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setUpdateNotice(null);
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [updateNotice]);
  useEffect(() => {
    saveStoredLayout({ sidebarWidth, sideWidth, filesOpen, projectId });
  }, [sidebarWidth, sideWidth, filesOpen, projectId]);
  useEffect(() => {
    if (!historyReloaded.current) {
      historyReloaded.current = true;
      return;
    }
    if (launcher.activeId === null) {
      void reloadHistory();
    }
  }, [
    launcher.activeId,
    launcher.sessions.length,
    runningCount,
    reloadHistory,
  ]);
  useEffect(() => {
    if (updateNotice?.kind !== "status" || updateNotice.isError) return;
    const timer = setTimeout(() => {
      setUpdateNotice(null);
    }, 6000);
    return () => clearTimeout(timer);
  }, [updateNotice]);

  const handleCheckUpdate = async () => {
    if (checkingUpdate || updateNotice?.kind === "downloading") return;
    setCheckingUpdate(true);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const status = await invoke<UpdateStatus>("check_for_update");
      if (!status.updateAvailable || !status.downloadUrl) {
        setUpdateNotice({ kind: "status", text: status.message });
        return;
      }
      const backendActivity = await invoke<UpdateActivity>(
        "get_update_activity",
      );
      const mergedActivity = mergeUpdateActivity(
        backendActivity,
        launcher.sessions,
      );
      setUpdateNotice({
        kind: "available",
        status,
        activity: mergedActivity,
        blockedAfterDownload: false,
      });
    } catch (error: unknown) {
      setUpdateNotice({
        kind: "status",
        text: error instanceof Error ? error.message : String(error),
        isError: true,
      });
    } finally {
      setCheckingUpdate(false);
    }
  };

  const handleDownloadUpdateOnly = async (status: UpdateStatus) => {
    if (!status.downloadUrl || updateNotice?.kind === "downloading") return;
    const targetVersion = status.latestVersion ?? "新版本";
    setUpdateNotice({ kind: "downloading", version: targetVersion });
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const prepared = await invoke<PreparedUpdateStatus>("download_update", {
        downloadUrl: status.downloadUrl,
        latestVersion: status.latestVersion,
      });
      setPreparedUpdate(prepared);
      setUpdateNotice({
        kind: "status",
        text: `${targetVersion} 更新包已下载就绪，当前会话不受影响`,
      });
    } catch (error: unknown) {
      setUpdateNotice({
        kind: "status",
        text: error instanceof Error ? error.message : String(error),
        isError: true,
      });
    }
  };

  const handleInstallUpdate = async (
    status: UpdateStatus,
    allowStopActive = false,
  ) => {
    if (!status.downloadUrl || updateNotice?.kind === "downloading") return;
    const targetVersion = status.latestVersion ?? "新版本";
    setUpdateNotice({ kind: "downloading", version: targetVersion });
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const outcome = await invoke<InstallUpdateOutcome>("install_update", {
        downloadUrl: status.downloadUrl,
        latestVersion: status.latestVersion,
        allowStopActive,
      });
      if (outcome.status === "blocked") {
        setPreparedUpdate({
          downloadUrl: status.downloadUrl,
          latestVersion: status.latestVersion,
          ready: outcome.installerReady,
        });
        setUpdateNotice({
          kind: "available",
          status,
          activity: mergeUpdateActivity(outcome.activity, launcher.sessions),
          blockedAfterDownload: true,
        });
        return;
      }
      setUpdateNotice({
        kind: "status",
        text: `更新已下载，正在退出并安装 ${outcome.version ?? targetVersion}`,
      });
    } catch (error: unknown) {
      setUpdateNotice({
        kind: "status",
        text: error instanceof Error ? error.message : String(error),
        isError: true,
      });
    }
  };

  const triggerPreparedInstall = async () => {
    if (
      checkingUpdate ||
      updateNotice?.kind === "downloading" ||
      !preparedUpdate
    )
      return;
    setCheckingUpdate(true);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const backendActivity = await invoke<UpdateActivity>(
        "get_update_activity",
      );
      const mergedActivity = mergeUpdateActivity(
        backendActivity,
        launcher.sessions,
      );
      const status: UpdateStatus = {
        currentVersion: "",
        latestVersion: preparedUpdate.latestVersion,
        updateAvailable: true,
        downloadUrl: preparedUpdate.downloadUrl,
        notes: null,
        channel: "stable",
        message: "",
      };
      if (mergedActivity.hasActiveWork) {
        setUpdateNotice({
          kind: "available",
          status,
          activity: mergedActivity,
          blockedAfterDownload: true,
        });
        return;
      }
      setCheckingUpdate(false);
      await handleInstallUpdate(status, false);
    } catch (error: unknown) {
      setUpdateNotice({
        kind: "status",
        text: error instanceof Error ? error.message : String(error),
        isError: true,
      });
    } finally {
      setCheckingUpdate(false);
    }
  };

  const restoreOneSession = async (
    item: RecoverableSession,
    resume: boolean,
  ): Promise<boolean> => {
    const tool = tools.tools.find(
      (candidate) =>
        candidate.id === item.toolId && candidate.state.kind === "installed",
    );
    if (!tool) return false;
    const project = projectForRecovery(item, projects.projects);
    const existingProject = projects.projects.find(
      (candidate) => candidate.path === item.projectPath,
    );
    if (existingProject) setProjectId(existingProject.id);
    const started = await launcher.start(project, tool, {
      resume,
      cliSessionId: resume ? (item.cliSessionId ?? undefined) : undefined,
      title: resume ? (item.title ?? undefined) : undefined,
    });
    if (started) {
      await updateRecovery.consumeSession(item.id);
    }
    return started;
  };
  const restoreAllSessions = async () => {
    const snapshot = updateRecovery.recovery;
    if (!snapshot || restoringUpdates) return;
    setRestoringUpdates(true);
    try {
      for (const item of snapshot.sessions) {
        await restoreOneSession(item, true);
      }
    } finally {
      setRestoringUpdates(false);
    }
  };

  const selectSession = (session: ActiveSession) => {
    const project = projects.projects.find(
      (p) => p.path === session.info.projectPath,
    );
    if (project) setProjectId(project.id);
    launcher.select(session.info.id);
    setSidebarOpen(false);
  };
  const requestClose = (session: ActiveSession) => {
    closeTriggerFocus.current = document.activeElement as HTMLElement | null;
    if (session.running) setCloseConfirmation(session);
    else void launcher.close(session.info.id);
  };
  const sessionActivities: Record<string, SessionActivity> = {};
  for (const s of launcher.sessions) {
    sessionActivities[s.info.id] = activity.getActivity(s);
  }
  const sessionLink = (session: ActiveSession) => {
    const state = sessionActivities[session.info.id] ?? "idle";
    const dotClass = session.statusUnknown
      ? " is-unknown"
      : !session.running
        ? ""
        : state === "working"
          ? " is-working"
          : " is-running";
    return (
      <button
        type="button"
        key={session.info.id}
        className="workspace-session-link"
        aria-current={
          launcher.activeId === session.info.id ? "true" : undefined
        }
        onClick={() => selectSession(session)}
        title={displayPath(session.info.projectPath)}
      >
        <span className={`session-dot${dotClass}`} aria-hidden="true" />
        <span
          className="session-agent-tag"
          data-agent={session.info.toolId}
          title={session.info.toolName}
        >
          {agentBadgeLabel(session.info.toolId, session.info.toolName)}
        </span>
        <span className="workspace-session-label">{session.label}</span>
        <small>
          {activityLabel(state, session.running, session.statusUnknown)}
        </small>
      </button>
    );
  };

  return (
    <main
      className={`session-workspace${hasSidePanel ? " has-side-panel" : ""}${sidebarOpen ? " is-sidebar-open" : ""}`}
      style={
        {
          "--sidebar-width": `${sidebarWidth}px`,
          "--side-panel-width": `${sideWidth}px`,
          gridTemplateColumns: `${sidebarWidth}px 6px minmax(0, 1fr)${
            hasSidePanel ? ` 6px ${sideWidth}px` : ""
          }`,
        } as CSSProperties
      }
    >
      {sidebarOpen ? (
        <button
          type="button"
          className="workspace-sidebar-backdrop"
          aria-label="收起项目与会话栏"
          onClick={() => setSidebarOpen(false)}
        />
      ) : null}
      <aside
        className="workspace-sidebar"
        aria-label="项目与会话"
        aria-hidden={narrowLayout && !sidebarOpen ? true : undefined}
        inert={narrowLayout && !sidebarOpen ? true : undefined}
      >
        <div className="workspace-brand">
          <span className="workspace-brand-icon" aria-hidden="true">
            &gt;_
          </span>
          <strong>{PRODUCT_NAME}</strong>
          <span className="workspace-local">本地</span>
        </div>
        <div className="sidebar-heading">
          <h2>项目</h2>
          <button
            type="button"
            className="sidebar-icon-button"
            aria-label="添加项目"
            title="添加已有项目目录"
            disabled={projectBusy}
            onClick={() => void projects.add()}
          >
            ＋
          </button>
        </div>
        <button
          type="button"
          className="workspace-new-session"
          onClick={launcher.showLauncher}
          disabled={launcher.starting}
        >
          ＋ 新建会话
        </button>
        {projects.loading ? (
          <p className="sidebar-message">正在读取项目…</p>
        ) : null}
        {projects.error ? (
          <p className="sidebar-error" role="alert">
            {projects.error}
          </p>
        ) : null}
        <ul className="workspace-projects">
          {projects.projects.map((project) => (
            <li key={project.id}>
              <button
                type="button"
                className="workspace-project"
                aria-current={
                  selectedProject?.id === project.id ? "true" : undefined
                }
                title={displayPath(project.path)}
                onClick={() => {
                  setProjectId(project.id);
                  const recent = launcher.sessions
                    .filter((s) => s.info.projectPath === project.path)
                    .slice(-1)[0];
                  if (recent) selectSession(recent);
                  else launcher.showLauncher();
                }}
              >
                <span aria-hidden="true">▱</span>
                <span>{project.name}</span>
              </button>
              {launcher.sessions
                .filter((s) => s.info.projectPath === project.path)
                .map(sessionLink)}
            </li>
          ))}
        </ul>
        {!projects.loading && !projects.projects.length ? (
          <p className="sidebar-message">
            添加已有目录，在项目里打开 Agent 会话。
          </p>
        ) : null}
        {launcher.sessions.some(
          (s) => !projects.projects.some((p) => p.path === s.info.projectPath),
        ) ? (
          <div className="sidebar-current-session">
            <h2>其他已打开会话</h2>
            {launcher.sessions
              .filter(
                (s) =>
                  !projects.projects.some((p) => p.path === s.info.projectPath),
              )
              .map(sessionLink)}
          </div>
        ) : null}
        <section className="sidebar-tools" aria-labelledby="tool-status-title">
          <div className="sidebar-heading sidebar-tools-heading">
            <h2 id="tool-status-title">工具状态</h2>
            <button
              type="button"
              className="sidebar-icon-button"
              aria-label="重新检测 CLI 工具"
              title="重新检测 CLI 工具"
              disabled={tools.loading || launcher.busy}
              onClick={() => void tools.refresh()}
            >
              ↻
            </button>
          </div>
          {tools.loading && tools.tools.length === 0 ? (
            <p className="sidebar-message">
              正在检测 Claude Code、Codex 和 Agy…
            </p>
          ) : (
            <ul className="sidebar-tool-list">
              {tools.tools.map((tool) => (
                <li key={tool.id} className="sidebar-tool-item">
                  <span
                    className={`status-chip status-${tool.state.kind}`}
                    title={detailText(tool.state)}
                  >
                    <span className="status-icon" aria-hidden="true">
                      {toolStatusIcon(tool.state.kind)}
                    </span>
                    <span>{badgeLabel(tool.state)}</span>
                  </span>
                  <strong>{tool.name}</strong>
                </li>
              ))}
            </ul>
          )}
          {tools.error ? (
            <div className="sidebar-inline-error" role="alert">
              <strong>工具检测失败</strong>
              <p>{tools.error}</p>
              <small>下一步：点击右上角重新检测。</small>
            </div>
          ) : null}
        </section>
        <div className="sidebar-bottom">
          {projects.projects.length ? (
            <details className="sidebar-details">
              <summary>项目管理（{projects.projects.length}）</summary>
              <ul className="sidebar-project-settings-list">
                {projects.projects.map((project) => {
                  const isCurrent = selectedProject?.id === project.id;
                  const hasSessions = launcher.sessions.some(
                    (s) => s.info.projectPath === project.path,
                  );
                  const removingThis =
                    projects.pendingRemoval?.id === project.id;
                  return (
                    <li
                      key={project.id}
                      className={`sidebar-project-setting-item${isCurrent ? " is-current" : ""}`}
                    >
                      <div className="sidebar-project-setting-head">
                        <strong>{project.name}</strong>
                        {isCurrent ? (
                          <span className="sidebar-current-badge">当前</span>
                        ) : null}
                      </div>
                      <p title={displayPath(project.path)}>
                        {displayPath(project.path)}
                      </p>
                      {removingThis ? (
                        <div className="sidebar-removal">
                          <p>移除“{project.name}”的列表记录，项目文件保留。</p>
                          <button
                            type="button"
                            className="ghost-button"
                            disabled={projectBusy}
                            onClick={projects.cancelRemove}
                          >
                            取消
                          </button>
                          <button
                            type="button"
                            className="danger-button"
                            disabled={projectBusy}
                            onClick={() => void projects.confirmRemove()}
                          >
                            确认移除
                          </button>
                        </div>
                      ) : (
                        <button
                          type="button"
                          className="ghost-button"
                          disabled={projectBusy || hasSessions}
                          title={
                            hasSessions
                              ? "请先关闭该项目的已打开会话"
                              : undefined
                          }
                          onClick={() => projects.requestRemove(project.id)}
                        >
                          移除项目记录
                        </button>
                      )}
                    </li>
                  );
                })}
              </ul>
            </details>
          ) : null}
          <p className="workspace-note">
            {launcher.sessions.length
              ? `${launcher.sessions.length} 个会话 · ${runningCount} 个进程运行中${unknownCount ? ` · ${unknownCount} 个状态未知` : ""}`
              : "本机 CLI 会话工作区"}
          </p>
        </div>
      </aside>
      <div
        className="workspace-splitter workspace-sidebar-splitter"
        role="separator"
        aria-orientation="vertical"
        aria-label="调整项目栏宽度"
        aria-valuemin={160}
        aria-valuemax={420}
        aria-valuenow={sidebarWidth}
        tabIndex={0}
        onPointerDown={(event) =>
          resizeWithPointer(event, sidebarWidth, setSidebarWidth, false)
        }
        onKeyDown={(event) =>
          resizeWithKeyboard(event, sidebarWidth, setSidebarWidth, false)
        }
      />

      <section className="workspace-main" aria-label="Agent 会话工作区">
        <header className="workspace-topbar">
          <button
            type="button"
            className="workspace-mobile-sidebar-toggle"
            aria-label="打开项目与会话栏"
            aria-expanded={sidebarOpen}
            onClick={() => setSidebarOpen((open) => !open)}
          >
            ☰
          </button>
          <div className="workspace-context">
            <span>
              {currentSession
                ? (projects.projects.find(
                    (p) => p.path === currentSession.info.projectPath,
                  )?.name ?? currentSession.info.toolName)
                : (selectedProject?.name ?? "工作区")}
            </span>
            <span className="workspace-context-separator">/</span>
            <strong>
              {currentSession ? currentSession.label : "新建会话"}
            </strong>
          </div>
          <div className="workspace-topbar-actions">
            <button
              type="button"
              className="ghost-button"
              disabled={!fileProject}
              aria-expanded={filesOpen && !!fileProject}
              aria-controls="project-files"
              onClick={() => {
                setFilesOpen((open) => !open);
                setRecordsOpen(false);
                setScheduleOpen(false);
              }}
            >
              文件
            </button>
            <button
              type="button"
              className="ghost-button"
              aria-pressed={launcher.paneIds.length === 2}
              disabled={!currentSession || launcher.sessions.length < 2}
              title="并排查看两个已有会话；选择其他会话会替换当前面板"
              onClick={() =>
                launcher.paneIds.length === 2
                  ? launcher.single()
                  : launcher.split()
              }
            >
              {launcher.paneIds.length === 2 ? "返回单屏" : "分屏查看"}
            </button>
            <div className="workspace-theme-picker" ref={themePickerRef}>
              <button
                type="button"
                className="ghost-button workspace-theme-trigger"
                aria-haspopup="menu"
                aria-expanded={themeMenuOpen}
                aria-label={`切换皮肤，当前：${getThemePreset(themeId).name}`}
                title={`皮肤：${getThemePreset(themeId).name}`}
                onClick={() => setThemeMenuOpen((open) => !open)}
              >
                <span
                  className="workspace-theme-swatch"
                  style={{ background: getThemePreset(themeId).swatch }}
                  aria-hidden="true"
                />
                <span>皮肤</span>
              </button>
              {themeMenuOpen ? (
                <div
                  className="workspace-theme-menu"
                  role="menu"
                  aria-label="选择界面皮肤"
                >
                  {THEMES.map((theme) => {
                    const active = theme.id === themeId;
                    return (
                      <button
                        key={theme.id}
                        type="button"
                        role="menuitemradio"
                        aria-checked={active}
                        className="workspace-theme-option"
                        onClick={() => selectTheme(theme.id)}
                      >
                        <span
                          className="workspace-theme-swatch"
                          style={{ background: theme.swatch }}
                          aria-hidden="true"
                        />
                        <span className="workspace-theme-option-copy">
                          <strong>{theme.name}</strong>
                          <small>{theme.description}</small>
                        </span>
                        {active ? (
                          <span
                            className="workspace-theme-check"
                            aria-hidden="true"
                          >
                            ✓
                          </span>
                        ) : null}
                      </button>
                    );
                  })}
                </div>
              ) : null}
            </div>
            <button
              type="button"
              className="ghost-button"
              aria-label="检查更新"
              disabled={checkingUpdate || updateNotice?.kind === "downloading"}
              onClick={() => void handleCheckUpdate()}
            >
              {checkingUpdate
                ? "检查中…"
                : updateNotice?.kind === "downloading"
                  ? "更新中…"
                  : "检查更新"}
            </button>
            {preparedUpdate ? (
              <button
                type="button"
                className="primary-button workspace-update-ready"
                disabled={
                  checkingUpdate || updateNotice?.kind === "downloading"
                }
                onClick={() => void triggerPreparedInstall()}
              >
                安装 {preparedUpdate.latestVersion ?? "更新"}
              </button>
            ) : null}
            <button
              type="button"
              className="ghost-button workspace-icon-action"
              aria-expanded={scheduleOpen}
              aria-controls="scheduled-tasks"
              aria-label={scheduleOpen ? "收起定时任务" : "定时任务"}
              title={
                scheduleOpen
                  ? "收起定时任务面板"
                  : "查看或管理 Agent 创建的定时任务"
              }
              onClick={() => {
                setScheduleOpen((open) => !open);
                setRecordsOpen(false);
                setFilesOpen(false);
              }}
            >
              <svg
                viewBox="0 0 24 24"
                width="16"
                height="16"
                aria-hidden="true"
              >
                <circle
                  cx="12"
                  cy="12"
                  r="8.25"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                />
                <path
                  d="M12 7.5v5l3.25 2"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            </button>
            <button
              type="button"
              className="ghost-button"
              aria-expanded={recordsOpen}
              aria-controls="session-records"
              onClick={() => {
                setRecordsOpen((open) => !open);
                setScheduleOpen(false);
                setFilesOpen(false);
              }}
            >
              {recordsOpen ? "收起任务记录" : "任务记录"}
            </button>
            <DshDispatchButton />
          </div>
        </header>
        {launcher.sessions.length ? (
          <div
            className="workspace-tabs"
            role="tablist"
            aria-label="已打开的会话"
          >
            {launcher.sessions.map((session) => (
              <div className="workspace-tab-item" key={session.info.id}>
                <button
                  type="button"
                  role="tab"
                  id={`tab-${session.info.id}`}
                  aria-controls={`pane-${session.info.id}`}
                  aria-selected={launcher.activeId === session.info.id}
                  tabIndex={
                    launcher.activeId === session.info.id ||
                    (!launcher.activeId && launcher.sessions[0] === session)
                      ? 0
                      : -1
                  }
                  className={`workspace-tab${
                    session.running &&
                    !session.statusUnknown &&
                    sessionActivities[session.info.id] === "working"
                      ? " is-working"
                      : ""
                  }`}
                  title={displayPath(session.info.projectPath)}
                  onClick={() => selectSession(session)}
                  onKeyDown={(event) => {
                    if (["Delete", "Backspace"].includes(event.key)) {
                      event.preventDefault();
                      requestClose(session);
                      return;
                    }
                    const keys = ["ArrowLeft", "ArrowRight", "Home", "End"];
                    if (!keys.includes(event.key)) return;
                    event.preventDefault();
                    const tabs = Array.from(
                      event.currentTarget.parentElement?.parentElement?.querySelectorAll<HTMLButtonElement>(
                        '[role="tab"]',
                      ) ?? [],
                    );
                    const index = tabs.indexOf(event.currentTarget);
                    const next =
                      event.key === "Home"
                        ? 0
                        : event.key === "End"
                          ? tabs.length - 1
                          : (index +
                              (event.key === "ArrowRight" ? 1 : -1) +
                              tabs.length) %
                            tabs.length;
                    tabs[next]?.focus();
                  }}
                >
                  <span
                    className={`session-dot${
                      session.statusUnknown
                        ? " is-unknown"
                        : !session.running
                          ? ""
                          : sessionActivities[session.info.id] === "working"
                            ? " is-working"
                            : " is-running"
                    }`}
                    aria-hidden="true"
                  />
                  <span
                    className="session-agent-tag"
                    data-agent={session.info.toolId}
                    title={session.info.toolName}
                  >
                    {agentBadgeLabel(
                      session.info.toolId,
                      session.info.toolName,
                    )}
                  </span>
                  <span className="workspace-tab-label">{session.label}</span>
                  {session.running &&
                  !session.statusUnknown &&
                  sessionActivities[session.info.id] === "working" ? (
                    <span className="workspace-tab-state is-working">
                      输出中
                    </span>
                  ) : null}
                </button>
                <button
                  type="button"
                  className="workspace-tab-close"
                  aria-label={`关闭 ${session.label}`}
                  onClick={() => requestClose(session)}
                >
                  ×
                </button>
              </div>
            ))}
            <button
              type="button"
              className="workspace-tab-add"
              aria-label="新建会话"
              onClick={launcher.showLauncher}
              disabled={launcher.starting}
            >
              ＋
            </button>
          </div>
        ) : null}
        {updateRecovery.error || updateRecovery.recovery ? (
          <section
            className="workspace-recovery-banner"
            aria-label="更新后恢复"
          >
            {updateRecovery.error ? (
              <div className="workspace-recovery-error" role="alert">
                <span>{updateRecovery.error}</span>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => void updateRecovery.reload()}
                >
                  重试
                </button>
              </div>
            ) : null}
            {updateRecovery.recovery ? (
              <>
                <div className="workspace-recovery-header">
                  <div>
                    <strong>
                      应用已更新
                      {updateRecovery.recovery.targetVersion
                        ? `至 ${updateRecovery.recovery.targetVersion}`
                        : ""}
                      ，发现更新前未结束的工作
                    </strong>
                    <span>
                      可恢复更新前正在执行的 CLI 会话（保留标题与项目目录）
                    </span>
                  </div>
                  <div className="workspace-recovery-header-actions">
                    {updateRecovery.recovery.sessions.length > 0 ? (
                      <button
                        type="button"
                        className="primary-button"
                        disabled={
                          restoringUpdates ||
                          launcher.busy ||
                          tools.loading ||
                          !updateRecovery.recovery.sessions.some((item) =>
                            tools.tools.some(
                              (tool) =>
                                tool.id === item.toolId &&
                                tool.state.kind === "installed",
                            ),
                          )
                        }
                        onClick={() => void restoreAllSessions()}
                      >
                        {restoringUpdates ? "正在恢复…" : "全部恢复并继续"}
                      </button>
                    ) : null}
                    <button
                      type="button"
                      className="ghost-button"
                      disabled={restoringUpdates}
                      onClick={() => void updateRecovery.dismiss()}
                    >
                      暂不恢复
                    </button>
                  </div>
                </div>
                {updateRecovery.recovery.sessions.length > 0 ? (
                  <ul className="workspace-recovery-list">
                    {updateRecovery.recovery.sessions.map((item) => {
                      const availableTool = tools.tools.find(
                        (tool) =>
                          tool.id === item.toolId &&
                          tool.state.kind === "installed",
                      );
                      const label =
                        item.title?.trim() ||
                        `${item.toolName} · ${displayPath(item.projectPath)}`;
                      return (
                        <li key={item.id} className="workspace-recovery-item">
                          <div className="workspace-recovery-meta">
                            <strong>{label}</strong>
                            <span title={displayPath(item.projectPath)}>
                              {item.toolName} · {displayPath(item.projectPath)}
                            </span>
                          </div>
                          <div className="workspace-recovery-item-actions">
                            <button
                              type="button"
                              className="ghost-button"
                              disabled={
                                restoringUpdates ||
                                launcher.busy ||
                                tools.loading ||
                                !availableTool
                              }
                              onClick={() => void restoreOneSession(item, true)}
                            >
                              恢复并继续
                            </button>
                            <button
                              type="button"
                              className="ghost-button"
                              disabled={
                                restoringUpdates ||
                                launcher.busy ||
                                tools.loading ||
                                !availableTool
                              }
                              onClick={() =>
                                void restoreOneSession(item, false)
                              }
                            >
                              新开终端
                            </button>
                          </div>
                        </li>
                      );
                    })}
                  </ul>
                ) : null}
                {updateRecovery.recovery.hadActiveDispatch ? (
                  <div className="workspace-recovery-task-note">
                    <span>
                      更新前任务工作台有受控任务正在执行，可前往「任务记录」核实进程并接回。
                    </span>
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => {
                        setRecordsOpen(true);
                        setScheduleOpen(false);
                        setFilesOpen(false);
                      }}
                    >
                      打开任务记录
                    </button>
                  </div>
                ) : null}
                {updateRecovery.recovery.interruptedScheduledTaskId ? (
                  <div className="workspace-recovery-task-note">
                    <span>
                      更新前定时任务（
                      {updateRecovery.recovery.interruptedScheduledTaskId}
                      ）被中断，可在「定时任务」中查看或立即重新运行。
                    </span>
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => {
                        setScheduleOpen(true);
                        setRecordsOpen(false);
                        setFilesOpen(false);
                      }}
                    >
                      打开定时任务
                    </button>
                  </div>
                ) : null}
              </>
            ) : null}
          </section>
        ) : null}
        {closeConfirmation ? (
          <div
            className="workspace-close-confirmation"
            role="alertdialog"
            aria-modal="true"
            aria-label="确认结束运行中的会话"
          >
            <span>
              “{closeConfirmation.label}”仍在
              {closeConfirmation.statusUnknown
                ? "后台运行，当前连接状态未知"
                : "运行"}
              。结束会停止这个 CLI，尚未提交的输入可能丢失。
            </span>
            <button
              type="button"
              className="ghost-button"
              autoFocus
              disabled={launcher.busy}
              onClick={() => setCloseConfirmation(null)}
            >
              继续使用
            </button>
            <button
              type="button"
              className="danger-button"
              disabled={launcher.busy}
              onClick={() => {
                const target = closeConfirmation;
                setCloseConfirmation(null);
                void launcher.close(target.info.id);
              }}
            >
              确认结束
            </button>
          </div>
        ) : null}
        <SessionWorkspace
          sessions={launcher.sessions}
          activities={sessionActivities}
          themeId={themeId}
          paneIds={launcher.paneIds}
          activeId={launcher.activeId}
          focusRequest={launcher.focusRequest}
          closingId={launcher.closingId}
          onFocus={(id) => {
            const session = launcher.sessions.find((s) => s.info.id === id);
            const project = projects.projects.find(
              (p) => p.path === session?.info.projectPath,
            );
            if (project) setProjectId(project.id);
            launcher.focus(id);
          }}
          onClose={(id) => void launcher.close(id)}
          onExit={launcher.handleExit}
          onConnection={launcher.handleConnection}
          onRename={launcher.rename}
          onPromptLine={(id, line) => void launcher.autoName(id, line)}
          onOutputActivity={activity.reportOutput}
          onUserInput={activity.reportInput}
        />
        {!currentSession ? (
          <div className="workspace-start">
            <div className="workspace-start-content">
              {launcher.startError ? (
                <div className="workspace-error-bar" role="alert">
                  <span
                    className="status-icon status-error-icon"
                    aria-hidden="true"
                  >
                    !
                  </span>
                  <div>
                    <strong>会话启动失败</strong>
                    <p>{launcher.startError}</p>
                    <small>下一步：检查所选工具状态后重试。</small>
                  </div>
                </div>
              ) : null}
              {tools.error ? (
                <div className="workspace-error-bar" role="alert">
                  <span
                    className="status-icon status-error-icon"
                    aria-hidden="true"
                  >
                    !
                  </span>
                  <div>
                    <strong>工具检测失败</strong>
                    <p>{tools.error}</p>
                    <small>下一步：点击左侧工具状态旁的重新检测。</small>
                  </div>
                </div>
              ) : null}
              <span className="workspace-start-mark" aria-hidden="true">
                &gt;_
              </span>
              <p className="workspace-kicker">AGENT WORKSPACE</p>
              <h1>
                {selectedProject
                  ? `在 ${selectedProject.name} 中打开 Agent`
                  : "从一个项目开始"}
              </h1>
              <p className="workspace-start-description">
                {selectedProject
                  ? "打开一个独立的 CLI 会话。已有会话继续保留，随时切换回来。"
                  : "选择已有项目目录，把 Agent 的终端会话放在一起管理。"}
              </p>
              {selectedProject ? (
                <>
                  <div
                    className="workspace-agent-options"
                    role="group"
                    aria-label="选择 CLI"
                  >
                    {tools.tools.map((tool) => (
                      <button
                        type="button"
                        key={tool.id}
                        className="workspace-agent-option"
                        aria-pressed={selectedTool?.id === tool.id}
                        disabled={
                          launcher.busy || tool.state.kind !== "installed"
                        }
                        title={detailText(tool.state)}
                        onClick={() => setToolId(tool.id)}
                      >
                        <strong>{tool.name}</strong>
                        <span>
                          {tool.state.kind === "installed"
                            ? tool.id === "dsh"
                              ? "打开 ACP 对话"
                              : "打开原生 CLI"
                            : badgeLabel(tool.state)}
                        </span>
                      </button>
                    ))}
                  </div>
                  {tools.loading ? (
                    <p className="workspace-start-description">
                      正在检测可用 CLI…
                    </p>
                  ) : null}
                  {!tools.loading && !selectedTool ? (
                    <p className="workspace-start-description">
                      尚未找到可用 CLI，可在左下角重新检测。
                    </p>
                  ) : null}
                  <button
                    type="button"
                    className="primary-button workspace-start-button"
                    disabled={projectBusy || tools.loading || !selectedTool}
                    onClick={() => {
                      if (selectedTool)
                        void launcher.start(selectedProject, selectedTool);
                    }}
                  >
                    {launcher.busy
                      ? "正在打开…"
                      : `打开 ${selectedTool?.name ?? "Agent"} 会话`}
                  </button>
                  <p
                    className="workspace-start-path"
                    title={displayPath(selectedProject.path)}
                  >
                    {displayPath(selectedProject.path)}
                  </p>
                </>
              ) : (
                <button
                  type="button"
                  className="primary-button workspace-start-button"
                  disabled={projectBusy}
                  onClick={() => void projects.add()}
                >
                  添加项目
                </button>
              )}

              {history.error || recentHistory.length ? (
                <section className="workspace-history" aria-label="最近会话">
                  <div className="workspace-history-heading">
                    <h2>最近会话</h2>
                    <div className="workspace-history-heading-actions">
                      <span>按原项目与工具新开</span>
                      {closedHistory.length > 20 ? (
                        <button
                          type="button"
                          className="ghost-button"
                          onClick={() => setHistoryExpanded((open) => !open)}
                        >
                          {historyExpanded
                            ? "收起"
                            : `展开全部（${closedHistory.length}）`}
                        </button>
                      ) : null}
                    </div>
                  </div>
                  {history.error ? (
                    <div className="workspace-history-error">
                      <p className="tools-error" role="alert">
                        {history.error}
                      </p>
                      <button
                        type="button"
                        className="ghost-button"
                        disabled={history.loading}
                        onClick={() => void history.reload()}
                      >
                        重新读取
                      </button>
                    </div>
                  ) : null}
                  {recentHistory.length ? (
                    <>
                      <ul className="workspace-history-list">
                        {recentHistory.map((record) => {
                          const reopenTool = installedToolForRecord(
                            record,
                            tools.tools,
                          );
                          const registeredTool = tools.tools.find(
                            (item) => item.id === record.toolId,
                          );
                          const reasonTitle = reopenTool
                            ? displayPath(record.projectPath)
                            : registeredTool
                              ? `${record.toolName}${badgeLabel(registeredTool.state)}`
                              : `未找到 ${record.toolName}`;
                          return (
                            <li
                              key={record.id}
                              className="workspace-history-item"
                            >
                              <div className="workspace-history-meta">
                                <div className="workspace-history-title-row">
                                  <span
                                    className="session-agent-tag"
                                    data-agent={record.toolId}
                                    title={record.toolName}
                                  >
                                    {agentBadgeLabel(
                                      record.toolId,
                                      record.toolName,
                                    )}
                                  </span>
                                  <strong>{historyDisplayTitle(record)}</strong>
                                </div>
                                <span title={displayPath(record.projectPath)}>
                                  {historySubtitleText(record)}
                                </span>
                              </div>
                              <div className="workspace-history-actions">
                                {[
                                  { label: "接续对话", resume: true },
                                  { label: "再次打开", resume: false },
                                ].map(({ label, resume }) => (
                                  <button
                                    key={label}
                                    type="button"
                                    className="ghost-button"
                                    disabled={
                                      launcher.busy ||
                                      tools.loading ||
                                      !reopenTool ||
                                      (resume && !record.cliSessionId)
                                    }
                                    title={
                                      resume && !record.cliSessionId
                                        ? "该会话未记录可恢复标识，可使用右侧「再次打开」"
                                        : reasonTitle
                                    }
                                    onClick={() => {
                                      if (!reopenTool) return;
                                      const existingProject =
                                        projects.projects.find(
                                          (p) => p.path === record.projectPath,
                                        );
                                      if (existingProject) {
                                        setProjectId(existingProject.id);
                                      }
                                      void launcher.start(
                                        existingProject ??
                                          projectFromRecord(record),
                                        reopenTool,
                                        resume
                                          ? {
                                              resume: true,
                                              cliSessionId:
                                                record.cliSessionId ??
                                                undefined,
                                              title: record.title ?? undefined,
                                            }
                                          : undefined,
                                      );
                                    }}
                                  >
                                    {label}
                                  </button>
                                ))}
                              </div>
                            </li>
                          );
                        })}
                      </ul>
                      <p className="workspace-history-note">
                        历史记录只保存项目位置、时间与结束状态；再次打开会按原项目与工具新开终端。
                      </p>
                    </>
                  ) : null}
                </section>
              ) : null}
              <p className="workspace-entry-boundary">
                此入口新开 CLI 会话；外部终端中的已有会话尚未接管。
              </p>
            </div>
          </div>
        ) : null}
      </section>
      {recordsOpen || scheduleOpen || (filesOpen && fileProject) ? (
        <div
          className="workspace-splitter workspace-side-panel-splitter"
          role="separator"
          aria-orientation="vertical"
          aria-label="调整右侧面板宽度"
          aria-valuemin={280}
          aria-valuemax={720}
          aria-valuenow={sideWidth}
          tabIndex={0}
          onPointerDown={(event) =>
            resizeWithPointer(event, sideWidth, setSideWidth, true)
          }
          onKeyDown={(event) =>
            resizeWithKeyboard(event, sideWidth, setSideWidth, true)
          }
        />
      ) : null}
      {filesOpen && fileProject ? (
        <ProjectFiles
          key={fileProject.id}
          project={fileProject}
          onClose={() => setFilesOpen(false)}
        />
      ) : null}
      {scheduleOpen ? (
        <ScheduledTasks
          onClose={() => setScheduleOpen(false)}
          onNotice={setTaskNotice}
        />
      ) : null}
      {taskNotice || updateNotice ? (
        <div className="workspace-notices">
          {updateNotice ? (
            <section
              className={`workspace-update-card${
                updateNotice.kind === "status" && updateNotice.isError
                  ? " is-error"
                  : ""
              }`}
              role={
                updateNotice.kind === "status" && updateNotice.isError
                  ? "alert"
                  : "status"
              }
              aria-label="版本更新"
            >
              {updateNotice.kind === "status" ? (
                <div className="workspace-update-status-row">
                  <div className="workspace-update-copy">
                    <strong>
                      {updateNotice.isError ? "检查更新失败" : "版本状态"}
                    </strong>
                    <p>{updateNotice.text}</p>
                  </div>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => setUpdateNotice(null)}
                  >
                    关闭
                  </button>
                </div>
              ) : updateNotice.kind === "downloading" ? (
                <div className="workspace-update-status-row">
                  <div className="workspace-update-copy">
                    <strong>正在准备更新</strong>
                    <p>正在下载并校验 {updateNotice.version}…</p>
                  </div>
                </div>
              ) : (
                <div className="workspace-update-available">
                  <div className="workspace-update-head">
                    <strong>
                      {updateNotice.blockedAfterDownload
                        ? `更新包 ${updateNotice.status.latestVersion ?? ""} 已就绪，当前仍有任务运行`
                        : `发现新版本 ${updateNotice.status.latestVersion}`}
                    </strong>
                    <span className="workspace-update-channel">
                      {updateNotice.status.channel === "stable"
                        ? "正式版"
                        : "测试版"}
                    </span>
                  </div>
                  {updateNotice.status.currentVersion ? (
                    <p className="workspace-update-versions">
                      当前 {updateNotice.status.currentVersion} →{" "}
                      {updateNotice.status.latestVersion}
                    </p>
                  ) : null}
                  {updateNotice.status.notes ? (
                    <pre className="workspace-update-notes">
                      {updateNotice.status.notes}
                    </pre>
                  ) : null}
                  {updateNotice.activity.hasActiveWork ? (
                    <div className="workspace-update-warning">
                      <p>
                        更新需要退出 AgentTerm
                        并终止运行中的进程；若现在更新，系统会在退出前保存会话，并在更新重启后提示恢复：
                      </p>
                      <ul>
                        {describeUpdateActivity(updateNotice.activity).map(
                          (line) => (
                            <li key={line}>{line}</li>
                          ),
                        )}
                      </ul>
                    </div>
                  ) : null}
                  <div className="workspace-update-actions">
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => setUpdateNotice(null)}
                    >
                      {updateNotice.blockedAfterDownload
                        ? "稍后安装"
                        : "暂不更新"}
                    </button>
                    {updateNotice.activity.hasActiveWork &&
                    preparedUpdate?.downloadUrl !==
                      updateNotice.status.downloadUrl ? (
                      <button
                        type="button"
                        className="ghost-button"
                        onClick={() =>
                          void handleDownloadUpdateOnly(updateNotice.status)
                        }
                      >
                        仅下载安装包（稍后安装）
                      </button>
                    ) : null}
                    {updateNotice.activity.hasActiveWork ? (
                      <button
                        type="button"
                        className="danger-button"
                        onClick={() =>
                          void handleInstallUpdate(updateNotice.status, true)
                        }
                      >
                        保存会话并立即更新
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="primary-button"
                        onClick={() =>
                          void handleInstallUpdate(updateNotice.status, false)
                        }
                      >
                        {preparedUpdate?.downloadUrl ===
                        updateNotice.status.downloadUrl
                          ? "立即重启并安装"
                          : "立即更新"}
                      </button>
                    )}
                  </div>
                </div>
              )}
            </section>
          ) : null}
          {taskNotice ? (
            <div className="workspace-task-notice" role="status">
              {taskNotice}
              <button type="button" onClick={() => setTaskNotice(null)}>
                关闭
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
      {recordsOpen ? (
        <aside
          className="workspace-records"
          id="session-records"
          aria-label="任务记录与交付"
        >
          <div className="workspace-records-heading">
            <strong>任务记录与交付</strong>
            <button
              type="button"
              className="sidebar-icon-button"
              aria-label="关闭任务记录"
              onClick={() => setRecordsOpen(false)}
            >
              ×
            </button>
          </div>
          <p className="workspace-records-note">
            手动 CLI
            需先明确关联到任务记录；受控执行入口会继续校验会话和任务版本。
          </p>
          <TaskWorkbench
            projects={projects.projects}
            projectBusy={projects.busy || projects.loading}
            projectError={projects.error}
            draft={draft}
            onDraftChange={setDraft}
            onAddProject={projects.add}
            onRefreshProjects={projects.refresh}
            currentSession={
              currentSession?.running ? currentSession.info : null
            }
          />
        </aside>
      ) : null}
    </main>
  );
}
