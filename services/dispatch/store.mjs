import { DatabaseSync } from "node:sqlite";
import { createHash, randomUUID } from "node:crypto";
import { lstatSync } from "node:fs";

export function demand(ok, message = "请求不符合任务契约") {
  if (!ok) throw new Error(message);
}
export function identifier(value) {
  demand(typeof value === "string" && /^[a-zA-Z0-9_-]{1,80}$/.test(value));
  return value;
}
export function text(value, limit = 8000) {
  demand(
    typeof value === "string" &&
      value.trim().length > 0 &&
      value.length <= limit,
  );
  return value;
}
export function digest(value) {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}
export function projectFingerprint(config) {
  return digest(
    Object.fromEntries(
      [
        "repository",
        "workspaceRoot",
        "git",
        "codex",
        "model",
        "allowedPaths",
        "checks",
      ].map((key) => [key, config[key] ?? null]),
    ),
  );
}
export function openDatabase(file) {
  try {
    demand(
      lstatSync(file).isFile() && !lstatSync(file).isSymbolicLink(),
      "数据库必须是普通文件",
    );
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const db = new DatabaseSync(file);
  db.exec(
    "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;",
  );
  return db;
}
export function transaction(db, fn) {
  db.exec("BEGIN IMMEDIATE");
  try {
    const result = fn();
    db.exec("COMMIT");
    return result;
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}
export function proposal(input) {
  const result = {
    requestId: identifier(input.requestId),
    projectId: identifier(input.projectId),
    baseCommit: text(input.baseCommit, 40),
    goal: text(input.goal),
    scope: text(input.scope),
    plan: text(input.plan, 12000),
    coordinator: "dsh",
    executionHash: text(input.executionHash, 64),
  };
  demand(/^[0-9a-f]{40}$/.test(result.baseCommit), "需要固定的 40 位 Git 提交");
  demand(/^[0-9a-f]{64}$/.test(result.executionHash), "需要执行配置摘要");
  return result;
}

// A sticky assignment deliberately has no expiry/requeue transition. Missing
// heartbeats cannot prove the previous process stopped.
export class TaskStore {
  constructor(file) {
    this.db = openDatabase(file);
    const version = this.db.prepare("PRAGMA user_version").get().user_version;
    demand(version === 0 || version === 1, "不支持的任务数据库版本");
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS tasks (
        id TEXT PRIMARY KEY, request_id TEXT UNIQUE NOT NULL, spec TEXT NOT NULL,
        spec_hash TEXT NOT NULL, state TEXT NOT NULL, device TEXT, attempt TEXT,
        heartbeat INTEGER, result TEXT, result_hash TEXT, summary TEXT,
        created INTEGER NOT NULL, updated INTEGER NOT NULL
      ) STRICT;
      CREATE UNIQUE INDEX IF NOT EXISTS one_assignment ON tasks((1))
        WHERE state IN ('assigned','running','awaiting_review','failed','recovery_required');
      PRAGMA user_version=1;
    `);
  }
  close() {
    this.db.close();
  }
  view(row) {
    if (!row) return null;
    return {
      id: row.id,
      ...JSON.parse(row.spec),
      specHash: row.spec_hash,
      state: row.state,
      deviceId: row.device,
      attemptId: row.attempt,
      heartbeatAt: row.heartbeat,
      result: row.result ? JSON.parse(row.result) : null,
      resultHash: row.result_hash,
      summary: row.summary,
      createdAt: row.created,
      updatedAt: row.updated,
    };
  }
  get(id) {
    const task = this.view(
      this.db.prepare("SELECT * FROM tasks WHERE id=?").get(identifier(id)),
    );
    demand(task, "任务不存在");
    return task;
  }
  list() {
    return this.db
      .prepare("SELECT * FROM tasks ORDER BY created DESC LIMIT 100")
      .all()
      .map((r) => this.view(r));
  }
  byRequest(requestId) {
    return this.view(
      this.db
        .prepare("SELECT * FROM tasks WHERE request_id=?")
        .get(identifier(requestId)),
    );
  }
  submit(input) {
    const spec = proposal(input),
      hash = digest(spec);
    return transaction(this.db, () => {
      const existing = this.db
        .prepare("SELECT * FROM tasks WHERE request_id=?")
        .get(spec.requestId);
      if (existing) {
        demand(existing.spec_hash === hash, "同一请求编号不能覆盖不同内容");
        return this.view(existing);
      }
      const id = randomUUID(),
        now = Date.now();
      this.db
        .prepare(
          "INSERT INTO tasks(id,request_id,spec,spec_hash,state,created,updated) VALUES(?,?,?,?,?,?,?)",
        )
        .run(
          id,
          spec.requestId,
          JSON.stringify(spec),
          hash,
          "proposed",
          now,
          now,
        );
      return this.get(id);
    });
  }
  approve(id, hash) {
    return transaction(this.db, () => {
      const task = this.get(id);
      demand(task.specHash === hash, "任务建议已变化，请重新核对");
      demand(["proposed", "queued"].includes(task.state), "该任务不能批准开工");
      this.db
        .prepare("UPDATE tasks SET state='queued', updated=? WHERE id=?")
        .run(Date.now(), id);
      return this.get(id);
    });
  }
  claim(deviceId, projects) {
    identifier(deviceId);
    demand(
      Array.isArray(projects) && projects.length > 0 && projects.length <= 32,
    );
    projects.forEach(identifier);
    return transaction(this.db, () => {
      const active = this.db
        .prepare(
          "SELECT * FROM tasks WHERE attempt IS NOT NULL AND state NOT IN ('accepted','abandoned')",
        )
        .get();
      if (active) return active.device === deviceId ? this.view(active) : null;
      const queued = this.db
        .prepare("SELECT * FROM tasks WHERE state='queued' ORDER BY created")
        .all()
        .find((r) => projects.includes(JSON.parse(r.spec).projectId));
      if (!queued) return null;
      this.db
        .prepare(
          "UPDATE tasks SET state='assigned',device=?,attempt=?,heartbeat=?,updated=? WHERE id=?",
        )
        .run(deviceId, randomUUID(), Date.now(), Date.now(), queued.id);
      return this.get(queued.id);
    });
  }
  identify(task, input) {
    demand(
      task.deviceId === input.deviceId && task.attemptId === input.attemptId,
      "执行身份不匹配",
    );
  }
  heartbeat(id, input) {
    return transaction(this.db, () => {
      const task = this.get(id);
      this.identify(task, input);
      if (["assigned", "running"].includes(task.state)) {
        demand(["assigned", "running"].includes(input.state));
        this.db
          .prepare("UPDATE tasks SET heartbeat=?,updated=? WHERE id=?")
          .run(Date.now(), Date.now(), id);
      }
      return this.get(id);
    });
  }
  start(id, input) {
    return transaction(this.db, () => {
      const task = this.get(id);
      this.identify(task, input);
      if (task.state !== "assigned") return { granted: false };
      this.db
        .prepare(
          "UPDATE tasks SET state='running',heartbeat=?,updated=? WHERE id=?",
        )
        .run(Date.now(), Date.now(), id);
      return { granted: true };
    });
  }
  finish(id, input) {
    return transaction(this.db, () => {
      const task = this.get(id);
      this.identify(task, input);
      const result = input.result;
      demand(
        result &&
          ["awaiting_review", "failed", "recovery_required"].includes(
            result.state,
          ),
      );
      if (result.state === "awaiting_review") {
        demand(
          /^[0-9a-f]{40}$/.test(result.commit) &&
            Array.isArray(result.checks) &&
            result.checks.length > 0 &&
            result.checks.every((check) => check.passed === true),
          "缺少固定提交或通过的验收证据",
        );
      }
      demand(JSON.stringify(result).length <= 24000, "结果过长");
      const hash = digest(result);
      if (task.resultHash) {
        demand(task.resultHash === hash, "执行结果已经保存，不接受覆盖");
        return task;
      }
      demand(
        ["assigned", "running"].includes(task.state),
        "当前阶段不能回传结果",
      );
      this.db
        .prepare(
          "UPDATE tasks SET state=?,result=?,result_hash=?,updated=? WHERE id=?",
        )
        .run(result.state, JSON.stringify(result), hash, Date.now(), id);
      return this.get(id);
    });
  }
  accept(id, hash) {
    return transaction(this.db, () => {
      const task = this.get(id);
      demand(
        ["awaiting_review", "accepted"].includes(task.state),
        "只能验收已通过测试的任务",
      );
      demand(task.resultHash === hash, "验收必须绑定当前结果");
      this.db
        .prepare("UPDATE tasks SET state='accepted',updated=? WHERE id=?")
        .run(Date.now(), id);
      return this.get(id);
    });
  }
  summarize(id, hash, summary) {
    const task = this.get(id);
    demand(task.resultHash && task.resultHash === hash, "汇总必须绑定当前结果");
    this.db
      .prepare("UPDATE tasks SET summary=?,updated=? WHERE id=?")
      .run(text(summary, 12000), Date.now(), id);
    return this.get(id);
  }
  abandon(id, hash) {
    return transaction(this.db, () => {
      const task = this.get(id);
      demand(
        ["failed", "recovery_required", "abandoned"].includes(task.state),
        "只能放弃失败或待核实任务",
      );
      demand(
        task.resultHash && task.resultHash === hash,
        "放弃必须绑定当前结果",
      );
      this.db
        .prepare("UPDATE tasks SET state='abandoned',updated=? WHERE id=?")
        .run(Date.now(), id);
      return this.get(id);
    });
  }
}

export class WorkerStore {
  constructor(file) {
    this.db = openDatabase(file);
    const version = this.db.prepare("PRAGMA user_version").get().user_version;
    demand(version === 0 || version === 2, "不支持的执行端数据库版本");
    this.db.exec(`CREATE TABLE IF NOT EXISTS attempts (
      id TEXT PRIMARY KEY, task TEXT NOT NULL, state TEXT NOT NULL, owner TEXT,
      result TEXT, delivered INTEGER NOT NULL DEFAULT 0
    ) STRICT; PRAGMA user_version=2;`);
  }
  close() {
    this.db.close();
  }
  record(task) {
    transaction(this.db, () => {
      const row = this.db
        .prepare("SELECT * FROM attempts WHERE id=?")
        .get(task.attemptId);
      if (row) {
        const old = JSON.parse(row.task);
        demand(
          old.id === task.id &&
            old.specHash === task.specHash &&
            old.deviceId === task.deviceId,
          "本机执行记录冲突",
        );
      } else {
        this.db
          .prepare("INSERT INTO attempts(id,task,state) VALUES(?,?,'received')")
          .run(task.attemptId, JSON.stringify(task));
      }
    });
  }
  get(id) {
    return this.db.prepare("SELECT * FROM attempts WHERE id=?").get(id);
  }
  start(id, owner) {
    return (
      this.db
        .prepare(
          "UPDATE attempts SET state='launching',owner=? WHERE id=? AND state='received'",
        )
        .run(owner, id).changes === 1
    );
  }
  finish(id, result) {
    this.db
      .prepare(
        "UPDATE attempts SET state='finished',result=? WHERE id=? AND state='launching'",
      )
      .run(JSON.stringify(result), id);
  }
  pending() {
    return this.db
      .prepare("SELECT * FROM attempts WHERE delivered=0 ORDER BY rowid")
      .all();
  }
  delivered(id) {
    this.db.prepare("UPDATE attempts SET delivered=1 WHERE id=?").run(id);
  }
}
