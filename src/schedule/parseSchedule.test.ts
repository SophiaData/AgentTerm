import { describe, expect, it } from "vitest";
import { parseScheduleRequest, scheduleLabel } from "./parseSchedule";

const context = {
  projectPath: "E:\\AgentTerm",
  toolId: "codex",
  model: "gpt-5",
};

describe("conversational schedules", () => {
  it("creates a daily task from one sentence", () => {
    expect(
      parseScheduleRequest("每天 09:30 总结今天的项目进展", context),
    ).toMatchObject({
      ok: true,
      draft: {
        projectPath: "E:\\AgentTerm",
        toolId: "codex",
        schedule: { kind: "daily", hour: 9, minute: 30 },
        enabled: true,
      },
    });
  });

  it("ignores ordinary conversation", () => {
    expect(parseScheduleRequest("帮我看一下这个报错", context)).toMatchObject({
      ok: false,
    });
  });

  it("understands Chinese clock times and periods", () => {
    expect(
      parseScheduleRequest("每天九点钟汇报 GitHub 热搜", context),
    ).toMatchObject({
      ok: true,
      draft: { schedule: { kind: "daily", hour: 9, minute: 0 } },
    });
    expect(
      parseScheduleRequest("每天晚上八点半汇总日报", context),
    ).toMatchObject({
      ok: true,
      draft: { schedule: { kind: "daily", hour: 20, minute: 30 } },
    });
    expect(
      parseScheduleRequest("每天九点钟定时给我汇报一下 GitHub 热搜", context),
    ).toMatchObject({
      ok: true,
      draft: {
        name: "汇报一下 GitHub 热搜",
        schedule: { kind: "daily", hour: 9, minute: 0 },
      },
    });
  });

  it("rejects a daily request without a concrete time", () => {
    expect(parseScheduleRequest("每天定时汇报 GitHub 热搜", context)).toEqual({
      ok: false,
      error: "请补充每天的几点几分，例如：每天 09:30 总结项目进展",
    });
  });

  it("formats schedule labels", () => {
    expect(scheduleLabel({ kind: "daily", hour: 9, minute: 0 })).toBe(
      "每天 09:00",
    );
    expect(scheduleLabel({ kind: "interval", minutes: 60 })).toBe("每 1 小时");
  });
});
