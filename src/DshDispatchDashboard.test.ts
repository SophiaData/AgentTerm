// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

afterEach(() => document.body.replaceChildren());

it("preserves focused approval controls across polling and sends the displayed proposal hash", async () => {
  document.body.innerHTML =
    '<button id="refresh">刷新</button><p id="error"></p><main id="tasks"></main>';
  const task = {
    id: "fixture",
    specHash: "approved-version",
    state: "proposed",
    goal: "合成任务",
    plan: "计划",
    projectId: "demo",
    baseCommit: "a".repeat(40),
  };
  const fetch = vi.fn(async (path: string, options?: { body: string }) => {
    if (path.endsWith("/approve")) {
      expect(JSON.parse(options!.body)).toEqual({
        specHash: "approved-version",
      });
      task.state = "queued";
    }
    return {
      ok: true,
      json: async () => (path === "/v1/tasks" ? { tasks: [{ ...task }] } : {}),
    };
  });
  let poll: () => Promise<void> = async () => {};
  runInNewContext(readFileSync("services/dispatch/dashboard.js", "utf8"), {
    document,
    fetch,
    setInterval: (callback: () => Promise<void>) => {
      poll = callback;
    },
  });
  await waitFor(() =>
    expect(document.querySelector("article button")).not.toBeNull(),
  );
  const button = document.querySelector<HTMLButtonElement>("article button")!;
  button.focus();
  await poll();
  await poll();
  expect(document.querySelector("article button")).toBe(button);
  expect(document.activeElement).toBe(button);
  button.click();
  await waitFor(() =>
    expect(document.querySelector("article p")?.textContent).toBe(
      "排队等待本机领取",
    ),
  );
  expect(
    fetch.mock.calls.filter(([path]) => path.endsWith("/approve")),
  ).toHaveLength(1);
});

const completedResult = {
  state: "awaiting_review",
  commit: "b".repeat(40),
  workspace: "E:/pilot/workspace-evidence",
  checks: [
    {
      command: ["node", "fixed-check.mjs"],
      passed: true,
      outputHash: "check-output-digest",
    },
  ],
};
const failedResult = {
  state: "failed",
  reason: '验收命令失败 <img src=x onerror="alert(1)">',
};

async function renderResultTask(task: {
  state: string;
  summary?: string | null;
  result?: Record<string, unknown> | null;
  resultHash?: string;
}) {
  document.body.innerHTML =
    '<button id="refresh">刷新</button><p id="error"></p><main id="tasks"></main>';
  const fetch = vi.fn(async () => ({
    ok: true,
    json: async () => ({
      tasks: [
        {
          id: "fixture",
          specHash: "proposal-version",
          goal: "合成任务",
          plan: "计划",
          projectId: "demo",
          baseCommit: "a".repeat(40),
          ...task,
        },
      ],
    }),
  }));
  let poll: () => Promise<void> = async () => {};
  runInNewContext(readFileSync("services/dispatch/dashboard.js", "utf8"), {
    document,
    fetch,
    setInterval: (callback: () => Promise<void>) => {
      poll = callback;
    },
  });
  await waitFor(() => expect(document.querySelector("article")).not.toBeNull());
  return { fetch, poll };
}

it.each([
  {
    name: "summary and complete delivery evidence",
    state: "awaiting_review",
    status: "测试通过，等待你验收",
    summary: 'DSH 汇总 <img src=x onerror="alert(1)">',
    result: completedResult,
  },
  {
    name: "delivery evidence without a summary",
    state: "awaiting_review",
    status: "测试通过，等待你验收",
    summary: null,
    result: completedResult,
  },
  {
    name: "failure details alongside a summary",
    state: "failed",
    status: "执行或测试失败",
    summary: "DSH：失败待处理",
    result: failedResult,
  },
  {
    name: "failure details without a summary",
    state: "failed",
    status: "执行或测试失败",
    summary: "",
    result: failedResult,
  },
  {
    name: "a placeholder for a null result",
    state: "running",
    status: "执行中",
    summary: null,
    result: null,
  },
  {
    name: "a summary and placeholder for a missing result",
    state: "running",
    status: "执行中",
    summary: "DSH：等待执行结果",
    result: undefined,
  },
])("shows $name as separate plain-text blocks", async (task) => {
  await renderResultTask(task);
  expect(
    Array.from(
      document.querySelectorAll("article h3"),
      (node) => node.textContent,
    ),
  ).toEqual([...(task.summary ? ["DSH 汇总"] : []), "原始执行结果"]);
  expect(
    Array.from(
      document.querySelectorAll("article pre"),
      (node) => node.textContent,
    ),
  ).toEqual([
    "计划",
    ...(task.summary ? [task.summary] : []),
    task.result ? JSON.stringify(task.result, null, 2) : "暂无执行结果。",
  ]);
  expect(document.querySelector("article img")).toBeNull();
  expect(document.querySelector("article p")?.textContent).toBe(task.status);
  expect(document.querySelector("article button")?.textContent).toBe(
    task.state === "awaiting_review"
      ? "确认结果并完成"
      : task.state === "failed"
        ? "核实停止并放弃（需先停 worker）"
        : undefined,
  );
});

it("keeps acceptance bound to the displayed result hash after summary and result updates", async () => {
  const task = {
    state: "awaiting_review",
    summary: null as string | null,
    result: completedResult,
    resultHash: "first-result-version",
  };
  const { fetch, poll } = await renderResultTask(task);
  task.summary = "DSH：交付完成，等待验收";
  await poll();
  expect(document.querySelector("article")?.textContent).toContain(task.summary);
  expect(document.querySelector("article pre:last-of-type")?.textContent).toBe(
    JSON.stringify(completedResult, null, 2),
  );

  task.result = { ...completedResult, commit: "c".repeat(40) };
  task.resultHash = "displayed-result-version";
  await poll();
  expect(document.querySelector("article pre:last-of-type")?.textContent).toBe(
    JSON.stringify(task.result, null, 2),
  );
  const button = document.querySelector<HTMLButtonElement>("article button")!;
  button.focus();
  await poll();
  expect(document.querySelector("article button")).toBe(button);
  expect(document.activeElement).toBe(button);
  button.click();
  await waitFor(() =>
    expect(fetch).toHaveBeenCalledWith("/v1/tasks/fixture/accept", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ resultHash: "displayed-result-version" }),
    }),
  );
});
