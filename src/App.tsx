import { PRODUCT_NAME, SUPPORTED_TARGETS } from "./meta";
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

  return (
    <main className="app-shell">
      <header className="brand" aria-label={PRODUCT_NAME}>
        <span className="brand-mark" aria-hidden="true">
          &gt;_
        </span>
        <span>{PRODUCT_NAME}</span>
      </header>

      {launcher.session ? (
        <SessionPanel
          session={launcher.session.info}
          running={launcher.session.running}
          exitReason={launcher.session.exitReason}
          busy={launcher.busy}
          onClose={launcher.close}
          onExit={launcher.handleExit}
        />
      ) : (
        <>
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
        </>
      )}

      <footer className="footer">Tauri 2 · Rust · React</footer>
    </main>
  );
}
