import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import type { Project } from "./projects/types";
import {
  acceptSnapshot,
  attemptLabels,
  latestAttempts,
  phaseLabels,
  requestOrder,
  type PlanInput,
  type Preview,
  type WorkbenchView,
} from "./workbenchState";

async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const { invoke: call } = await import("@tauri-apps/api/core");
  return call<T>(command, args);
}
function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function TaskCard({
  view,
  busy,
  preview,
  onReference,
  onPreview,
  onCancel,
}: {
  view: WorkbenchView;
  busy: boolean;
  preview: Preview | null;
  onReference: (relativePath: string, label: string) => void;
  onPreview: (id: string) => void;
  onCancel: () => void;
}) {
  const active = view.ledger.active;
  const document = view.document;
  if (!active)
    return (
      <p className="workbench-empty">
        当前没有主任务。选择已有项目，登记本轮目标。
      </p>
    );
  const editable = !active.recoveryRequired;
  return (
    <article className="task-card">
      <div className="tools-header">
        <h2>当前主任务</h2>
        <span className="badge">
          {active.recoveryRequired
            ? "待核实"
            : (phaseLabels[active.phase] ?? "未知状态 · 待核实")}
        </span>
      </div>
      <p className="task-id">{active.taskId}</p>
      <p className="task-path">{active.projectId}</p>
      {active.recoveryRequired && (
        <p role="status">
          这项任务来自先前进程或执行状态待核实。占用继续保留，目前仅可查看。
        </p>
      )}
      {document ? (
        <>
          <h3>{document.plan.goal}</h3>
          <dl className="task-facts">
            <dt>负责人</dt>
            <dd>{document.plan.owner}</dd>
            <dt>工作范围</dt>
            <dd>{document.plan.scope}</dd>
            <dt>产物目录</dt>
            <dd className="task-path">{document.plan.outputDir}</dd>
          </dl>
          <h3>执行与验收</h3>
          <ol className="task-steps">
            {document.plan.steps.map((step) => {
              const attempts = latestAttempts(document.events).filter(
                (event) => event.stepId === step.id,
              );
              return (
                <li key={step.id}>
                  <strong>{step.title}</strong>
                  <span className="task-assignee">{step.assignee}</span>
                  {attempts.length === 0 ? (
                    <p className="tool-detail">尚无执行记录</p>
                  ) : (
                    attempts.map((attempt) => (
                      <div className="task-attempt" key={attempt.attemptId}>
                        <span className="badge">
                          {attemptLabels[attempt.state]}
                        </span>
                        <small>尝试 {attempt.attemptId}</small>
                        <p className="task-summary">{attempt.summary}</p>
                      </div>
                    ))
                  )}
                </li>
              );
            })}
          </ol>
          <p className="tool-detail">
            报告是执行与审查记录；“结果已提交”不代表主任务完成或生产批准。
          </p>
          <h3>产物与版本</h3>
          {document.artifacts.length === 0 ? (
            <p className="tool-detail">尚未登记产物。</p>
          ) : (
            <ul className="task-artifacts">
              {document.artifacts.map((artifact) => (
                <li key={artifact.id}>
                  <div>
                    <strong>{artifact.label}</strong>
                    <span className="badge">v{artifact.version}</span>
                  </div>
                  <p className="task-path">{artifact.relativePath}</p>
                  <small>
                    SHA-256 {artifact.sha256} · {artifact.bytes} 字节
                  </small>
                  <button
                    type="button"
                    className="ghost-button"
                    disabled={busy}
                    onClick={() => onPreview(artifact.id)}
                  >
                    查看此版本
                  </button>
                </li>
              ))}
            </ul>
          )}
          {editable && (
            <form
              className="artifact-form"
              onSubmit={(event) => {
                event.preventDefault();
                const form = new FormData(event.currentTarget);
                onReference(
                  String(form.get("path") ?? ""),
                  String(form.get("label") ?? ""),
                );
              }}
            >
              <label>
                已有文件（相对于产物目录）
                <input
                  name="path"
                  placeholder="report.md 或 model.sql"
                  required
                  disabled={busy}
                />
              </label>
              <label>
                产物名称
                <input name="label" maxLength={120} required disabled={busy} />
              </label>
              <button className="refresh-button" disabled={busy}>
                登记版本
              </button>
              <small>
                仅引用项目内已存在的 SQL、Markdown、TXT、JSON 文件，最大 1 MiB。
              </small>
            </form>
          )}
          {preview &&
            document.artifacts.some((item) => item.id === preview.id) && (
              <section className="artifact-preview" aria-label="产物内容">
                <h3>已核对版本的内容</h3>
                <pre>{preview.content}</pre>
              </section>
            )}
        </>
      ) : (
        <p role="status">
          此任务尚无计划文档。未派发且归属当前进程时，可以补全计划。
        </p>
      )}
      {editable && active.phase === "reserved" && (
        <button className="ghost-button" disabled={busy} onClick={onCancel}>
          取消未派发预留
        </button>
      )}
    </article>
  );
}

function PlanForm({
  projects,
  busy,
  activeProject,
  onSubmit,
}: {
  projects: Project[];
  busy: boolean;
  activeProject?: string;
  onSubmit: (input: PlanInput) => void;
}) {
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const values = new FormData(event.currentTarget);
    onSubmit({
      projectId: activeProject ?? String(values.get("project") ?? ""),
      goal: String(values.get("goal") ?? ""),
      owner: String(values.get("owner") ?? ""),
      scope: String(values.get("scope") ?? ""),
      outputDir: String(values.get("output") ?? "."),
    });
  }
  return (
    <form className="task-form" onSubmit={submit}>
      <h2>{activeProject ? "补全任务计划" : "预留一个主任务"}</h2>
      <label>
        已有项目
        <select
          name="project"
          defaultValue={activeProject ?? ""}
          disabled={busy || !!activeProject}
          required
        >
          <option value="">请选择项目</option>
          {projects.map((project) => (
            <option value={project.id} key={project.id}>
              {project.name}
            </option>
          ))}
        </select>
      </label>
      <label>
        本轮目标
        <textarea
          name="goal"
          required
          maxLength={4000}
          disabled={busy}
          rows={3}
        />
      </label>
      <label>
        负责人
        <input name="owner" required maxLength={60} disabled={busy} />
      </label>
      <label>
        工作范围
        <textarea
          name="scope"
          required
          maxLength={2000}
          disabled={busy}
          rows={2}
          placeholder="例如：只分析本地 SQL 草稿与报告"
        />
      </label>
      <label>
        已有产物目录
        <input name="output" defaultValue="." required disabled={busy} />
        <small>相对于项目目录；“.” 表示项目根目录，不会创建目录。</small>
      </label>
      <button
        className="refresh-button"
        disabled={busy || projects.length === 0}
      >
        {activeProject ? "保存计划" : "预留并保存计划"}
      </button>
      <p className="tool-detail">
        预留只记录目标与占用；CLI 自动派发仍待验收。
      </p>
    </form>
  );
}

export default function TaskWorkbench({
  projects,
  onProjects,
}: {
  projects: Project[];
  onProjects: () => void;
}) {
  const [view, setView] = useState<WorkbenchView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const order = useRef(requestOrder());
  const mutation = useRef(false);
  const load = useCallback(async () => {
    const request = order.current.next();
    setLoading(true);
    setPreview(null);
    try {
      const next = await invoke<WorkbenchView>("workbench_snapshot");
      if (order.current.current(request)) {
        setView((previous) => acceptSnapshot(previous, next));
        setError(null);
      }
    } catch (cause) {
      if (order.current.current(request)) setError(message(cause));
    } finally {
      if (order.current.current(request)) setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
    const requests = order.current;
    return () => {
      requests.next();
    };
  }, [load]);
  async function act(command: string, args?: Record<string, unknown>) {
    if (mutation.current) return;
    mutation.current = true;
    setBusy(true);
    setError(null);
    setPreview(null);
    order.current.next();
    try {
      await invoke(command, args);
      await load();
    } catch (cause) {
      await load();
      setError(message(cause));
    } finally {
      mutation.current = false;
      setBusy(false);
    }
  }
  function ticket() {
    if (!view?.ledger.active) return {};
    return {
      taskId: view.ledger.active.taskId,
      generation: view.ledger.generation,
      expectedRevision: view.ledger.revision,
    };
  }
  async function showPreview(id: string) {
    if (mutation.current) return;
    const request = order.current.next();
    setPreview(null);
    setBusy(true);
    try {
      const value = await invoke<Preview>("preview_task_artifact", {
        ...ticket(),
        artifactId: id,
      });
      if (order.current.current(request)) {
        setPreview(value);
        setError(null);
      }
    } catch (cause) {
      if (order.current.current(request)) setError(message(cause));
    } finally {
      if (order.current.current(request)) setBusy(false);
    }
  }
  const active = view?.ledger.active;
  const canPlan =
    !!view &&
    (!active ||
      (!view.document &&
        active.phase === "reserved" &&
        !active.recoveryRequired));
  return (
    <section className="workbench" aria-labelledby="workbench-title">
      <div className="tools-header">
        <div>
          <p className="eyebrow">数据开发工作台</p>
          <h1 id="workbench-title">一个主任务，逐步交付。</h1>
        </div>
        <button
          className="refresh-button"
          disabled={busy || loading}
          onClick={() => void load()}
        >
          刷新
        </button>
      </div>
      <p className="tool-detail">
        执行与独立验收顺序接力。等待或待核实时，主任务仍保持占用。
      </p>
      {loading && <p role="status">正在读取任务…</p>}
      {error && (
        <p className="tools-error" role="alert">
          {error}
        </p>
      )}
      {error?.includes("尚未初始化") && (
        <button
          className="refresh-button"
          disabled={busy}
          onClick={() => void act("task_ledger_initialize")}
        >
          初始化本机任务账本
        </button>
      )}
      {view && (
        <TaskCard
          view={view}
          busy={busy || loading || !!error}
          preview={preview}
          onReference={(relativePath, label) =>
            void act("reference_task_artifact", {
              ...ticket(),
              relativePath,
              label,
            })
          }
          onPreview={(id) => void showPreview(id)}
          onCancel={() => void act("cancel_main_task_reservation", ticket())}
        />
      )}
      {canPlan && (
        <PlanForm
          projects={projects}
          busy={busy || loading || !!error}
          activeProject={active?.projectId}
          onSubmit={(input) =>
            void act(active ? "attach_task_plan" : "create_workbench_task", {
              ...ticket(),
              input,
            })
          }
        />
      )}
      {projects.length === 0 && (
        <p className="workbench-empty">
          先添加一个已有项目。
          <button className="ghost-button" onClick={onProjects}>
            管理项目
          </button>
        </p>
      )}
    </section>
  );
}
