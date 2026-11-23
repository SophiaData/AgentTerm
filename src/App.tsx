import { useEffect, useRef, useState } from "react";
import { PRODUCT_NAME } from "./meta";
import TaskWorkbench from "./TaskWorkbench";
import ProjectFiles from "./files/ProjectFiles";
import { displayPath } from "./projects/displayPath";
import { useProjects } from "./projects/useProjects";
import SessionWorkspace from "./terminal/SessionWorkspace";
import {
  folderName,
  formatSessionTime,
  historyStatusText,
  installedToolForRecord,
  projectFromRecord,
  recentClosedHistory,
} from "./terminal/sessionHistory";
import { useLauncher, type ActiveSession } from "./terminal/useLauncher";
import { useSessionHistory } from "./terminal/useSessionHistory";
import { badgeLabel, detailText } from "./tools/types";
import { useTools } from "./tools/useTools";

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
import { emptyPlan } from "./workbenchState";

export default function App() {
  const tools = useTools();
  const projects = useProjects();
  const launcher = useLauncher();
  const history = useSessionHistory();
  const [projectId, setProjectId] = useState<string | null>(null);
  const [toolId, setToolId] = useState<string | null>(null);
  const [recordsOpen, setRecordsOpen] = useState(false);
  const [filesOpen, setFilesOpen] = useState(true);
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
  const activeSessionIds = new Set(launcher.sessions.map((s) => s.info.id));
  const recentHistory = recentClosedHistory(history.records, activeSessionIds);
  const runningCount = launcher.sessions.filter((s) => s.running).length;
  const historyReloaded = useRef(false);
  const reloadHistory = history.reload;
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
  const selectSession = (session: ActiveSession) => {
    const project = projects.projects.find(
      (p) => p.path === session.info.projectPath,
    );
    if (project) setProjectId(project.id);
    launcher.select(session.info.id);
  };
  const sessionLink = (session: ActiveSession) => (
    <button
      type="button"
      key={session.info.id}
      className="workspace-session-link"
      aria-current={launcher.activeId === session.info.id ? "true" : undefined}
      onClick={() => selectSession(session)}
      title={displayPath(session.info.projectPath)}
    >
      <span
        className={`session-dot${session.running ? " is-running" : ""}`}
        aria-hidden="true"
      />
      <span>{session.label}</span>
      <small>{session.running ? "运行中" : "已结束"}</small>
    </button>
  );

  return (
    <main
      className={`session-workspace${recordsOpen ? " with-records" : ""}${filesOpen && fileProject ? " with-files" : ""}`}
    >
      <aside className="workspace-sidebar" aria-label="项目与会话">
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
          {selectedProject ? (
            <details className="sidebar-details">
              <summary>项目设置</summary>
              <p>{displayPath(selectedProject.path)}</p>
              {projects.pendingRemoval ? (
                <div className="sidebar-removal">
                  <p>
                    移除“{projects.pendingRemoval.name}
                    ”的列表记录，项目文件保留。
                  </p>
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
                  disabled={
                    projectBusy ||
                    launcher.sessions.some(
                      (s) => s.info.projectPath === selectedProject.path,
                    )
                  }
                  onClick={() => projects.requestRemove(selectedProject.id)}
                >
                  移除项目记录
                </button>
              )}
            </details>
          ) : null}
          <p className="workspace-note">
            {launcher.sessions.length
              ? `${launcher.sessions.length} 个会话 · ${launcher.sessions.filter((s) => s.running).length} 个进程运行中`
              : "本机 CLI 会话工作区"}
          </p>
        </div>
      </aside>

      <section className="workspace-main" aria-label="Agent 会话工作区">
        <header className="workspace-topbar">
          <div className="workspace-context">
            <span>
              {currentSession
                ? (projects.projects.find(
                    (p) => p.path === currentSession.info.projectPath,
                  )?.name ?? currentSession.info.toolName)
                : (selectedProject?.name ?? "工作区")}
            </span>
            <span className="workspace-context-separator">/</span>
            <strong>{currentSession ? "终端会话" : "新建会话"}</strong>
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
            <button
              type="button"
              className="ghost-button"
              aria-expanded={recordsOpen}
              aria-controls="session-records"
              onClick={() => {
                setRecordsOpen((open) => !open);
                setFilesOpen(false);
              }}
            >
              {recordsOpen ? "收起任务记录" : "任务记录"}
            </button>
          </div>
        </header>
        {launcher.sessions.length ? (
          <div
            className="workspace-tabs"
            role="tablist"
            aria-label="已打开的会话"
          >
            {launcher.sessions.map((session) => (
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
                className="workspace-tab"
                key={session.info.id}
                title={displayPath(session.info.projectPath)}
                onClick={() => selectSession(session)}
                onKeyDown={(event) => {
                  const keys = ["ArrowLeft", "ArrowRight", "Home", "End"];
                  if (!keys.includes(event.key)) return;
                  event.preventDefault();
                  const tabs = Array.from(
                    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>(
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
                  className={`session-dot${session.running ? " is-running" : ""}`}
                  aria-hidden="true"
                />
                {session.label}
              </button>
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
        <SessionWorkspace
          sessions={launcher.sessions}
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
                            ? "打开原生 CLI"
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
                    <span>按原项目与工具新开</span>
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
                                <strong>
                                  {record.toolName} ·{" "}
                                  {folderName(record.projectPath)}
                                </strong>
                                <span title={displayPath(record.projectPath)}>
                                  {historyStatusText(record)} ·{" "}
                                  {formatSessionTime(record.startedAtMs)}
                                </span>
                              </div>
                              <button
                                type="button"
                                className="ghost-button"
                                disabled={
                                  launcher.busy || tools.loading || !reopenTool
                                }
                                title={reasonTitle}
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
                                  );
                                }}
                              >
                                再次打开
                              </button>
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
      {filesOpen && fileProject ? (
        <ProjectFiles
          key={fileProject.id}
          project={fileProject}
          onClose={() => setFilesOpen(false)}
        />
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
