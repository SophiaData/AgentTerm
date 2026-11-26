import type { Project } from "../projects/types";
import { folderName } from "../terminal/sessionHistory";
import type { ActiveSession } from "../terminal/useLauncher";
import type { RecoverableSession, UpdateActivity } from "./types";

export function mergeUpdateActivity(
  backend: UpdateActivity,
  frontendSessions: ActiveSession[],
): UpdateActivity {
  const seen = new Set(backend.liveSessions.map((session) => session.id));
  const merged = [...backend.liveSessions];
  for (const session of frontendSessions) {
    if (!session.running || seen.has(session.info.id)) continue;
    seen.add(session.info.id);
    merged.push(session.info);
  }
  return {
    ...backend,
    liveSessions: merged,
    hasActiveWork:
      merged.length > 0 ||
      backend.dispatchActive ||
      backend.scheduledTaskId !== null,
  };
}

export function describeUpdateActivity(activity: UpdateActivity): string[] {
  const lines: string[] = [];
  if (activity.liveSessions.length > 0) {
    const labels = activity.liveSessions.map((session) => {
      const title = session.title?.trim();
      return (
        title || `${session.toolName} · ${folderName(session.projectPath)}`
      );
    });
    lines.push(
      `运行中的终端会话（${activity.liveSessions.length} 个）：${labels.join("、")}`,
    );
  }
  if (activity.dispatchActive) {
    lines.push("任务工作台中有正在执行的受控任务");
  }
  if (activity.scheduledTaskId) {
    lines.push(`定时任务（${activity.scheduledTaskId}）正在执行`);
  }
  return lines;
}

export function projectForRecovery(
  session: RecoverableSession,
  projects: Project[],
): Project {
  return (
    projects.find((project) => project.path === session.projectPath) ?? {
      id: session.projectPath,
      name: folderName(session.projectPath),
      path: session.projectPath,
    }
  );
}
