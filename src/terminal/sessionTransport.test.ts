import { afterEach, describe, expect, it, vi } from "vitest";
import { connectSession, type SessionBridge } from "./sessionTransport";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function fixture() {
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  const removed: string[] = [];
  const invoke = vi
    .fn<(command: string, args: Record<string, unknown>) => Promise<unknown>>()
    .mockResolvedValue({
      data: "hello",
      offset: 5,
      running: true,
    });
  const bridge: SessionBridge = {
    invoke: async <T>(command: string, args: Record<string, unknown>) =>
      (await invoke(command, args)) as T,
    listen: async <T>(
      name: string,
      listener: (event: { payload: T }) => void,
    ) => {
      listeners.set(name, listener as (event: { payload: unknown }) => void);
      return () => {
        removed.push(name);
        listeners.delete(name);
      };
    },
  };
  const output = vi.fn();
  const phase = vi.fn();
  const exit = vi.fn();
  const problem = vi.fn();
  const connect = () => {
    const connection = connectSession("one", bridge, {
      output,
      phase,
      exit,
      problem,
    });
    cleanup.push(connection.dispose);
    return connection;
  };
  const emit = (event: string, payload: unknown) =>
    listeners.get(`session-${event}-one`)?.({ payload });
  return {
    invoke,
    bridge,
    output,
    phase,
    exit,
    problem,
    connect,
    emit,
    removed,
  };
}

const cleanup: (() => void)[] = [];
afterEach(() => {
  for (const dispose of cleanup.splice(0)) dispose();
  vi.useRealTimers();
});

describe("existing terminal connection", () => {
  it("reconciles a snapshot with overlapping live output exactly once", async () => {
    const f = fixture();
    const snapshot = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => snapshot.promise as Promise<never>);
    const c = f.connect();
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledOnce());
    f.emit("output", { data: "hello!", endOffset: 6 });
    snapshot.resolve({ data: "hello", offset: 5, running: true });
    await c.ready;
    expect(f.output.mock.calls.flat().join("")).toBe("hello!");
    expect(f.phase).toHaveBeenLastCalledWith("ready", null);
    expect(f.invoke.mock.calls.map(([command]) => command)).toEqual([
      "session_snapshot",
    ]);
  });

  it("reports an early exit only after final output and only once", async () => {
    const f = fixture();
    const snapshot = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => snapshot.promise as Promise<never>);
    const c = f.connect();
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledOnce());
    f.emit("exit", { id: "one", reason: "exit 1" });
    expect(f.exit).not.toHaveBeenCalled();
    snapshot.resolve({ data: "failure", offset: 7, running: false });
    await c.ready;
    f.emit("exit", { id: "one", reason: "duplicate" });
    c.send("ignored");
    expect(f.output).toHaveBeenCalledWith("failure");
    expect(f.exit).toHaveBeenCalledExactlyOnceWith("one", "exit 1");
    expect(f.invoke).toHaveBeenCalledOnce();
  });

  it("cleans a listener that resolves after disposal", async () => {
    const f = fixture();
    const listener = deferred<() => void>();
    const unsubscribe = vi.fn();
    f.bridge.listen = () => listener.promise;
    const c = f.connect();
    c.dispose();
    listener.resolve(unsubscribe);
    await c.ready;
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(f.invoke).not.toHaveBeenCalled();
  });

  it("does not replay a late snapshot or report it ready after disposal", async () => {
    const f = fixture();
    const snapshot = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => snapshot.promise as Promise<never>);
    const c = f.connect();
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledOnce());
    c.dispose();
    snapshot.resolve({ data: "late", offset: 4, running: true });
    await c.ready;
    expect(f.output).not.toHaveBeenCalled();
    expect(f.removed).toHaveLength(2);
    expect(f.phase).toHaveBeenCalledExactlyOnceWith("connecting", null);
  });

  it("shows a snapshot failure and cleans both subscriptions", async () => {
    const f = fixture();
    f.invoke.mockRejectedValueOnce(new Error("offline"));
    const c = f.connect();
    await c.ready;
    c.send("must not send");
    expect(f.phase).toHaveBeenLastCalledWith("error", expect.any(String));
    expect(f.removed).toHaveLength(2);
    expect(f.invoke).toHaveBeenCalledOnce();
    expect(f.exit).not.toHaveBeenCalled();
  });

  it("cleans the first subscription when the second subscription fails", async () => {
    const f = fixture();
    const original = f.bridge.listen;
    f.bridge.listen = (name, listener) =>
      name.includes("output")
        ? Promise.reject(new Error("listen failed"))
        : original(name, listener);
    await f.connect().ready;
    expect(f.removed).toEqual(["session-exit-one"]);
    expect(f.phase).toHaveBeenLastCalledWith("error", expect.any(String));
    expect(f.invoke).not.toHaveBeenCalled();
  });

  it("sends fast input in order with one write in flight", async () => {
    const f = fixture();
    const c = f.connect();
    await c.ready;
    const first = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => first.promise as Promise<never>);
    c.send("中");
    c.send("文");
    c.send("\r");
    expect(f.invoke).toHaveBeenCalledTimes(2);
    first.resolve(null);
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledTimes(4));
    expect(f.invoke.mock.calls.slice(1).map(([, args]) => args.data)).toEqual([
      "中",
      "文",
      "\r",
    ]);
  });

  it("never retries failed input or sends queued input after an uncertain write", async () => {
    const f = fixture();
    const c = f.connect();
    await c.ready;
    const write = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => write.promise as Promise<never>);
    c.send("first");
    c.send("must not send");
    write.reject(new Error("unknown delivery"));
    await vi.waitFor(() =>
      expect(f.phase).toHaveBeenLastCalledWith(
        "error",
        expect.stringContaining("不会自动重发"),
      ),
    );
    c.send("also blocked");
    expect(f.invoke).toHaveBeenCalledTimes(3);
    expect(f.invoke).toHaveBeenLastCalledWith("block_session_input", {
      sessionId: "one",
    });
    expect(f.removed).toHaveLength(0);
    f.emit("output", { data: "alive", endOffset: 10 });
    expect(f.output).toHaveBeenLastCalledWith("alive");
    expect(f.exit).not.toHaveBeenCalled();
  });

  it("bounds pending output while waiting for a snapshot", async () => {
    const f = fixture();
    const snapshot = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => snapshot.promise as Promise<never>);
    const c = f.connect();
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledOnce());
    f.emit("output", { data: "x".repeat(512 * 1024 + 1), endOffset: 600_000 });
    expect(f.phase).toHaveBeenLastCalledWith(
      "error",
      expect.stringContaining("输出过多"),
    );
    snapshot.resolve({ data: "", offset: 0, running: true });
    await c.ready;
    expect(f.output).not.toHaveBeenCalled();
  });

  it("delivers a large paste in bounded chunks instead of disconnecting", async () => {
    const f = fixture();
    const c = f.connect();
    await c.ready;
    // 90 KB of Chinese text: a single paste larger than the old 64 KB cap.
    const paste = "中".repeat(30_000);
    c.send(paste);
    await vi.waitFor(() =>
      expect(f.invoke.mock.calls.length).toBeGreaterThan(5),
    );
    const chunks = f.invoke.mock.calls
      .slice(1)
      .map(([, args]) => args.data as string);
    expect(chunks.join("")).toBe(paste);
    expect(chunks.every((chunk) => chunk.length > 0)).toBe(true);
    // Every chunk stays under the 16 KB IPC limit, so no single write can hog
    // the write path.
    expect(
      chunks.every(
        (chunk) => new TextEncoder().encode(chunk).byteLength <= 16 * 1024,
      ),
    ).toBe(true);
    expect(f.phase).not.toHaveBeenLastCalledWith("error", expect.anything());
    expect(f.problem).not.toHaveBeenCalled();
  });

  it("refuses only the overflowing input and keeps the connection", async () => {
    const f = fixture();
    const c = f.connect();
    await c.ready;
    const stuck = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => stuck.promise as Promise<never>);
    // Queue more than the 1 MiB backlog while the first write never settles.
    c.send("x".repeat(600 * 1024));
    c.send("y".repeat(600 * 1024));
    expect(f.problem).toHaveBeenLastCalledWith(expect.stringContaining("积压"));
    // The connection is not torn down and nothing after the refusal is queued.
    expect(f.phase).toHaveBeenLastCalledWith("ready", null);
    stuck.resolve(null);
    await vi.waitFor(() =>
      expect(
        f.invoke.mock.calls
          .slice(1)
          .map(([, args]) => args.data as string)
          .join(""),
      ).toBe("x".repeat(600 * 1024)),
    );
    expect(f.phase).not.toHaveBeenLastCalledWith("error", expect.anything());
  });

  it("treats a backend backlog refusal as recoverable", async () => {
    const f = fixture();
    const c = f.connect();
    await c.ready;
    f.invoke.mockRejectedValueOnce(
      "终端输入积压过多，请等待当前输入写入后再试",
    );
    c.send("abc");
    await vi.waitFor(() =>
      expect(f.problem).toHaveBeenLastCalledWith(
        expect.stringContaining("积压"),
      ),
    );
    expect(f.phase).not.toHaveBeenLastCalledWith("error", expect.anything());
    c.send("next");
    await vi.waitFor(() =>
      expect(f.invoke).toHaveBeenLastCalledWith("write_session_input", {
        sessionId: "one",
        data: "next",
      }),
    );
    expect(
      f.invoke.mock.calls.some(
        ([command]) => command === "block_session_input",
      ),
    ).toBe(false);
  });

  it("blocks input after a partially delivered paste and preserves the block across reconnect", async () => {
    const f = fixture();
    const delivered: string[] = [];
    let blocked = false;
    let writes = 0;
    f.invoke.mockImplementation(async (command, args) => {
      if (command === "session_snapshot") {
        return {
          data: "hello",
          offset: 5,
          running: true,
          inputBlocked: blocked,
        };
      }
      if (command === "block_session_input") {
        blocked = true;
        return null;
      }
      writes += 1;
      if (writes === 2)
        throw new Error("终端输入积压过多，请等待当前输入写入后再试");
      delivered.push(args.data as string);
      return null;
    });
    const connection = f.connect();
    await connection.ready;
    connection.send("\x1b[200~" + "中🙂".repeat(14000) + "\x1b[201~");
    await vi.waitFor(() => expect(blocked).toBe(true));
    expect(delivered.join("")).toContain("\x1b[200~");
    expect(delivered.join("")).not.toContain("\x1b[201~");
    expect(f.phase).toHaveBeenLastCalledWith(
      "error",
      expect.stringContaining("一部分"),
    );
    connection.send("\r");
    expect(writes).toBe(2);
    expect(f.problem).not.toHaveBeenCalled();
    f.emit("output", { data: "alive", endOffset: 10 });
    expect(f.output).toHaveBeenLastCalledWith("alive");
    connection.dispose();
    const reconnected = f.connect();
    await reconnected.ready;
    expect(f.phase).toHaveBeenLastCalledWith(
      "error",
      expect.stringContaining("仅恢复显示"),
    );
    reconnected.send("must stay blocked\r");
    expect(writes).toBe(2);
  });

  it("ignores a late write failure after disposal", async () => {
    const f = fixture();
    const connection = f.connect();
    await connection.ready;
    const pending = deferred<unknown>();
    f.invoke.mockImplementationOnce(() => pending.promise);
    connection.send("test");
    connection.dispose();
    pending.reject(new Error("终端输入积压过多"));
    await Promise.resolve();
    await Promise.resolve();
    expect(f.problem).not.toHaveBeenCalled();
    expect(f.invoke).toHaveBeenCalledTimes(2);
  });

  it("times out a stalled connection and discards its late response", async () => {
    vi.useFakeTimers();
    const f = fixture();
    const listener = deferred<() => void>();
    const unsubscribe = vi.fn();
    f.bridge.listen = () => listener.promise;
    const c = f.connect();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(f.phase).toHaveBeenLastCalledWith(
      "error",
      expect.stringContaining("连接超时"),
    );
    listener.resolve(unsubscribe);
    await c.ready;
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(f.invoke).not.toHaveBeenCalled();
  });
});
