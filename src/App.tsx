import { useState } from "react";
import { PRODUCT_NAME } from "./meta";
import TaskWorkbench from "./TaskWorkbench";
import { useProjects } from "./projects/useProjects";
import SessionPanel from "./terminal/SessionPanel";
import { useLauncher, type ActiveSession } from "./terminal/useLauncher";
import { badgeLabel, detailText } from "./tools/types";
import { useTools } from "./tools/useTools";
import { emptyPlan } from "./workbenchState";

export default function App() {
  const tools = useTools();
  const projects = useProjects();
  const launcher = useLauncher();
  const [projectId, setProjectId] = useState<string | null>(null);
  const [toolId, setToolId] = useState<string | null>(null);
  const [recordsOpen, setRecordsOpen] = useState(false);
  const [draft, setDraft] = useState(emptyPlan);
  const selectedProject =
    projects.projects.find((p) => p.id === projectId) ?? projects.projects[0];
  const selectedTool =
    tools.tools.find((t) => t.id === toolId && t.state.kind === "installed") ??
    tools.tools.find((t) => t.state.kind === "installed");
  const currentSession = launcher.session;
  const projectBusy = projects.loading || projects.busy || launcher.busy;
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
      title={session.info.projectPath}
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
    <main className={`session-workspace${recordsOpen ? " with-records" : ""}`}>
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
                title={project.path}
                onClick={() => {
                  setProjectId(project.id);
                  const recent = launcher.sessions
                    .filter((s) => s.info.projectPath === project.path)
                    .slice(-1)[0];
                  if (recent) launcher.select(recent.info.id);
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
        <div className="sidebar-bottom">
          {selectedProject ? (
            <details className="sidebar-details">
              <summary>项目设置</summary>
              <p>{selectedProject.path}</p>
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
          <details className="sidebar-details">
            <summary>CLI 工具状态</summary>
            {tools.tools.map((tool) => (
              <div key={tool.id} className="sidebar-tool-status">
                <strong>
                  {tool.name} · {badgeLabel(tool.state)}
                </strong>
                <p>{detailText(tool.state)}</p>
              </div>
            ))}
            <button
              type="button"
              className="ghost-button"
              disabled={tools.loading || launcher.busy}
              onClick={() => void tools.refresh()}
            >
              {tools.loading ? "检测中…" : "重新检测"}
            </button>
          </details>
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
          <button
            type="button"
            className="ghost-button"
            aria-expanded={recordsOpen}
            aria-controls="session-records"
            onClick={() => setRecordsOpen((open) => !open)}
          >
            {recordsOpen ? "收起任务记录" : "任务记录"}
          </button>
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
                title={session.info.projectPath}
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
        {launcher.sessions.map((session) => (
          <div
            key={session.info.id}
            className="workspace-terminal-stage"
            id={`pane-${session.info.id}`}
            role="tabpanel"
            aria-labelledby={`tab-${session.info.id}`}
            hidden={launcher.activeId !== session.info.id}
          >
            <SessionPanel
              session={session.info}
              label={session.label}
              active={launcher.activeId === session.info.id}
              running={session.running}
              exitReason={session.exitReason}
              busy={launcher.closingId === session.info.id}
              closeError={session.closeError}
              onClose={() => void launcher.close(session.info.id)}
              onExit={launcher.handleExit}
            />
          </div>
        ))}
        {!currentSession ? (
          <div className="workspace-start">
            <div className="workspace-start-content">
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
                    title={selectedProject.path}
                  >
                    {selectedProject.path}
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
              {launcher.startError ? (
                <p className="tools-error" role="alert">
                  {launcher.startError}
                </p>
              ) : null}
              {tools.error ? (
                <p className="tools-error" role="alert">
                  {tools.error}
                </p>
              ) : null}
              <p className="workspace-entry-boundary">
                此入口新开 CLI 会话；外部终端中的已有会话尚未接管。
              </p>
            </div>
          </div>
        ) : null}
      </section>
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
            手动 CLI 与任务记录尚未自动关联。
          </p>
          <TaskWorkbench
            projects={projects.projects}
            projectBusy={projects.busy || projects.loading}
            projectError={projects.error}
            draft={draft}
            onDraftChange={setDraft}
            onAddProject={projects.add}
            onRefreshProjects={projects.refresh}
          />
        </aside>
      ) : null}
    </main>
  );
}
