import { useCallback, useEffect, useState } from "react";
import type { Project } from "../projects/types";
import { parseScheduleRequest } from "./parseSchedule";
import type { ScheduledTask } from "./types";

type Props = {
  projects: Project[];
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
    : date.toLocaleString("zh-CN", { hour12: false });
}

export default function ScheduledTasks({ context, onClose, onNotice }: Props) {
  const [tasks, setTasks] = useState<ScheduledTask[]>([]);
  const [request, setRequest] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedRun, setSelectedRun] = useState<string | null>(null);

  const load = useCallback(async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    setTasks(await invoke<ScheduledTask[]>("list_scheduled_tasks"));
  }, []);

  useEffect(() => {
    void load().catch((cause) => setError(errorText(cause)));
  }, [load]);

  const save = async () => {
    const draft = parseScheduleRequest(request, context);
    if (!draft) {
      setError("请直接说任务，例如：每天 09:30 总结项目进展");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("save_scheduled_task", {
        draft: { ...draft, model: draft.model.trim() || null },
      });
      setRequest("");
      await load();
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
      className="workspace-records"
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
      <div className="schedule-panel">
        <p className="workspace-records-note">
          直接说想让当前 Agent 定时做什么。这里只查看、暂停和查看结果。
        </p>
        {error ? <p role="alert">{error}</p> : null}
        <form
          className="schedule-form"
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <textarea
            aria-label="用一句话添加定时任务"
            placeholder="例如：每天 09:30 总结项目进展"
            value={request}
            onChange={(event) => setRequest(event.target.value)}
          />
          <button className="primary-button" disabled={busy}>
            交给当前 Agent
          </button>
        </form>
        <ul className="schedule-list">
          {tasks.map((task) => (
            <li key={task.id} className="schedule-card">
              <strong>{task.name}</strong>
              <span>
                {task.toolId}
                {task.model ? ` · ${task.model}` : " · 默认模型"}
              </span>
              <span>
                {task.enabled ? `下次 ${timeText(task.nextRunAtMs)}` : "已停用"}
              </span>
              <div>
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
                  {task.enabled ? "停用" : "启用"}
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
                  disabled={busy}
                  onClick={() =>
                    void act("remove_scheduled_task", { id: task.id })
                  }
                >
                  删除
                </button>
              </div>
              {task.runs[0] ? (
                <button
                  type="button"
                  onClick={() =>
                    setSelectedRun(selectedRun === task.id ? null : task.id)
                  }
                >
                  最近：{task.runs[0].status} ·{" "}
                  {timeText(task.runs[0].finishedAtMs)}
                </button>
              ) : (
                <span>尚无运行记录</span>
              )}
              {selectedRun === task.id && task.runs[0] ? (
                <pre>{task.runs[0].output || "无输出"}</pre>
              ) : null}
            </li>
          ))}
        </ul>
      </div>
    </aside>
  );
}
