import { describe, expect, it } from "vitest";
import { parseScheduleRequest } from "./parseSchedule";

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
      projectPath: "E:\\AgentTerm",
      toolId: "codex",
      intervalMinutes: 1440,
      enabled: true,
    });
  });

  it("ignores ordinary conversation", () => {
    expect(parseScheduleRequest("帮我看一下这个报错", context)).toBeNull();
  });
});
