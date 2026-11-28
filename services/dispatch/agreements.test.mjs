import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import {
  TaskStore,
  WorkerStore,
  digest,
  requireAgreement,
  projectFingerprint,
} from "./store.mjs";
import { startServer, client } from "./server.mjs";
import { watch } from "./coordinator.mjs";
import { askClaude, generateAgreement } from "./agreements.mjs";
import {
  documents,
  confirmation,
  saveDraft,
  confirmAgreement,
} from "./fixtures/agreements.mjs";
const root = () => mkdtempSync(join(tmpdir(), "agentterm-dispatch-test-"));
const spec = () => ({
  requestId: "agreement-task",
  projectId: "demo",
  baseCommit: "a".repeat(40),
  goal: "展示后端数据",
  scope: "src/",
  plan: "前端展示，后端提供 API",
  executionHash: "b".repeat(64),
});

test("两次确认门：未生成/未批准不能领取，旧 worker 不能领取新协议任务", () => {
  const store = new TaskStore(join(root(), "s.sqlite"));
  try {
    const task = store.submit(spec());
    assert.equal(store.claim("w", ["demo"], 2), null);
    assert.throws(() => store.approveAgreement(task.id, confirmation(task)));
    const planning = store.approve(task.id, task.specHash);
    assert.equal(planning.state, "planning");
    assert.equal(store.claim("w", ["demo"], 2), null);
    const ready = saveDraft(store, task);
    assert.equal(store.claim("w", ["demo"], 2), null);
    assert.throws(() => store.approve(task.id, task.specHash));
    assert.throws(() =>
      store.approveAgreement(task.id, {
        ...confirmation(ready),
        agreementHash: "old",
      }),
    );
    store.approveAgreement(task.id, confirmation(ready));
    assert.equal(store.claim("w", ["demo"]), null);
    const claimed = store.claim("w", ["demo"], 2);
    requireAgreement(claimed);
    assert.throws(() =>
      requireAgreement({ ...claimed, approvedAgreementHash: null }),
    );
    assert.throws(() =>
      requireAgreement({
        ...claimed,
        agreement: {
          ...claimed.agreement,
          documents: { ...documents, tasks: "changed" },
        },
      }),
    );
    assert.throws(() => store.start(task.id, claimed));
    assert.equal(
      store.start(task.id, {
        ...claimed,
        agreementHash: claimed.approvedAgreementHash,
      }).granted,
      true,
    );
    assert.throws(() =>
      store.finish(task.id, {
        ...claimed,
        result: {
          state: "awaiting_review",
          commit: "c".repeat(40),
          checks: [{ passed: true }],
        },
      }),
    );
  } finally {
    store.close();
  }
});

test("约定换版撤销确认，历史保留，旧响应和旧确认不能覆盖新版本", () => {
  const file = join(root(), "s.sqlite");
  let store = new TaskStore(file);
  const queued = confirmAgreement(store, store.submit(spec()));
  store.regenerateAgreement(queued.id, confirmation(queued));
  assert.equal(store.claim("w", ["demo"], 2), null);
  const next = saveDraft(store, queued, "second-generation", {
    ...documents,
    interfaces: "API-2 新协议",
  });
  assert.equal(next.agreementRevision, 2);
  assert.notEqual(next.agreement.hash, queued.agreement.hash);
  assert.throws(() => store.approveAgreement(next.id, confirmation(queued)));
  assert.throws(() =>
    store.saveAgreement(next.id, {
      operationId: "fixture-generation",
      specHash: next.specHash,
      documents,
    }),
  );
  store.close();
  store = new TaskStore(file);
  try {
    assert.equal(store.get(next.id).state, "agreement_review");
    assert.deepEqual(
      store.agreementHistory(next.id).map((a) => a.revision),
      [1, 2],
    );
    store.approveAgreement(next.id, confirmation(next));
    const assigned = store.claim("w", ["demo"], 2);
    assert.throws(() => store.regenerateAgreement(next.id, confirmation(next)));
    const local = new WorkerStore(join(root(), "w.sqlite"));
    try {
      local.record(assigned);
      assert.throws(() =>
        local.record({ ...assigned, approvedAgreementHash: "old" }),
      );
    } finally {
      local.close();
    }
  } finally {
    store.close();
  }
});

test("生成许可持久且一次性，中断重启保持暂停，手动重新生成隔离迟到结果", () => {
  const file = join(root(), "s.sqlite");
  let store = new TaskStore(file);
  const task = store.submit(spec());
  store.approve(task.id, task.specHash);
  const identity = { operationId: "first", specHash: task.specHash };
  assert.equal(store.beginAgreement(task.id, identity).granted, true);
  store.close();
  store = new TaskStore(file);
  try {
    assert.equal(store.get(task.id).state, "drafting");
    assert.equal(store.beginAgreement(task.id, identity).granted, false);
    assert.equal(store.claim("w", ["demo"], 2), null);
    store.regenerateAgreement(task.id, confirmation(store.get(task.id)));
    const ready = saveDraft(store, task, "second");
    assert.throws(() =>
      store.saveAgreement(task.id, { ...identity, documents }),
    );
    assert.equal(
      store.saveAgreement(task.id, {
        operationId: "second",
        specHash: task.specHash,
        documents,
      }).agreement.hash,
      ready.agreement.hash,
    );
    store.failAgreement(task.id, { operationId: "second" });
    assert.equal(store.get(task.id).state, "agreement_review");
  } finally {
    store.close();
  }
});

test("旧数据库升级保留历史/已分配任务，拒绝旧程序误开新数据库", () => {
  const file = join(root(), "legacy.sqlite"),
    db = new DatabaseSync(file);
  db.exec(
    `CREATE TABLE tasks(id TEXT PRIMARY KEY, request_id TEXT UNIQUE NOT NULL, spec TEXT NOT NULL, spec_hash TEXT NOT NULL,state TEXT NOT NULL,device TEXT,attempt TEXT,heartbeat INTEGER,result TEXT,result_hash TEXT,summary TEXT,created INTEGER NOT NULL,updated INTEGER NOT NULL) STRICT; PRAGMA user_version=1;`,
  );
  const input = spec();
  db.prepare(
    "INSERT INTO tasks(id,request_id,spec,spec_hash,state,device,attempt,created,updated) VALUES(?,?,?,?,?,?,?,?,?)",
  ).run(
    "old",
    input.requestId,
    JSON.stringify(input),
    digest(input),
    "assigned",
    "w",
    "old-attempt",
    1,
    1,
  );
  db.close();
  const store = new TaskStore(file);
  try {
    const old = store.claim("w", ["demo"]);
    assert.equal(old.attemptId, "old-attempt");
    assert.equal(old.agreement, null);
    assert.equal(store.start(old.id, old).granted, true);
    assert.equal(store.db.prepare("PRAGMA user_version").get().user_version, 3);
  } finally {
    store.close();
  }
});

test("HTTP 完整约定链路：重传只保存一次，生成失败不派发，恢复后重新确认", async () => {
  const directory = root(),
    config = {
      stateDirectory: directory,
      claude: "fixture",
      projects: {
        demo: { repository: directory, allowedPaths: ["src/"], checks: [] },
      },
    };
  const server = await startServer({
    database: join(directory, "s.sqlite"),
    port: 0,
    projects: config.projects,
  });
  const api = client(server.url);
  try {
    const task = await api("/v1/tasks", {
      ...spec(),
      executionHash: projectFingerprint(config.projects.demo),
    });
    await api(`/v1/tasks/${task.id}/approve`, { specHash: task.specHash });
    let calls = 0;
    const lossy = async (path, input) => {
      const value = await api(path, input);
      if (path.endsWith("agreement-save")) throw Error("lost");
      return value;
    };
    await generateAgreement(config, task, lossy, async () => {
      calls++;
      return documents;
    });
    await generateAgreement(config, task, api, async () => {
      calls++;
      return documents;
    });
    assert.equal(calls, 1);
    let current = await api(`/v1/tasks/${task.id}`);
    assert.equal(current.state, "agreement_review");
    assert.equal(
      (await api(`/v1/tasks/${task.id}/agreements`)).agreements.length,
      1,
    );
    await api(
      `/v1/tasks/${task.id}/agreement-regenerate`,
      confirmation(current),
    );
    await generateAgreement(config, current, api, async () => {
      throw Error("synthetic private error not exposed");
    });
    current = await api(`/v1/tasks/${task.id}`);
    assert.equal(current.state, "planning_failed");
    assert.doesNotMatch(current.agreementError, /private/);
    assert.equal(
      (
        await api("/v1/claim", {
          deviceId: "w",
          projects: ["demo"],
          protocolVersion: 2,
        })
      ).task,
      null,
    );
    await api(
      `/v1/tasks/${task.id}/agreement-regenerate`,
      confirmation(current),
    );
    await generateAgreement(config, current, api, async () => documents);
    current = await api(`/v1/tasks/${task.id}`);
    await api(`/v1/tasks/${task.id}/agreement-approve`, confirmation(current));
    assert.equal(
      (
        await api("/v1/claim", {
          deviceId: "w",
          projects: ["demo"],
          protocolVersion: 2,
        })
      ).task.agreementRevision,
      2,
    );
  } finally {
    await server.close();
  }
});

test("Claude 输出契约拒绝缺件/失败/过长，关闭工具、MCP和持久会话", async () => {
  const invoke = async (program, args, options) => {
    assert.equal(program, process.execPath);
    for (const flag of [
      "--safe-mode",
      "--json-schema",
      "--strict-mcp-config",
      "--no-session-persistence",
      "--disable-slash-commands",
    ])
      assert.ok(args.includes(flag));
    assert.equal(args[args.indexOf("--tools") + 1], "");
    assert.equal(options.input, "test");
    return {
      code: 0,
      output: JSON.stringify({
        type: "result",
        subtype: "success",
        is_error: false,
        result: JSON.stringify(documents),
      }),
    };
  };
  assert.deepEqual(
    await askClaude(process.execPath, root(), "test", invoke),
    documents,
  );
  assert.deepEqual(
    await askClaude(process.execPath, root(), "test", async () => ({
      code: 0,
      output: JSON.stringify({
        type: "result",
        subtype: "success",
        structured_output: documents,
      }),
    })),
    documents,
  );
  for (const payload of [
    {},
    { type: "result", subtype: "error", result: "{}" },
    {
      type: "result",
      subtype: "success",
      result: JSON.stringify({ ...documents, interfaces: "a".repeat(6001) }),
    },
  ]) {
    await assert.rejects(
      askClaude(process.execPath, root(), "test", async () => ({
        code: 0,
        output: JSON.stringify(payload),
      })),
    );
  }
});

test("恢复 watch 继续生成约定，重复 watcher 不覆盖已保存约定", async () => {
  const directory = root();
  const config = {
    stateDirectory: directory,
    projects: { demo: { repository: directory, checks: [] } },
  };
  const server = await startServer({
    database: join(directory, "s.sqlite"),
    port: 0,
    projects: config.projects,
  });
  config.url = server.url;
  try {
    const task = server.store.submit(spec());
    server.store.approve(task.id, task.specHash);
    const abort = new AbortController();
    let calls = 0;
    await watch(
      config,
      task.id,
      abort.signal,
      async () => {
        throw Error("unexpected DSH call");
      },
      () => {},
      async () => {
        calls++;
        abort.abort();
        return documents;
      },
    );
    assert.equal(calls, 1);
    assert.equal(server.store.get(task.id).state, "agreement_review");
    assert.equal(server.store.claim("w", ["demo"], 2), null);
  } finally {
    await server.close();
  }
});
