import type { Project } from "../projects/types";
import type { ToolStatus } from "../tools/types";
import type { SessionHistoryRecord } from "./types";

export function folderName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || path;
}

export function historyDisplayTitle(record: SessionHistoryRecord): string {
  const custom = record.title?.trim();
  if (custom) return custom;
  return `${record.toolName} · ${folderName(record.projectPath)}`;
}

export function historySubtitleText(record: SessionHistoryRecord): string {
  const base = `${historyStatusText(record)} · ${formatSessionTime(record.startedAtMs)}`;
  if (record.title?.trim()) {
    return `${record.toolName} · ${folderName(record.projectPath)} · ${base}`;
  }
  return base;
}

export function projectFromRecord(record: SessionHistoryRecord): Project {
  return {
    id: record.projectPath,
    name: folderName(record.projectPath),
    path: record.projectPath,
  };
}

export function historyStatusText(record: SessionHistoryRecord): string {
  if (record.endedAtMs !== null) {
    return record.exitReason ? `已结束 · ${record.exitReason}` : "已结束";
  }
  return "上次未正常退出";
}

export function formatSessionTime(timestampMs: number): string {
  const date = new Date(timestampMs);
  if (Number.isNaN(date.getTime())) return "时间未知";
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function recentClosedHistory(
  records: SessionHistoryRecord[],
  activeSessionIds: ReadonlySet<string>,
  limit = 20,
): SessionHistoryRecord[] {
  return records
    .filter((record) => !activeSessionIds.has(record.id))
    .slice(0, limit);
}

export function installedToolForRecord(
  record: SessionHistoryRecord,
  tools: ToolStatus[],
): ToolStatus | null {
  const tool = tools.find((item) => item.id === record.toolId);
  return tool?.state.kind === "installed" ? tool : null;
}
