const labels = {
  proposed: "等待你确认开工",
  planning: "方案已确认，等待生成约定（需运行 coordinate 或 watch）",
  drafting: "正在生成约定；中断后可重新生成",
  planning_failed: "约定生成未完成，等待处理",
  agreement_review: "约定已保存，等待你确认开工",
  queued: "排队等待本机领取",
  assigned: "本机已领取",
  running: "执行中",
  awaiting_review: "测试通过，等待你验收",
  failed: "执行或测试失败",
  recovery_required: "执行状态待核实，请勿重复开工",
  accepted: "已验收",
  abandoned: "已放弃，记录保留",
};
let busy = false;
let displayed = "";
const disconnected = (task) =>
  ["assigned", "running"].includes(task.state) &&
  Date.now() - task.heartbeatAt > 30000;
async function refresh() {
  if (busy) return;
  busy = true;
  try {
    const response = await fetch("/v1/tasks");
    if (!response.ok) throw new Error("无法读取任务");
    const { tasks } = await response.json();
    const version = JSON.stringify(
      tasks.map((task) => [
        task.id,
        task.specHash,
        task.state,
        task.resultHash,
        task.summary,
        task.agreementRevision,
        task.agreement?.hash,
        task.approvedAgreementHash,
        task.agreementError,
        task.generationId,
        disconnected(task),
      ]),
    );
    document.getElementById("error").textContent = "";
    if (displayed === version) return;
    displayed = version;
    const main = document.getElementById("tasks");
    main.replaceChildren();
    if (!tasks.length)
      main.textContent = "还没有任务。请让 DSH 提交第一份任务建议。";
    for (const task of tasks) {
      const card = document.createElement("article");
      const title = document.createElement("h2");
      title.textContent = task.goal;
      const status = document.createElement("p");
      status.textContent = labels[task.state] || task.state;
      if (task.state === "proposed" && task.workflow === "agreement-v1")
        status.textContent = "等待你确认前后端拆分方案";
      if (disconnected(task)) {
        status.textContent += " · 本机暂时失联，保留原任务，不重复派发";
      }
      const plan = document.createElement("pre");
      plan.textContent = task.plan;
      const identity = document.createElement("small");
      identity.textContent = `项目 ${task.projectId} · 基线 ${task.baseCommit.slice(0, 8)} · 任务 ${task.id}`;
      card.append(title, status, plan, identity);
      const showAgreement = (agreement, parent) => {
        const heading = document.createElement("h3");
        heading.textContent = `约定第 ${agreement.revision} 版 · ${agreement.hash}`;
        parent.append(heading);
        for (const [key, name] of [
          ["interfaces", "接口约定"],
          ["tasks", "前后端任务清单"],
          ["acceptance", "验收清单"],
        ]) {
          const title = document.createElement("h4");
          title.textContent = name;
          const content = document.createElement("pre");
          content.textContent = agreement.documents[key];
          parent.append(title, content);
        }
      };
      if (task.agreement) {
        showAgreement(task.agreement, card);
        if (["planning", "drafting", "planning_failed"].includes(task.state)) {
          const note = document.createElement("p");
          note.textContent = "以上为上一版约定；正在重新生成，原确认已失效。";
          card.append(note);
        }
        const history = document.createElement("details");
        const caption = document.createElement("summary");
        caption.textContent = "查看已保存版本";
        history.append(caption);
        let loaded = false;
        history.ontoggle = async () => {
          if (!history.open || loaded) return;
          loaded = true;
          try {
            const response = await fetch(`/v1/tasks/${task.id}/agreements`);
            if (!response.ok) throw new Error("无法读取约定历史");
            const { agreements } = await response.json();
            for (const agreement of agreements)
              showAgreement(agreement, history);
          } catch (error) {
            loaded = false;
            document.getElementById("error").textContent = error.message;
          }
        };
        card.append(history);
      }
      if (task.agreementError) {
        const error = document.createElement("p");
        error.textContent = task.agreementError;
        card.append(error);
      }
      if (task.summary) {
        const summaryTitle = document.createElement("h3");
        summaryTitle.textContent = "DSH 汇总";
        const summary = document.createElement("pre");
        summary.textContent = task.summary;
        card.append(summaryTitle, summary);
      }
      const resultTitle = document.createElement("h3");
      resultTitle.textContent = "原始执行结果";
      const result = document.createElement("pre");
      result.textContent = task.result
        ? JSON.stringify(task.result, null, 2)
        : "暂无执行结果。";
      card.append(resultTitle, result);
      const action =
        task.state === "proposed"
          ? "approve"
          : task.state === "agreement_review"
            ? "agreement-approve"
            : task.state === "awaiting_review"
              ? "accept"
              : ["failed", "recovery_required"].includes(task.state)
                ? "abandon"
                : null;
      if (action) {
        const button = document.createElement("button");
        button.textContent =
          action === "approve"
            ? task.workflow === "agreement-v1"
              ? "确认拆分方案，生成约定"
              : "确认方案并开工"
            : action === "agreement-approve"
              ? "确认三份约定并开工"
              : action === "accept"
                ? "确认结果并完成"
                : "核实停止并放弃（需先停 worker）";
        button.onclick = async () => {
          button.disabled = true;
          try {
            const res = await fetch(`/v1/tasks/${task.id}/${action}`, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify(
                action === "approve"
                  ? { specHash: task.specHash }
                  : action === "agreement-approve"
                    ? {
                        specHash: task.specHash,
                        agreementHash: task.agreement.hash,
                        revision: task.agreementRevision,
                      }
                    : { resultHash: task.resultHash },
              ),
            });
            const value = await res.json();
            if (!res.ok) throw new Error(value.error);
            await refresh();
          } catch (error) {
            document.getElementById("error").textContent = error.message;
          } finally {
            button.disabled = false;
          }
        };
        card.append(document.createElement("p"), button);
      }
      if (
        task.workflow === "agreement-v1" &&
        !task.attemptId &&
        ["drafting", "planning_failed", "agreement_review", "queued"].includes(
          task.state,
        )
      ) {
        const regenerate = document.createElement("button");
        regenerate.textContent = "重新生成约定（撤销开工确认）";
        regenerate.onclick = async () => {
          regenerate.disabled = true;
          try {
            const response = await fetch(
              `/v1/tasks/${task.id}/agreement-regenerate`,
              {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({
                  specHash: task.specHash,
                  revision: task.agreementRevision,
                  agreementHash: task.agreement?.hash ?? null,
                  generationId: task.generationId,
                }),
              },
            );
            const value = await response.json();
            if (!response.ok) throw new Error(value.error);
            await refresh();
          } catch (error) {
            document.getElementById("error").textContent = error.message;
          } finally {
            regenerate.disabled = false;
          }
        };
        card.append(regenerate);
      }
      main.append(card);
    }
    document.getElementById("error").textContent = "";
  } catch (error) {
    document.getElementById("error").textContent = error.message;
  } finally {
    busy = false;
  }
}
document.getElementById("refresh").onclick = refresh;
setInterval(refresh, 5000);
void refresh();
