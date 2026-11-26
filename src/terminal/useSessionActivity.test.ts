// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ActiveSession } from "./useLauncher";
import {
  IDLE_SETTLE_MS,
  INPUT_ECHO_SUPPRESS_MS,
  activityLabel,
  hasMeaningfulOutput,
  useSessionActivity,
} from "./useSessionActivity";

function makeSession(id: string, running = true): ActiveSession {
  return {
    info: {
      id,
      toolId: "claude",
      toolName: "Claude Code",
      projectPath: "E:\\AgentTerm",
    },
    label: `Claude Code · ${id}`,
    titleSource: "default",
    running,
    statusUnknown: false,
    exitReason: null,
    closeError: null,
  };
}

describe("useSessionActivity", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("transitions idle -> working -> idle when output settles and never claims completed", () => {
    vi.useFakeTimers();
    const s1 = makeSession("s1");
    const s2 = makeSession("s2");
    const { result } = renderHook(
      ({ sessions }) => useSessionActivity(sessions),
      { initialProps: { sessions: [s1, s2] } },
    );

    expect(result.current.getActivity(s2)).toBe("idle");
    expect(activityLabel("idle", true, false)).toBe("运行中");
    act(() => {
      result.current.reportOutput("s2", "正在分析数据表...\n");
    });
    expect(result.current.getActivity(s2)).toBe("working");
    expect(activityLabel("working", true, false)).toBe("输出中");

    act(() => {
      vi.advanceTimersByTime(IDLE_SETTLE_MS);
    });
    expect(result.current.getActivity(s2)).toBe("idle");
    expect(activityLabel("idle", true, false)).toBe("运行中");
  });

  it("ignores ANSI control-only chunks and immediate user keystroke echoes", () => {
    vi.useFakeTimers();
    const s1 = makeSession("s1");
    const { result } = renderHook(
      ({ sessions }) => useSessionActivity(sessions),
      { initialProps: { sessions: [s1] } },
    );

    expect(hasMeaningfulOutput("\x1b[?25h\x1b[?25l")).toBe(false);
    expect(hasMeaningfulOutput("\x1b[32mOK\x1b[0m")).toBe(true);

    act(() => {
      result.current.reportOutput("s1", "\x1b[?25h\x1b[0G");
    });
    expect(result.current.getActivity(s1)).toBe("idle");

    act(() => {
      result.current.reportInput("s1");
      result.current.reportOutput("s1", "a");
    });
    expect(result.current.getActivity(s1)).toBe("idle");

    act(() => {
      vi.advanceTimersByTime(INPUT_ECHO_SUPPRESS_MS + 10);
      result.current.reportOutput("s1", "Agent reply chunk");
    });
    expect(result.current.getActivity(s1)).toBe("working");
  });

  it("returns idle when session exits", () => {
    vi.useFakeTimers();
    const s1 = makeSession("s1", true);
    const { result, rerender } = renderHook(
      ({ sessions }) => useSessionActivity(sessions),
      { initialProps: { sessions: [s1] } },
    );
    act(() => {
      result.current.reportOutput("s1", "hello");
    });
    expect(result.current.getActivity(s1)).toBe("working");
    const stopped = makeSession("s1", false);
    rerender({ sessions: [stopped] });
    expect(result.current.getActivity(stopped)).toBe("idle");
  });
});
