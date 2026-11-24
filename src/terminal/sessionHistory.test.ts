import { describe, expect, it } from "vitest";
import type { ToolStatus } from "../tools/types";
import {
  folderName,
  formatSessionTime,
  historyDisplayTitle,
  historyStatusText,
  historySubtitleText,
  installedToolForRecord,
  projectFromRecord,
  recentClosedHistory,
} from "./sessionHistory";
import type { SessionHistoryRecord } from "./types";

function makeRecord(
  overrides: Partial<SessionHistoryRecord> = {},
): SessionHistoryRecord {
  return {
    id: "s-1",
    toolId: "claude",
    toolName: "Claude Code",
    projectPath: "E:\\AgentTerm",
    startedAtMs: 1_763_650_000_000,
    endedAtMs: null,
    exitReason: null,
    outputBytes: null,
    ...overrides,
  };
}

describe("session history helpers", () => {
  it("extracts folder names across Windows and Unix paths", () => {
    expect(folderName("E:\\AgentTerm")).toBe("AgentTerm");
    expect(folderName("E:\\dw-modeling-template\\")).toBe(
      "dw-modeling-template",
    );
    expect(folderName("/Users/dev/agentterm")).toBe("agentterm");
  });

  it("builds a project candidate from a history record", () => {
    expect(projectFromRecord(makeRecord())).toEqual({
      id: "E:\\AgentTerm",
      name: "AgentTerm",
      path: "E:\\AgentTerm",
    });
  });

  it("distinguishes exited sessions from unclosed crashes", () => {
    expect(historyStatusText(makeRecord())).toBe("上次未正常退出");
    expect(
      historyStatusText(makeRecord({ endedAtMs: 1_763_650_100_000 })),
    ).toBe("已结束");
    expect(
      historyStatusText(
        makeRecord({
          endedAtMs: 1_763_650_100_000,
          exitReason: "已停止",
        }),
      ),
    ).toBe("已结束 · 已停止");
  });

  it("filters out live sessions and limits displayed recent entries", () => {
    const records = Array.from({ length: 8 }, (_, index) =>
      makeRecord({ id: `s-${index}` }),
    );
    const active = new Set(["s-0", "s-2"]);
    expect(recentClosedHistory(records, active).map((r) => r.id)).toEqual([
      "s-1",
      "s-3",
      "s-4",
      "s-5",
      "s-6",
    ]);
  });

  it("only enables reopening when the original tool is installed", () => {
    const tools: ToolStatus[] = [
      {
        id: "claude",
        name: "Claude Code",
        state: { kind: "installed", path: "claude.exe", version: "2.1.326" },
      },
      {
        id: "codex",
        name: "Codex",
        state: { kind: "missing" },
      },
    ];
    expect(installedToolForRecord(makeRecord(), tools)?.id).toBe("claude");
    expect(
      installedToolForRecord(makeRecord({ toolId: "codex" }), tools),
    ).toBeNull();
    expect(
      installedToolForRecord(makeRecord({ toolId: "agy" }), tools),
    ).toBeNull();
  });

  it("handles invalid timestamps without throwing", () => {
    expect(formatSessionTime(Number.NaN)).toBe("时间未知");
    expect(formatSessionTime(1_763_650_000_000)).not.toBe("时间未知");
  });

  it("displays custom session titles while keeping tool and folder context", () => {
    expect(historyDisplayTitle(makeRecord())).toBe("Claude Code · AgentTerm");
    const named = makeRecord({ title: "修复订单指标" });
    expect(historyDisplayTitle(named)).toBe("修复订单指标");
    expect(historySubtitleText(named)).toContain("Claude Code · AgentTerm");
  });
});
