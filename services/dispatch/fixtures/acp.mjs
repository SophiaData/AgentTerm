import { createInterface } from "node:readline";
const lines = createInterface({ input: process.stdin });
const send = (value) =>
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...value }) + "\n");
let waiting;
for await (const line of lines) {
  const request = JSON.parse(line);
  if (request.method === "initialize")
    send({ id: request.id, result: { protocolVersion: 1 } });
  if (request.method === "session/close") send({ id: request.id, result: {} });
  if (request.method === "session/new")
    send({
      id: request.id,
      result: {
        sessionId: "fixture",
        configOptions: [{ id: "model", currentValue: "deepseek-v4-flash" }],
      },
    });
  if (request.method === "session/prompt") {
    waiting = request.id;
    send({
      id: "permission",
      method: "session/request_permission",
      params: {},
    });
  }
  if (request.id === "permission") {
    if (request.result?.outcome?.outcome !== "cancelled") process.exit(2);
    send({
      method: "session/update",
      params: {
        sessionId: "fixture",
        update: {
          sessionUpdate: "agent_message_chunk",
          content: {
            type: "text",
            text: "建议：修改目标文件，然后运行固定验收；等待用户确认。",
          },
        },
      },
    });
    send({ id: waiting, result: { stopReason: "end_turn" } });
  }
}
