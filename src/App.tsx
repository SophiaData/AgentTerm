import { useState } from "react";
import { PRODUCT_NAME } from "./meta";
import TaskWorkbench from "./TaskWorkbench";
import { useProjects } from "./projects/useProjects";
import SessionPanel from "./terminal/SessionPanel";
import { useLauncher } from "./terminal/useLauncher";
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
                onClick={() => setProjectId(project.id)}
              >
                <span aria-hidden="true">▱</span>
                <span>{project.name}</span>
              </button>
              {currentSession?.info.projectPath === project.path ? (
                <button
                  type="button"
                  className="workspace-session-link"
                  onClick={() => setRecordsOpen(false)}
                >
                  <span
                    className={`session-dot${currentSession.running ? " is-running" : ""}`}
                    aria-hidden="true"
                  />
                  <span>{currentSession.info.toolName}</span>
                  <small>{currentSession.running ? "运行中" : "已结束"}</small>
                </button>
              ) : null}
            </li>
          ))}
        </ul>
        {!projects.loading && !projects.projects.length ? (
          <p className="sidebar-message">
            添加已有目录，在项目里打开 Agent 会话。
          </p>
        ) : null}
        {currentSession &&
        !projects.projects.some(
          (project) => project.path === currentSession.info.projectPath,
        ) ? (
          <div className="sidebar-current-session">
            <h2>当前会话</h2>
            <button
              type="button"
              className="workspace-session-link"
              onClick={() => setRecordsOpen(false)}
            >
              <span
                className={`session-dot${currentSession.running ? " is-running" : ""}`}
                aria-hidden="true"
              />
              <span>{currentSession.info.toolName}</span>
              <small>{currentSession.running ? "运行中" : "已结束"}</small>
            </button>
            <p className="sidebar-message">
              查看其他项目会保留当前会话。结束后可以在另一项目启动。
            </p>
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
                  disabled={projectBusy || !!currentSession}
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
          <p className="workspace-note">单个活动 CLI 会话 · 本机执行</p>
        </div>
      </aside>

      <section className="workspace-main" aria-label="Agent 会话工作区">
        <header className="workspace-topbar">
          <div className="workspace-context">
            <span>
              {currentSession
                ? currentSession.info.toolName
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
        {currentSession ? (
          <div className="workspace-terminal-stage">
            <SessionPanel
              key={currentSession.info.id}
              session={currentSession.info}
              active
              running={currentSession.running}
              exitReason={currentSession.exitReason}
              busy={launcher.busy}
              closeError={launcher.closeError}
              onClose={launcher.close}
              onExit={launcher.handleExit}
            />
          </div>
        ) : (
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
                  ? "在完整的 CLI 中对话、执行命令和处理审批。会话会留在这个工作区。"
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
        )}
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
