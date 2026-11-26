import { useCallback, useEffect, useRef, useState } from "react";
import type { ActiveSession } from "./useLauncher";

export type SessionActivity = "idle" | "working" | "done";

export const OUTPUT_THROTTLE_MS = 300;
export const IDLE_COMPLETE_MS = 2500;
export const ACTIVE_DONE_CLEAR_MS = 4500;

export function activityLabel(
  activity: SessionActivity,
  running: boolean,
  statusUnknown: boolean,
): string {
  if (!running) return "已结束";
  if (statusUnknown) return "状态未知";
  if (activity === "working") return "执行中";
  if (activity === "done") return "已完成";
  return "运行中";
}

export function useSessionActivity(
  sessions: ActiveSession[],
  activeId: string | null,
) {
  const [activities, setActivities] = useState<Record<string, SessionActivity>>(
    {},
  );
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;

  const lastEmitRef = useRef<Map<string, number>>(new Map());
  const idleTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(
    new Map(),
  );
  const doneClearTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(
    new Map(),
  );

  const clearTimers = useCallback((id: string) => {
    const idleTimer = idleTimersRef.current.get(id);
    if (idleTimer !== undefined) {
      clearTimeout(idleTimer);
      idleTimersRef.current.delete(id);
    }
    const doneTimer = doneClearTimersRef.current.get(id);
    if (doneTimer !== undefined) {
      clearTimeout(doneTimer);
      doneClearTimersRef.current.delete(id);
    }
  }, []);

  useEffect(() => {
    const liveRunning = new Set(
      sessions.filter((s) => s.running).map((s) => s.info.id),
    );
    for (const id of [...idleTimersRef.current.keys()]) {
      if (!liveRunning.has(id)) clearTimers(id);
    }
    for (const id of [...doneClearTimersRef.current.keys()]) {
      if (!liveRunning.has(id)) clearTimers(id);
    }
    for (const id of [...lastEmitRef.current.keys()]) {
      if (!liveRunning.has(id)) lastEmitRef.current.delete(id);
    }
  }, [sessions, clearTimers]);

  useEffect(() => {
    if (!activeId) return;
    if (activities[activeId] !== "done") return;
    if (doneClearTimersRef.current.has(activeId)) return;
    const timer = setTimeout(() => {
      doneClearTimersRef.current.delete(activeId);
      setActivities((prev) =>
        prev[activeId] === "done" ? { ...prev, [activeId]: "idle" } : prev,
      );
    }, ACTIVE_DONE_CLEAR_MS);
    doneClearTimersRef.current.set(activeId, timer);
  }, [activeId, activities]);

  useEffect(() => {
    const idleTimers = idleTimersRef.current;
    const doneTimers = doneClearTimersRef.current;
    return () => {
      for (const timer of idleTimers.values()) clearTimeout(timer);
      for (const timer of doneTimers.values()) clearTimeout(timer);
      idleTimers.clear();
      doneTimers.clear();
    };
  }, []);

  const reportOutput = useCallback((id: string) => {
    const target = sessionsRef.current.find((s) => s.info.id === id);
    if (!target?.running) return;

    const now = Date.now();
    const last = lastEmitRef.current.get(id) ?? 0;
    if (now - last < OUTPUT_THROTTLE_MS && idleTimersRef.current.has(id)) {
      return;
    }
    lastEmitRef.current.set(id, now);

    const existingDoneTimer = doneClearTimersRef.current.get(id);
    if (existingDoneTimer !== undefined) {
      clearTimeout(existingDoneTimer);
      doneClearTimersRef.current.delete(id);
    }

    setActivities((prev) =>
      prev[id] === "working" ? prev : { ...prev, [id]: "working" },
    );

    const prevIdle = idleTimersRef.current.get(id);
    if (prevIdle !== undefined) clearTimeout(prevIdle);

    const timer = setTimeout(() => {
      idleTimersRef.current.delete(id);
      const stillRunning = sessionsRef.current.some(
        (s) => s.info.id === id && s.running,
      );
      if (!stillRunning) return;
      setActivities((prev) =>
        prev[id] === "working" ? { ...prev, [id]: "done" } : prev,
      );
    }, IDLE_COMPLETE_MS);
    idleTimersRef.current.set(id, timer);
  }, []);

  const reportInput = useCallback((id: string) => {
    const doneTimer = doneClearTimersRef.current.get(id);
    if (doneTimer !== undefined) {
      clearTimeout(doneTimer);
      doneClearTimersRef.current.delete(id);
    }
    setActivities((prev) =>
      prev[id] === "done" ? { ...prev, [id]: "idle" } : prev,
    );
  }, []);

  const getActivity = useCallback(
    (session: ActiveSession): SessionActivity => {
      if (!session.running || session.statusUnknown) return "idle";
      return activities[session.info.id] ?? "idle";
    },
    [activities],
  );

  return {
    getActivity,
    reportOutput,
    reportInput,
  };
}
