import type { ScheduleSpec } from "./types";

export function scheduleLabel(schedule?: ScheduleSpec | null): string {
  if (!schedule) return "旧版间隔";
  if (schedule.kind === "daily") {
    return `每天 ${String(schedule.hour).padStart(2, "0")}:${String(schedule.minute).padStart(2, "0")}`;
  }
  if (schedule.minutes % 1440 === 0) return `每 ${schedule.minutes / 1440} 天`;
  if (schedule.minutes % 60 === 0) return `每 ${schedule.minutes / 60} 小时`;
  return `每 ${schedule.minutes} 分钟`;
}
