import { useCallback, useEffect, useState } from "react";
import type { Project } from "../projects/types";
import type { ToolStatus } from "../tools/types";
import {
  EMPTY_SCHEDULE_DRAFT,
  type ScheduledTask,
  type ScheduledTaskDraft,
} from "./types";

type Props = {
  projects: Project[];
  tools: ToolStatus[];
  onClose: () => void;
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

export default function ScheduledTasks({ projects, tools, onClose }: Props) {
  const [tasks, setTasks] = useState<ScheduledTask[]>([]);
  const [draft, setDraft] = useState<ScheduledTaskDraft>(EMPTY_SCHEDULE_DRAFT);
  const [editingId, setEditingId] = useState<string | null>(null);
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
    setBusy(true);
    setError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const payload = {
        ...draft,
        model: draft.model.trim() || null,
        projectPath: draft.projectPath || projects[0]?.path || "",
      };
      await invoke(
        editingId ? "update_scheduled_task" : "save_scheduled_task",
        {
          id: editingId,
          draft: payload,
        },
      );
      setDraft({
        ...EMPTY_SCHEDULE_DRAFT,
        projectPath: projects[0]?.path ?? "",
      });
      setEditingId(null);
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
          到点后按所选 CLI 和模型执行一条指令。权限由该 CLI
          自己的配置决定，AgentTerm 不额外限制。同一时间只运行一个任务。
        </p>
        {error ? <p role="alert">{error}</p> : null}
        <form
          className="schedule-form"
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <input
            aria-label="任务名称"
            placeholder="任务名称"
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
          />
          <select
            aria-label="项目"
            value={draft.projectPath || projects[0]?.path || ""}
            onChange={(e) =>
              setDraft({ ...draft, projectPath: e.target.value })
            }
          >
            {projects.map((project) => (
              <option key={project.id} value={project.path}>
                {project.name}
              </option>
            ))}
          </select>
          <select
            aria-label="CLI"
            value={draft.toolId}
            onChange={(e) => setDraft({ ...draft, toolId: e.target.value })}
          >
            {tools.map((tool) => (
              <option key={tool.id} value={tool.id}>
                {tool.name}
              </option>
            ))}
          </select>
          <input
            aria-label="模型"
            placeholder="模型（可留空）"
            value={draft.model}
            onChange={(e) => setDraft({ ...draft, model: e.target.value })}
          />
          <textarea
            aria-label="任务指令"
            placeholder="到点执行的指令"
            value={draft.prompt}
            onChange={(e) => setDraft({ ...draft, prompt: e.target.value })}
          />
          <label>
            间隔（分钟）
            <input
              aria-label="间隔分钟"
              type="number"
              min={1}
              max={10080}
              value={draft.intervalMinutes}
              onChange={(e) =>
                setDraft({ ...draft, intervalMinutes: Number(e.target.value) })
              }
            />
          </label>
          <label>
            超时（分钟）
            <input
              aria-label="超时分钟"
              type="number"
              min={1}
              max={180}
              value={draft.timeoutMinutes}
              onChange={(e) =>
                setDraft({ ...draft, timeoutMinutes: Number(e.target.value) })
              }
            />
          </label>
          <label>
            <input
              type="checkbox"
              checked={draft.enabled}
              onChange={(e) =>
                setDraft({ ...draft, enabled: e.target.checked })
              }
            />
            启用
          </label>
          <button className="primary-button" disabled={busy}>
            {editingId ? "保存修改" : "添加定时任务"}
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
                  onClick={() => {
                    setEditingId(task.id);
                    setDraft({ ...task, model: task.model ?? "" });
                  }}
                >
                  编辑
                </button>
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
