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
  /** Non-fatal notice: the connection stays usable. */
  problem: (message: string) => void;
};

const MAX_PENDING_OUTPUT = 512 * 1024;
/// Unsent input allowed before new input is refused. This is backlog — a large
/// paste is split into `INPUT_CHUNK_BYTES` pieces, never rejected for its size.
const MAX_PENDING_INPUT = 1024 * 1024;
/// Keeps each IPC write small so one paste cannot occupy the write path for
/// long, and so a stuck CLI is noticed piece by piece.
const INPUT_CHUNK_BYTES = 16 * 1024;
const encoder = new TextEncoder();

/** Split on code-point boundaries so no character is ever cut in half. */
function splitInput(data: string, limit: number): string[] {
  if (encoder.encode(data).byteLength <= limit) return [data];
  const pieces: string[] = [];
  let current = "";
  let bytes = 0;
  for (const character of data) {
    const size = encoder.encode(character).byteLength;
    if (bytes + size > limit && current) {
      pieces.push(current);
      current = "";
      bytes = 0;
    }
    current += character;
    bytes += size;
  }
  if (current) pieces.push(current);
  return pieces;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

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
  const subscribe = async <T>(
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
  const timer = setTimeout(
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
    } catch (error) {
      const message = errorText(error);
      if (message.includes("积压")) {
        // The backend refused before writing anything, so nothing was
        // delivered in part: drop the queue, keep the connection usable.
        input.length = 0;
        inputBytes = 0;
        callbacks.problem(
          "终端输入积压过多，尚未发送的内容已丢弃。请等待当前输出结束后重试。",
        );
        return;
      }
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
      // Refuse only this input. Earlier input is unaffected and the session
      // stays connected; disconnecting here used to throw away large pastes
      // outright (#88).
      callbacks.problem(
        "待发送的输入已积压超过 1 MiB，本次输入未发送。请等待当前内容写入后重试。",
      );
      return;
    }
    for (const piece of splitInput(data, INPUT_CHUNK_BYTES)) {
      const pieceBytes = encoder.encode(piece).byteLength;
      inputBytes += pieceBytes;
      input.push({ data: piece, bytes: pieceBytes });
    }
    void drain();
  };

  return { ready, send, dispose };
}
