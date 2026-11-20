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
  const invoke = vi.fn<SessionBridge["invoke"]>().mockResolvedValue({
    data: "hello",
    offset: 5,
    running: true,
  });
  const bridge: SessionBridge = {
    invoke,
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
  const connect = () => {
    const connection = connectSession("one", bridge, { output, phase, exit });
    cleanup.push(connection.dispose);
    return connection;
  };
  const emit = (event: string, payload: unknown) =>
    listeners.get(`session-${event}-one`)?.({ payload });
  return { invoke, bridge, output, phase, exit, connect, emit, removed };
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
    expect(f.invoke).toHaveBeenCalledTimes(2);
    expect(f.removed).toHaveLength(2);
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

  it("bounds input without sending oversized paste content", async () => {
    const f = fixture();
    const c = f.connect();
    await c.ready;
    c.send("中".repeat(30_000));
    expect(f.phase).toHaveBeenLastCalledWith(
      "error",
      expect.stringContaining("输入积压"),
    );
    expect(f.invoke).toHaveBeenCalledOnce();
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
