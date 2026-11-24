# 安全说明

## 报告漏洞

请使用 GitHub 仓库的 **Security → Report a vulnerability** 私密报告功能。
不要在公开 Issue 中披露可利用细节、账号信息、Cookie、令牌或用户数据。

报告请包含受影响版本、平台、最小复现、影响范围和是否已观察到真实执行。
维护者确认前，请不要对第三方环境做破坏性验证。

## 信任边界

- AgentTerm 使用当前系统用户权限运行本地 CLI；CLI 自身权限策略仍是主要边界。
- 定时任务不使用 shell 解释，Windows 只接受直接 `.exe` 启动。
- 前端通过固定 Tauri 命令访问本地能力，渲染进程没有 Node.js 集成。
- 更新包必须来自本仓库 Release，并通过 GitHub 提供的 SHA-256 digest 校验。

## 当前限制

macOS 包尚未签名和公证；安装包更新仍可能触发系统安全提示。不要从镜像、
聊天附件或非官方链接安装 AgentTerm。
