import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { demand, TaskStore, projectFingerprint } from "./store.mjs";

export async function body(request) {
  demand(
    request.headers["content-type"] === "application/json",
    "需要 JSON 请求",
  );
  let buffer = "";
  request.setEncoding("utf8");
  for await (const chunk of request) {
    buffer += chunk;
    demand(Buffer.byteLength(buffer) <= 65536, "请求过长");
  }
  return JSON.parse(buffer);
}
export async function startServer({
  database,
  port = 47321,
  projects,
  accept = async () => {},
  abandon = async () => {
    throw new Error("未配置原生停止核实入口");
  },
}) {
  const store = new TaskStore(database);
  const server = createServer(async (request, response) => {
    const json = (code, value) => {
      response.writeHead(code, {
        "Content-Type": "application/json; charset=utf-8",
        "Cache-Control": "no-store",
      });
      response.end(JSON.stringify(value));
    };
    try {
      const authority = `127.0.0.1:${server.address().port}`;
      demand(request.headers.host === authority, "拒绝外部 Host");
      demand(
        !request.headers.origin ||
          request.headers.origin === `http://${authority}`,
        "拒绝外部网页请求",
      );
      if (
        request.method === "GET" &&
        ["/", "/dashboard.js"].includes(request.url)
      ) {
        const script = request.url.endsWith(".js");
        response.writeHead(200, {
          "Content-Type": script
            ? "text/javascript; charset=utf-8"
            : "text/html; charset=utf-8",
          "Content-Security-Policy":
            "default-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'",
          "Cache-Control": "no-store",
          "X-Content-Type-Options": "nosniff",
        });
        response.end(
          readFileSync(
            new URL(
              script ? "./dashboard.js" : "./dashboard.html",
              import.meta.url,
            ),
          ),
        );
        return;
      }
      if (request.method === "GET" && request.url === "/v1/tasks")
        return json(200, { tasks: store.list() });
      if (request.method === "GET" && request.url === "/v1/projects")
        return json(200, { projects: Object.keys(projects) });
      if (request.method === "GET" && request.url.startsWith("/v1/requests/"))
        return json(200, {
          task: store.byRequest(request.url.slice("/v1/requests/".length)),
        });
      if (request.method === "POST" && request.url === "/v1/tasks") {
        const spec = await body(request);
        demand(Object.hasOwn(projects, spec.projectId), "项目未在执行端登记");
        demand(
          spec.executionHash === projectFingerprint(projects[spec.projectId]),
          "执行配置已变化，请重新生成建议",
        );
        return json(200, store.submit(spec));
      }
      if (request.method === "POST" && request.url === "/v1/claim") {
        const input = await body(request);
        return json(200, { task: store.claim(input.deviceId, input.projects) });
      }
      const match =
        /^\/v1\/tasks\/([a-zA-Z0-9_-]+)(?:\/(approve|start|heartbeat|result|accept|summary|abandon))?$/.exec(
          request.url,
        );
      demand(match, "接口不存在");
      const [, id, action] = match;
      if (request.method === "GET" && !action) return json(200, store.get(id));
      demand(request.method === "POST" && action, "接口不存在");
      const input = await body(request);
      if (action === "approve")
        return json(200, store.approve(id, input.specHash));
      if (action === "start") return json(200, store.start(id, input));
      if (action === "heartbeat") return json(200, store.heartbeat(id, input));
      if (action === "result") return json(200, store.finish(id, input));
      if (action === "summary")
        return json(200, store.summarize(id, input.resultHash, input.summary));
      if (action === "accept") {
        const task = store.get(id);
        demand(task.state === "awaiting_review" || task.state === "accepted");
        demand(task.resultHash === input.resultHash, "验收版本不匹配");
        if (task.state !== "accepted") await accept(task);
        return json(200, store.accept(id, input.resultHash));
      }
      if (action === "abandon") {
        const task = store.get(id);
        demand(
          ["failed", "recovery_required", "abandoned"].includes(task.state),
        );
        demand(
          task.resultHash && task.resultHash === input.resultHash,
          "放弃版本不匹配",
        );
        if (task.state !== "abandoned") await abandon(task);
        return json(200, store.abandon(id, input.resultHash));
      }
    } catch (error) {
      // No request bodies, environment or child output in the service log.
      json(400, {
        error: error instanceof SyntaxError ? "无效 JSON" : error.message,
      });
    }
  });
  server.requestTimeout = 15000;
  server.headersTimeout = 10000;
  try {
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(port, "127.0.0.1", resolve);
    });
  } catch (error) {
    store.close();
    throw error;
  }
  return {
    store,
    url: `http://127.0.0.1:${server.address().port}`,
    close: async () => {
      await new Promise((resolve) => server.close(resolve));
      store.close();
    },
  };
}

export function client(url) {
  const target = new URL(url);
  demand(
    target.protocol === "http:" &&
      target.hostname === "127.0.0.1" &&
      target.pathname === "/",
    "第一阶段仅允许本机服务",
  );
  return async (path, input) => {
    const response = await fetch(new URL(path, target), {
      method: input === undefined ? "GET" : "POST",
      headers:
        input === undefined ? {} : { "Content-Type": "application/json" },
      body: input === undefined ? undefined : JSON.stringify(input),
      signal: AbortSignal.timeout(10000),
      redirect: "error",
    });
    const value = await response.json();
    demand(response.ok, value.error || "任务服务请求失败");
    return value;
  };
}
