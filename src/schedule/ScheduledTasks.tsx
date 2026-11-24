import { useCallback, useEffect, useState } from "react";
import { scheduleLabel } from "./parseSchedule";
import type { ScheduledTask } from "./types";

type Props = {
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

export default function ScheduledTasks({ onClose, onNotice }: Props) {
  const [tasks, setTasks] = useState<ScheduledTask[]>([]);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    setTasks(await invoke<ScheduledTask[]>("list_scheduled_tasks"));
    setError(null);
    setLoading(false);
  }, []);

  useEffect(() => {
    void load().catch((cause) => {
      setError(errorText(cause));
      setLoading(false);
    });
  }, [load]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const refresh = () => {
      void load().catch((cause) => setError(errorText(cause)));
    };
    const timer = window.setInterval(refresh, 5_000);
    void import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen("scheduled-tasks-changed", () => {
          refresh();
        }),
      )
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // The next open or manual action still reads authoritative backend data.
      });
    return () => {
      disposed = true;
      window.clearInterval(timer);
      unlisten?.();
    };
  }, [load]);

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
        {loading ? (
          <div className="schedule-empty" aria-live="polite">
            <span>正在读取 Agent 创建的定时任务…</span>
          </div>
        ) : !tasks.length ? (
          <div className="schedule-empty">
            <strong>还没有定时任务</strong>
            <span>回到会话里直接告诉 Agent 要按时做什么。</span>
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
                {task.sourceSessionId ? (
                  <p className="schedule-source">Agent 会话创建</p>
                ) : null}
                <details className="schedule-result">
                  <summary>查看任务指令</summary>
                  <pre>{task.prompt}</pre>
                </details>
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

      <section
        className="schedule-agent-note"
        aria-label="由 Agent 创建定时任务"
      >
        <span>创建入口在会话里</span>
        <strong>“每天 09:00 汇总项目进展”</strong>
        <p>
          Agent 会结合当前项目、CLI 和对话内容创建任务；这里只负责查看和管理。
        </p>
        <button type="button" onClick={onClose}>
          回到会话创建
        </button>
      </section>
    </aside>
  );
}
