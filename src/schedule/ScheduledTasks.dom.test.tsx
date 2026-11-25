// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import ScheduledTasks from "./ScheduledTasks";

const { invoke, listen } = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(async () => () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));

afterEach(() => {
  cleanup();
  invoke.mockReset();
  listen.mockClear();
});

const task = {
  id: "task-1",
  name: "每日进展",
  projectPath: "E:\\AgentTerm",
  toolId: "codex",
  model: null,
  prompt: "汇总进展",
  intervalMinutes: 1440,
  schedule: { kind: "daily", hour: 9, minute: 0 },
  timeoutMinutes: 20,
  enabled: false,
  approvalPending: true,
  nextRunAtMs: Date.now() + 60_000,
  runs: [],
  sourceSessionId: "session-1",
};

describe("ScheduledTasks interactions", () => {
  it("requires explicit enable for an Agent-created task", async () => {
    invoke
      .mockResolvedValueOnce([task])
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce(null);
    const user = userEvent.setup();
    render(<ScheduledTasks onClose={vi.fn()} onNotice={vi.fn()} />);

    expect(await screen.findByText("待确认")).toBeTruthy();
    // A pending draft must not be runnable from the panel either; the backend
    // rejects it as well, so both layers agree.
    expect(
      (screen.getByRole("button", { name: "立即运行" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    await user.click(screen.getByRole("button", { name: "确认启用" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_scheduled_task_enabled", {
        id: "task-1",
        enabled: true,
      }),
    );
  });

  it("asks before permanently deleting a task", async () => {
    invoke
      .mockResolvedValueOnce([
        { ...task, enabled: true, approvalPending: false },
      ])
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce(null);
    const user = userEvent.setup();
    render(<ScheduledTasks onClose={vi.fn()} onNotice={vi.fn()} />);

    await user.click(await screen.findByRole("button", { name: "删除" }));
    expect(invoke).not.toHaveBeenCalledWith("remove_scheduled_task", {
      id: "task-1",
    });
    expect(screen.getByRole("alert").textContent).toContain("永久删除");
    await user.click(screen.getByRole("button", { name: "确认删除" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("remove_scheduled_task", {
        id: "task-1",
      }),
    );
  });

  it("reports refresh failure separately after a successful command", async () => {
    invoke
      .mockResolvedValueOnce([
        { ...task, enabled: true, approvalPending: false },
      ])
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("disk busy"));
    const user = userEvent.setup();
    render(<ScheduledTasks onClose={vi.fn()} onNotice={vi.fn()} />);

    await user.click(await screen.findByRole("button", { name: "暂停" }));
    expect(
      await screen.findByText(/操作已成功，但无法刷新任务列表/),
    ).toBeTruthy();
    expect(screen.queryByText(/定时任务操作失败/)).toBeNull();
  });

  it("offers a stop action while a run is in flight", async () => {
    invoke
      .mockResolvedValueOnce([
        { ...task, enabled: true, approvalPending: false },
      ])
      .mockResolvedValueOnce("task-1")
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce(null);
    const user = userEvent.setup();
    render(<ScheduledTasks onClose={vi.fn()} onNotice={vi.fn()} />);

    expect(await screen.findByText("正在运行…")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "停止运行" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("cancel_scheduled_task", {}),
    );
  });
});
