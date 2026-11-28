// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import DshDispatchButton from "./DshDispatchButton";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
afterEach(() => {
  cleanup();
  invoke.mockReset();
});
it("opens the fixed local coordination entry and shows actionable service errors", async () => {
  invoke.mockRejectedValueOnce("请先启动本机 DSH 任务服务");
  render(<DshDispatchButton />);
  fireEvent.click(screen.getByRole("button", { name: "DSH 协调" }));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain("请先启动"),
  );
  expect(invoke).toHaveBeenCalledWith("open_dispatch_dashboard");
  invoke.mockResolvedValueOnce(undefined);
  fireEvent.click(screen.getByRole("button", { name: "DSH 协调" }));
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
});
