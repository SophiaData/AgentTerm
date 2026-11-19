# AgentTerm

集中管理 Claude Code、Codex、Agy 等 AI CLI 的轻量会话管理器。桌面 GUI 采用 Tauri 2 + Rust + 原生 TypeScript/CSS（无 React 等 UI 框架），按需加载 xterm.js 展示受管 PTY 终端；终端 TUI 采用 Rust + Ratatui。支持 macOS Apple Silicon 与 Windows x64。

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

该命令检查项目规范、格式、lint、类型、前端 DOM 交互测试、Rust 测试与模板契约。前端无框架运行时依赖，DOM 测试使用仅开发时安装的 happy-dom。测试证据与边界见 [docs/testing.md](docs/testing.md)。

## 文档

- [产品需求](docs/PRD.md)
- [架构与技术约束](docs/architecture.md)
- [功能台账](docs/tasks.md)
- [协作手册](docs/workflow.md)

## 使用方式

桌面版：添加项目 → 选择项目 → 选择已安装工具 → 点击「启动终端」。新终端会话在 AgentTerm 内显示，切换标签继续运行；关闭运行中会话须确认。关闭主窗口仅隐藏，不会停止会话；完全退出程序时回收其受管理的子进程。移除项目只删除应用记录，不删除文件夹。**本阶段只管理从 AgentTerm 内启动的会话**，不接管外部终端；退出进程后仍可连接会话需要后续的独立守护服务。

TUI 版：在已有终端中运行 `agentterm`，选择项目和工具，交给原生 CLI；CLI 退出后返回 TUI 列表。

## 双端安装包

GitHub Actions 的 **Build desktop packages** 工作流可手动构建 macOS arm64 和 Windows x64 安装包，并保存为工作流附件。普通构建不会自动发布正式版本。

本仓库基于 [SophiaData agent-project-template](https://github.com/SophiaData/agent-project-template) 的工程规范初始化。
