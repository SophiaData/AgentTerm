import type { ScheduledTaskDraft } from "./types";

export type ScheduleContext = {
  projectPath: string;
  toolId: string;
  model?: string | null;
};

export function parseScheduleRequest(
  request: string,
  context: ScheduleContext,
): ScheduledTaskDraft | null {
  const text = request.trim();
  if (!context.projectPath || !context.toolId || text.length < 2) return null;
  const every = text.match(/(?:每|每隔)\s*(\d{1,5})\s*(分钟|小时|天)/);
  const daily = text.match(/每天\s*(\d{1,2})(?::|：)(\d{2})/);
  let intervalMinutes = 60;
  if (every) {
    const amount = Number(every[1]);
    intervalMinutes =
      every[2] === "小时"
        ? amount * 60
        : every[2] === "天"
          ? amount * 1440
          : amount;
  } else if (daily) {
    intervalMinutes = 1440;
  } else if (!/(每天|定时|提醒|检查|总结|报告)/.test(text)) {
    return null;
  }
  if (intervalMinutes < 1 || intervalMinutes > 10080) return null;
  const name = Array.from(text).slice(0, 24).join("");
  return {
    name,
    projectPath: context.projectPath,
    toolId: context.toolId,
    model: context.model?.trim() ?? "",
    prompt: text,
    intervalMinutes,
    timeoutMinutes: 20,
    enabled: true,
  };
}
