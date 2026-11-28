import { askDsh, safeDirectory } from "./process.mjs";
import { client } from "./server.mjs";
import { demand, identifier, text, projectFingerprint } from "./store.mjs";

export async function coordinate(config, input, ask = askDsh) {
  identifier(input.requestId);
  identifier(input.projectId);
  demand(Object.hasOwn(config.projects, input.projectId), "项目未登记");
  demand(/^[0-9a-f]{40}$/.test(input.baseCommit), "需要固定 Git 提交");
  text(input.goal);
  text(input.scope);
  const api = client(config.url);
  // A repeated submission reuses the stored proposal, even when DSH wording
  // would differ. Never create a new task ID merely because the reply was lost.
  const { task: existing } = await api(`/v1/requests/${input.requestId}`);
  if (existing) {
    demand(
      ["projectId", "baseCommit", "goal", "scope"].every(
        (key) => existing[key] === input[key],
      ),
      "请求编号已经用于其他内容",
    );
    return existing;
  }
  const project = config.projects[input.projectId];
  const plan = await ask(
    config.dsh,
    safeDirectory(project.repository),
    [
      "你是 AgentTerm 的 DSH 总协调者。当前仅制定一个任务的执行建议，不执行代码修改，不调用工具，不创建子代理。",
      "用中文给出简短步骤和验收要点。不要宣称已经完成或通过测试。用户确认后另一个执行器才会开工。",
      `项目：${input.projectId}；固定基线：${input.baseCommit}`,
      `目标：${input.goal}`,
      `范围：${input.scope}`,
      `本机登记的验收命令：${JSON.stringify(project.checks)}`,
    ].join("\n"),
  );
  return api("/v1/tasks", {
    ...input,
    plan,
    executionHash: projectFingerprint(project),
  });
}

export async function watch(
  config,
  taskId,
  signal,
  ask = askDsh,
  notify = (message) => process.stdout.write(message + "\n"),
) {
  identifier(taskId);
  const api = client(config.url);
  let last = "";
  while (!signal.aborted) {
    let task;
    try {
      task = await api(`/v1/tasks/${taskId}`);
    } catch {
      if (last !== "offline") notify("任务服务失联；任务保留，等待重连。");
      last = "offline";
      await new Promise((done) => setTimeout(done, 3000));
      continue;
    }
    const stale =
      ["assigned", "running"].includes(task.state) &&
      Date.now() - task.heartbeatAt > 30000;
    const state = stale ? "offline-worker" : task.state;
    if (state !== last) {
      notify(
        stale
          ? "执行端失联：保留原执行，不重复派发，请检查本机。"
          : `任务 ${task.id}：${task.state}`,
      );
      last = state;
    }
    if (task.resultHash) {
      if (!task.summary) {
        const summary = await ask(
          config.dsh,
          safeDirectory(config.projects[task.projectId].repository),
          [
            "你是 DSH 总协调者。只根据下面的程序证据用中文汇总，不调用工具、不修改代码。",
            "失败或待核实时明确通知用户，不能建议自动重跑未知执行。通过测试仅代表待人工验收。",
            `目标：${task.goal}`,
            `结果：${JSON.stringify(task.result)}`,
          ].join("\n"),
        );
        await api(`/v1/tasks/${task.id}/summary`, {
          resultHash: task.resultHash,
          summary,
        });
        notify(summary);
      } else notify(task.summary);
      return;
    }
    await new Promise((done) => setTimeout(done, 3000));
  }
}
