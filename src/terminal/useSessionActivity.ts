import { useCallback, useEffect, useRef, useState } from "react";
import type { ActiveSession } from "./useLauncher";

export type SessionActivity = "idle" | "working";

export const OUTPUT_THROTTLE_MS = 300;
export const IDLE_SETTLE_MS = 4000;
export const INPUT_ECHO_SUPPRESS_MS = 180;

// Strip common ANSI CSI/OSC/single-char escape sequences without control-regex
// lint exceptions so cursor blinks or focus reports never trigger "working".
export function hasMeaningfulOutput(text: string): boolean {
  let index = 0;
  while (index < text.length) {
    const code = text.charCodeAt(index);
    if (code === 0x1b) {
      index += 1;
      if (index >= text.length) break;
      const next = text.charCodeAt(index);
      if (next === 0x5b) {
        // CSI sequence: ESC [ ... [@-~]
        index += 1;
        while (index < text.length) {
          const b = text.charCodeAt(index);
          index += 1;
          if (b >= 0x40 && b <= 0x7e) break;
        }
      } else if (next === 0x5d) {
        // OSC sequence: ESC ] ... (BEL | ST)
        index += 1;
        while (index < text.length) {
          const b = text.charCodeAt(index);
          if (b === 0x07) {
            index += 1;
            break;
          }
          if (b === 0x1b && text.charCodeAt(index + 1) === 0x5c) {
            index += 2;
            break;
          }
          index += 1;
        }
      } else {
        index += 1;
      }
      continue;
    }
    if (code > 0x20 && code !== 0x7f) {
      return true;
    }
    index += 1;
  }
  return false;
}

export function activityLabel(
  activity: SessionActivity,
  running: boolean,
  statusUnknown: boolean,
): string {
  if (!running) return "已结束";
  if (statusUnknown) return "状态未知";
  if (activity === "working") return "输出中";
  return "运行中";
}

export function useSessionActivity(sessions: ActiveSession[]) {
  const [activities, setActivities] = useState<Record<string, SessionActivity>>(
    {},
  );
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;

  const lastEmitRef = useRef<Map<string, number>>(new Map());
  const lastInputRef = useRef<Map<string, number>>(new Map());
  const idleTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(
    new Map(),
  );

  const clearTimers = useCallback((id: string) => {
    const idleTimer = idleTimersRef.current.get(id);
    if (idleTimer !== undefined) {
      clearTimeout(idleTimer);
      idleTimersRef.current.delete(id);
    }
  }, []);

  useEffect(() => {
    const liveRunning = new Set(
      sessions.filter((s) => s.running).map((s) => s.info.id),
    );
    for (const id of [...idleTimersRef.current.keys()]) {
      if (!liveRunning.has(id)) clearTimers(id);
    }
    for (const id of [...lastEmitRef.current.keys()]) {
      if (!liveRunning.has(id)) lastEmitRef.current.delete(id);
    }
    for (const id of [...lastInputRef.current.keys()]) {
      if (!liveRunning.has(id)) lastInputRef.current.delete(id);
    }
  }, [sessions, clearTimers]);

  useEffect(() => {
    const idleTimers = idleTimersRef.current;
    return () => {
      for (const timer of idleTimers.values()) clearTimeout(timer);
      idleTimers.clear();
    };
  }, []);

  const reportOutput = useCallback((id: string, text?: string) => {
    const target = sessionsRef.current.find((s) => s.info.id === id);
    if (!target?.running) return;

    const now = Date.now();
    if (text !== undefined) {
      if (!hasMeaningfulOutput(text)) return;
      const lastInput = lastInputRef.current.get(id) ?? 0;
      if (now - lastInput < INPUT_ECHO_SUPPRESS_MS && text.length <= 16) {
        return;
      }
    }

    const last = lastEmitRef.current.get(id) ?? 0;
    if (now - last < OUTPUT_THROTTLE_MS && idleTimersRef.current.has(id)) {
      return;
    }
    lastEmitRef.current.set(id, now);

    setActivities((prev) =>
      prev[id] === "working" ? prev : { ...prev, [id]: "working" },
    );

    const prevIdle = idleTimersRef.current.get(id);
    if (prevIdle !== undefined) clearTimeout(prevIdle);

    const timer = setTimeout(() => {
      idleTimersRef.current.delete(id);
      setActivities((prev) =>
        prev[id] === "working" ? { ...prev, [id]: "idle" } : prev,
      );
    }, IDLE_SETTLE_MS);
    idleTimersRef.current.set(id, timer);
  }, []);

  const reportInput = useCallback((id: string) => {
    lastInputRef.current.set(id, Date.now());
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
