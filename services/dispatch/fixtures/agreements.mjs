export const documents = {
  interfaces:
    "API-1 GET /value → 200 {value:string}；失败返回 500 {error:string}。",
  tasks: "FE-1 展示 API-1 value，覆盖 AC-1；BE-1 返回 API-1 JSON，覆盖 AC-2。",
  acceptance:
    "AC-1 前端显示 value；AC-2 后端返回字符串 value；错误返回 error。",
};
export function confirmation(task) {
  return {
    specHash: task.specHash,
    revision: task.agreementRevision,
    agreementHash: task.agreement?.hash ?? null,
    generationId: task.generationId,
  };
}
export function saveDraft(
  store,
  task,
  operationId = "fixture-generation",
  value = documents,
) {
  const identity = { operationId, specHash: task.specHash };
  store.beginAgreement(task.id, identity);
  return store.saveAgreement(task.id, { ...identity, documents: value });
}
export function confirmAgreement(store, task) {
  store.approve(task.id, task.specHash);
  const ready = saveDraft(store, task);
  return store.approveAgreement(task.id, confirmation(ready));
}
