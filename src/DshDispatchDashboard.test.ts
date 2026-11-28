// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";

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
  document.body.replaceChildren();
});
