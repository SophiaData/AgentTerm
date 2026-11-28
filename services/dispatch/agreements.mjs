import { randomUUID } from "node:crypto";
import { existsSync, lstatSync } from "node:fs";
import { isAbsolute } from "node:path";
import { run, safeDirectory, standaloneEnvironment } from "./process.mjs";
import { agreementDocuments, demand } from "./store.mjs";

export async function askClaude(binary, cwd, prompt, invoke = run) {
  demand(
    typeof binary === "string" &&
      isAbsolute(binary) &&
      existsSync(binary) &&
      lstatSync(binary).isFile(),
    "请配置 Claude Code 原生程序路径",
  );
  if (process.platform === "win32")
    demand(binary.toLowerCase().endsWith(".exe"), "Claude Code 需要原生 exe");
  const result = await invoke(
    binary,
    [
      "--print",
      "--output-format",
      "json",
      "--json-schema",
      JSON.stringify({
        type: "object",
        additionalProperties: false,
        required: ["interfaces", "tasks", "acceptance"],
        properties: Object.fromEntries(
          ["interfaces", "tasks", "acceptance"].map((key) => [
            key,
            { type: "string", minLength: 1, maxLength: 6000 },
          ]),
        ),
      }),
      "--tools",
      "",
      "--strict-mcp-config",
      "--mcp-config",
      '{"mcpServers":{}}',
      "--safe-mode",
      "--disable-slash-commands",
      "--no-session-persistence",
      "--permission-mode",
      "dontAsk",
      "--settings",
      '{"disableAllHooks":true}',
      "--system-prompt",
      "你是接口约定编写者。只能根据输入生成约定，不调用工具，不修改文件，不执行任务。返回严格 JSON 对象，键为 interfaces、tasks、acceptance，值为中文 Markdown 字符串。未知业务细节列为待确认，不编造已完成或已测试。总输出不超过 16 KB。",
    ],
    {
      cwd,
      input: prompt,
      env: standaloneEnvironment(),
      timeout: 180000,
      limit: 128 * 1024,
    },
  );
  demand(result.code === 0, "Claude Code 未成功返回约定");
  let envelope, documents;
  try {
    envelope = JSON.parse(result.output);
    documents = envelope.structured_output ?? JSON.parse(envelope.result);
  } catch {
    throw new Error("Claude Code 未返回有效约定 JSON");
  }
  demand(
    envelope.type === "result" &&
      envelope.subtype === "success" &&
      !envelope.is_error,
    "Claude Code 未完成约定生成",
  );
  return agreementDocuments(documents);
}

// A durable one-shot generation claim prevents two watchers from generating
// simultaneously. Interrupted drafts wait for an explicit regeneration; an old
// generation cannot overwrite a replacement. No code executor runs here.
export async function generateAgreement(config, task, api, ask = askClaude) {
  const operationId = randomUUID();
  const identity = { operationId, specHash: task.specHash };
  const permit = await api(`/v1/tasks/${task.id}/agreement-begin`, identity);
  if (!permit.granted) return false;
  try {
    const project = config.projects[task.projectId];
    const documents = await ask(
      config.claude,
      safeDirectory(config.stateDirectory),
      [
        "按以下已确认方案输出三份约定。interfaces 写请求/响应、错误、字段和兼容规则；tasks 分前端/后端，引用接口及验收编号；acceptance 写可判定检查清单、失败条件。",
        "这是规划阶段，资料不足应明确标注待确认。不能把验收建议当作已经登记的可执行命令。",
        `固定基线：${task.baseCommit}\n目标：${task.goal}\n范围：${task.scope}\nDSH 拆分方案：${task.plan}`,
        `允许修改路径：${JSON.stringify(project.allowedPaths)}\n固定程序验收：${JSON.stringify(project.checks)}`,
        task.agreement
          ? `上一版约定（本次重新生成需重新确认）：${JSON.stringify(task.agreement.documents)}`
          : "",
      ].join("\n"),
    );
    await api(`/v1/tasks/${task.id}/agreement-save`, {
      ...identity,
      documents,
    });
    return true;
  } catch {
    // If saving succeeded but its response was lost, fail is an idempotent
    // no-op. If the service is offline, drafting remains persisted for review.
    await api(`/v1/tasks/${task.id}/agreement-fail`, identity).catch(() => {});
    return false;
  }
}
