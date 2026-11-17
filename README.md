# AgentTerm

集中管理 Claude Code、Codex、Agy 等命令行 AI 工具的轻量桌面应用。采用 Tauri 2、Rust、React 与 TypeScript，目标平台为 macOS Apple Silicon 和 Windows x64。

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
