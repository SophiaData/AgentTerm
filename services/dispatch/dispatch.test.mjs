import test from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { request as httpRequest } from "node:http";
import { fileURLToPath } from "node:url";
import { TaskStore, WorkerStore, projectFingerprint } from "./store.mjs";
import { startServer, client } from "./server.mjs";
import { Worker, executeTask, workerLock } from "./worker.mjs";
import { run, askDsh, standaloneEnvironment } from "./process.mjs";
import { coordinate, watch } from "./coordinator.mjs";

const fixtureRoot = fileURLToPath(new URL("./fixtures/", import.meta.url));
function root() {
  return mkdtempSync(join(tmpdir(), "agentterm-dispatch-test-"));
}
function spec(requestId = "request-1") {
  return {
    requestId,
    projectId: "demo",
    baseCommit: "a".repeat(40),
    goal: "完成一个测试任务",
    scope: "src/",
    plan: "DSH：修改后运行测试。",
    executionHash: projectFingerprint(projects.demo),
  };
}
function approve(store, input = spec()) {
  const task = store.submit(input);
  return store.approve(task.id, task.specHash);
}
function result() {
  return {
    state: "awaiting_review",
    commit: "b".repeat(40),
    checks: [{ passed: true }],
  };
}
const projects = { demo: { repository: "/synthetic" } };

test("独立 CLI 不继承桌面会话 IPC，模型和工作目录沙箱仍保留", () => {
  const input = {
    CODEX_WINDOWS_REGISTERED_CORE: "desktop-fixture",
    CODEX_APP_TOOLS_PIPE_PATH: "fixture-pipe",
    CODEX_THREAD_ID: "fixture-thread",
    CODEX_HOME: "fixture-profile",
    CODEX_PERMISSION_PROFILE: "workspace",
    PATH: "fixture-path",
  };
  assert.deepEqual(standaloneEnvironment(input), {
    CODEX_HOME: "fixture-profile",
    CODEX_PERMISSION_PROFILE: "workspace",
    PATH: "fixture-path",
  });
  assert.equal(input.CODEX_THREAD_ID, "fixture-thread");
});

test("任务建议幂等、人工确认绑定摘要，离线队列跨服务重启保存", () => {
  const file = join(root(), "server.sqlite");
  let store = new TaskStore(file);
  const proposed = store.submit(spec());
  assert.equal(store.submit(spec()).id, proposed.id);
  assert.throws(() => store.submit({ ...spec(), goal: "另一个任务" }));
  assert.equal(store.claim("device", ["demo"]), null);
  assert.throws(() => store.approve(proposed.id, "stale"));
  store.approve(proposed.id, proposed.specHash);
  store.close();
  store = new TaskStore(file);
  const claimed = store.claim("device", ["demo"]);
  assert.equal(claimed.id, proposed.id);
  assert.equal(store.claim("device", ["demo"]).attemptId, claimed.attemptId);
  assert.equal(store.claim("other-device", ["demo"]), null);
  store.close();
});

test("领取在两个数据库连接间互斥，心跳不使终态回退", () => {
  const file = join(root(), "server.sqlite"),
    one = new TaskStore(file),
    two = new TaskStore(file);
  approve(one);
  approve(one, spec("second"));
  const task = one.claim("one", ["demo"]);
  assert.equal(two.claim("two", ["demo"]), null);
  const identity = { deviceId: "one", attemptId: task.attemptId };
  const finished = one.finish(task.id, { ...identity, result: result() });
  assert.equal(
    two.heartbeat(task.id, { ...identity, state: "running" }).state,
    "awaiting_review",
  );
  assert.throws(() => one.accept(task.id, "old"));
  one.accept(task.id, finished.resultHash);
  assert.equal(two.claim("two", ["demo"]).requestId, "second");
  one.close();
  two.close();
});

test("旧身份与冲突结果被拒绝；相同回执重传幂等", () => {
  const store = new TaskStore(join(root(), "server.sqlite"));
  approve(store);
  const task = store.claim("device", ["demo"]);
  const input = {
    deviceId: "device",
    attemptId: task.attemptId,
    result: result(),
  };
  assert.throws(() => store.finish(task.id, { ...input, attemptId: "old" }));
  const first = store.finish(task.id, input);
  assert.equal(store.finish(task.id, input).resultHash, first.resultHash);
  assert.throws(() =>
    store.finish(task.id, { ...input, result: { state: "failed" } }),
  );
  store.close();
});

test("放弃须先取得原生停止证明，旧回执重传不影响下一任务", async () => {
  let stopped = false,
    releases = 0;
  const service = await startServer({
    database: join(root(), "s.sqlite"),
    port: 0,
    projects,
    abandon: async () => {
      releases++;
      if (!stopped) throw new Error("尚未停止");
    },
  });
  const api = client(service.url);
  try {
    approve(service.store);
    approve(service.store, spec("next"));
    const task = service.store.claim("device", ["demo"]);
    const finished = service.store.finish(task.id, {
      ...task,
      result: { state: "failed" },
    });
    await assert.rejects(
      api(`/v1/tasks/${task.id}/abandon`, { resultHash: "old" }),
    );
    assert.equal(releases, 0);
    await assert.rejects(
      api(`/v1/tasks/${task.id}/abandon`, { resultHash: finished.resultHash }),
      /尚未停止/,
    );
    assert.equal(service.store.claim("other", ["demo"]), null);
    stopped = true;
    assert.equal(
      (
        await api(`/v1/tasks/${task.id}/abandon`, {
          resultHash: finished.resultHash,
        })
      ).state,
      "abandoned",
    );
    assert.equal(service.store.claim("other", ["demo"]).requestId, "next");
    await api(`/v1/tasks/${task.id}/abandon`, {
      resultHash: finished.resultHash,
    });
    assert.equal(releases, 2);
    assert.equal(service.store.get(task.id).result.state, "failed");
  } finally {
    await service.close();
  }
});

test("执行端启动意图 CAS，重复投递不会重复取得执行资格", () => {
  const directory = root(),
    server = new TaskStore(join(directory, "s.sqlite"));
  approve(server);
  const task = server.claim("device", ["demo"]);
  const one = new WorkerStore(join(directory, "w.sqlite")),
    two = new WorkerStore(join(directory, "w.sqlite"));
  one.record(task);
  two.record(task);
  assert.equal(one.start(task.attemptId, "one"), true);
  assert.equal(two.start(task.attemptId, "two"), false);
  assert.throws(() => two.record({ ...task, specHash: "other" }));
  one.close();
  two.close();
  server.close();
});

test("服务端启动许可只发一次，心跳不消耗许可，重启后仍不可重领", () => {
  const file = join(root(), "s.sqlite");
  let store = new TaskStore(file);
  approve(store);
  const task = store.claim("device", ["demo"]);
  const identity = { deviceId: "device", attemptId: task.attemptId };
  store.heartbeat(task.id, { ...identity, state: "running" });
  assert.equal(store.get(task.id).state, "assigned");
  assert.equal(store.start(task.id, identity).granted, true);
  assert.equal(store.start(task.id, identity).granted, false);
  store.close();
  store = new TaskStore(file);
  assert.equal(store.start(task.id, identity).granted, false);
  assert.throws(() => store.start(task.id, { ...identity, attemptId: "old" }));
  store.close();
});

test("本机记录缺失而服务端已发启动许可时，禁止再启动执行器", async () => {
  const directory = root(),
    service = await startServer({
      database: join(directory, "s.sqlite"),
      port: 0,
      projects,
    });
  let starts = 0;
  const worker = new Worker({
    database: join(directory, "new-worker.sqlite"),
    url: service.url,
    deviceId: "device",
    projects,
    bridge: async () => {},
    execute: async () => {
      starts++;
      return result();
    },
  });
  try {
    approve(service.store);
    const task = service.store.claim("device", ["demo"]);
    service.store.start(task.id, {
      deviceId: "device",
      attemptId: task.attemptId,
    });
    await worker.tick();
    assert.equal(starts, 0);
    assert.equal(service.store.get(task.id).state, "recovery_required");
  } finally {
    worker.close();
    await service.close();
  }
});

test("HTTP 边界拒绝外部网页、错误 Host 和超长输入", async () => {
  const service = await startServer({
    database: join(root(), "s.sqlite"),
    port: 0,
    projects,
  });
  try {
    assert.equal(
      (
        await fetch(service.url + "/v1/tasks", {
          headers: { Origin: "https://evil.test" },
        })
      ).status,
      400,
    );
    const wrongHost = await new Promise((done, reject) => {
      const request = httpRequest(
        service.url + "/v1/tasks",
        { headers: { Host: "evil.test" } },
        (response) => {
          response.resume();
          response.on("end", () => done(response.statusCode));
        },
      );
      request.on("error", reject);
      request.end();
    });
    assert.equal(wrongHost, 400);
    assert.equal(
      (
        await fetch(service.url + "/v1/tasks", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ goal: "x".repeat(70000) }),
        })
      ).status,
      400,
    );
    const task = await client(service.url)("/v1/tasks", spec());
    assert.equal(task.state, "proposed");
  } finally {
    await service.close();
  }
});

test("回传成功但回执丢失：重启后只补传，不再执行", async () => {
  const directory = root(),
    service = await startServer({
      database: join(directory, "s.sqlite"),
      port: 0,
      projects,
    });
  let starts = 0;
  const bridge = async () => ({ ok: true });
  const options = {
    database: join(directory, "w.sqlite"),
    url: service.url,
    deviceId: "device",
    projects,
    bridge,
    execute: async () => {
      starts++;
      return result();
    },
  };
  let worker = new Worker(options);
  try {
    const task = approve(service.store);
    const real = worker.api;
    worker.api = async (path, input) => {
      const response = await real(path, input);
      if (path.endsWith("/result")) throw new Error("合成丢回执");
      return response;
    };
    await assert.rejects(worker.tick());
    assert.equal(service.store.get(task.id).state, "awaiting_review");
    worker.close();
    worker = new Worker(options);
    await worker.tick();
    await worker.tick();
    assert.equal(starts, 1);
    assert.equal(worker.store.pending().length, 0);
  } finally {
    worker.close();
    await service.close();
  }
});

test("完成回执丢失后先人工验收，补传不会再次占用或执行", async () => {
  const directory = root(),
    service = await startServer({
      database: join(directory, "s.sqlite"),
      port: 0,
      projects,
    });
  let bridgeCalls = 0;
  const worker = new Worker({
    database: join(directory, "w.sqlite"),
    url: service.url,
    deviceId: "device",
    projects,
    bridge: async () => {
      bridgeCalls++;
    },
    execute: async () => result(),
  });
  try {
    const task = approve(service.store),
      real = worker.api;
    worker.api = async (path, input) => {
      const response = await real(path, input);
      if (path.endsWith("/result")) throw new Error("丢回执");
      return response;
    };
    await assert.rejects(worker.tick());
    service.store.accept(task.id, service.store.get(task.id).resultHash);
    const before = bridgeCalls;
    worker.api = real;
    await worker.tick();
    assert.equal(bridgeCalls, before);
    assert.equal(worker.store.pending().length, 0);
  } finally {
    worker.close();
    await service.close();
  }
});

test("执行过程中服务断线，本地结果保留，服务恢复后补传", async () => {
  const directory = root(),
    database = join(directory, "s.sqlite");
  let service = await startServer({ database, port: 0, projects });
  const port = Number(new URL(service.url).port);
  let starts = 0;
  const worker = new Worker({
    database: join(directory, "w.sqlite"),
    url: service.url,
    deviceId: "device",
    projects,
    bridge: async () => {},
    execute: async () => {
      starts++;
      await service.close();
      return result();
    },
  });
  const task = approve(service.store);
  try {
    await assert.rejects(worker.tick());
    assert.equal(worker.store.pending()[0].state, "finished");
    service = await startServer({ database, port, projects });
    await worker.tick();
    assert.equal(service.store.get(task.id).state, "awaiting_review");
    assert.equal(starts, 1);
  } finally {
    worker.close();
    await service.close();
  }
});

test("启动前后崩溃留下不确定状态，只转待核实，禁止重跑", async () => {
  const directory = root(),
    service = await startServer({
      database: join(directory, "s.sqlite"),
      port: 0,
      projects,
    });
  let starts = 0;
  const worker = new Worker({
    database: join(directory, "w.sqlite"),
    url: service.url,
    deviceId: "device",
    projects,
    bridge: async () => {},
    execute: async () => {
      starts++;
      return result();
    },
  });
  try {
    approve(service.store);
    const task = service.store.claim("device", ["demo"]);
    worker.store.record(task);
    worker.store.start(task.attemptId, "dead-worker");
    await worker.tick();
    await worker.tick();
    assert.equal(starts, 0);
    assert.equal(service.store.get(task.id).state, "recovery_required");
    assert.equal(service.store.claim("other", ["demo"]), null);
  } finally {
    worker.close();
    await service.close();
  }
});

test("同机执行端实例锁阻止第二个运行进程", async () => {
  // Isolated test port; production uses the fixed 47322 across configurations.
  const net = await import("node:net");
  const probe = net.createServer();
  await new Promise((done) => probe.listen(0, "127.0.0.1", done));
  const port = probe.address().port;
  await new Promise((done) => probe.close(done));
  const unlock = await workerLock(port);
  try {
    await assert.rejects(workerLock(port));
  } finally {
    await unlock();
  }
});

test("DSH 经真实 ACP 子进程形成建议，并拒绝执行权限请求", async () => {
  const answer = await askDsh("fixture", root(), "规划任务", {
    timeout: 5000,
    command: {
      program: process.execPath,
      args: [join(fixtureRoot, "acp.mjs")],
      env: process.env,
    },
  });
  assert.match(answer, /等待用户确认/);
});

test("DSH 协调提交与结果汇总，重复请求不重复规划", async () => {
  const directory = root(),
    config = {
      projects: { demo: { repository: directory, checks: [] } },
      dsh: "fixture",
    };
  const service = await startServer({
    database: join(directory, "s.sqlite"),
    port: 0,
    projects: config.projects,
  });
  config.url = service.url;
  let calls = 0;
  const ask = async () => {
    calls++;
    return "DSH 生成的计划或汇总";
  };
  try {
    const task = await coordinate(config, spec(), ask);
    assert.equal((await coordinate(config, spec(), ask)).id, task.id);
    assert.equal(calls, 1);
    service.store.approve(task.id, task.specHash);
    const claimed = service.store.claim("device", ["demo"]);
    service.store.finish(task.id, {
      deviceId: "device",
      attemptId: claimed.attemptId,
      result: result(),
    });
    const notices = [];
    await watch(config, task.id, new AbortController().signal, ask, (message) =>
      notices.push(message),
    );
    assert.equal(calls, 2);
    assert.equal(service.store.get(task.id).summary, "DSH 生成的计划或汇总");
    assert.equal(notices.length, 2);
  } finally {
    await service.close();
  }
});

test("真实 Git 独立目录、执行器完成事件与验收结果绑定提交", async () => {
  const directory = root(),
    repo = join(directory, "repo"),
    work = join(directory, "work");
  mkdirSync(repo);
  mkdirSync(work);
  mkdirSync(join(repo, "src"));
  writeFileSync(join(repo, "src/value.txt"), "before\n");
  const git = async (args) => {
    const response = await run("git", args, { cwd: repo });
    assert.equal(response.code, 0);
    return response.output.trim();
  };
  await git(["init"]);
  await git(["add", "."]);
  await git([
    "-c",
    "user.name=Fixture",
    "-c",
    "user.email=fixture@localhost",
    "-c",
    "commit.gpgsign=false",
    "commit",
    "-m",
    "baseline",
  ]);
  const base = await git(["rev-parse", "HEAD"]);
  const task = {
    ...spec(),
    id: "task",
    attemptId: "attempt",
    baseCommit: base,
  };
  const bridge = async (_, action, request) => {
    assert.equal(action, "exec");
    const isAgent = request.binary === "fixture-agent";
    if (isAgent) {
      assert.equal(
        request.args[request.args.indexOf("--sandbox") + 1],
        "workspace-write",
      );
      if (process.platform === "win32")
        assert.ok(request.args.includes('windows.sandbox="unelevated"'));
    }
    const response = await run(
      isAgent ? process.execPath : request.binary,
      isAgent ? [join(fixtureRoot, "executor.mjs")] : request.args,
      { cwd: request.cwd, input: request.input },
    );
    return {
      ok: response.code === 0,
      output: response.output,
      reason: "NonZero",
    };
  };
  const config = {
    repository: repo,
    workspaceRoot: work,
    git: "git",
    codex: "fixture-agent",
    model: "deepseek-v4-flash",
    allowedPaths: ["src/"],
    checks: [
      {
        program: process.execPath,
        args: [
          "-e",
          "if(require('node:fs').readFileSync('src/value.txt','utf8')!=='done\\n')process.exit(1)",
        ],
      },
    ],
  };
  task.executionHash = projectFingerprint(config);
  const finished = await executeTask(task, config, bridge);
  assert.equal(finished.state, "awaiting_review");
  assert.notEqual(finished.commit, base);
  assert.deepEqual(finished.changedFiles, ["src/value.txt"]);
  assert.equal(readFileSync(join(repo, "src/value.txt"), "utf8"), "before\n");
  assert.ok(existsSync(join(work, "attempt/.git")));
  assert.ok(finished.checks[0].outputHash);
  await assert.rejects(executeTask(task, config, bridge), /已经存在/);
  const fail = {
    ...config,
    checks: [{ program: process.execPath, args: ["-e", "process.exit(1)"] }],
  };
  await assert.rejects(
    executeTask({ ...task, attemptId: "changed-config" }, fail, bridge),
    /不一致/,
  );
  await assert.rejects(
    executeTask(
      {
        ...task,
        attemptId: "failed-attempt",
        executionHash: projectFingerprint(fail),
      },
      fail,
      bridge,
    ),
    /未通过/,
  );
  const mutatingCheck = {
    ...config,
    checks: [
      {
        program: process.execPath,
        args: [
          "-e",
          "require('node:fs').writeFileSync('src/value.txt','unchecked')",
        ],
      },
    ],
  };
  await assert.rejects(
    executeTask(
      {
        ...task,
        attemptId: "mutating-check",
        executionHash: projectFingerprint(mutatingCheck),
      },
      mutatingCheck,
      bridge,
    ),
    /验收过程改变/,
  );
});
