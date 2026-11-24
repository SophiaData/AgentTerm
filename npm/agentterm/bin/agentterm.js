#!/usr/bin/env node
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const executable =
  process.platform === "win32"
    ? join(root, "vendor", "agentterm.exe")
    : join(root, "vendor", "agentterm");

if (!existsSync(executable)) {
  console.error("未找到 AgentTerm 桌面程序，请重新安装 agentterm 包。");
  process.exit(1);
}

const child = spawn(executable, [], {
  detached: true,
  stdio: "ignore",
  windowsHide: false,
});
child.unref();
