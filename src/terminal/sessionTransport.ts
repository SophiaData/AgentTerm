import { advance } from "./offsetStitch";

type Chunk = { data: string; endOffset: number };
type Snapshot = { data: string; offset: number; running: boolean };
type Exit = { id: string; reason: string };
export type ConnectionPhase = "connecting" | "ready" | "exited" | "error";

export type SessionBridge = {
  invoke: <T>(command: string, args: Record<string, unknown>) => Promise<T>;
  listen: <T>(
    name: string,
    listener: (event: { payload: T }) => void,
  ) => Promise<() => void>;
};

type Callbacks = {
  output: (text: string) => void;
  phase: (phase: ConnectionPhase, error: string | null) => void;
  exit: (id: string, reason: string) => void;
};

const MAX_PENDING_OUTPUT = 512 * 1024;
const MAX_PENDING_INPUT = 64 * 1024;
const encoder = new TextEncoder();

/** Reconnects to an existing PTY. Never starts a process or retries input. */
export function connectSession(
  id: string,
  bridge: SessionBridge,
  callbacks: Callbacks,
) {
  let active = true;
  let initialized = false;
  let ended = false;
  let offset = -1;
  let pendingExit: Exit | null = null;
  let pendingBytes = 0;
  let inputBytes = 0;
  let writing = false;
  const pending: Chunk[] = [];
  const input: { data: string; bytes: number }[] = [];
  const subscriptions: (() => void)[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;

  const dispose = () => {
    active = false;
    clearTimeout(timer);
    for (const unsubscribe of subscriptions.splice(0)) unsubscribe();
    pending.length = 0;
    input.length = 0;
    pendingBytes = 0;
    inputBytes = 0;
  };
  const fail = (message: string) => {
    if (!active) return;
    dispose();
    callbacks.phase("error", message);
  };
  const reportExit = (exit: Exit) => {
    if (!active || ended) return;
    ended = true;
    input.length = 0;
    inputBytes = 0;
    callbacks.phase("exited", null);
    callbacks.exit(id, exit.reason);
  };
  const output = (chunk: Chunk) => {
    if (!active || ended) return;
    if (!initialized) {
      pendingBytes += encoder.encode(chunk.data).byteLength;
      if (pendingBytes > MAX_PENDING_OUTPUT) {
        fail("终端连接期间输出过多，已暂停接收。请重新连接显示。");
      } else {
        pending.push(chunk);
      }
      return;
    }
    const next = advance(chunk, offset);
    if (next.text) callbacks.output(next.text);
    offset = next.offset;
  };
  const subscribe = async <T,>(
    name: string,
    listener: (event: { payload: T }) => void,
  ) => {
    const unsubscribe = await bridge.listen<T>(name, listener);
    if (!active) {
      unsubscribe();
      return false;
    }
    subscriptions.push(unsubscribe);
    return true;
  };

  callbacks.phase("connecting", null);
  timer = setTimeout(
    () => fail("终端连接超时。会话可能仍在运行，请重新连接显示。"),
    10_000,
  );
  const ready = (async () => {
    try {
      if (
        !(await subscribe<Exit>(`session-exit-${id}`, ({ payload }) => {
          if (!active || payload.id !== id) return;
          if (initialized) reportExit(payload);
          else pendingExit = payload;
        }))
      )
        return;
      if (
        !(await subscribe<Chunk>(`session-output-${id}`, ({ payload }) =>
          output(payload),
        ))
      )
        return;
      const snapshot = await bridge.invoke<Snapshot>("session_snapshot", {
        sessionId: id,
      });
      if (!active) return;
      clearTimeout(timer);
      if (snapshot.data) callbacks.output(snapshot.data);
      offset = snapshot.offset;
      initialized = true;
      for (const chunk of pending) output(chunk);
      pending.length = 0;
      pendingBytes = 0;
      if (pendingExit || !snapshot.running) {
        reportExit(pendingExit ?? { id, reason: "进程已结束" });
      } else {
        callbacks.phase("ready", null);
      }
    } catch {
      fail(
        "无法连接终端输出或读取会话状态。会话可能仍在运行，请重新连接显示。",
      );
    }
  })();

  const drain = async () => {
    if (writing) return;
    writing = true;
    try {
      while (active && !ended && input.length) {
        const next = input.shift()!;
        await bridge.invoke("write_session_input", {
          sessionId: id,
          data: next.data,
        });
        inputBytes = Math.max(0, inputBytes - next.bytes);
      }
    } catch {
      fail(
        "输入未确认送达，已暂停后续输入。请重新连接后检查终端；刚才的内容不会自动重发。",
      );
    } finally {
      writing = false;
    }
  };
  const send = (data: string) => {
    if (!active || !initialized || ended || !data) return;
    const bytes = encoder.encode(data).byteLength;
    if (inputBytes + bytes > MAX_PENDING_INPUT) {
      fail(
        "输入积压过多，已暂停发送。请重新连接并检查已收到的内容，再分段输入。",
      );
      return;
    }
    inputBytes += bytes;
    input.push({ data, bytes });
    void drain();
  };

  return { ready, send, dispose };
}
