// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ActiveSession } from "./useLauncher";
import {
  ACTIVE_DONE_CLEAR_MS,
  IDLE_COMPLETE_MS,
  activityLabel,
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

  it("transitions idle -> working -> done -> cleared when viewed or typed", () => {
    vi.useFakeTimers();
    const s1 = makeSession("s1");
    const s2 = makeSession("s2");
    const { result, rerender } = renderHook(
      ({ sessions, activeId }) => useSessionActivity(sessions, activeId),
      { initialProps: { sessions: [s1, s2], activeId: "s1" as string | null } },
    );

    expect(result.current.getActivity(s2)).toBe("idle");
    act(() => {
      result.current.reportOutput("s2");
    });
    expect(result.current.getActivity(s2)).toBe("working");
    expect(activityLabel("working", true, false)).toBe("执行中");

    act(() => {
      vi.advanceTimersByTime(IDLE_COMPLETE_MS);
    });
    expect(result.current.getActivity(s2)).toBe("done");
    expect(activityLabel("done", true, false)).toBe("已完成");

    // Background done state stays visible until switched to
    act(() => {
      vi.advanceTimersByTime(ACTIVE_DONE_CLEAR_MS * 2);
    });
    expect(result.current.getActivity(s2)).toBe("done");

    // Switching to s2 schedules auto-clear after viewing
    rerender({ sessions: [s1, s2], activeId: "s2" });
    expect(result.current.getActivity(s2)).toBe("done");
    act(() => {
      vi.advanceTimersByTime(ACTIVE_DONE_CLEAR_MS);
    });
    expect(result.current.getActivity(s2)).toBe("idle");

    // Typing clears done immediately
    act(() => {
      result.current.reportOutput("s2");
      vi.advanceTimersByTime(IDLE_COMPLETE_MS);
    });
    expect(result.current.getActivity(s2)).toBe("done");
    act(() => {
      result.current.reportInput("s2");
    });
    expect(result.current.getActivity(s2)).toBe("idle");
  });

  it("returns idle when session exits", () => {
    vi.useFakeTimers();
    const s1 = makeSession("s1", true);
    const { result, rerender } = renderHook(
      ({ sessions, activeId }) => useSessionActivity(sessions, activeId),
      { initialProps: { sessions: [s1], activeId: "s1" as string | null } },
    );
    act(() => {
      result.current.reportOutput("s1");
    });
    expect(result.current.getActivity(s1)).toBe("working");
    const stopped = makeSession("s1", false);
    rerender({ sessions: [stopped], activeId: "s1" });
    expect(result.current.getActivity(stopped)).toBe("idle");
  });
});
