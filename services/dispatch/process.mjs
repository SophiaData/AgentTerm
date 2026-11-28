import { spawn } from "node:child_process";
import { existsSync, realpathSync, lstatSync } from "node:fs";
import { dirname, resolve, join, isAbsolute } from "node:path";
import { demand } from "./store.mjs";

export function run(
  program,
  args,
  {
    cwd,
    input = "",
    env = process.env,
    timeout = 30000,
    limit = 1024 * 1024,
  } = {},
) {
  return new Promise((resolveResult, reject) => {
    const child = spawn(program, args, {
      cwd,
      env,
      windowsHide: true,
      stdio: ["pipe", "pipe", "ignore"],
      shell: false,
    });
    let output = "",
      settled = false;
    child.stdout.setEncoding("utf8");
    const end = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolveResult(value);
    };
    const timer = setTimeout(() => {
      child.kill();
      end(new Error("进程超时，需核实执行状态"));
    }, timeout);
    child.stdout.on("data", (chunk) => {
      output += chunk;
      if (Buffer.byteLength(output) > limit) {
        child.kill();
        end(new Error("进程输出超限"));
      }
    });
    child.stdin.on("error", () => {});
    child.once("error", () => end(new Error("无法启动指定程序")));
    child.once("close", (code) => end(null, { code, output }));
    child.stdin.end(input);
  });
}
export function nativeBridge(binary) {
  demand(
    isAbsolute(binary) && existsSync(binary),
    "请配置已构建的 agentterm-dispatch-bridge 路径",
  );
  return async (task, action, details = {}) => {
    try {
      const result = await run(binary, [], {
        input: JSON.stringify({
          action,
          taskId: task.id,
          attemptId: task.attemptId,
          ...details,
        }),
        timeout: ((details.timeoutSeconds || 300) + 15) * 1000,
      });
      demand(result.code === 0, "原生执行入口拒绝操作，请核对主任务占用与配置");
      return JSON.parse(result.output);
    } catch (error) {
      if (action === "exec") error.uncertain = true;
      throw error;
    }
  };
}
export function dshCommand(binary) {
  demand(isAbsolute(binary) && existsSync(binary), "DSH 可执行路径无效");
  if (process.platform === "win32" && binary.toLowerCase().endsWith(".cmd")) {
    const resources = resolve(dirname(binary), "../../..");
    const exe = resolve(resources, "../DeepSeek Harness.exe");
    const cli = join(
      resources,
      "app.asar/dsh/node_modules/@deepseek-ai/dsh-desktop-host/lib/cli.js",
    );
    demand(
      existsSync(exe) && existsSync(join(resources, "app.asar")),
      "无法解析 DSH Desktop 入口",
    );
    return {
      program: exe,
      args: ["--expose-internals", cli],
      env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" },
    };
  }
  demand(!/\.(cmd|bat|ps1)$/i.test(binary), "需要 DSH 原生程序");
  return { program: binary, args: [], env: process.env };
}
export function safeDirectory(path) {
  demand(
    isAbsolute(path) &&
      lstatSync(path).isDirectory() &&
      !lstatSync(path).isSymbolicLink(),
    "需要已有真实目录",
  );
  return realpathSync(path);
}

// Fresh ACP context for each coordination operation. Permission requests are
// refused: the coordinator proposes work; the worker executes it after approval.
export async function askDsh(
  binary,
  cwd,
  prompt,
  { timeout = 180000, command } = {},
) {
  const spec = command || dshCommand(binary);
  const child = spawn(spec.program, [...spec.args, "--profile", "acp"], {
    cwd,
    env: spec.env,
    windowsHide: true,
    stdio: ["pipe", "pipe", "ignore"],
    shell: false,
  });
  const pending = new Map();
  child.stdout.setEncoding("utf8");
  let seq = 0,
    buffer = "",
    answer = "",
    sessionId = null,
    bytes = 0;
  const send = (value) => child.stdin.write(JSON.stringify(value) + "\n");
  const request = (method, params) =>
    new Promise((resolveResult, reject) => {
      const id = ++seq;
      pending.set(id, { resolve: resolveResult, reject });
      send({ jsonrpc: "2.0", id, method, params });
    });
  const abort = (message) => {
    for (const item of pending.values()) item.reject(new Error(message));
    pending.clear();
  };
  const timer = setTimeout(() => {
    abort("DSH 协调超时");
    child.kill();
  }, timeout);
  child.stdin.on("error", () => abort("DSH 输入通道已关闭"));
  child.once("error", () => abort("DSH 启动失败"));
  child.once("exit", () => abort("DSH 提前退出"));
  child.stdout.on("data", (chunk) => {
    bytes += chunk.length;
    if (bytes > 2 * 1024 * 1024) {
      abort("DSH 输出超限");
      child.kill();
      return;
    }
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end);
      buffer = buffer.slice(end + 1);
      let value;
      try {
        value = JSON.parse(line);
      } catch {
        continue;
      }
      if (value.method === "session/request_permission") {
        send({
          jsonrpc: "2.0",
          id: value.id,
          result: { outcome: { outcome: "cancelled" } },
        });
      } else if (
        value.method === "session/update" &&
        value.params?.sessionId === sessionId
      ) {
        const update = value.params.update;
        if (
          update.sessionUpdate === "agent_message_chunk" &&
          update.content?.type === "text"
        )
          answer += update.content.text;
      } else if (value.method && value.id !== undefined) {
        send({
          jsonrpc: "2.0",
          id: value.id,
          error: { code: -32601, message: "Unsupported" },
        });
      } else if (pending.has(value.id)) {
        const item = pending.get(value.id);
        pending.delete(value.id);
        if (value.error) item.reject(new Error("DSH 协议请求失败"));
        else item.resolve(value.result);
      }
    }
  });
  try {
    await request("initialize", {
      protocolVersion: 1,
      clientCapabilities: {},
      clientInfo: { name: "agentterm-dispatch", version: "1" },
    });
    const opened = await request("session/new", { cwd, mcpServers: [] });
    sessionId = opened.sessionId;
    demand(typeof sessionId === "string", "DSH 未返回会话编号");
    const modelOption = opened.configOptions?.find(
      (o) => o.category === "model" || o.id === "model",
    );
    const currentModel =
      modelOption?.currentValue ?? opened.models?.currentModelId;
    demand(
      typeof currentModel === "string" && /deepseek/i.test(currentModel),
      "DSH 协调模型必须为 DeepSeek，请先在 DSH 中配置",
    );
    const result = await request("session/prompt", {
      sessionId,
      prompt: [{ type: "text", text: prompt }],
    });
    demand(
      result.stopReason === "end_turn" &&
        answer.trim() &&
        answer.length <= 12000,
      "DSH 未形成完整建议",
    );
    return answer.trim();
  } finally {
    let closeTimer;
    if (sessionId && !child.killed && child.exitCode === null) {
      try {
        await Promise.race([
          request("session/close", { sessionId }),
          new Promise((_, reject) => {
            closeTimer = setTimeout(
              () => reject(new Error("DSH 关闭超时")),
              3000,
            );
          }),
        ]);
      } catch {
        /* EOF asks the profile to drain remaining agents. */
      }
    }
    clearTimeout(closeTimer);
    clearTimeout(timer);
    child.stdin.end();
    if (child.exitCode === null) {
      await new Promise((done) => {
        const shutdown = setTimeout(() => {
          child.kill();
          done();
        }, 3000);
        child.once("exit", () => {
          clearTimeout(shutdown);
          done();
        });
      });
    }
  }
}
