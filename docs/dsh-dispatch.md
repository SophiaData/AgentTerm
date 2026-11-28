# DSH 本机协调与开工约定

状态：第一阶段及真实小任务已验收并合入 PR #105；本次增加开工约定，检查与实际验收范围见 [测试记录](testing.md)。协调者固定 DSH；本机服务与执行端通过 HTTP 分离，当前只监听
127.0.0.1，不宣称已经部署服务器或具备远程鉴权。本阶段一个项目任务同时执行，
网络重试只补传领取/回执，不创建新的执行尝试。

## 任务契约与流程

任务包含稳定 requestId、projectId、固定 Git baseCommit、goal、scope、DSH plan、executionHash。
executionHash 绑定本机执行器、模型、路径范围与验收命令，配置变化后旧任务拒绝执行。
服务器不接收任意命令或本机路径。仓库路径、执行器和验收命令由本机项目登记配置提供。
DSH 通过 ACP 根据任务描述生成前后端拆分建议。新任务先确认建议摘要，进入 planning；
Claude Code 生成三份约定后进入 agreement_review，再确认约定才进入 queued。

新任务：proposed → planning → drafting → agreement_review → queued → assigned → running → awaiting_review / failed / recovery_required。
生成失败进入 planning_failed；中断保留 drafting，等待人工重新生成。
执行结果通过验收命令也只进入 awaiting_review；人工确认结果后 accepted。
失败和待核实保持占用，本阶段不自动返工或重派；三轮返工属于下一阶段。
先停止 worker，再由用户点击核实放弃；原生组件确认全部执行具有持久停止证明后才能
进入 abandoned 并释放占用，原任务及结果保留。无法证明停止时仍拒绝释放。

服务端与执行端各用 Node 内置 SQLite，事务及 FULL 同步保存。领取绑定 deviceId 与
attemptId，断线不回收、不自动过期重派。执行端在创建进程前 CAS 登记启动意图，
再获取服务端仅发一次的启动许可；许可回执丢失时进入待核实，禁止再次启动。
重启后看到未完成启动意图只上报 recovery_required，不重复启动。
结果本地先保存，再回传；相同结果重传幂等，不同内容或旧身份回报拒绝。

## 运行边界

- Node >=22.22.2（仓库既有运行要求），增加内置 node:sqlite 使用，无第三方数据库依赖。
- 执行进程与 Tauri 窗口独立，同一 Windows 用户身份运行，登录信息留在原 CLI 中。
- 每次任务使用独立本地 Git clone，固定基线；不共享源仓库 .git，不修改全局配置。
- 当前仅本机，HTTP 拒绝外部 Host/Origin；同一 OS 用户不是隔离边界，不能开放公网。
- 程序超时或退出无法证明后代进程已经停止时保持待核实；不假定 PID 或心跳就是停止证明。
- 原生 companion 复用原单主任务账本和进程管理，桌面受管任务与新执行端竞争同一占用。
  原手动终端保持原有用途，不宣称被此账本约束。
- DSH 规划和汇总使用 DeepSeek；用户本轮明确授权 Codex 使用当前账号默认模型，不改全局设置。
- 不自动合并、发布、安装；不替代真实账号与原生交互验收。

## 验收

正常领取与结果回传、离线排队、重复领取、并发执行端竞争、服务重启、执行端启动前后
崩溃、完成回执丢失补传、旧回报拒绝、DSH 任务建议和人工审批、固定代码验收失败。
测试使用合成仓库与可控进程夹具；本机仅定向验证，完整检查通过 GitHub。

## 本机启动

1. 从本次 GitHub Windows 质量门下载 `dispatch-windows-<commit>`，包含原生执行组件。
   不需要在 D 盘编译。开发者也可以在 E 盘 target 目录构建
   `cargo build --manifest-path src-tauri/Cargo.toml --bin agentterm-dispatch-bridge`。
2. 把 `services/dispatch/config.example.json` 复制到自己的运行目录，填写真实路径。
   `claude` 填已安装的 Claude Code 原生路径（Windows 为 exe）；沿用其现有账号和模型，
   不改全局配置。版本需支持 `--safe-mode`、`--tools`、`--strict-mcp-config` 等当前调用参数。
   配置不包含凭据；不要复制 CLI 的登录文件。仓库、workspaceRoot 必须已存在且互不嵌套。
   `codex` 必须指向原生 exe，不能填 npm 的 cmd/ps1 包装；`model: "default"` 使用本机账号默认模型，
   也可显式填写本机支持的模型。配置摘要绑定该选择，默认选择下实际模型由 CLI 当前配置决定。
   Windows 默认传入 `windows.sandbox="unelevated"`，也可在登记配置中选择已完成初始化的
   `elevated`；始终使用 `workspace-write`。独立 CLI 不继承桌面专属 IPC/包身份变量，
   登录与模型配置仍由原 CLI 读取，不改全局配置。只返回 `turn.completed` 不代表修改成功，
   最终以固定验收为准。
3. 在两个独立终端启动：

   ```text
   node services/dispatch/cli.mjs serve E:/AgentTerm-dispatch/config.json
   node services/dispatch/cli.mjs worker E:/AgentTerm-dispatch/config.json
   ```

   使用 Windows 后台启动时选择隐藏窗口；第一阶段不自动安装开机自启。
   服务端固定 47321，执行端固定 47322 作为本机单实例锁，后者不提供业务接口。

4. 创建任务 JSON（同一 requestId 重试不能改变内容）：

   ```json
   {
     "requestId": "demo-001",
     "projectId": "demo",
     "baseCommit": "替换为仓库实际的40位提交",
     "goal": "修复一个小功能并通过测试",
     "scope": "只修改已登记的 src/ 和 test/"
   }
   ```

   ```text
   node services/dispatch/cli.mjs coordinate E:/AgentTerm-dispatch/config.json E:/AgentTerm-dispatch/task.json
   ```

5. DSH 生成拆分建议，打开 `http://127.0.0.1:47321/` 点击“确认拆分方案，生成约定”。
   保持 coordinate/watch 运行，Claude Code 返回接口约定、前后端任务清单、验收清单。
   核对三份内容后点击“确认三份约定并开工”；缺少任何一份或未确认均不能领取。
   AgentTerm 顶栏的“DSH 协调”打开同一面板。执行完成后检查独立 clone 的提交与测试摘要，
   在面板确认完成后才释放主任务占用。协调 CLI 会显示状态变化并请求 DSH 汇总。
6. 协调 CLI 退出后，使用 `watch <配置路径> <任务ID>` 重新跟进；使用 `status <配置路径>` 查看状态。

## 约定版本与恢复

- 三份内容为 `interfaces`、`tasks`、`acceptance`，各非空且最长 6000 字符，总 JSON 不超过 16 KB。
  内容为纯文本 Markdown；面板不执行其中的 HTML。接口/任务/验收编号的业务对应由用户确认，
  程序校验完整性和版本，不宣称已自动证明接口语义正确。
- `agreements` 表按 task/revision 追加，摘要绑定 specHash、revision、author 和三份内容。
  面板可查看已保存版本；GET `/v1/tasks/:id/agreements` 返回历史。人工确认绑定 specHash/revision/hash。
- “重新生成约定”立即撤销排队确认。生成操作有独立 operationId；响应丢失不重复保存，
  旧生成不能覆盖新生成。drafting 在重启后保留；需人工重新生成，不能凭心跳判定完成。
  点击重新生成不会自动确认新的约定。已经 assigned 的任务禁止换版，需求变化须提交新任务。
- coordinate/watch 负责推动 planning；仅开服务和 worker 不会生成约定。生成失败会在面板显示，
  修复 Claude 配置后人工重试。模型没有编写/验收项目代码；本项只生成规划材料。
- Claude Code 使用 print JSON + JSON Schema 结构化输出、safe-mode、空工具集、空 MCP 配置、禁用 hooks/skills 和会话持久化；
  不传模型覆盖。它依据已确认任务/DSH 方案与登记检查生成内容，不读取整个源仓库；
  信息不足必须标注待确认。此限制不是针对任意恶意程序的系统隔离。
- worker 协议版本 2 才能领取新任务，启动许可携带约定 hash，执行提示包含完整三份内容；
  程序通过的结果携带 agreementHash/revision，服务端拒绝不同版本。固定验收命令仍由本机登记，
  不执行模型生成的命令。文本清单的业务验收仍需后续主 Agent/集成复核及人工确认。
- 服务数据库从 v1 事务迁移至 v3；旧任务保留原状态和审批语义，不追改已经批准或执行中的任务。
  新建任务一律采用新协议，客户端不能选择跳过。执行端数据库仍为 v2。
  升级前应停止旧服务并备份 SQLite（包含 WAL 状态）；旧程序拒绝 v3，回滚需恢复升级前备份，
  不能让旧程序直接打开新库。当前改动未迁移用户正在运行的第一阶段服务。

## 当前明确限制

- 不确定、失败状态保留占用，不自动修复、不自动重派。面板可在 worker 停止后核实放弃；
  若原生组件被强杀、停止回执没有持久保存，即使 PID 消失也不能释放。保留数据库和执行记录，
  此类未知进程树恢复仍需后续能力，不能清库来重跑。
- 通知目前是协调 CLI 与面板中的状态/DSH 汇总，没有接钉钉或外部消息。
- 每个原生进程最多 300 秒、输出有界；长任务超时会进入待核实，阶段二再增加有界续作。
- 同一用户可直接调用本机接口；未做远程身份认证，不能将监听地址改成公网地址使用。
- 配置与产物路径校验是交付检查，执行器的文件访问仍依赖其 workspace-write 沙箱，
  不是对任意不可信程序的系统级隔离；验收程序必须由本机用户登记并信任。
  验收应使用允许修改范围以外的可信检查脚本或固定内联断言，避免执行者同时改弱验收条件。
  程序先保存交付提交，再运行验收，验收后提交和工作区必须未变化。
- DSH 的工具审批请求会被拒绝，但无审批工具能力取决于 DSH；提示词不构成系统隔离。
- 本机真实 DSH ACP 连通性已通过。早期误将 Codex 限为 DeepSeek，模型通道拒绝；
  用户已明确本项目按各自现有模型执行。Windows 合成任务已验证真实 DSH 规划、Codex 写入、
  固定验收、断线期间保存结果、恢复补传、DSH 汇总、面板确认与占用释放；只启动一次 Codex。
  此为主代理执行的合成验收，不替代用户项目验收或桌面安装验收。
