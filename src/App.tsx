import { PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";
import { useState } from "react";
import TaskWorkbench from "./TaskWorkbench";
import ProjectList from "./projects/ProjectList";
import { useProjects } from "./projects/useProjects";
import SessionPanel from "./terminal/SessionPanel";
import { useLauncher } from "./terminal/useLauncher";
import ToolList from "./tools/ToolList";
import { useTools } from "./tools/useTools";

export default function App() {
  const tools = useTools();
  const projects = useProjects();
  const launcher = useLauncher();
  const [page, setPage] = useState<"workbench" | "terminal">("workbench");

  return (
    <main className="app-shell">
      <header className="brand" aria-label={PRODUCT_NAME}>
        <span className="brand-mark" aria-hidden="true">
          &gt;_
        </span>
        <span>{PRODUCT_NAME}</span>
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
          项目与独立终端
        </button>
      </nav>

      {page === "workbench" ? (
        <TaskWorkbench
          projects={projects.projects}
          onProjects={() => setPage("terminal")}
        />
      ) : launcher.session ? (
        <SessionPanel
          session={launcher.session.info}
          running={launcher.session.running}
          exitReason={launcher.session.exitReason}
          busy={launcher.busy}
          closeError={launcher.closeError}
          onClose={launcher.close}
          onExit={launcher.handleExit}
        />
      ) : (
        <div className="launcher-content">
          <section className="status-card" aria-labelledby="foundation-title">
            <p className="eyebrow">MILESTONE 1</p>
            <h1 id="foundation-title">一个入口，管理你的 AI 终端。</h1>
            <p className="description">
              选择项目，再选择 AI
              工具，即可在内置终端里开始工作。关闭窗口只会收起 AgentTerm。
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
            busy={projects.busy || launcher.busy}
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
      )}

      <footer className="footer">Tauri 2 · Rust · React</footer>
    </main>
  );
}
