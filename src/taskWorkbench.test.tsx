import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { TaskCard } from "./TaskWorkbench";
import {
  acceptSnapshot,
  latestAttempts,
  requestOrder,
  type AttemptEvent,
  type WorkbenchView,
} from "./workbenchState";

function view(): WorkbenchView {
  return {
    ledger: {
      revision: 5,
      generation: 1,
      active: {
        taskId: "task-1",
        projectId: "/project",
        phase: "waitingInput",
        recoveryRequired: false,
      },
    },
    document: {
      revision: 4,
      plan: {
        taskId: "task-1",
        generation: 1,
        projectId: "/project",
        goal: "核对 SQL 草稿",
        owner: "数据工程师",
        scope: "仅本地分析",
        outputDir: "/project/reports",
        steps: [
          { id: "worker", title: "执行", assignee: "执行 Agent" },
          { id: "reviewer", title: "独立验收", assignee: "验收 Agent" },
        ],
      },
      events: [],
      artifacts: [
        {
          id: "artifact-1",
          relativePath: "model.sql",
          sha256: "abc123",
          bytes: 18,
          version: 1,
          label: "SQL 草稿",
        },
      ],
    },
  };
}
const start: AttemptEvent = {
  id: "1:2:0:start",
  sequence: 4,
  attemptId: "1:2:0",
  stepId: "worker",
  state: "running",
  summary: "开始",
};
const end: AttemptEvent = {
  ...start,
  id: "1:2:0:result",
  sequence: 5,
  state: "reported",
  summary: "结果已提交",
};
const callbacks = {
  onReference: () => {},
  onPreview: () => {},
  onCancel: () => {},
  onRecover: () => {},
};

describe("task workbench", () => {
  it("reduces duplicate/out-of-order events without regressing a completed attempt", () => {
    expect(latestAttempts([end, start, end, start])).toEqual([end]);
    const second = {
      ...start,
      id: "1:3:1:start",
      sequence: 6,
      attemptId: "1:3:1",
      stepId: "reviewer",
    };
    expect(latestAttempts([second, end, start])).toEqual([end, second]);
  });
  it("rejects older ledger and document snapshots", () => {
    const current = view();
    const olderLedger = view();
    olderLedger.ledger.revision = 4;
    expect(acceptSnapshot(current, olderLedger)).toBe(current);
    const olderDocument = view();
    olderDocument.document!.revision = 2;
    expect(acceptSnapshot(current, olderDocument)).toBe(current);
    const newer = view();
    newer.document!.revision = 5;
    expect(acceptSnapshot(current, newer)).toBe(newer);
  });
  it("accepts a newer task and discards old request completions", () => {
    const order = requestOrder();
    const first = order.next();
    const second = order.next();
    expect(order.current(first)).toBe(false);
    expect(order.current(second)).toBe(true);
    const incoming = view();
    incoming.ledger.revision = 8;
    incoming.ledger.generation = 2;
    expect(acceptSnapshot(view(), incoming)).toBe(incoming);
  });
  it("renders goal, owner, scope, version and distinct main/attempt states", () => {
    const current = view();
    current.document!.events = [end, start, end];
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    for (const text of [
      "核对 SQL 草稿",
      "数据工程师",
      "仅本地分析",
      "等待确认",
      "结果已提交",
      "SHA-256",
      "v1",
      "model.sql",
    ]) {
      expect(html).toContain(text);
    }
    expect(html.match(/尝试 /g)).toHaveLength(1);
    expect(html).not.toContain("取消未派发预留");
    expect(html).toContain("不代表主任务完成");
  });
  it("recovery keeps read access but hides reference and cancellation actions", () => {
    const current = view();
    current.ledger.active!.recoveryRequired = true;
    current.ledger.active!.phase = "reserved";
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).toContain("待核实");
    expect(html).toContain("查看此版本");
    expect(html).not.toContain("登记版本");
    expect(html).not.toContain("取消未派发预留");
    expect(html).toContain("核实并接管预留");
    expect(html).toContain("核实并释放未派发预留");
  });
  it("never offers reservation recovery for dispatched or unknown work", () => {
    for (const phase of [
      "running",
      "pausing",
      "recoveryRequired",
      "unexpected",
    ]) {
      const current = view();
      current.ledger.active!.recoveryRequired = true;
      current.ledger.active!.phase = phase;
      const html = renderToStaticMarkup(
        <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
      );
      expect(html).not.toContain("核实并接管预留");
      expect(html).not.toContain("核实并释放未派发预留");
    }
    const current = view();
    current.ledger.active!.phase = "reserved";
    current.ledger.active!.recoveryRequired = true;
    current.document!.events = [start];
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).not.toContain("核实并接管预留");
  });
  it("recovered reservations remain undispatched and can be cancelled by their owner", () => {
    const current = view();
    current.ledger.active!.phase = "recoveredReserved";
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).toContain("已接管预留");
    expect(html).toContain("取消未派发预留");
    expect(html).not.toContain("核实并接管预留");
  });
  it("shows missing metadata and permits cancelling only an untouched reservation", () => {
    const current = view();
    current.document = null;
    current.ledger.active!.phase = "reserved";
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).toContain("尚无计划文档");
    expect(html).toContain("取消未派发预留");
  });
  it("escapes report/file content and never renders a preview for another task", () => {
    const current = view();
    current.document!.events = [
      { ...end, summary: "<script>alert(1)</script>" },
    ];
    const props = { view: current, busy: false, ...callbacks };
    const html = renderToStaticMarkup(
      <TaskCard
        {...props}
        preview={{
          id: "artifact-1",
          sha256: "abc123",
          content: "<img src=x onerror=alert(1)>",
        }}
      />,
    );
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain("&lt;img");
    expect(html).not.toContain("<script>");
    const stale = renderToStaticMarkup(
      <TaskCard
        {...props}
        preview={{
          id: "old-task",
          sha256: "abc123",
          content: "old-private-content",
        }}
      />,
    );
    expect(stale).not.toContain("old-private-content");
  });
});
