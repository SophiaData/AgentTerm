import { PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";
import { useState } from "react";
import TaskWorkbench from "./TaskWorkbench";
import ProjectList from "./projects/ProjectList";
import { useProjects } from "./projects/useProjects";
import SessionPanel from "./terminal/SessionPanel";
import { useLauncher } from "./terminal/useLauncher";
import ToolList from "./tools/ToolList";
import { useTools } from "./tools/useTools";
import { emptyPlan } from "./workbenchState";

export default function App() {
  const tools = useTools();
  const projects = useProjects();
  const launcher = useLauncher();
  const [page, setPage] = useState<"workbench" | "terminal">("workbench");
  const [draft, setDraft] = useState(emptyPlan);

  return (
    <main className="app-shell">
      <header className="brand" aria-label={PRODUCT_NAME}>
        <span className="brand-mark" aria-hidden="true">
          &gt;_
        </span>
        <div className="brand-copy">
          <span>{PRODUCT_NAME}</span>
          <small>数据开发工作台</small>
        </div>
        <span className="local-badge">本地工作区</span>
      </header>

      <nav className="app-tabs" aria-label="工作区">
        <button
          className="ghost-button"
          aria-current={page === "workbench" ? "page" : undefined}
          onClick={() => setPage("workbench")}
        >
          任务工作台
        </button>
        <button
          className="ghost-button"
          aria-current={page === "terminal" ? "page" : undefined}
          onClick={() => setPage("terminal")}
        >
          项目与终端
          {launcher.session
            ? launcher.session.running
              ? " · 运行中"
              : " · 已结束"
            : ""}
        </button>
      </nav>

      {page === "workbench" ? (
        <TaskWorkbench
          projects={projects.projects}
          projectBusy={projects.busy || projects.loading}
          projectError={projects.error}
          draft={draft}
          onDraftChange={setDraft}
          onAddProject={projects.add}
          onRefreshProjects={projects.refresh}
        />
      ) : null}
      {launcher.session ? (
        <SessionPanel
          key={launcher.session.info.id}
          session={launcher.session.info}
          active={page === "terminal"}
          running={launcher.session.running}
          exitReason={launcher.session.exitReason}
          busy={launcher.busy}
          closeError={launcher.closeError}
          onClose={launcher.close}
          onExit={launcher.handleExit}
        />
      ) : page === "terminal" ? (
        <div className="launcher-content">
          <section className="status-card" aria-labelledby="foundation-title">
            <p className="eyebrow">项目与工具</p>
            <h1 id="foundation-title">独立终端</h1>
            <p className="description">
              选择项目与工具，打开交互终端。这里的会话由你手动操作，不会自动推进任务工作台中的计划。
            </p>
            <div className="target-list" aria-label="支持的平台">
              {SUPPORTED_TARGETS.map((target) => (
                <span className="target" key={target.id}>
                  {target.label}
                </span>
              ))}
            </div>
          </section>

          <ProjectList
            projects={projects.projects}
            tools={tools.tools}
            busy={projects.busy || projects.loading || launcher.busy}
            error={projects.error}
            startError={launcher.startError}
            pendingRemoval={projects.pendingRemoval}
            onAdd={projects.add}
            onRequestRemove={projects.requestRemove}
            onCancelRemove={projects.cancelRemove}
            onConfirmRemove={projects.confirmRemove}
            onStart={launcher.start}
          />

          <ToolList
            tools={tools.tools}
            loading={tools.loading}
            error={tools.error}
            onRefresh={tools.refresh}
          />
        </div>
      ) : null}

      <footer className="footer">
        计划与交付保存在本机<span>关闭窗口会收起应用</span>
      </footer>
    </main>
  );
}
