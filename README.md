# AgentTerm

AgentTerm 的产品方向是本地优先的轻量数据开发 Agent 工作台：接入任务，安排专业 Agent，组织交接与独立验收，集中展示产物和证据。

**当前代码包含 CLI 管理底座，以及单主任务预留账本的 Rust 后端。** 账本提供初始化、查看、预留和取消未执行预留的命令；尚未连接任务页面、CLI 派发或 Teambition，手动终端仍不受它控制。原有功能的验收状态见 [功能台账](docs/tasks.md)，不能把规划中的完整工作台当作已经实现。

下一阶段参考 Orca 的任务派发、结果回报与恢复能力，复用现有 CLI、数仓工具和检查器。主 Agent 在整个本地工作台内同时最多推进一个主任务；首版执行者与验收者顺序接力。具体范围、完成条件与恢复规则见 [产品需求](docs/PRD.md) 和 [架构约束](docs/architecture.md)。

桌面底座采用 Tauri 2、Rust、React 与 TypeScript，目标平台为 macOS Apple Silicon 和 Windows x64。现有 TUI 保留为工具启动入口；工作台首先以现有桌面界面承载任务页。

## 本地启动

开发环境需要 Node.js 22.13+、pnpm、Rust 和 Python 3；**不需要完整 Xcode**。

```bash
pnpm install
pnpm dev:app
```

## 质量检查

```bash
pnpm check
```

该命令检查项目规范、格式、lint、类型、前端测试、Rust 测试和模板契约。测试证据与边界见 [docs/testing.md](docs/testing.md)。

## 文档

- [产品需求](docs/PRD.md)
- [架构与技术约束](docs/architecture.md)
- [功能台账](docs/tasks.md)
- [协作手册](docs/workflow.md)

## 双端安装包

GitHub Actions 的 **Build desktop packages** 工作流可手动构建 macOS arm64 和 Windows x64 安装包，并保存为工作流附件。普通构建不会自动发布正式版本。

本仓库基于 [SophiaData agent-project-template](https://github.com/SophiaData/agent-project-template) 的工程规范初始化。
