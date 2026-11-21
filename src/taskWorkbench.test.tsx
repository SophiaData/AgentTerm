import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PlanForm, TaskCard } from "./TaskWorkbench";
import {
  acceptSnapshot,
  actionsBlocked,
  emptyPlan,
  latestAttempts,
  phaseMessages,
  requestOrder,
  planProblem,
  selectedProject,
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
  onResumeAfterRecovery: () => {},
  onRefresh: () => {},
  currentSession: null,
  onAssociateSession: () => {},
  onDispatch: () => {},
  onStop: () => {},
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
    expect(html).not.toContain("取消这项任务");
    expect(html).toContain("不代表主任务完成");
  });
  it("shows the durable relay phase while a controlled attempt is running", () => {
    const current = view();
    current.ledger.active!.phase = "verifying";
    current.document!.events = [end];
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={true} preview={null} {...callbacks} />,
    );
    expect(html).toContain("独立验收中");
    expect(html).toContain(phaseMessages.verifying);
    expect(html).toContain("结果已提交");
    expect(html).not.toContain("开始受控执行");
  });
  it("offers a stop request only while controlled execution is active", () => {
    const current = view();
    current.ledger.active!.phase = "running";
    const running = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(running).toContain("请求停止受控执行");
    expect(running).toContain("停止请求会先写入任务记录");

    current.ledger.active!.phase = "pausing";
    const pausing = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(pausing).not.toContain("请求停止受控执行");
    expect(pausing).toContain("正在暂停");
  });
  it("offers association only for the current session in the task project", () => {
    const current = view();
    const sameProject = {
      id: "session-1",
      toolId: "claude",
      toolName: "Claude Code",
      projectPath: "/project",
    };
    const sameProjectHtml = renderToStaticMarkup(
      <TaskCard
        view={current}
        busy={false}
        preview={null}
        {...callbacks}
        currentSession={sameProject}
      />,
    );
    expect(sameProjectHtml).toContain("尚未关联 CLI 会话");
    expect(sameProjectHtml).toContain("关联当前会话");

    current.document!.sessionLink = {
      id: "session-1",
      toolId: "claude",
      toolName: "Claude Code",
      projectPath: "/project",
    };
    const linkedHtml = renderToStaticMarkup(
      <TaskCard
        view={current}
        busy={false}
        preview={null}
        {...callbacks}
        currentSession={sameProject}
      />,
    );
    expect(linkedHtml).toContain("已关联 CLI 会话");
    expect(linkedHtml).not.toContain("关联当前会话");

    const otherProjectHtml = renderToStaticMarkup(
      <TaskCard
        view={view()}
        busy={false}
        preview={null}
        {...callbacks}
        currentSession={{
          ...sameProject,
          id: "session-2",
          projectPath: "/other",
        }}
      />,
    );
    expect(otherProjectHtml).not.toContain("关联当前会话");
  });
  it("offers controlled dispatch only after the linked Claude session and reservation match", () => {
    const current = view();
    current.ledger.active!.phase = "reserved";
    current.document!.sessionLink = {
      id: "session-1",
      toolId: "claude",
      toolName: "Claude Code",
      projectPath: "/project",
    };
    const linked = {
      id: "session-1",
      toolId: "claude",
      toolName: "Claude Code",
      projectPath: "/project",
    };
    const html = renderToStaticMarkup(
      <TaskCard
        view={current}
        busy={false}
        preview={null}
        {...callbacks}
        currentSession={linked}
      />,
    );
    expect(html).toContain("开始受控执行");
    expect(html).toContain("当前终端不会被接管");

    const codexHtml = renderToStaticMarkup(
      <TaskCard
        view={current}
        busy={false}
        preview={null}
        {...callbacks}
        currentSession={{ ...linked, toolId: "codex", toolName: "Codex" }}
      />,
    );
    expect(codexHtml).not.toContain("开始受控执行");
  });
  it("recovery keeps read access but hides reference and cancellation actions", () => {
    const current = view();
    current.ledger.active!.recoveryRequired = true;
    current.ledger.active!.phase = "reserved";
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).toContain("需要核实");
    expect(html).toContain("查看内容");
    expect(html).not.toContain("保存文件版本");
    expect(html).not.toContain("取消这项任务");
    expect(html).toContain("核实并接回任务");
    expect(html).toContain("结束本次安排");
    expect(html).not.toContain("核实并结束安排");
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
      expect(html).not.toContain("核实并接回任务");
      expect(html).not.toContain("结束本次安排");
    }
    const current = view();
    current.ledger.active!.phase = "reserved";
    current.ledger.active!.recoveryRequired = true;
    current.document!.events = [start];
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).not.toContain("核实并接回任务");
  });
  it("shows process evidence before offering a dispatched-task recovery", () => {
    const current = view();
    current.ledger.active!.recoveryRequired = true;
    current.ledger.active!.phase = "recoveryRequired";
    current.document!.events = [start];
    current.recovery = {
      attempts: [
        {
          attemptId: start.attemptId,
          stepId: start.stepId,
          observation: "alive",
          safeToResume: false,
        },
      ],
      allExited: false,
    };
    const blocked = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(blocked).toContain("仍在运行");
    expect(blocked).toContain("重新核对");
    expect(blocked).not.toContain("核实并接回任务");

    current.recovery = {
      attempts: [
        {
          ...current.recovery.attempts[0],
          observation: "exited",
          safeToResume: true,
        },
      ],
      allExited: true,
    };
    const ready = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(ready).toContain("已确认退出");
    expect(ready).toContain("核实并接回任务");
  });
  it("recovered reservations remain undispatched and can be cancelled by their owner", () => {
    const current = view();
    current.ledger.active!.phase = "recoveredReserved";
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).toContain("已接回任务");
    expect(html).toContain("取消这项任务");
    expect(html).not.toContain("核实并接回任务");
    expect(html).not.toContain("确认取消任务");
    expect(html).toContain("受控执行会按执行 Agent");
  });
  it("shows missing metadata and permits cancelling only an untouched reservation", () => {
    const current = view();
    current.document = null;
    current.ledger.active!.phase = "reserved";
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).toContain("尚无计划文档");
    expect(html).toContain("取消这项任务");
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
    const changedVersion = renderToStaticMarkup(
      <TaskCard
        {...props}
        preview={{
          id: "artifact-1",
          sha256: "old",
          content: "changed-version-content",
        }}
      />,
    );
    expect(changedVersion).not.toContain("changed-version-content");
  });
  it("auto-selects only an unambiguous project and never silently replaces a removed selection", () => {
    const projects = [{ id: "/one" }, { id: "/two" }];
    expect(selectedProject(emptyPlan(), projects)).toBe("");
    expect(selectedProject(emptyPlan(), projects.slice(0, 1))).toBe("/one");
    expect(
      selectedProject(
        { ...emptyPlan(), projectId: "/removed" },
        projects.slice(0, 1),
      ),
    ).toBe("");
    expect(selectedProject(emptyPlan(), [], "/active")).toBe("/active");
  });
  it("rejects whitespace-only plans without discarding the input", () => {
    const input = {
      ...emptyPlan(),
      projectId: "/one",
      goal: "核对数据",
      scope: "本地 SQL",
    };
    expect(planProblem(input)).toBeNull();
    expect(planProblem({ ...input, goal: "  \n" })).toContain("任务");
    expect(planProblem({ ...input, scope: "  " })).toContain("范围");
    expect(planProblem({ ...input, owner: "" })).toContain("负责人");
    expect(planProblem({ ...input, outputDir: "" })).toContain("目录");
    expect(planProblem({ ...input, projectId: "" })).toContain("项目");
    expect(input.goal).toBe("核对数据");
  });
  it("allows correction after a rejected action only when the latest state can be read", () => {
    expect(actionsBlocked(false, false, null)).toBe(false);
    expect(actionsBlocked(false, false, "读取失败")).toBe(true);
    expect(actionsBlocked(true, false, null)).toBe(true);
    expect(actionsBlocked(false, true, null)).toBe(true);
  });
  it("keeps creation understandable and project addition inside the form", () => {
    const html = renderToStaticMarkup(
      <PlanForm
        projects={[{ id: "/one", name: "数据项目", path: "/one" }]}
        busy={false}
        projectBusy={false}
        draft={{ ...emptyPlan(), goal: "尚未保存的目标" }}
        onDraftChange={() => {}}
        onAddProject={() => {}}
        onSubmit={() => {}}
      />,
    );
    expect(html).toContain('value="/one" selected=""');
    expect(html).toContain("尚未保存的目标");
    expect(html).toContain("添加项目");
    expect(html).toContain("保存任务计划");
    expect(html).toContain("自动执行入口尚未开放");
    expect(html).not.toContain("初始化本机任务账本");
    expect(html).not.toContain("预留一个主任务");
  });
  it("labels failed worker attempts without misreporting a review failure", () => {
    const current = view();
    current.document!.events = [
      { ...start, state: "failed", summary: "启动失败" },
    ];
    const html = renderToStaticMarkup(
      <TaskCard view={current} busy={false} preview={null} {...callbacks} />,
    );
    expect(html).toContain("此步骤未通过");
    expect(html).not.toContain("验收未通过");
  });
});
