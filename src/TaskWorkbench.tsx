import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import type { Project } from "./projects/types";
import { displayPath } from "./projects/displayPath";
import type { SessionInfo } from "./terminal/types";
import {
  acceptSnapshot,
  actionsBlocked,
  attemptLabels,
  emptyPlan,
  latestAttempts,
  isReservation,
  phaseLabels,
  phaseMessages,
  planProblem,
  requestOrder,
  recoveryObservationLabels,
  selectedProject,
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
  projectName,
  onReference,
  onPreview,
  onCancel,
  onRecover,
  onResumeAfterRecovery,
  onReleaseCompleted,
  onRefresh,
  currentSession,
  onAssociateSession,
  onReassociateSession,
  onDispatch,
  onStop,
}: {
  view: WorkbenchView;
  busy: boolean;
  preview: Preview | null;
  projectName?: string;
  onReference: (relativePath: string, label: string) => void;
  onPreview: (id: string) => void;
  onCancel: () => void;
  onRecover: (release: boolean) => void;
  onResumeAfterRecovery: () => void;
  onReleaseCompleted?: () => void;
  onRefresh: () => void;
  currentSession: SessionInfo | null;
  onAssociateSession: () => void;
  onReassociateSession: () => void;
  onDispatch: () => void;
  onStop: () => void;
}) {
  const [confirmation, setConfirmation] = useState<
    "cancel" | "release" | "complete" | null
  >(null);
  const confirmationRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (confirmation) confirmationRef.current?.focus();
  }, [confirmation]);
  const active = view.ledger.active;
  const document = view.document;
  if (!active) return null;
  const editable = !active.recoveryRequired;
  const reserved = isReservation(active.phase);
  const canRecover =
    active.recoveryRequired &&
    reserved &&
    (!document || document.events.length === 0);
  const recovery = view.recovery;
  const canManuallyCompleteRecovery =
    active.recoveryRequired &&
    !!recovery?.attempts.length &&
    recovery.attempts.every((attempt) =>
      ["exited", "launchUncertain"].includes(attempt.observation),
    );
  const sessionLink = document?.sessionLink ?? null;
  const canAssociateSession =
    !!document &&
    !!currentSession &&
    currentSession.projectPath === active.projectId &&
    !sessionLink &&
    !active.recoveryRequired;
  const canDispatch =
    !!document &&
    isReservation(active.phase) &&
    !active.recoveryRequired &&
    !!sessionLink &&
    !!currentSession &&
    currentSession.id === sessionLink.id &&
    currentSession.projectPath === active.projectId &&
    currentSession.toolId === sessionLink.toolId &&
    currentSession.toolId === "claude";
  const canReassociate =
    !active.recoveryRequired &&
    active.phase === "recoveredReserved" &&
    !!sessionLink &&
    !!currentSession &&
    currentSession.id !== sessionLink.id &&
    currentSession.projectPath === active.projectId &&
    currentSession.toolId === sessionLink.toolId;
  const canStop =
    !active.recoveryRequired &&
    (active.phase === "running" ||
      active.phase === "verifying" ||
      active.phase === "pausing");
  const attempts = latestAttempts(document?.events ?? []);
  return (
    <article className="task-card">
      <div className="task-heading">
        <div>
          <p className="eyebrow">当前任务</p>
          <h2>{document?.plan.goal ?? "待补全的任务计划"}</h2>
        </div>
        <span
          className={`badge ${active.recoveryRequired ? "badge-attention" : "badge-installed"}`}
        >
          {active.recoveryRequired
            ? "需要核实"
            : (phaseLabels[active.phase] ?? "未知状态 · 待核实")}
        </span>
      </div>
      <div className="task-meta">
        <span>{projectName ?? displayPath(active.projectId)}</span>
        {document && <span>负责人 · {document.plan.owner}</span>}
      </div>
      {document ? (
        <section className="session-link-card" aria-label="关联 CLI 会话">
          <div>
            <strong>
              {sessionLink ? "已关联 CLI 会话" : "尚未关联 CLI 会话"}
            </strong>
            <p>
              {sessionLink
                ? `${sessionLink.toolName} · ${sessionLink.id}`
                : currentSession &&
                    currentSession.projectPath === active.projectId
                  ? `当前会话：${currentSession.toolName}`
                  : "打开同一项目下的 CLI 会话后，可在这里明确关联。"}
            </p>
          </div>
          {canAssociateSession ? (
            <button
              type="button"
              className="ghost-button"
              disabled={busy}
              onClick={onAssociateSession}
            >
              关联当前会话
            </button>
          ) : null}
          {canReassociate ? (
            <div>
              <p>
                原会话结束后可关联当前会话，原关联记录保留。此操作不会自动执行任务。
              </p>
              <button
                type="button"
                className="ghost-button"
                disabled={busy}
                onClick={onReassociateSession}
              >
                将已接回任务关联到当前会话
              </button>
            </div>
          ) : null}
          {document.sessionLinkHistory?.length ? (
            <details>
              <summary>历史会话关联</summary>
              <ul>
                {document.sessionLinkHistory.map((link) => (
                  <li key={link.id}>
                    {link.toolName} · {link.id}
                  </li>
                ))}
              </ul>
            </details>
          ) : null}
        </section>
      ) : null}
      {active.recoveryRequired ? (
        <section
          className="task-notice notice-attention"
          aria-label="核实任务状态"
        >
          <h3>先确认这项任务的状态</h3>
          <p>
            任务来自另一个窗口或上次运行。核实前会保留当前任务，避免重复执行。
          </p>
          {canRecover ? (
            <>
              <p>
                这项任务尚未开始执行。检查原进程已退出后，可以接回计划，或结束本次安排。
              </p>
              <div className="action-row">
                <button
                  className="primary-button"
                  disabled={busy}
                  onClick={() => onRecover(false)}
                >
                  核实并接回任务
                </button>
                <button
                  className="ghost-button"
                  disabled={busy}
                  onClick={() => setConfirmation("release")}
                >
                  结束本次安排
                </button>
              </div>
            </>
          ) : (
            <>
              <p>
                这是一次受管执行的待核实状态。系统会读取已记录的进程身份；只在全部旧尝试确认退出后，才允许接回任务。
              </p>
              {recovery?.attempts.length ? (
                <ul className="recovery-attempts">
                  {recovery.attempts.map((attempt) => (
                    <li key={attempt.attemptId}>
                      <span>{attempt.stepId}</span>
                      <strong>
                        {recoveryObservationLabels[attempt.observation]}
                      </strong>
                      <small>{attempt.attemptId}</small>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="recovery-empty">
                  尚未读取到可核实的执行尝试，继续保持占用。
                </p>
              )}
              <div className="action-row">
                <button
                  className="ghost-button"
                  disabled={busy}
                  onClick={onRefresh}
                >
                  重新核对
                </button>
                {recovery?.allExited ? (
                  <button
                    className="primary-button"
                    disabled={busy}
                    onClick={onResumeAfterRecovery}
                  >
                    核实并接回任务
                  </button>
                ) : null}
                {canManuallyCompleteRecovery ? (
                  <button
                    className="danger-button"
                    disabled={busy}
                    onClick={() => setConfirmation("complete")}
                  >
                    人工核实并结束任务
                  </button>
                ) : null}
              </div>
            </>
          )}
        </section>
      ) : !reserved ? (
        <section
          className="task-notice notice-attention"
          aria-label="受控执行状态"
          aria-live="polite"
        >
          <h3>{phaseLabels[active.phase] ?? "任务状态待核实"}</h3>
          <p>
            {phaseMessages[active.phase] ??
              "当前状态无法识别，系统保留任务占用，请重新读取后再决定下一步。"}
          </p>
          {active.phase === "waitingInput" ? (
            <div className="action-row">
              <button
                type="button"
                className="primary-button"
                disabled={busy}
                onClick={() => setConfirmation("complete")}
              >
                确认完成并释放任务
              </button>
              <span>
                系统会再次核验执行进程；进程全部退出后才能开始下一项任务。
              </span>
            </div>
          ) : null}
        </section>
      ) : (
        <section className="task-notice" aria-label="下一步">
          <h3>
            {document
              ? canDispatch
                ? "计划已保存，可以开始受控执行"
                : "计划已保存，可以整理交付文件"
              : "补全计划后继续"}
          </h3>
          <p>
            {document
              ? "受控执行会按执行 Agent → 验收 Agent 顺序运行，只使用受限的本地只读 relay；当前终端不会被接管。"
              : "你可以在这里查看计划、添加已有文件并核对版本。"}
          </p>
          {canDispatch ? (
            <div className="action-row">
              <button
                type="button"
                className="primary-button"
                disabled={busy}
                onClick={onDispatch}
              >
                {busy ? "正在执行受控接力…" : "开始受控执行"}
              </button>
              <span>执行结果会回写任务记录，仍需人工核对验收意见。</span>
            </div>
          ) : document && sessionLink && isReservation(active.phase) ? (
            <p className="tool-detail">
              选择已关联且仍运行中的 Claude Code 会话后，才会显示受控执行入口。
            </p>
          ) : null}
        </section>
      )}
      {document ? (
        <>
          <section className="task-section" aria-labelledby="scope-title">
            <h3 id="scope-title">工作范围</h3>
            <p className="task-summary">{document.plan.scope}</p>
            <details className="task-details">
              <summary>项目路径与任务详情</summary>
              <dl className="task-facts">
                <dt>项目路径</dt>
                <dd className="task-path">{displayPath(active.projectId)}</dd>
                <dt>交付目录</dt>
                <dd className="task-path">{document.plan.outputDir}</dd>
                <dt>任务编号</dt>
                <dd className="task-id">{active.taskId}</dd>
              </dl>
            </details>
          </section>
          <section className="task-section" aria-labelledby="progress-title">
            <div className="section-heading">
              <h3 id="progress-title">执行进度</h3>
              <span className="section-caption">执行与验收按顺序进行</span>
            </div>
            <ol className="task-steps">
              <li className="step-saved">
                <span className="step-number" aria-hidden="true">
                  01
                </span>
                <div>
                  <strong>任务计划</strong>
                  <p className="tool-detail">已保存</p>
                </div>
              </li>
              {document.plan.steps.map((step, index) => {
                const stepAttempts = attempts.filter(
                  (event) => event.stepId === step.id,
                );
                return (
                  <li key={step.id}>
                    <span className="step-number" aria-hidden="true">
                      {String(index + 2).padStart(2, "0")}
                    </span>
                    <div className="step-body">
                      <strong>{step.title}</strong>
                      <p className="task-assignee">{step.assignee}</p>
                      {stepAttempts.length === 0 ? (
                        <p className="tool-detail">尚未开始</p>
                      ) : (
                        stepAttempts.map((attempt) => (
                          <div className="task-attempt" key={attempt.attemptId}>
                            <span className="badge">
                              {attemptLabels[attempt.state]}
                            </span>
                            <p className="task-summary">{attempt.summary}</p>
                            <details className="task-details">
                              <summary>执行记录详情</summary>
                              <small>尝试 {attempt.attemptId}</small>
                            </details>
                          </div>
                        ))
                      )}
                    </div>
                  </li>
                );
              })}
            </ol>
            <p className="tool-detail">
              结果已提交不代表主任务完成，仍需核对验收结果。
            </p>
          </section>
          <section className="task-section" aria-labelledby="artifacts-title">
            <div className="section-heading">
              <h3 id="artifacts-title">交付文件</h3>
              <span className="section-caption">
                {document.artifacts.length} 个版本
              </span>
            </div>
            {document.artifacts.length === 0 ? (
              <div className="artifact-empty">
                <strong>还没有交付文件</strong>
                <p>
                  将已有 SQL、设计文档或报告关联到任务，之后可以查看内容和版本。
                </p>
              </div>
            ) : (
              <ul className="task-artifacts">
                {document.artifacts.map((artifact) => (
                  <li key={artifact.id}>
                    <div className="artifact-topline">
                      <div>
                        <strong>{artifact.label}</strong>
                        <span className="badge">v{artifact.version}</span>
                      </div>
                      <button
                        type="button"
                        className="ghost-button"
                        disabled={busy}
                        onClick={() => onPreview(artifact.id)}
                      >
                        查看内容
                      </button>
                    </div>
                    <p className="task-path">{artifact.relativePath}</p>
                    <details className="task-details">
                      <summary>版本校验信息</summary>
                      <small>
                        SHA-256 {artifact.sha256} · {artifact.bytes} 字节
                      </small>
                    </details>
                  </li>
                ))}
              </ul>
            )}
            {editable && (
              <details
                className="artifact-add"
                open={document.artifacts.length === 0 ? true : undefined}
              >
                <summary>添加交付文件</summary>
                <form
                  className="artifact-form"
                  onSubmit={(event) => {
                    event.preventDefault();
                    const form = new FormData(event.currentTarget);
                    onReference(
                      String(form.get("path") ?? "").trim(),
                      String(form.get("label") ?? "").trim(),
                    );
                  }}
                >
                  <label>
                    文件路径
                    <input
                      name="path"
                      placeholder="例如 reports/check.md"
                      required
                      disabled={busy}
                      aria-describedby="artifact-path-hint"
                    />
                  </label>
                  <label>
                    显示名称
                    <input
                      name="label"
                      placeholder="例如 数据核对报告"
                      maxLength={120}
                      required
                      disabled={busy}
                    />
                  </label>
                  <small id="artifact-path-hint">
                    相对于交付目录；支持已有 SQL、Markdown、TXT、JSON，最大 1
                    MiB。
                  </small>
                  <button className="primary-button" disabled={busy}>
                    保存文件版本
                  </button>
                </form>
              </details>
            )}
            {preview &&
              document.artifacts.some(
                (item) =>
                  item.id === preview.id && item.sha256 === preview.sha256,
              ) && (
                <section className="artifact-preview" aria-label="文件内容">
                  <h3>已核对版本的内容</h3>
                  <pre>{preview.content}</pre>
                </section>
              )}
          </section>
        </>
      ) : (
        <p className="tool-detail">
          这项任务尚无计划文档。接回当前未执行的任务后，可以补全计划。
        </p>
      )}
      {editable && reserved && (
        <div className="task-end">
          <button
            className="ghost-button"
            disabled={busy}
            onClick={() => setConfirmation("cancel")}
          >
            取消这项任务
          </button>
          <span>仅适用于尚未执行的任务</span>
        </div>
      )}
      {canStop && active.phase !== "pausing" && (
        <div className="task-end">
          <button className="danger-button" disabled={busy} onClick={onStop}>
            请求停止受控执行
          </button>
          <span>停止请求会先写入任务记录；确认前任务仍保持占用。</span>
        </div>
      )}
      {confirmation && (
        <section
          ref={confirmationRef}
          tabIndex={-1}
          className="confirmation-panel"
          role="group"
          aria-label="确认结束任务"
        >
          <h3>
            {confirmation === "cancel"
              ? "取消这项尚未执行的任务？"
              : confirmation === "release"
                ? "核实后结束本次安排？"
                : "确认任务已完成并释放占用？"}
          </h3>
          <p>
            {confirmation === "complete"
              ? "请先人工核对交付结果和进程状态。释放后计划和文件记录保留，可以安排下一项任务。"
              : "计划和文件记录会保留。确认后才能安排下一项任务。"}
          </p>
          <div className="action-row">
            <button
              className="ghost-button"
              disabled={busy}
              onClick={() => setConfirmation(null)}
            >
              返回任务
            </button>
            <button
              className="danger-button"
              disabled={busy}
              onClick={() => {
                const action = confirmation;
                setConfirmation(null);
                if (action === "cancel") onCancel();
                else if (action === "release") onRecover(true);
                else onReleaseCompleted?.();
              }}
            >
              {confirmation === "cancel"
                ? "确认取消任务"
                : confirmation === "release"
                  ? "核实并结束安排"
                  : "确认完成并释放"}
            </button>
          </div>
        </section>
      )}
    </article>
  );
}

export function PlanForm({
  projects,
  busy,
  projectBusy,
  activeProject,
  draft,
  onDraftChange,
  onAddProject,
  onSubmit,
}: {
  projects: Project[];
  busy: boolean;
  projectBusy: boolean;
  activeProject?: string;
  draft: PlanInput;
  onDraftChange: (input: PlanInput) => void;
  onAddProject: () => void;
  onSubmit: (input: PlanInput) => void;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  const projectId = selectedProject(draft, projects, activeProject);
  function change(field: keyof PlanInput, value: string) {
    setProblem(null);
    onDraftChange({ ...draft, [field]: value });
  }
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const input = {
      ...draft,
      projectId,
      goal: draft.goal.trim(),
      owner: draft.owner.trim(),
      scope: draft.scope.trim(),
      outputDir: draft.outputDir.trim(),
    };
    const issue = planProblem(input);
    if (issue) {
      setProblem(issue);
      return;
    }
    onSubmit(input);
  }
  return (
    <div className="plan-layout">
      <form className="task-form" onSubmit={submit}>
        <div>
          <p className="eyebrow">{activeProject ? "继续整理" : "新任务"}</p>
          <h2>{activeProject ? "补全任务计划" : "安排下一项工作"}</h2>
          <p className="tool-detail">写清目标和范围，让计划与交付放在一起。</p>
        </div>
        <div className="project-picker">
          <label>
            所属项目
            <select
              name="project"
              value={projectId}
              onChange={(event) => change("projectId", event.target.value)}
              disabled={busy || projectBusy || !!activeProject}
              required
            >
              <option value="">
                {projects.length === 0 ? "先添加一个本地项目" : "请选择项目"}
              </option>
              {activeProject &&
                !projects.some((project) => project.id === activeProject) && (
                  <option value={activeProject}>{activeProject}</option>
                )}
              {projects.map((project) => (
                <option value={project.id} key={project.id}>
                  {project.name}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="ghost-button"
            disabled={busy || projectBusy}
            onClick={onAddProject}
          >
            {projectBusy ? "正在读取…" : "添加项目"}
          </button>
        </div>
        <label>
          任务目标
          <textarea
            name="goal"
            value={draft.goal}
            onChange={(event) => change("goal", event.target.value)}
            placeholder="例如：定位昨天订单数据减少的原因，输出核对报告"
            required
            maxLength={4000}
            disabled={busy}
            rows={3}
          />
        </label>
        <label>
          工作范围
          <textarea
            name="scope"
            value={draft.scope}
            onChange={(event) => change("scope", event.target.value)}
            placeholder="说明需要处理的文件、对象和边界，例如：只分析本地 SQL 与日志"
            required
            maxLength={2000}
            disabled={busy}
            rows={2}
          />
        </label>
        <details className="plan-options">
          <summary>负责人和交付目录</summary>
          <div className="form-options-grid">
            <label>
              负责人
              <input
                name="owner"
                value={draft.owner}
                onChange={(event) => change("owner", event.target.value)}
                maxLength={60}
                disabled={busy}
              />
            </label>
            <label>
              交付目录
              <input
                name="output"
                value={draft.outputDir}
                onChange={(event) => change("outputDir", event.target.value)}
                disabled={busy}
                aria-describedby="output-hint"
              />
            </label>
          </div>
          <small id="output-hint">
            默认使用项目目录（.）。也可填写项目内已有的相对目录。
          </small>
        </details>
        {problem && (
          <p className="form-error" role="alert">
            {problem}
          </p>
        )}
        <div className="form-submit">
          <button
            className="primary-button"
            disabled={busy || projectBusy || projects.length === 0}
          >
            {busy ? "正在处理…" : "保存任务计划"}
          </button>
          <span>暂存内容在切换页面后保留，关闭应用后不保留。</span>
        </div>
      </form>
      <aside className="plan-guide" aria-label="任务流程说明">
        <p className="eyebrow">从计划到交付</p>
        <ol>
          <li>
            <strong>明确要做什么</strong>
            <p>选择项目，写下目标和允许处理的范围。</p>
          </li>
          <li>
            <strong>围绕同一任务推进</strong>
            <p>记录执行与验收过程，始终保留当前任务。</p>
          </li>
          <li>
            <strong>核对交付内容</strong>
            <p>查看文件和版本，确认结果是否符合预期。</p>
          </li>
        </ol>
        <div className="availability-note">
          <strong>当前可用</strong>
          <p>计划保存、文件版本和未执行任务的恢复。自动执行入口尚未开放。</p>
        </div>
      </aside>
    </div>
  );
}

export default function TaskWorkbench({
  projects,
  projectBusy,
  projectError,
  draft,
  onDraftChange,
  onAddProject,
  onRefreshProjects,
  currentSession,
}: {
  projects: Project[];
  projectBusy: boolean;
  projectError: string | null;
  draft: PlanInput;
  onDraftChange: (input: PlanInput) => void;
  onAddProject: () => void;
  onRefreshProjects: () => void;
  currentSession: SessionInfo | null;
}) {
  const [view, setView] = useState<WorkbenchView | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [updated, setUpdated] = useState<string | null>(null);
  const order = useRef(requestOrder());
  const mutation = useRef(false);
  const reading = useRef(false);
  const mounted = useRef(true);
  const statusPoll = useRef<ReturnType<typeof setInterval> | null>(null);
  const statusPollInFlight = useRef(false);
  const load = useCallback(async (): Promise<boolean> => {
    const request = order.current.next();
    reading.current = true;
    setLoading(true);
    setPreview(null);
    try {
      const next = await invoke<WorkbenchView>("workbench_snapshot");
      if (!mounted.current || !order.current.current(request)) return false;
      setView((previous) => acceptSnapshot(previous, next));
      setReadError(null);
      setUpdated(
        new Date().toLocaleTimeString([], {
          hour: "2-digit",
          minute: "2-digit",
          second: "2-digit",
        }),
      );
      return true;
    } catch (cause) {
      if (mounted.current && order.current.current(request))
        setReadError(message(cause));
      return false;
    } finally {
      if (mounted.current && order.current.current(request)) {
        reading.current = false;
        setLoading(false);
      }
    }
  }, []);
  const stopStatusPolling = useCallback(() => {
    if (statusPoll.current !== null) {
      clearInterval(statusPoll.current);
      statusPoll.current = null;
    }
  }, []);
  const pollStatus = useCallback(async () => {
    if (!mounted.current || statusPollInFlight.current) return;
    statusPollInFlight.current = true;
    try {
      const next = await invoke<WorkbenchView>("workbench_snapshot");
      if (mounted.current) {
        setView((previous) => acceptSnapshot(previous, next));
      }
    } catch (cause) {
      if (mounted.current) setReadError(message(cause));
    } finally {
      statusPollInFlight.current = false;
    }
  }, []);
  const pollingTask = view?.dispatchActive
    ? view.ledger.active?.taskId
    : undefined;
  useEffect(() => {
    if (!pollingTask) return;
    statusPoll.current = setInterval(() => void pollStatus(), 350);
    return stopStatusPolling;
  }, [pollingTask, pollStatus, stopStatusPolling]);
  useEffect(() => {
    mounted.current = true;
    void load();
    const requests = order.current;
    const refreshOnFocus = () => {
      if (!mutation.current && !reading.current) void load();
    };
    window.addEventListener("focus", refreshOnFocus);
    return () => {
      mounted.current = false;
      stopStatusPolling();
      requests.next();
      window.removeEventListener("focus", refreshOnFocus);
    };
  }, [load, stopStatusPolling]);
  async function act(
    command: string,
    args?: Record<string, unknown>,
    success = "操作已保存。",
  ) {
    if (mutation.current || reading.current) return;
    mutation.current = true;
    setBusy(true);
    setActionError(null);
    setNotice(null);
    setPreview(null);
    order.current.next();
    try {
      await invoke(command, args);
      if (!mounted.current) return;
      if (await load()) {
        setNotice(success);
        return true;
      }
    } catch (cause) {
      if (!mounted.current) return;
      await load();
      if (mounted.current) setActionError(message(cause));
    } finally {
      mutation.current = false;
      if (mounted.current) setBusy(false);
    }
    return false;
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
    if (mutation.current || reading.current) return;
    mutation.current = true;
    const request = order.current.next();
    setPreview(null);
    setActionError(null);
    setNotice(null);
    setBusy(true);
    try {
      const value = await invoke<Preview>("preview_task_artifact", {
        ...ticket(),
        artifactId: id,
      });
      if (mounted.current && order.current.current(request)) setPreview(value);
    } catch (cause) {
      if (mounted.current && order.current.current(request)) {
        await load();
        if (mounted.current) setActionError(message(cause));
      }
    } finally {
      mutation.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  const active = view?.ledger.active;
  const canPlan =
    !!view &&
    (!active ||
      (!view.document &&
        isReservation(active.phase) &&
        !active.recoveryRequired));
  const blocked = actionsBlocked(busy, loading, readError);
  const needsSetup = readError?.includes("尚未初始化");
  return (
    <section
      className="workbench"
      aria-labelledby="workbench-title"
      aria-busy={loading || busy}
    >
      <div className="workbench-header">
        <div>
          <h1 id="workbench-title">任务工作台</h1>
          <p className="tool-detail">整理计划，跟进执行，核对每一次交付。</p>
        </div>
        <div className="workbench-toolbar">
          <span className="occupancy">
            <span className={`status-dot ${active ? "occupied" : ""}`} />
            {readError || !view
              ? "状态待确认"
              : active
                ? "1 项当前任务"
                : "可以安排新任务"}
          </span>
          <button
            className="ghost-button"
            disabled={busy || loading}
            onClick={() => {
              setActionError(null);
              setNotice(null);
              void load();
              onRefreshProjects();
            }}
          >
            {loading ? "更新中…" : "刷新状态"}
          </button>
        </div>
      </div>
      <p className="sync-status" role="status">
        {loading
          ? "正在读取任务…"
          : updated
            ? `最近核对 ${updated} · 返回窗口时自动核对`
            : "等待读取任务"}
      </p>
      {needsSetup ? (
        <section className="setup-card">
          <span className="setup-mark" aria-hidden="true">
            01
          </span>
          <h2>开始管理你的第一个任务</h2>
          <p>
            启用本地任务记录后，可以保存计划、关联文件，并在下次打开时继续查看。
          </p>
          <button
            className="primary-button"
            disabled={busy || loading}
            onClick={() =>
              void act(
                "task_ledger_initialize",
                undefined,
                "本地任务记录已启用，可以安排任务。",
              )
            }
          >
            启用任务记录
          </button>
        </section>
      ) : (
        readError && (
          <section className="feedback feedback-error" role="alert">
            <strong>暂时无法确认任务状态</strong>
            <p>已暂停任务操作，请重新读取后继续。</p>
            <details>
              <summary>错误详情</summary>
              <p>{readError}</p>
            </details>
            <button
              className="ghost-button"
              disabled={busy || loading}
              onClick={() => void load()}
            >
              重新读取状态
            </button>
          </section>
        )
      )}
      {actionError && (
        <section className="feedback feedback-error" role="alert">
          <strong>这次操作未能确认成功</strong>
          <p>{actionError}</p>
          <p>
            {readError
              ? "请先重新读取任务状态。"
              : "当前任务状态已重新核对，可以修改输入或再次操作。"}
          </p>
          <button
            className="ghost-button"
            disabled={busy}
            onClick={() => setActionError(null)}
          >
            收起提示
          </button>
        </section>
      )}
      {notice && !readError && (
        <p className="feedback feedback-success" role="status">
          {notice}
        </p>
      )}
      {projectError && (
        <section className="feedback feedback-error" role="alert">
          <strong>项目列表需要重新核对</strong>
          <p>{projectError}</p>
          <button
            className="ghost-button"
            disabled={projectBusy}
            onClick={onRefreshProjects}
          >
            重试读取项目
          </button>
        </section>
      )}
      {view && (
        <TaskCard
          key={active?.taskId ?? "idle"}
          view={view}
          busy={blocked}
          preview={preview}
          projectName={
            projects.find((project) => project.id === active?.projectId)?.name
          }
          currentSession={currentSession}
          onReference={(relativePath, label) =>
            void act(
              "reference_task_artifact",
              { ...ticket(), relativePath, label },
              "文件版本已保存，可以查看内容。",
            )
          }
          onPreview={(id) => void showPreview(id)}
          onCancel={() =>
            void act(
              "cancel_main_task_reservation",
              ticket(),
              "任务已取消，计划和文件记录已保留。",
            )
          }
          onRecover={(release) =>
            void act(
              "recover_main_task_reservation",
              { ...ticket(), release },
              release
                ? "已结束本次安排，可以登记下一项任务。"
                : "任务已接回，可以继续整理计划和文件。",
            )
          }
          onResumeAfterRecovery={() =>
            void act(
              "resume_main_task_after_recovery",
              ticket(),
              "旧执行尝试已确认退出，任务已接回，可以继续派发。",
            )
          }
          onReleaseCompleted={() =>
            void act(
              "release_completed_task",
              ticket(),
              "任务已完成并释放，可以安排下一项任务。",
            )
          }
          onRefresh={() => {
            setActionError(null);
            setNotice(null);
            void load();
          }}
          onAssociateSession={() =>
            void act(
              "associate_task_session",
              { ...ticket(), sessionId: currentSession?.id },
              "当前 CLI 会话已关联到任务记录。",
            )
          }
          onReassociateSession={() =>
            void act(
              "reassociate_task_session_after_recovery",
              {
                ...ticket(),
                previousSessionId: view.document?.sessionLink?.id,
                sessionId: currentSession?.id,
              },
              "已关联当前会话，原关联记录保留；可明确启动新的执行尝试。",
            )
          }
          onDispatch={() =>
            void act(
              "dispatch_controlled_task",
              { ...ticket(), sessionId: currentSession?.id },
              "受控执行已提交，正在跟踪执行与停止状态。",
            )
          }
          onStop={() =>
            void act(
              "request_stop_controlled_task",
              ticket(),
              "停止请求已记录，系统正在核实受控进程是否已结束。",
            )
          }
        />
      )}
      {active?.recoveryRequired &&
        isReservation(active.phase) &&
        !projects.some((project) => project.id === active.projectId) && (
          <div className="project-reconnect">
            <p>接回任务前，请重新添加对应的已有项目。</p>
            <button
              className="ghost-button"
              disabled={busy || projectBusy}
              onClick={onAddProject}
            >
              添加对应项目
            </button>
          </div>
        )}
      {canPlan && (
        <PlanForm
          projects={projects}
          busy={blocked}
          projectBusy={projectBusy}
          activeProject={active?.projectId}
          draft={draft}
          onDraftChange={onDraftChange}
          onAddProject={onAddProject}
          onSubmit={(input) => {
            if (mutation.current || reading.current) return;
            onDraftChange({ ...input });
            void act(
              active ? "attach_task_plan" : "create_workbench_task",
              { ...ticket(), input },
              "任务计划已保存。自动执行尚未开放，可先添加交付文件。",
            ).then((saved) => {
              if (saved && mounted.current) onDraftChange(emptyPlan());
            });
          }}
        />
      )}
    </section>
  );
}
