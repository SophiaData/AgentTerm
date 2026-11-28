import { readFileSync, mkdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { startServer, client } from "./server.mjs";
import { coordinate, watch } from "./coordinator.mjs";
import { runWorker, workerLock } from "./worker.mjs";
import { nativeBridge } from "./process.mjs";
import { demand } from "./store.mjs";

const [command, configFile, inputFile] = process.argv.slice(2);
if (!configFile || command === "--help") {
  console.log(
    "AgentTerm DSH 本机协调\nnode services/dispatch/cli.mjs <serve|worker|coordinate|watch|status> <本机配置.json> [任务.json|任务ID]\n说明：docs/dsh-dispatch.md",
  );
} else {
  const abort = new AbortController();
  process.once("SIGINT", () => abort.abort());
  process.once("SIGTERM", () => abort.abort());
  try {
    const config = JSON.parse(readFileSync(resolve(configFile), "utf8"));
    demand(
      config.url === "http://127.0.0.1:47321",
      "本机服务地址固定为 http://127.0.0.1:47321",
    );
    demand(config.projects && typeof config.projects === "object");
    const home = resolve(config.stateDirectory);
    mkdirSync(home, { recursive: true });
    if (command === "serve") {
      const bridge = nativeBridge(config.bridge);
      const server = await startServer({
        database: join(home, "server.sqlite"),
        projects: config.projects,
        accept: (task) => bridge(task, "release"),
        abandon: async (task) => {
          const unlock = await workerLock().catch(() => {
            throw new Error("请先停止本机 worker，再核实放弃");
          });
          try {
            await bridge(task, "abandon");
          } finally {
            await unlock();
          }
        },
      });
      console.log(`DSH 协调任务面板：${server.url}`);
      await new Promise((done) => {
        if (abort.signal.aborted) done();
        else abort.signal.addEventListener("abort", done, { once: true });
      });
      await server.close();
    } else if (command === "worker") {
      await runWorker(config, join(home, "worker.sqlite"), abort.signal);
    } else if (command === "coordinate") {
      const input = JSON.parse(readFileSync(resolve(inputFile), "utf8"));
      const task = await coordinate(config, input);
      console.log(
        `任务 ${task.id} 已保存，请在 ${config.url} 先确认拆分方案，再确认约定开工。`,
      );
      await watch(config, task.id, abort.signal);
    } else if (command === "watch") {
      await watch(config, inputFile, abort.signal);
    } else if (command === "status") {
      const { tasks } = await client(config.url)("/v1/tasks");
      console.log(
        JSON.stringify(
          tasks.map(({ id, state, projectId, summary }) => ({
            id,
            state,
            projectId,
            summary,
          })),
          null,
          2,
        ),
      );
    } else throw new Error("未知命令");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
