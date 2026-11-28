import { randomUUID } from "node:crypto";
import { mkdirSync, lstatSync, existsSync } from "node:fs";
import { join, resolve, relative, isAbsolute } from "node:path";
import { createServer } from "node:net";
import { demand, digest, WorkerStore, projectFingerprint } from "./store.mjs";
import { client } from "./server.mjs";
import { nativeBridge, safeDirectory } from "./process.mjs";

export async function workerLock(port = 47322) {
  const server = createServer((socket) => socket.end());
  await new Promise((resolveReady, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolveReady);
  });
  return () => new Promise((done) => server.close(done));
}

export function validateProject(config) {
  const repository = safeDirectory(config.repository);
  const workspaceRoot = safeDirectory(config.workspaceRoot);
  const relation = relative(repository, workspaceRoot);
  demand(
    relation !== "" && (relation.startsWith("..") || isAbsolute(relation)),
    "执行目录不能位于源仓库内",
  );
  for (const path of [config.git, config.codex]) {
    demand(
      isAbsolute(path) && existsSync(path) && lstatSync(path).isFile(),
      "需要已安装的原生 Git/Codex 程序",
    );
    if (process.platform === "win32")
      demand(
        path.toLowerCase().endsWith(".exe"),
        "执行端只接受原生 exe，不能使用命令包装",
      );
  }
  demand(
    config.model === undefined ||
      (typeof config.model === "string" &&
        /^[a-zA-Z0-9_./-]{1,120}$/.test(config.model)),
    "执行模型名称无效",
  );
  demand(
    config.windowsSandbox === undefined ||
      ["unelevated", "elevated"].includes(config.windowsSandbox),
    "Windows 沙箱模式无效",
  );
  demand(
    Array.isArray(config.allowedPaths) && config.allowedPaths.length > 0,
    "需要登记允许修改的相对路径",
  );
  demand(
    config.allowedPaths.every(
      (p) =>
        typeof p === "string" &&
        p &&
        !p.includes("..") &&
        !p.includes("\\") &&
        !isAbsolute(p),
    ),
  );
  demand(
    Array.isArray(config.checks) &&
      config.checks.length > 0 &&
      config.checks.length <= 8,
    "需要固定的验收命令",
  );
  for (const check of config.checks) {
    demand(
      isAbsolute(check.program) &&
        existsSync(check.program) &&
        Array.isArray(check.args),
      "验收命令必须为本机已登记的绝对程序路径",
    );
    demand(check.args.every((a) => typeof a === "string"));
    if (process.platform === "win32")
      demand(check.program.toLowerCase().endsWith(".exe"));
  }
  return {
    ...config,
    repository,
    workspaceRoot,
    approvedHash: projectFingerprint(config),
  };
}

// Caller persists launch intent before invoking this function. The native
// companion owns Job Objects/process groups and the shared desktop task slot.
export async function executeTask(task, config, bridge) {
  demand(
    task.executionHash === (config.approvedHash || projectFingerprint(config)),
    "执行配置与已批准建议不一致，请重新确认",
  );
  const workspace = join(config.workspaceRoot, task.attemptId);
  const control = join(config.workspaceRoot, `${task.attemptId}-evidence`);
  demand(
    !existsSync(workspace) && !existsSync(control),
    "执行目录已经存在，需人工核实",
  );
  mkdirSync(control);
  const noHooks = join(control, "empty-hooks");
  mkdirSync(noHooks);
  let sequence = 0;
  const exec = async (program, args, cwd, input = "") => {
    const result = await bridge(task, "exec", {
      binary: program,
      args,
      cwd,
      input,
      identityPath: join(control, `process-${++sequence}.json`),
      timeoutSeconds: 300,
    });
    if (!result.ok) {
      const stage =
        program === config.codex
          ? "Codex"
          : program === config.git
            ? "Git"
            : "验收";
      const error = new Error(`${stage}执行未通过：${result.reason}`);
      error.uncertain = result.reason !== "NonZero";
      throw error;
    }
    return result.output;
  };
  const git = (args, cwd = workspace) =>
    exec(
      config.git,
      [
        "-c",
        `core.hooksPath=${noHooks}`,
        "-c",
        "core.fsmonitor=false",
        ...args,
      ],
      cwd,
    );
  await git(
    [
      "clone",
      "--no-hardlinks",
      "--no-checkout",
      "--",
      config.repository,
      workspace,
    ],
    config.workspaceRoot,
  );
  await git(["checkout", "--detach", task.baseCommit]);
  const before = (await git(["rev-parse", "HEAD"])).trim();
  demand(before === task.baseCommit, "基线提交不匹配");
  const prompt = [
    "你正在执行一个已由用户确认的 AgentTerm 任务。仅处理本任务，不发布、不推送、不修改全局配置。",
    `目标：${task.goal}`,
    `范围：${task.scope}`,
    `DSH 计划：${task.plan}`,
    `允许修改路径：${config.allowedPaths.join(", ")}。不得读写凭据。`,
    "在当前独立目录完成最小改动。不要启动遗留后台服务。返回完成情况；程序会独立运行验收。",
  ].join("\n");
  const output = await exec(
    config.codex,
    [
      "exec",
      "--json",
      "--ephemeral",
      ...(config.model && config.model !== "default"
        ? ["--model", config.model]
        : []),
      "--sandbox",
      "workspace-write",
      ...(process.platform === "win32"
        ? ["-c", `windows.sandbox="${config.windowsSandbox || "unelevated"}"`]
        : []),
      "-c",
      'approval_policy="never"',
      "-C",
      workspace,
      "-",
    ],
    workspace,
    prompt,
  );
  const events = output
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch {
        return null;
      }
    });
  demand(
    events.some((event) => event?.type === "turn.completed") &&
      !events.some(
        (event) => event?.type === "turn.failed" || event?.type === "error",
      ),
    "执行器未返回有效完成事件",
  );
  await git(["merge-base", "--is-ancestor", task.baseCommit, "HEAD"]);
  const changed = new Set(
    [
      ...(await git(["diff", "--name-only", "-z", task.baseCommit])).split(
        "\0",
      ),
      ...(
        await git(["ls-files", "--others", "--exclude-standard", "-z"])
      ).split("\0"),
    ].filter(Boolean),
  );
  for (const file of changed) {
    demand(
      config.allowedPaths.some(
        (p) => file === p || (p.endsWith("/") && file.startsWith(p)),
      ),
      "产物包含未授权路径",
    );
    demand(
      !/(^|\/)(\.env(?:\.|$)|.*\.(?:pem|key)$|credentials|auth\.json)/i.test(
        file,
      ),
      "产物包含不允许提交的文件类型",
    );
    const full = resolve(workspace, file);
    demand(!relative(workspace, full).startsWith(".."), "产物路径越界");
    if (existsSync(full))
      demand(
        !lstatSync(full).isSymbolicLink() && lstatSync(full).nlink === 1,
        "产物不能为链接",
      );
  }
  await git(["add", "--all"]);
  const staged = (await git(["diff", "--cached", "--name-only"])).trim();
  if (staged)
    await git([
      "-c",
      "user.name=AgentTerm",
      "-c",
      "user.email=agentterm@localhost",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "-m",
      `task: ${task.id}`,
    ]);
  const commit = (await git(["rev-parse", "HEAD"])).trim();
  demand(/^[0-9a-f]{40}$/.test(commit), "没有有效交付提交");
  const checks = [];
  for (const check of config.checks) {
    const result = await exec(check.program, check.args, workspace);
    checks.push({
      command: [check.program, ...check.args],
      passed: true,
      outputHash: digest(result),
    });
  }
  demand(
    (await git(["rev-parse", "HEAD"])).trim() === commit &&
      !(await git(["status", "--porcelain", "--untracked-files=all"])).trim(),
    "验收过程改变了交付内容，不能标记通过",
  );
  return {
    state: "awaiting_review",
    commit,
    baseCommit: task.baseCommit,
    executionHash: task.executionHash,
    workspace,
    checks,
    changedFiles: [...changed],
  };
}

export class Worker {
  constructor({
    database,
    url,
    deviceId,
    projects,
    bridge,
    execute = executeTask,
  }) {
    this.store = new WorkerStore(database);
    this.api = client(url);
    this.deviceId = deviceId;
    this.projects = projects;
    this.bridge = bridge;
    this.execute = execute;
    this.owner = randomUUID();
  }
  async deliver(row) {
    const task = JSON.parse(row.task),
      result = JSON.parse(row.result);
    const remote = await this.api(`/v1/tasks/${task.id}`);
    if (
      ["accepted", "abandoned"].includes(remote.state) &&
      remote.resultHash === digest(result)
    ) {
      this.store.delivered(row.id);
      return;
    }
    await this.bridge(task, "finish", { state: result.state });
    await this.api(`/v1/tasks/${task.id}/result`, {
      deviceId: task.deviceId,
      attemptId: task.attemptId,
      result,
    });
    this.store.delivered(row.id);
  }
  async tick() {
    // No new claim until all locally persisted results have been acknowledged.
    for (const row of this.store.pending()) {
      if (row.state === "finished") await this.deliver(row);
      else if (row.state === "launching") {
        this.store.finish(row.id, {
          state: "recovery_required",
          reason: "执行端中断；旧执行需人工核实，未重新启动",
        });
        await this.deliver(this.store.get(row.id));
        return;
      }
    }
    const { task } = await this.api("/v1/claim", {
      deviceId: this.deviceId,
      projects: Object.keys(this.projects),
    });
    if (!task || !["assigned", "running"].includes(task.state)) return;
    this.store.record(task);
    const row = this.store.get(task.attemptId);
    if (row.state === "finished") {
      if (!row.delivered) await this.deliver(row);
      return;
    }
    await this.bridge(task, "claim", {
      projectPath: this.projects[task.projectId].repository,
    });
    if (!this.store.start(task.attemptId, this.owner)) return;
    // A lost permit response is deliberately uncertain. Restart recovery does
    // not ask for a second permit or re-execute the task.
    const permit = await this.api(`/v1/tasks/${task.id}/start`, {
      deviceId: this.deviceId,
      attemptId: task.attemptId,
    });
    if (!permit.granted) {
      this.store.finish(task.attemptId, {
        state: "recovery_required",
        reason: "服务端启动许可已使用，需核实旧执行",
      });
      await this.deliver(this.store.get(task.attemptId));
      return;
    }
    let heartbeatBusy = false;
    const heartbeat = async () => {
      if (heartbeatBusy) return;
      heartbeatBusy = true;
      try {
        await this.api(`/v1/tasks/${task.id}/heartbeat`, {
          deviceId: this.deviceId,
          attemptId: task.attemptId,
          state: "running",
        });
      } catch {
        /* Offline execution retains ownership and local result. */
      } finally {
        heartbeatBusy = false;
      }
    };
    const timer = setInterval(heartbeat, 5000);
    let result;
    try {
      await heartbeat();
      result = await this.execute(
        task,
        this.projects[task.projectId],
        this.bridge,
      );
    } catch (error) {
      result = {
        state: error.uncertain ? "recovery_required" : "failed",
        reason: error.message,
      };
    } finally {
      clearInterval(timer);
    }
    this.store.finish(task.attemptId, result);
    await this.deliver(this.store.get(task.attemptId));
  }
  close() {
    this.store.close();
  }
}

export async function runWorker(config, database, signal) {
  const projects = Object.fromEntries(
    Object.entries(config.projects).map(([id, value]) => [
      id,
      validateProject(value),
    ]),
  );
  const bridge = nativeBridge(config.bridge);
  const unlock = await workerLock();
  let worker;
  try {
    worker = new Worker({
      database,
      url: config.url,
      deviceId: config.deviceId,
      projects,
      bridge,
    });
    let unavailable = false,
      delay = 3000;
    while (!signal.aborted) {
      try {
        await worker.tick();
        if (unavailable)
          process.stderr.write("任务服务已恢复，继续核对持久记录。\n");
        unavailable = false;
        delay = 3000;
      } catch {
        if (!unavailable)
          process.stderr.write(
            "任务服务暂不可用或任务占用待处理；保留记录，稍后核对。\n",
          );
        unavailable = true;
        delay = Math.min(delay * 2, 30000);
      }
      if (!signal.aborted) await new Promise((done) => setTimeout(done, delay));
    }
  } finally {
    worker?.close();
    await unlock();
  }
}
