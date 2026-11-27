// @vitest-environment jsdom
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AcpSessionView from "./AcpSessionView";
import type { AcpSnapshot } from "./types";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (channel: string, handler: (event: { payload: unknown }) => void) => {
      mocks.listeners.set(channel, handler);
      return () => {
        mocks.listeners.delete(channel);
      };
    },
  ),
}));

const baseProps = {
  session: {
    id: "acp-session-1",
    toolId: "dsh",
    toolName: "DeepSeek Harness",
    projectPath: "E:\\AgentTerm",
  },
  visible: true,
  focused: true,
  focusRequest: 0,
  running: true,
  busy: false,
  onExit: vi.fn(),
  onConnection: vi.fn(),
  onPromptLine: vi.fn(),
  onOutputActivity: vi.fn(),
  onUserInput: vi.fn(),
};

beforeEach(() => {
  vi.clearAllMocks();
  mocks.listeners.clear();
});

afterEach(() => {
  cleanup();
});

describe("AcpSessionView", () => {
  it("loads snapshot, streams updates, sends prompt, handles permission, and cancels turn", async () => {
    const readySnapshot: AcpSnapshot = {
      phase: "ready",
      busy: false,
      cliSessionId: "sess_123",
      messages: [],
      toolCalls: [],
      configOptions: [
        {
          id: "model",
          name: "Model",
          category: "model",
          currentValue: '["deepseek-official","deepseek-v4-flash"]',
          choices: [
            {
              value: '["deepseek-official","deepseek-v4-flash"]',
              name: "deepseek-v4-flash",
              group: "DeepSeek",
            },
            {
              value: '["magpie","dsh-fe/deepseek-v4-flash"]',
              name: "DeepSeek V4 Flash · dsh-fe",
              group: "Magpie",
            },
          ],
        },
      ],
      version: 2,
    };
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "acp_snapshot") return readySnapshot;
      return undefined;
    });

    render(<AcpSessionView {...baseProps} />);
    await waitFor(() =>
      expect(baseProps.onConnection).toHaveBeenCalledWith(
        "acp-session-1",
        "ready",
        null,
      ),
    );

    const modelSelect = screen.getByLabelText("Model") as HTMLSelectElement;
    expect(modelSelect.querySelector('optgroup[label="Magpie"]')).toBeTruthy();
    expect(modelSelect.disabled).toBe(false);
    fireEvent.change(modelSelect, {
      target: { value: '["magpie","dsh-fe/deepseek-v4-flash"]' },
    });
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("acp_set_config_option", {
        sessionId: "acp-session-1",
        configId: "model",
        value: '["magpie","dsh-fe/deepseek-v4-flash"]',
      }),
    );

    const input = screen.getByLabelText("发送消息给 DeepSeek Harness");
    fireEvent.change(input, { target: { value: "检查构建脚本" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("acp_send_prompt", {
        sessionId: "acp-session-1",
        text: "检查构建脚本",
      }),
    );
    expect(baseProps.onUserInput).toHaveBeenCalledWith("acp-session-1");
    expect(baseProps.onPromptLine).toHaveBeenCalledWith(
      "acp-session-1",
      "检查构建脚本",
    );

    const emitState = mocks.listeners.get("acp-state-acp-session-1");
    expect(emitState).toBeTruthy();

    act(() => {
      emitState?.({
        payload: {
          phase: "ready",
          busy: true,
          cliSessionId: "sess_123",
          messages: [
            { id: "m1", role: "user", text: "检查构建脚本" },
            { id: "m2", role: "assistant", text: "正在查看 package.json…" },
          ],
          toolCalls: [
            {
              id: "call-1",
              title: "读取 package.json",
              status: "in_progress",
              detail: null,
            },
          ],
          pendingPermission: {
            requestId: "7",
            title: "允许运行 pnpm check？",
            description: "执行全量校验命令",
            options: [
              { optionId: "allow-once", name: "允许一次", kind: "allow_once" },
              { optionId: "reject-once", name: "拒绝", kind: "reject_once" },
            ],
          },
          version: 3,
        } satisfies AcpSnapshot,
      });
    });

    expect(screen.getByText("正在查看 package.json…")).toBeTruthy();
    expect(screen.getByText("读取 package.json")).toBeTruthy();
    expect(
      screen.getByRole("region", { name: "权限确认" }).textContent,
    ).toContain("允许运行 pnpm check？");
    expect(baseProps.onOutputActivity).toHaveBeenCalledWith(
      "acp-session-1",
      "acp-update",
    );

    fireEvent.click(screen.getByRole("button", { name: "允许一次" }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("acp_respond_permission", {
        sessionId: "acp-session-1",
        requestId: "7",
        optionId: "allow-once",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "停止本轮" }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("acp_cancel_turn", {
        sessionId: "acp-session-1",
      }),
    );

    const feed = document.querySelector(".acp-feed") as HTMLDivElement;
    Object.defineProperty(feed, "scrollHeight", {
      value: 500,
      configurable: true,
    });
    Object.defineProperty(feed, "clientHeight", {
      value: 200,
      configurable: true,
    });
    feed.scrollTop = 100;
    fireEvent.scroll(feed);
    fireEvent.click(screen.getByRole("button", { name: "回到底部" }));
    expect(feed.scrollTop).toBe(500);
  });

  it("pauses auto-scroll when scrolled up and disables config selects while busy", async () => {
    const readySnapshot: AcpSnapshot = {
      phase: "ready",
      busy: false,
      cliSessionId: "sess_scroll",
      messages: [{ id: "m1", role: "user", text: "第一句" }],
      toolCalls: [],
      configOptions: [
        {
          id: "model",
          name: "Model",
          category: "model",
          currentValue: '["magpie","dsh-fe/deepseek-v4-flash"]',
          choices: [
            {
              value: '["magpie","dsh-fe/deepseek-v4-flash"]',
              name: "DeepSeek V4 Flash · dsh-fe",
              group: "Magpie",
            },
          ],
        },
      ],
      version: 1,
    };
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "acp_snapshot") return readySnapshot;
      return undefined;
    });

    render(<AcpSessionView {...baseProps} />);
    const modelSelect = (await screen.findByLabelText(
      "Model",
    )) as HTMLSelectElement;
    expect(modelSelect.disabled).toBe(false);

    const feed = document.querySelector(".acp-feed") as HTMLDivElement;
    Object.defineProperty(feed, "scrollHeight", {
      value: 500,
      configurable: true,
    });
    Object.defineProperty(feed, "clientHeight", {
      value: 200,
      configurable: true,
    });
    feed.scrollTop = 100;
    fireEvent.scroll(feed);

    const emitState = mocks.listeners.get("acp-state-acp-session-1");
    expect(emitState).toBeTruthy();
    act(() => {
      emitState?.({
        payload: {
          ...readySnapshot,
          busy: true,
          messages: [
            ...readySnapshot.messages,
            { id: "m2", role: "assistant", text: "流式输出中" },
          ],
          version: 2,
        },
      });
    });

    expect(modelSelect.disabled).toBe(true);
    expect(feed.scrollTop).toBe(100);

    fireEvent.click(screen.getByRole("button", { name: "回到底部" }));
    expect(feed.scrollTop).toBe(500);

    act(() => {
      emitState?.({
        payload: {
          ...readySnapshot,
          busy: false,
          messages: [
            ...readySnapshot.messages,
            { id: "m2", role: "assistant", text: "流式输出完成" },
          ],
          version: 3,
        },
      });
    });
    expect(modelSelect.disabled).toBe(false);
    expect(feed.scrollTop).toBe(500);
  });

  it("renders TUI timeline, thought stream, tool input/output, markdown code, usage, Esc cancel, and prompt history", async () => {
    const richSnapshot: AcpSnapshot = {
      phase: "ready",
      busy: false,
      cliSessionId: "sess_tui",
      messages: [
        { id: "m1", role: "user", text: "第一条命令" },
        { id: "m2", role: "thought", text: "分析仓库结构" },
        {
          id: "m3",
          role: "assistant",
          text: "结果如下：\n```ts\nconst ok = true;\n```\n详见 `index.ts`",
        },
      ],
      toolCalls: [
        {
          id: "call-1",
          title: "read_file",
          status: "completed",
          input: '{"path":"index.ts"}',
          detail: "export const ok = true;",
        },
      ],
      timeline: [
        { kind: "message", id: "m1" },
        { kind: "message", id: "m2" },
        { kind: "tool", id: "call-1" },
        { kind: "message", id: "m3" },
      ],
      usage: { used: 12400, size: 1000000 },
      version: 1,
    };
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "acp_snapshot") return richSnapshot;
      return undefined;
    });

    render(<AcpSessionView {...baseProps} />);
    await waitFor(() =>
      expect(screen.getByLabelText("上下文占用").textContent).toBe(
        "上下文 12.4k / 1000k (1%)",
      ),
    );

    const feed = document.querySelector(".acp-feed") as HTMLDivElement;
    const childClasses = Array.from(feed.children).map((el) => el.className);
    expect(childClasses).toEqual([
      "acp-message is-user",
      "acp-thought-card",
      "acp-tool-card",
      "acp-message is-assistant",
    ]);
    expect(screen.getByText("const ok = true;")).toBeTruthy();
    expect(screen.getByText('{"path":"index.ts"}')).toBeTruthy();
    expect(screen.getByText("export const ok = true;")).toBeTruthy();

    const input = screen.getByLabelText(
      "发送消息给 DeepSeek Harness",
    ) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "检查版本" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("acp_send_prompt", {
        sessionId: "acp-session-1",
        text: "检查版本",
      }),
    );

    // ArrowUp recalls sent prompt when input cursor is at 0
    input.setSelectionRange(0, 0);
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input.value).toBe("检查版本");

    // ArrowDown returns to empty draft
    input.setSelectionRange(input.value.length, input.value.length);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input.value).toBe("");

    // Escape cancels active busy turn
    const emitState = mocks.listeners.get("acp-state-acp-session-1");
    act(() => {
      emitState?.({
        payload: {
          ...richSnapshot,
          busy: true,
          version: 2,
        },
      });
    });
    fireEvent.keyDown(input, { key: "Escape" });
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("acp_cancel_turn", {
        sessionId: "acp-session-1",
      }),
    );
  });
});
