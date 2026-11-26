import { describe, expect, it } from "vitest";
import type { ActiveSession } from "../terminal/useLauncher";
import {
  describeUpdateActivity,
  mergeUpdateActivity,
  projectForRecovery,
} from "./updateHelpers";

describe("update helpers", () => {
  it("merges frontend running and unknown sessions without duplicating backend entries", () => {
    const backend = {
      liveSessions: [
        {
          id: "s-1",
          toolId: "claude",
          toolName: "Claude Code",
          projectPath: "E:\\AgentTerm",
          title: "修登录",
        },
      ],
      dispatchActive: false,
      scheduledTaskId: null,
      hasActiveWork: true,
    };
    const frontend: ActiveSession[] = [
      {
        info: backend.liveSessions[0],
        label: "修登录",
        titleSource: "manual",
        running: true,
        statusUnknown: false,
        exitReason: null,
        closeError: null,
      },
      {
        info: {
          id: "s-2",
          toolId: "codex",
          toolName: "Codex",
          projectPath: "E:\\Other",
          title: null,
        },
        label: "Codex · 2",
        titleSource: "default",
        running: true,
        statusUnknown: true,
        exitReason: null,
        closeError: null,
      },
      {
        info: {
          id: "s-3",
          toolId: "agy",
          toolName: "Agy",
          projectPath: "E:\\Done",
        },
        label: "Agy · 3",
        titleSource: "default",
        running: false,
        statusUnknown: false,
        exitReason: "已停止",
        closeError: null,
      },
    ];

    const merged = mergeUpdateActivity(backend, frontend);
    expect(merged.liveSessions.map((s) => s.id)).toEqual(["s-1", "s-2"]);
    expect(merged.hasActiveWork).toBe(true);
    expect(
      describeUpdateActivity({
        ...merged,
        dispatchActive: true,
        scheduledTaskId: "daily-sync",
      }),
    ).toEqual([
      "运行中的终端会话（2 个）：修登录、Codex · Other",
      "任务工作台中有正在执行的受控任务",
      "定时任务（daily-sync）正在执行",
    ]);
  });

  it("resolves registered project or falls back to recovery path", () => {
    const recovered = {
      id: "s-1",
      toolId: "claude",
      toolName: "Claude Code",
      projectPath: "E:\\Unregistered\\Demo",
    };
    expect(projectForRecovery(recovered, [])).toEqual({
      id: "E:\\Unregistered\\Demo",
      name: "Demo",
      path: "E:\\Unregistered\\Demo",
    });
  });
});
