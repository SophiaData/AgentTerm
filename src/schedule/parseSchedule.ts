import type { ScheduleSpec, ScheduledTaskDraft } from "./types";

export type ScheduleContext = {
  projectPath: string;
  toolId: string;
  model?: string | null;
};

export type ParseScheduleResult =
  { ok: true; draft: ScheduledTaskDraft } | { ok: false; error: string };

const CHINESE_DIGITS: Record<string, number> = {
  零: 0,
  〇: 0,
  一: 1,
  二: 2,
  两: 2,
  三: 3,
  四: 4,
  五: 5,
  六: 6,
  七: 7,
  八: 8,
  九: 9,
};

function parseNumber(value: string): number | null {
  if (/^\d+$/.test(value)) return Number(value);
  if (value === "半") return 0.5;
  if (value === "十") return 10;
  if (value.length === 2 && value[0] === "十") {
    const tail = CHINESE_DIGITS[value[1]];
    return tail === undefined ? null : 10 + tail;
  }
  if (value.length === 2 && value[1] === "十") {
    const head = CHINESE_DIGITS[value[0]];
    return head === undefined ? null : head * 10;
  }
  if (value.length === 3 && value[1] === "十") {
    const head = CHINESE_DIGITS[value[0]];
    const tail = CHINESE_DIGITS[value[2]];
    return head === undefined || tail === undefined ? null : head * 10 + tail;
  }
  return CHINESE_DIGITS[value] ?? null;
}

function normalizeDaily(
  hour: number,
  minute: number,
  period?: string,
): ScheduleSpec | null {
  let normalizedHour = hour;
  if (
    (period === "下午" || period === "傍晚" || period === "晚上") &&
    hour < 12
  ) {
    normalizedHour += 12;
  }
  if (period === "中午" && hour < 11) normalizedHour += 12;
  if (
    !Number.isInteger(normalizedHour) ||
    !Number.isInteger(minute) ||
    normalizedHour < 0 ||
    normalizedHour > 23 ||
    minute < 0 ||
    minute > 59
  ) {
    return null;
  }
  return { kind: "daily", hour: normalizedHour, minute };
}

function dailySchedule(text: string): ScheduleSpec | null {
  const colon = text.match(
    /(?:每天|每日)\s*(早上|上午|中午|下午|傍晚|晚上)?\s*(\d{1,2})\s*[:：]\s*(\d{1,2})/,
  );
  if (colon) {
    return normalizeDaily(Number(colon[2]), Number(colon[3]), colon[1]);
  }
  const point = text.match(
    /(?:每天|每日)\s*(早上|上午|中午|下午|傍晚|晚上)?\s*([零〇一二两三四五六七八九十0-9]{1,3})\s*[点时]\s*钟?\s*(半|[零〇一二两三四五六七八九十0-9]{1,3})?\s*分?/,
  );
  if (!point) return null;
  const hour = parseNumber(point[2]);
  const minute = point[3] === "半" ? 30 : point[3] ? parseNumber(point[3]) : 0;
  if (hour === null || minute === null) return null;
  return normalizeDaily(hour, minute, point[1]);
}

function intervalSchedule(text: string): ScheduleSpec | null {
  if (/每半小时/.test(text)) return { kind: "interval", minutes: 30 };
  if (/每小时/.test(text)) return { kind: "interval", minutes: 60 };
  const every = text.match(
    /(?:每隔|每)\s*([零〇一二两三四五六七八九十0-9]{1,3})\s*(分钟|小时|天)/,
  );
  if (!every) return null;
  const amount = parseNumber(every[1]);
  if (amount === null || !Number.isFinite(amount)) return null;
  const minutes =
    every[2] === "小时"
      ? amount * 60
      : every[2] === "天"
        ? amount * 1440
        : amount;
  if (!Number.isInteger(minutes) || minutes < 1 || minutes > 10080) return null;
  return { kind: "interval", minutes };
}

function taskName(text: string): string {
  const withoutSchedule = text
    .replace(
      /(?:每天|每日)\s*(?:早上|上午|中午|下午|傍晚|晚上)?\s*(?:[零〇一二两三四五六七八九十0-9]{1,3}\s*[点时]\s*钟?(?:\s*(?:半|[零〇一二两三四五六七八九十0-9]{1,3})?\s*分?)?|[0-9]{1,2}\s*[:：]\s*[0-9]{1,2})/,
      "",
    )
    .replace(
      /(?:每隔|每)\s*(?:[零〇一二两三四五六七八九十0-9]{1,3})\s*(?:分钟|小时|天)/,
      "",
    )
    .replace(/^(?:请|帮我|麻烦你?)/, "")
    .replace(/^定时/, "")
    .replace(/^给我/, "")
    .trim()
    .replace(/^[,，。；;：:\s]+|[,，。；;：:\s]+$/g, "");
  const candidate = withoutSchedule || text;
  return Array.from(candidate).slice(0, 24).join("");
}

export function parseScheduleRequest(
  request: string,
  context: ScheduleContext,
): ParseScheduleResult {
  const text = request.trim();
  if (!context.projectPath || !context.toolId) {
    return { ok: false, error: "请先打开一个项目和 Agent 会话" };
  }
  if (text.length < 2) {
    return { ok: false, error: "请说出要定时做的事情" };
  }
  let schedule = intervalSchedule(text);
  if (!schedule && /每天|每日/.test(text)) {
    schedule = dailySchedule(text);
    if (!schedule) {
      return {
        ok: false,
        error: "请补充每天的几点几分，例如：每天 09:30 总结项目进展",
      };
    }
  }
  if (!schedule) {
    return {
      ok: false,
      error: "请说明频率，例如：每天 09:30 总结项目，或每隔 30 分钟检查一次",
    };
  }
  return {
    ok: true,
    draft: {
      name: taskName(text),
      projectPath: context.projectPath,
      toolId: context.toolId,
      model: context.model?.trim() ?? "",
      prompt: text,
      schedule,
      timeoutMinutes: 20,
      enabled: true,
    },
  };
}

export function scheduleLabel(schedule?: ScheduleSpec | null): string {
  if (!schedule) return "旧版间隔";
  if (schedule.kind === "daily") {
    return `每天 ${String(schedule.hour).padStart(2, "0")}:${String(schedule.minute).padStart(2, "0")}`;
  }
  if (schedule.minutes % 1440 === 0) return `每 ${schedule.minutes / 1440} 天`;
  if (schedule.minutes % 60 === 0) return `每 ${schedule.minutes / 60} 小时`;
  return `每 ${schedule.minutes} 分钟`;
}
