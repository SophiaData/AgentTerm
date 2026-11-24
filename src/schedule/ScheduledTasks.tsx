import { useCallback, useEffect, useState } from "react";
import { parseScheduleRequest, scheduleLabel } from "./parseSchedule";
import type { ScheduledTask, ScheduledTaskDraft } from "./types";

type Props = {
  context: { projectPath: string; toolId: string; model?: string | null };
  onClose: () => void;
  onNotice: (message: string) => void;
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function timeText(value: number): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "时间未知"
    : date.toLocaleString("zh-CN", {
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        hour12: false,
      });
}

function projectName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) || path;
}

function toolName(id: string): string {
  return id === "claude" ? "Claude Code" : id === "codex" ? "Codex" : "Agy";
}

function taskSchedule(task: ScheduledTask) {
  return (
    task.schedule ??
    (task.intervalMinutes
      ? { kind: "interval" as const, minutes: task.intervalMinutes }
      : null)
  );
}

export default function ScheduledTasks({ context, onClose, onNotice }: Props) {
  const [tasks, setTasks] = useState<ScheduledTask[]>([]);
  const [request, setRequest] = useState("");
  const [pending, setPending] = useState<ScheduledTaskDraft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    setTasks(await invoke<ScheduledTask[]>("list_scheduled_tasks"));
  }, []);

  useEffect(() => {
    void load().catch((cause) => setError(errorText(cause)));
  }, [load]);

  const preview = () => {
    const result = parseScheduleRequest(request, context);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setError(null);
    setPending(result.draft);
  };

  const confirm = async () => {
    if (!pending) return;
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("save_scheduled_task", {
        draft: { ...pending, model: pending.model.trim() || null },
      });
      setPending(null);
      setRequest("");
      await load();
      onNotice("定时任务已添加");
    } catch (cause) {
      setError(`无法保存定时任务：${errorText(cause)}`);
    } finally {
      setBusy(false);
    }
  };

  const act = async (command: string, args: Record<string, unknown>) => {
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke(command, args);
      await load();
      if (command === "run_scheduled_task_now")
        onNotice("定时任务已运行，结果已更新");
    } catch (cause) {
      setError(`定时任务操作失败：${errorText(cause)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside
      className="workspace-records schedule-workspace"
      id="scheduled-tasks"
      aria-label="定时任务"
    >
      <div className="workspace-records-heading">
        <strong>定时任务</strong>
        <button
          type="button"
          className="sidebar-icon-button"
          aria-label="关闭定时任务"
          onClick={onClose}
        >
          ×
        </button>
      </div>

      <div className="schedule-scroll">
        {error ? (
          <p className="schedule-error" role="alert">
            {error}
          </p>
        ) : null}
        {!tasks.length ? (
          <div className="schedule-empty">
            <strong>还没有定时任务</strong>
            <span>在下面用一句话告诉 Agent 要按时做什么。</span>
          </div>
        ) : (
          <ul className="schedule-list">
            {tasks.map((task) => (
              <li
                key={task.id}
                className={`schedule-card${task.enabled ? " is-enabled" : " is-disabled"}`}
              >
                <div className="schedule-card-head">
                  <strong>{task.name}</strong>
                  <span>{task.enabled ? "启用" : "停用"}</span>
                </div>
                <p className="schedule-meta">
                  {scheduleLabel(taskSchedule(task))}
                  <i>·</i>
                  {toolName(task.toolId)}
                  {task.model ? ` · ${task.model}` : ""}
                </p>
                <p className="schedule-next">
                  {task.enabled
                    ? `下次 ${timeText(task.nextRunAtMs)}`
                    : "不会自动运行"}
                </p>
                <div className="schedule-actions">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      void act("set_scheduled_task_enabled", {
                        id: task.id,
                        enabled: !task.enabled,
                      })
                    }
                  >
                    {task.enabled ? "暂停" : "启用"}
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      void act("run_scheduled_task_now", { id: task.id })
                    }
                  >
                    立即运行
                  </button>
                  <button
                    type="button"
                    className="is-danger"
                    disabled={busy}
                    onClick={() =>
                      void act("remove_scheduled_task", { id: task.id })
                    }
                  >
                    删除
                  </button>
                </div>
                {task.runs[0] ? (
                  <details className="schedule-result">
                    <summary>
                      最近 {task.runs[0].status} ·{" "}
                      {timeText(task.runs[0].finishedAtMs)}
                    </summary>
                    <pre>{task.runs[0].output || "无输出"}</pre>
                  </details>
                ) : (
                  <p className="schedule-no-run">尚无运行记录</p>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="schedule-composer">
        {pending ? (
          <div className="schedule-preview">
            <span>
              {scheduleLabel(pending.schedule)} ·{" "}
              {projectName(pending.projectPath)} · {toolName(pending.toolId)}
            </span>
            <strong>{pending.name}</strong>
            <div>
              <button
                type="button"
                className="primary-button"
                disabled={busy}
                onClick={() => void confirm()}
              >
                确认安排
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => setPending(null)}
              >
                取消
              </button>
            </div>
          </div>
        ) : (
          <>
            <textarea
              aria-label="用一句话添加定时任务"
              rows={2}
              placeholder="例如：每天九点汇报 GitHub 热搜"
              value={request}
              onChange={(event) => setRequest(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  preview();
                }
              }}
            />
            <button
              type="button"
              className="primary-button"
              disabled={busy || !request.trim()}
              onClick={preview}
            >
              安排
            </button>
          </>
        )}
      </div>
    </aside>
  );
}
