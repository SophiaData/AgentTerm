const labels = {
  proposed: "等待你确认开工",
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
async function refresh() {
  if (busy) return;
  busy = true;
  try {
    const response = await fetch("/v1/tasks");
    if (!response.ok) throw new Error("无法读取任务");
    const { tasks } = await response.json();
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
      if (
        ["assigned", "running"].includes(task.state) &&
        Date.now() - task.heartbeatAt > 30000
      ) {
        status.textContent += " · 本机暂时失联，保留原任务，不重复派发";
      }
      const plan = document.createElement("pre");
      plan.textContent = task.plan;
      const identity = document.createElement("small");
      identity.textContent = `项目 ${task.projectId} · 基线 ${task.baseCommit.slice(0, 8)} · 任务 ${task.id}`;
      card.append(title, status, plan, identity);
      if (task.result) {
        const result = document.createElement("pre");
        result.textContent =
          task.summary || JSON.stringify(task.result, null, 2);
        card.append(result);
      }
      const action =
        task.state === "proposed"
          ? "approve"
          : task.state === "awaiting_review"
            ? "accept"
            : ["failed", "recovery_required"].includes(task.state)
              ? "abandon"
              : null;
      if (action) {
        const button = document.createElement("button");
        button.textContent =
          action === "approve"
            ? "确认方案并开工"
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
