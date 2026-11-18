import type { Project } from "./types";

type Props = {
  projects: Project[];
  busy: boolean;
  error: string | null;
  pendingRemoval: Project | null;
  onAdd: () => void;
  onRequestRemove: (id: string) => void;
  onCancelRemove: () => void;
  onConfirmRemove: () => void;
};

export default function ProjectList({
  projects,
  busy,
  error,
  pendingRemoval,
  onAdd,
  onRequestRemove,
  onCancelRemove,
  onConfirmRemove,
}: Props) {
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
              ) : (
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => onRequestRemove(project.id)}
                  disabled={busy}
                >
                  移除
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
