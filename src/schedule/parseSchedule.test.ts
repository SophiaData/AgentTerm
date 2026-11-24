import { describe, expect, it } from "vitest";
import { scheduleLabel } from "./parseSchedule";

describe("schedule labels", () => {
  it("formats daily and interval schedules", () => {
    expect(scheduleLabel({ kind: "daily", hour: 9, minute: 0 })).toBe(
      "每天 09:00",
    );
    expect(scheduleLabel({ kind: "interval", minutes: 60 })).toBe("每 1 小时");
    expect(scheduleLabel({ kind: "interval", minutes: 30 })).toBe("每 30 分钟");
    expect(scheduleLabel(null)).toBe("旧版间隔");
  });
});
