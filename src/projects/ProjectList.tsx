import { useState } from "react";
import type { Project } from "./types";
import type { ToolStatus } from "../tools/types";

type Props = {
  projects: Project[];
  tools: ToolStatus[];
  busy: boolean;
  error: string | null;
  startError: string | null;
  pendingRemoval: Project | null;
  onAdd: () => void;
  onRequestRemove: (id: string) => void;
  onCancelRemove: () => void;
  onConfirmRemove: () => void;
  onStart: (project: Project, tool: ToolStatus) => void;
};

export default function ProjectList({
  projects,
  tools,
  busy,
  error,
  startError,
  pendingRemoval,
  onAdd,
  onRequestRemove,
  onCancelRemove,
  onConfirmRemove,
  onStart,
}: Props) {
  const [launcherFor, setLauncherFor] = useState<string | null>(null);
  const installedTools = tools.filter(
    (tool) => tool.state.kind === "installed",
  );

  return (
    <section className="projects" aria-labelledby="projects-title">
      <div className="tools-header">
        <h2 id="projects-title">项目</h2>
        <button
          type="button"
          className="refresh-button"
          onClick={onAdd}
          disabled={busy}
        >
          添加项目
        </button>
      </div>

      {error ? (
        <p className="tools-error" role="alert">
          {error}
        </p>
      ) : null}
      {startError ? (
        <p className="tools-error" role="alert">
          {startError}
        </p>
      ) : null}

      {projects.length === 0 ? (
        <p className="tools-empty">
          还没有项目。点击「添加项目」，选择一个本地文件夹即可开始。
        </p>
      ) : (
        <ul className="project-list">
          {projects.map((project) => (
            <li className="project-row" key={project.id}>
              <div className="project-info">
                <span className="project-name">{project.name}</span>
                <span className="project-path" title={project.path}>
                  {project.path}
                </span>
              </div>

              {pendingRemoval?.id === project.id ? (
                <div className="project-confirm">
                  <span className="project-confirm-text">
                    只移除记录，不删除文件夹。
                  </span>
                  <button
                    type="button"
                    className="danger-button"
                    onClick={onConfirmRemove}
                    disabled={busy}
                  >
                    确认移除
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={onCancelRemove}
                    disabled={busy}
                  >
                    取消
                  </button>
                </div>
              ) : launcherFor === project.id ? (
                <div
                  className="project-confirm"
                  role="group"
                  aria-label="选择工具"
                >
                  {installedTools.length === 0 ? (
                    <span className="project-confirm-text">
                      没有已安装的 AI 工具
                    </span>
                  ) : (
                    installedTools.map((tool) => (
                      <button
                        type="button"
                        className="refresh-button"
                        key={tool.id}
                        onClick={() => {
                          setLauncherFor(null);
                          onStart(project, tool);
                        }}
                        disabled={busy}
                      >
                        {tool.name}
                      </button>
                    ))
                  )}
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => setLauncherFor(null)}
                    disabled={busy}
                  >
                    取消
                  </button>
                </div>
              ) : (
                <div className="project-confirm">
                  <button
                    type="button"
                    className="refresh-button"
                    onClick={() => setLauncherFor(project.id)}
                    disabled={busy}
                  >
                    启动
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => onRequestRemove(project.id)}
                    disabled={busy}
                  >
                    移除
                  </button>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
