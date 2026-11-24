# Testing policy

## issues #55–#66 与 v0.2.0-rc.6（2026-11-25）

- 本轮按仓库 `project-policy.json` 逐项执行质量门。Windows 环境没有 `python3`
  命令别名，因此 `pnpm check` 包装器在命令解析处退出；随后用 `python`
  执行同一份 13 项检查，未跳过检查项。完整 Git Bash/Linux 原命令由 GitHub CI
  再执行一次。
- 前端：135 passed；Prettier、ESLint、TypeScript、Vite 生产构建通过。
- Rust：203 passed / 2 ignored；`cargo fmt --check` 与
  `cargo clippy -- -D warnings` 通过。新增覆盖包括超过 1 MiB 的输出排空、
  进程树超时、调度 tick、并发运行拒绝、四套存储归档压实、文档锁短竞争重试、
  会话列表对账、更新器 SemVer/资产/digest 分支。
- TUI：15 passed；格式与 clippy 通过，release 构建通过。
- 模板契约：13 passed；治理检查确认 15 个必需文件存在。
- 本确认绑定当前 `codex/issues-55-66` 分支，尚未冒充 GitHub CI、合并、Release
  或 Windows 原生实机验收。最终提交的 CI、Windows 覆盖安装和窗口截图结果将在
  完成后补入本节。

## Agent 原生定时任务与 rc.5（2026-11-24）

- 交互改造由 PR #52 交付并合入 `main`，合并提交 `25ca6dd362e0c14797b471a067584e5a4fc7db30`。CI `45386670629` 的 Linux `quality` 与 Windows `windows-frontend` 均通过；Windows Rust 测试启动时的 `STATUS_ENTRYPOINT_NOT_FOUND` 通过给测试链接目标嵌入 Common Controls v6 manifest 修复，未跳过测试。
- PR #52 使用 Windows 构建 `45387134963` 生成 rc.4；同目录替换后继续发现 Agy 报错 `unknown setting "mcpServers"`。因此没有把“会话能启动”当成 Agy 集成通过，转由 PR #53 修复。
- PR #53 将 Agy 改为使用其标准 `~/.gemini/config/mcp_config.json`，只在会话期间合并 `agentterm_scheduler`，退出时只移除自己的条目。CI `45389835964` 两端通过；Windows 构建 `45390261144` 成功并生成 rc.5。
- 本机完整 Rust 回归：183 passed / 2 ignored，`cargo clippy --lib -- -D warnings` 通过；前端 123 passed，TypeScript、Prettier、模板治理通过。新增 Agy 配置回归验证保留其他 MCP，并验证清理只删除 AgentTerm 条目。
- rc.5 安装器 SHA-256 为 `71c8b289f90c533af1ca3ff635fe6789a8047b2941056d8ce9da09c1be297063`。安装器退出 0，覆盖同一目录 `E:\AgentTerm\app-25ca6dd`；实际 EXE 为 11464704 bytes，SHA-256 `04af13a322425d12bee324c718172c13d545b029ef47cfeb15588f7d03a3c29b`。桌面与开始菜单快捷方式都指向该目录，测试时只存在一个 AgentTerm 进程。
- Windows UI Automation 实际启动 Agy 1.5.15，会话显示“运行中”；其配置指向本机 `agentterm-scheduler`，MCP initialize 返回协议 `2024-11-05`，工具列表为 `schedule_task,list_scheduled_tasks`。确认结束 Agy 后，进程退出且 `mcp_config.json` 中 AgentTerm 条目被移除。
- 定时任务面板实机显示紧凑任务卡，运行结果折叠，底部只显示“创建入口在会话里”的说明，不再有自然语言输入框或手工表单。旧版曾经误建的“每 1 小时”任务仍保留，需用户在新面板中删除后重新通过 Agent 创建。
- 真实交互由主 Agent 串行自检；不宣称独立评审或用户验收。macOS 真机、真实模型发出工具调用和长期定时触发仍需单独验收。

## 项目文件浏览切片（2026-11-23）

- 用户明确要求解决 CLI 查看文件不方便的问题。本切片增加当前项目文件面板：目录展开/收起、刷新、点击只读预览、关闭预览及打开系统资源管理器。面板跟随当前项目，与任务记录互斥，现有终端保持挂载；窄窗以抽屉展示。
- 后端只接受已登记项目 ID 与相对路径；覆盖目录穿越/绝对路径/替代数据流拒绝、普通文件检查、链接目录拒绝、中文/BOM 文本读取、1 MiB 预览限制和 1000 项目录限制。新增 Rust 回归尚待本 PR 的 GitHub CI 执行；不把静态检查写成后端运行通过。
- 本地只运行相关四个前端测试文件，31 passed；包括旧文件迟到回包不覆盖新选择、关闭预览后迟到错误不显示、项目间状态隔离、刷新获得 CLI 更新、读取失败重试，以及文件中的 HTML/script 以纯文本呈现。TypeScript 与改动范围 ESLint、格式检查通过。
- 原地址修复的 CI `45207152043` 在本次需求追加后主动取消，不计通过。文件浏览与地址修复共用正式 [PR #36](https://github.com/SophiaData/AgentTerm/pull/36)，完整检查、合并、安装包和原生核验结果以该 PR 正文与 Checks 为准。下方 `52a9a59` 是此前已安装版本，不含本次两项界面增量。
- 本节是提交前验证记录。文件编辑、删除、重命名、自动目录监听、非文本内嵌预览不在本切片内；大文件/非文本文件使用系统资源管理器入口。Agy 真实对话和完整停止恢复仍单列，#12/M4 不因此完成。

## 首页地址前缀修复（2026-11-23）

- 用户反馈首页地址“乱码”。定向回读首页首个项目，实际存储路径为 `\\?\E:\AgentTerm` 且目录存在；`projects.rs::normalize_project_dir` 使用 Windows `canonicalize`，首页原先直接显示该内部长路径前缀。此处是路径呈现问题，未发现这条地址的字符编码损坏。
- 前端 `displayPath` 仅将明确的 `\\?\盘符:\` 与 `\\?\UNC\服务器\共享` 转为普通地址，在首页地址、项目设置、项目/会话提示与最近会话提示使用。其他路径保持原样；后端、项目 ID、持久化、路径比较与启动参数继续使用原路径，没有迁移或改写项目记录。
- 本地定向回归先在旧实现得到 2 failed（盘符及 UNC 的首页地址），修复后 `pnpm exec vitest run src/App.test.tsx src/projects/displayPath.test.ts --maxWorkers=1 --no-file-parallelism` 为 20 passed；包含中文和空格、网络共享、普通 Windows/macOS 路径、未知设备路径不误改，以及最近会话提示。仅运行必要的前端回归，完整质量检查和打包继续在 GitHub 串行进行。
- 本节记录修复提交前的复现和验证。下方 `52a9a59` 安装证据来自修复前的包，不包含本次地址显示修复；本修复 PR 的最新检查、合并与打包结果以其 Checks / 正文为准。Agy 真实对话、完整停止恢复及 #12/M4 仍独立验收。

## Windows 安装与启动核验（2026-11-23）

- 用户明确要求“安装一下吧”，并要求确认无问题的改动提交合并，不长期留在草稿。按本次授权下载构建 `45169830049` 的 `windows-x64-nsis` 附件，实际归档 SHA-256 与 GitHub 返回的 `14c79752a1c291d90a6ca16718fdad1ad1388df1ca20797c0b21208537998bda` 一致；来源为已通过 CI 的 `main` 提交 `52a9a59585a9746327d83127fe29369d70ddcf2b`。
- 安装器 `AgentTerm_0.2.0-rc.1_x64-setup.exe` 为 2462406 bytes，SHA-256 `35b82d150f21b93005fd75efc6c6ebbf512dd9bd505bf7d477879b1d136c3e1e`。核对 Tauri CLI `2.12.5` 的 NSIS 模板后，以 `/S /UPDATE /D=E:\AgentTerm\app-52a9a59` 安装到新目录；`/UPDATE` 跳过旧版卸载，安装器 PID 31408 退出码为 0。
- 实际安装 `E:\AgentTerm\app-52a9a59\agentterm.exe` 为 10791424 bytes，SHA-256 `262e03f4fadd4e8a16c963cb35f5303400cb41544560da58747879f6292f99cf`；卸载注册信息的安装位置已指向新目录，版本仍为 `0.2.0-rc.1`，辨别本轮构建须同时核对提交和摘要。
- 安装前已在 `backups/install-52a9a59-2026-11-23` 备份旧可执行文件、卸载程序、安装信息和快捷方式。旧目录 `E:\AgentTerm\app-a7c59bf` 的两个程序文件安装前后摘要一致；桌面与开始菜单的 `AgentTerm.lnk` 均已指向新 EXE，工作目录为新安装目录。
- 新版进程 PID 23224，实测可执行路径为新目录，窗口标题 `AgentTerm`、窗口句柄非零、`Responding=True`。该证据仅证明安装和窗口启动，不代表 Agy 会话、中文交互、停止恢复或 #12/M4 完整验收通过；没有创建正式 Release。
- 本轮直接执行授权的附件下载与 NSIS 安装均成功；没有使用此前被拒绝的临时 MSI 解包方案。下方“尚未下载/安装/启动”属于旧轮记录。唯一下一项仍为在模型可用后，对本次固定包验收 Agy 真实交互及执行→停止→重启核实→换绑接续。

## PR #35 合并与 main 安装包（2026-11-23）

- 用户明确授权“测试通过就合并打包”。PR 最终提交 `7cd670a8f73da2f988b76801410829dfecb14daa` 的 [CI 45165330066](https://github.com/SophiaData/AgentTerm/actions/runs/45165330066) Linux/Windows 两端成功后，[PR #35](https://github.com/SophiaData/AgentTerm/pull/35) 已合入 `main`，合并提交 `52a9a59585a9746327d83127fe29369d70ddcf2b`；合并后的文件内容与已验证 PR 相同。
- 合并后的 [CI 45167546361](https://github.com/SophiaData/AgentTerm/actions/runs/45167546361) 两端成功：Linux 13 项质量检查通过，后端 169 passed / 2 ignored、TUI 17 passed；Windows 后端 164 passed / 2 ignored，两项 Agy 默认目录及 PATH 优先级回归均通过。全部在 GitHub 串行完成，本地未重复编译。
- 等 `main` CI 完成后，才启动 [Windows 构建 45169830049](https://github.com/SophiaData/AgentTerm/actions/runs/45169830049)，同一合并提交的 Windows x64 NSIS 安装包构建成功，用时 11 分 41 秒；本次未构建独立 TUI 包。
- 附件 [windows-x64-nsis](https://github.com/SophiaData/AgentTerm/actions/runs/45169830049)，2462588 bytes，GitHub 附件 SHA-256 `14c79752a1c291d90a6ca16718fdad1ad1388df1ca20797c0b21208537998bda`，回读时未过期，保留至 2027-02-21。此摘要属于 GitHub 附件，不代表解包后的安装器或应用 EXE。
- 本轮仅完成合并与构建，没有下载、安装或启动新包，没有创建正式 Release；此前下载/临时 MSI 解包的自动审批阻塞未重试或绕过，模型代理本轮未重新探测。Agy 应用内检测、中文交互及真实停止恢复仍待验收，#12/M4 不因此标记完成。下一项为使用此 `main` 包完成原生验收。
- 以下是各轮历史证据；旧的“草稿/不合并”和旧包链接不代表最新交付状态。本段记录合并及打包时的状态，后续安装结果见上一节；构建状态与下载入口同时记录在 PR #35 正文。

## Agy 默认安装目录检测补修（2026-11-23）

- 用户要求继续适配 Agy；本机 `Get-Command agy` 未找到入口，但定向检查 `%LOCALAPPDATA%\agy\bin\agy.exe` 后，真实 `--version` 返回 `1.5.15`，`--help` 正常退出。该目录不在当前 PATH 中，原 `candidate_search_dirs` 又仅补查 npm/pnpm 等目录，因此漏检；不能据 PATH 缺失断言 Agy 未安装。
- 安装目录依据为 [Antigravity 官方排障说明](https://antigravity.google/docs/cli/troubleshooting/) 与本机实际路径。Windows 搜索现在追加该默认目录，仍优先使用用户已有 PATH；检测和手动终端启动共用此搜索入口。未修改全局 PATH、账号、模型或 Agy 设置。
- 新增 Windows 回归：无 PATH 条目时从默认目录识别版本（含路径空格、目录缺失）；显式 PATH 安装优先于默认安装。测试使用临时 `.cmd` 夹具，不冒充真实 Agy 对话。完整质量检查和打包走 GitHub 串行，本地不重新构建 Tauri。
- 代码 `a7f48874da5103acfb42294412f49516753a0cde` 的 [CI 45159685193](https://github.com/SophiaData/AgentTerm/actions/runs/45159685193) 已全部成功：Linux 13 项质量门通过，后端 169/2 ignored、TUI 17；Windows 后端 164/2 ignored，新增两项 Agy 回归均通过。单个 DeepSeek 串行静态评审 PASS；改动文件格式、字节与 diff 检查通过。
- 同一代码的 [Windows 包 45163296814](https://github.com/SophiaData/AgentTerm/actions/runs/45163296814) 已构建成功。附件 `windows-x64-nsis`，2461314 bytes，GitHub 附件 SHA-256 `0afce80495e73470c9813a85c36de894c073b70a71044cf6f16076ef50e60a39`；摘要不代表安装器或应用 EXE。先完成 CI，再评审、打包；无本地完整编译，未创建正式 Release。
- 尚未验证：新包内 Agy 检测与交互终端、中文输入/回复、快捷键、退出及历史重开。真实 CLI `--version/--help` 仅证明入口可执行，不等于完整终端验收。任务抽屉受控派发仍只开放 Claude，未为 Agy/Codex 新增自动派发或原生 resume。收尾回读 `127.0.0.1:15721` 仍无监听；此前下载/临时 MSI 解包命令被自动审批以 `blocked by policy` 拒绝且未给细因，未重试或绕过，新包未安装运行。
- 非阻塞评审建议：后续加强生产目录拼装入口的回归；当前测试直接调用默认目录辅助函数并手工拼接 PATH 顺序，生产调用关系已静态核对。本轮不为建议项扩大改动。

## #12 串行评审补修（2026-11-23）

- 根据用户最新 D2 要求，单个 DeepSeek 子代理只读评审，主代理等待结束后修复。首轮静态结论 FAIL：非 Linux Unix 错用 `/proc` 缺失判退出；停止与后续阶段写入竞争可能把任务留在同一 owner 的 `pausing`。旧规则中“无子代理”仅适用于 11 月 21 日的历史工作。
- 停止竞态回归先在旧实现实测失败：`stop_racing_with_handoff_or_final_report_keeps_task_recoverable`，实际 `Pausing`、预期 `RecoveryRequired`，0 passed / 1 failed；修复后执行转验收和最后报告两个竞争窗口均通过。
- 修复：Unix 只在 `kill(pid, 0)` 明确返回 ESRCH 时证明退出；`/proc` 仅用于 Linux 身份核对。macOS 活进程身份暂归 `Unknown`，继续阻断恢复。后台异常后有界读取当前版本并推进待核实，仅允许本次派发的版本；持久化仍失败时，空闲后台加执行阶段显示只读核实入口，人工接回再次验证全部旧进程已退出，并持有运行锁防止同时新派发。
- Windows 本地 `pnpm check` 13 项全部通过：前端 89、Rust 后端 162 passed / 2 ignored、TUI 15、模板契约 13；单 worker、单 Rust job。新增 5 项回归覆盖停止竞争、迟到失败不覆盖新尝试/owner、后台结束后同 owner 接回、身份未知继续占用、活进程无启动身份不得判退出。另有 Unix 非法 PID 断言，须由 Unix CI 执行。跨平台旧夹具已改用实测退出的子进程 PID，不以 `u32::MAX` 假定退出。
- 本轮全量检查保留既有 `sessions.rs` 测试未使用 Result 和 TUI `serde` 未使用依赖警告；未据此扩大改动。临时 Python/pnpm 原生入口只作用于检查进程 PATH，构建和测试缓存位于 E 盘。
- 第二轮单个 DeepSeek 只读复审 PASS（仅静态）；15 个交付文件字节检查通过。代码提交 `c322c4ab1b3f08d386a07f1a9dabb3257f411247` 已推送 PR #35。[GitHub 质量检查 45149490139](https://github.com/SophiaData/AgentTerm/actions/runs/45149490139) 两 job 均成功，Linux 后端 169/2 ignored、TUI 17，Windows 后端 162/2 ignored。最后跨平台夹具调整后的定向测试亦通过；Windows 结果不替代 macOS 真机。
- [Windows 包 45151496125](https://github.com/SophiaData/AgentTerm/actions/runs/45151496125) 成功，绑定 `c322c4a`。附件 `windows-x64-nsis`，2471787 bytes，GitHub 附件摘要 `sha256:3233abf9f36f40237b11a644aa9f5630b07a205329791c8cc1225f4e97067c67`；不是解包后 EXE 摘要。提前触发的 45149507161 因与延迟出现的 CI 短暂重叠而主动取消，不计通过；最终构建在 CI 全部成功后才触发。11 月 21 日旧包不含本轮增量。
- 11 月 23 日环境回读：PR #35 仍为未合并草稿。`127.0.0.1:15721` 无监听，24-byte 账本 SHA-256 仍为 `70871f764d93687bb184524eea2cd1651610d49eb544a35e22cff2d8f8509be0`。收尾时旧 PID19268 已不在，39860 的路径/创建时间未变；本轮未操作旧应用，退出原因未查。此前自动审批拒绝未解除，未重试下载/解包，新包未安装或启动；原生验收仍为 0 项通过，#12/M4 保持未完成。
- 本轮测试程序、生成 schema、E 盘编译与测试目录已移入回收站，备份保留；回收不计物理空间释放。用户纠正后，后续完整测试/打包优先在 GitHub 串行执行，本地只做必要最小复现，避免重复编译及堆积缓存。
- 首轮建议项留待原生验收：短暂轮询错误成功后仍需手动刷新；会话换绑在持锁前检查状态，存在刚结束会话被绑定的窗口，派发前仍会重新校验。未将建议项扩入本次修复。

## #12 停止与重启接续修复（2026-11-21，本轮）

基线 `30c4204` 的 CI `44893208852` 已通过。本轮在 `codex/recovery-acceptance` 上顺序实施与主 Agent 自检，不调用开发或评审子 Agent。以下记录属于本轮；下方历史切片不代表当前整体验收。

- 复现：新增 `restart_during_running_exposes_recovery_without_rewriting_the_ledger` 在修复前失败，原因是旧 owner 的 `Running` 任务未返回恢复评估（0 passed / 1 failed，exit 101）。修复后通过。
- 本地 Windows：`pnpm check` 的 13 项全部通过；前端 89 passed，Rust 后端 157 passed / 2 ignored，TUI 15 passed，模板契约 13 passed。两个 ignored 是现有由父测试调用的子进程入口。全部串行、单 worker / 单 Rust job。
- 新覆盖：执行/验收/暂停/等待阶段崩溃后接回、读评估不改账本、活协调进程阻止接管、旧票据拒绝、会话历史持久化与换绑冲突、UI 恢复换绑入口、已持久化用户停止原因不被后台替换。
- 本机 `python3` 与 Python 子进程对 `pnpm` 的解析缺失，通过本轮临时原生命令入口调用既有 Python/Node 解决，不更改全局配置。首轮格式错误核实为 Git CRLF 检出转换，恢复与 HEAD 完全相同的 LF 字节后原格式门通过，无全项目格式内容改动。
- 固定代码版本：`3c7824baebe71e0026f1680fc700ff5fafb0e5ab`，草稿 [PR #35](https://github.com/SophiaData/AgentTerm/pull/35)，[质量检查 44897184829](https://github.com/SophiaData/AgentTerm/actions/runs/44897184829)；运行结果以该链接回读为准。本段后续仅更新证据，不更改包内代码。
- [Windows-only 打包 44897226917](https://github.com/SophiaData/AgentTerm/actions/runs/44897226917) 已成功，绑定上述代码提交。附件 `windows-x64-nsis`，ID `14704842120`，2470415 bytes，GitHub 附件摘要 `sha256:1dbaa9ca8d7db155facda414cba6b995fb9b54ded35e828fe433450212afd959`；这是附件摘要，不冒充解包后 EXE 摘要。未创建正式 Release。
- 原生验收 BLOCKED / 0 项通过：下载构建附件并启动临时 MSI 解包工具的整组命令被自动审批拒绝，返回 `blocked by policy`，没有更具体原因；拒绝发生于执行前，目标下载目录与 MSI 文件均不存在。没有换工具或拆命令重试。另对 `127.0.0.1:15721` 做 TCP 探针，当前无监听且连接失败，真实模型回复无法验证。
- 未安装或启动新包；旧应用 PID 19268、39860 的路径和创建时间保持不变。空账本仍为 24 bytes，SHA-256 `70871f764d93687bb184524eea2cd1651610d49eb544a35e22cff2d8f8509be0`，与测试前备份相同。没有创建业务任务、修改全局模型配置或使用全局键盘鼠标注入。真实执行/停止/重启/接续、审批、物理输入均不能以本地或 CI 测试代替。
- 尚未交付：#13 证据失效与完成释放、外部作业撤销、CLI 原生对话 resume、macOS 真机验收。#12 与 M4 暂不关闭。

## 验收现状与下一项（2026-11-20）

当前双终端分屏代码6a0615d4d44ac0e165109324112a176bd7edd44a通过串行CI44747204310：两端前端75、Linux Rust123、Windows Rust114、TUI17，13项质量门通过。新增14项前端回归；最初b14322f8候选CI44746013910因补正跨项目焦点同步而主动取消，不计作失败或通过。分屏覆盖：显示与输入焦点分离、拖动/键盘调比例、窄空间上下布局、双侧独立尺寸同步、单屏/新建页保持挂载。新增回归检查替换当前格、关闭另一格、失败保留、自然退出和只剩一个会话；原生验收使用离线PTY测试程序，UI Automation只定向调用控件；不使用系统键盘/鼠标注入。Windows-only打包44749553746成功，artifact14635751234；已安装并用真实Claude Code 2.1.326补验启动与会话管理，详见下节。测试结果绑定本轮提交，不沿用下方多会话结果冒充。

### 会话历史与再次打开切片（2026-11-21，本地自动测试、CI与原生验收通过）

- 本地验证覆盖 Rust 128 passed / 2 ignored（含 `session_history` 9 项：配置校验、起止跨实例重载、重复/缺失拒绝、50 条窗口裁剪、尾行损坏/无换行截断/摘要篡改失败关闭、锁冲突、非法时间戳拒收，以及 `sessions` 4 项：未配置拒查、单次配置、显式停止记 `已停止`、退出清理记 `应用退出`），前端 Vitest 82 passed（新增 10 项覆盖 `sessionHistory` 辅助函数与首页 `最近会话` 渲染/错误重读/缺失工具禁用），`eslint`、`tsc --noEmit`、`prettier --check`、`vite build`、TUI 15 passed、模板治理 13 项全部通过。
- 本机工具链限制：Windows 未安装 `rustfmt`/`clippy` 组件，这两项静态门禁由 Ubuntu CI 执行并通过；Windows 安装包已完成桌面重启后真实“再次打开”验收，详见下方 Codex 原生桌面验收段。
- 前序真实 Claude Code 2.1.326 补验补充（绑定已安装 6a0615d4 包 / PR #30 `2779327b` CI44757159573 成功）：定向 `WM_CHAR` 输入中文、数字 `1234567890` 与标点均完整回显，切标签未提交内容保留，退格与 `/help` 自动补全有效；合成 Enter/CR 未触发提交且会话 JSONL 无新记录，提交、方向键定位、物理键盘/IME、审批与分隔线拖动仍为 NOT RUN。

### Windows Codex 解析与乱码修复（2026-11-21，本地单测、CI与真机通过）

- 本地验证：`cargo test -- --test-threads=1` 132 passed / 2 ignored（新增 `tools::tests::windows_skips_extensionless_and_ps1_shims_and_prefers_exe_over_cmd`、`sessions::tests::trim_buffer_front_avoids_splitting_ansi_escape_sequences`、`sessions::tests::spawn_pty_strips_verbatim_paths_and_sets_utf8_env`、`sessions::tests::build_pty_command_wraps_windows_cmd_and_strips_verbatim_cwd`）；前端 Vitest 83 passed（新增 `offsetStitch` UTF-8 continuation bytes 对齐测试），`eslint`、`tsc --noEmit`、`vite build`、TUI 15 passed、模板治理 13 项通过。
- 过程教训与限制：在无控制台句柄（`GetConsoleWindow()==0`）测试进程中直接从单元测试启动真实 ConPTY `cmd.exe /D /C` 会挂起或异常退出（`0xFFFFFFFF`/`0xC000013A`），因此将命令构建与路径/UTF-8 规则抽为纯函数单元覆盖；真机桌面窗口下的 Codex 启动、中文无乱码渲染与重启后再次打开已由下方独立验收补足。

### Windows Codex 原生桌面验收（2026-11-21，已通过）

- 新包 `E:\AgentTerm\app-a7c59bf\agentterm.exe` 与旧版 `E:\AgentTerm\app\agentterm.exe` 并行运行；旧版 `PID 19268` 及其中 3 个 Claude 会话未关闭或接管，新版使用独立 `PID 23264`/重启后 `PID 39860`。
- 新版真实识别并启动 Codex `v0.169.0`；定向窗口消息输入 `请只回复：中文测试通过` 后，终端完整显示中文输入与 Codex 回复 `中文测试通过`，截图证据为 `.trash/2026-11-19/agentterm-split-codex-chinese.png` 与 `.trash/2026-11-19/agentterm-split-codex-response.png`。
- 结束新版 Codex 会话并重启新版应用后，首页重新读取持久化历史，显示原项目/工具的最近会话；点击“再次打开”成功新建 Codex 终端并显示运行态。该功能是新开进程，不是 CLI 原生 `resume` 或旧实例接管。
- 分屏补验：两个真实 Codex 会话可进入双终端布局；分隔线竖向状态支持定向 UIA 焦点与 `Right` 微调，`Home`/`End` 设置边界、`Enter` 复位均生效；窗口缩窄至 900px 后自动切换为横向上下布局，分隔控件尺寸由约 `8×625` 变为 `668×8`。本轮未验证真实物理键盘/IME、`Shift+Tab` 审批快捷键与物理鼠标拖动；定向 UIA/WM_CHAR 验证未使用全局键盘鼠标注入。

### #12 启动身份、停止请求与停止证据基础切片（2026-11-21）

- Rust 单线程测试：146 passed、2 ignored、0 failed；新增账本显式接续、活进程阻断、接续后新 dispatch 隔离旧回报测试，覆盖停止请求幂等、启动事件先落盘、进程身份回写、停止请求先写启动事件、终止事件为 `Unknown`、确认停止证据、`RecoveryRequired` 占用保留，以及启动边界未知/身份匹配/身份不匹配/进程已退出分类。`cargo check --manifest-path src-tauri/Cargo.toml` 通过。
- 实现边界：启动事件先写入，子进程加入隔离范围后记录 PID、可执行文件和平台可用的启动 token；停止请求先持久化，再停止进程；停止确认失败不释放占用。重启核实只读，`Exited` 仍需人工确认后才能显式接续；`Alive`/`Reused`/`Unknown` 继续阻断。接续迁移到新 owner 和新 revision，旧回报不能通过新 dispatch 校验。正常退出保存已确认停止证据；超时、异常、残留、结果未知或停止不确定保持占用并待核实。
- 当前仍未实现前端停止/接续命令、前端受管派发、外部作业撤销、真实CLI权限验收和用户验收；因此 #12 与 M4 均未完成。下一项是把接续入口接到受控人工核实界面，并继续覆盖多旧尝试、迟到回报持久记录和跨重启竞态。

### #12 功能侧恢复核实界面（2026-11-21）

- 后端 `workbench_snapshot` 在 `RecoveryRequired` 任务下返回每个未结束尝试的只读进程观察结果；新增 `resume_main_task_after_recovery`，在命令内重新检查 task ID、generation、revision、计划归属及所有旧尝试是否均已 `Exited`，通过后才执行账本 CAS 到 `RecoveredReserved`。命令不启动 CLI，也不释放任务占用。
- 前端任务抽屉展示尝试编号、步骤和观察结果；活跃、PID复用、启动身份缺失或未知状态只显示“重新核对”，全部退出后才显示“核实并接回任务”。前端回归 15 passed；Rust command 定向测试 13 passed；前端 `tsc --noEmit`、Rust `cargo check` 和 `git diff --check` 通过。
- 该阶段当时的全量结果为 Rust 148 passed / 2 ignored、前端 Vitest 84 passed；随后已继续完成会话关联切片，当前结果见下方。Windows 未安装 `rustfmt`，项目固定的 `pnpm check` 仍被 `python3` 命令缺失阻塞；本阶段没有把功能标为完成。

### #12 真实 CLI 会话与任务记录显式关联（2026-11-21）

- 新增任务文档 `sessionLink` 和 `associate_task_session` command。关联前后端核对 session ID 对应的 PTY 仍在运行、任务与会话项目路径一致、task ID/generation/revision 未变化；绑定只允许从无到有，同值重复操作不追加记录，其他会话换绑被拒绝。
- 前端任务抽屉在当前任务存在且当前会话仍运行、项目一致时显示“关联当前会话”；跨项目、已退出或已有其他绑定时不显示。绑定后回读显示工具和 session ID，任务记录与会话关系可追溯。
- 本地验证：前端全量 Vitest 85 passed，Rust 单线程 150 passed / 2 ignored，`pnpm build`、`tsc --noEmit`、本轮文件 Prettier、`cargo check` 和 `git diff --check` 通过。未做真实桌面点击验收、未启动真实 Agent 任务、未接受控派发；本轮仍不把 #12 标为完成。
- 质量检查：`pnpm lint`、`pnpm typecheck`、`pnpm test`（83 passed）和 `pnpm build` 通过；`pnpm check` 及 `python scripts/run_quality.py` 被项目固定的 `python3` 命令阻塞。`pnpm format:check` 仍报告仓库既有 48 个文件格式不符；Windows 未安装 `rustfmt`/`clippy` 组件，本轮未将其失败改写为通过。

### #12 受控派发最小入口（2026-11-21）

- 后端新增 `dispatch_controlled_task`：只接受已有计划、当前账本的 `taskId/generation/revision`、已关联且仍运行的同项目会话；绑定信息、工具类型和计划归属由 Rust 从后端状态重新读取，前端不能自填路径、工具或授权。
- 当前仅允许 Claude Code 适配器。relay 固定使用 `--tools ""`、空 MCP 配置、禁止 MCP 工具和无外部操作提示；执行 Agent 与验收 Agent 顺序运行，当前手动终端保持独立，结果写回任务事件。
- 前端任务抽屉在条件满足时显示“开始受控执行”，Codex/Agy、跨项目、未绑定、会话结束、待核实、非预留阶段和过期 revision 均不显示或被后端拒绝。重复点击由前端单请求锁和后端阶段/revision 校验共同阻止。
- 本轮定向验证：任务页 17 passed；Rust `task_commands` 15 passed；`pnpm typecheck` 和 `cargo check --manifest-path src-tauri/Cargo.toml` 通过。全量测试、构建、格式检查和单线程 Rust 全套尚待收尾执行。
- 未验收：真实桌面点击、真实 Claude 账号/权限边界、真实受控接力输出、停止按钮、完成释放、持续状态推送和外部会话接管；因此不关闭 #12 或 M4。

### #12 受控派发状态联动（2026-11-21）

- 前端只在 `dispatch_controlled_task` 请求持续期间短轮询 `workbench_snapshot`，回读后端已持久化的 `running`、`verifying`、`waitingInput`、`waitingApproval`、`pausing` 和 `recoveryRequired` 阶段；任务卡同时展示当前尝试事件，普通手动 CLI 不受影响。
- 新增回归覆盖 `verifying` 阶段的任务卡、阶段说明和已提交尝试；快照沿用账本 revision/generation 与任务文档 revision 的旧数据保护。前端全量 Vitest 87 passed，`pnpm typecheck`、`pnpm build`、目标文件 Prettier 和 `git diff --check` 通过。
- 这只是状态可观察性切片，不代表真实桌面持续推送、停止/完成释放、验收证据闸门或真实 Claude 账号权限已通过；`pnpm check` 仍受本机缺少 `python3` 影响，Windows `rustfmt`/`clippy` 仍由 CI 补验。

### #12 停止请求入口（2026-11-21）

- `dispatch_controlled_task` 现在后台启动既有顺序 relay，前端命令立即返回并继续读取持久状态；任务卡在 `running/verifying` 显示停止入口。
- 停止顺序由后端固定为“写入停止原因 → 请求进程控制 → 推进 `pausing`”；relay 在停止竞态中重新读取账本 revision，再写入 `RecoveryRequired`，避免旧票据覆盖停止后的新状态。
- 本地验证：Rust 单线程 152 passed / 2 ignored，停止证据定向测试通过，`cargo check`、前端测试 19 passed、lint、typecheck、build、Prettier 通过；尚未做真实桌面点击、真实 Claude 权限、停止后的用户验收和完成释放。`pnpm check` 仍因本机没有 `python3` 退出 1。

前序多会话切片（9eea1dc5）：从单个会话改为多个独立CLI会话；以下保留该切片的历史自动测试结果，原生现状以上方最新分屏段为准。新回归覆盖两个会话独立身份/选择、重复启动保护、后台退出归属、关闭互不影响、停止失败保留、退出竞争、自然退出释放缓存、关闭过程中切换与启动失败重试；App检查切换/新建页仍保留所有终端面板，后端检查运行会话禁止forget、已结束输出按ID释放。代码9eea1dc54dd9a2ba3693baf6d03b7086465447e0通过串行CI44726264171：前端61（两端）、Linux Rust123、Windows Rust114、TUI17；13项质量门通过。两端各2个ignored仍为既有子进程测试入口。新增10项前端和3项后端回归；当时原生未安装/验收，主Agent自检不能冒充独立审查。

### 双终端原生补验（真实Claude Code）

NSIS包绑定6a0615d4，安装器2,369,811 bytes、SHA256 `280d468e18dfabbda63c7989c4f1b63dae525ccc02ee805f2bf2619f1ccee5cc`；安装器PID36124退出0。实际应用10,460,672 bytes、SHA256 `ca4b394b800d8693594e21db0e375eac11dded396158e0c38e9fbeffe9326b7c`。

最初用同名离线程序测试布局，用户指出不是Claude Code；已停止应用36388和回显33464/10980，回收替身及缓存33,634,783 bytes，后续不得把此段当真实CLI验收。当前改用本机`C:/Users/Administrator/.local/bin/claude.exe`（378,067,616 bytes，`--version`为2.1.326，SHA256 `44e85ac6d3e36b924a0d631d3f69c13a6907491115d71e97c8eb6160c82b2aed`），没有读取或修改CLI账号配置，也未向CLI发送任务。

真实应用PID19268，两个原生Claude会话PID10520/21144。依次新建、左右分屏、选标签、切单屏再恢复、任务抽屉开关、850px窗口上下排列均取得UIA与截图；前后两个PID、父PID、可执行路径和创建时间完全相同。第三会话39604新建后原两进程仍在；结束确认默认聚焦取消，取消保留，确认后只有第三进程退出，原两进程继续存在并可重新分屏。原生控件Toggle/Selection/ExpandCollapse按支持模式调用，没有系统键盘或鼠标注入。

首个关闭探针在CLI初始化时未出现确认，未判通过；稳定后重试取得确认/取消/退出回读。拖动分隔线、键盘改比例、实际输入/IME/粘贴、模型回复、审批/CLI快捷键与恢复未做原生验收。分屏数值边界和状态转换由本轮前端回归覆盖，不能替代鼠标/键盘实测。当前仍#4进行中，不称真实CLI完整验收通过。

证据位于既有`E:/dw-modeling-template/.trash/2026-11-19`：`agentterm-split-native-acceptance.json`含哈希与断言；`agentterm-split-real-final.png`是真实双Claude截图，`agentterm-split-real-stacked.png`为窄窗口；进程前后回读文件与UIA均同前缀。真实两个空闲会话留在窗口供用户操作；没有派发模型任务。当前WebView缓存约69.16MiB，仍被该窗口使用，预算512MiB，E盘约93.81GiB可用；不能清理正在使用的数据，也不把回收站等同于物理空间释放。

前序单会话工作区5f08cdf0的CI44719966384、文档65b5281f的CI44721767144成功：前端51、Linux Rust120、Windows Rust111、TUI17。单会话布局打包44723791668已主动取消，没有下载/安装。以上是历史状态；现装应用已更新为上节6a0615d4分屏包。

旧ASCII缺数字结果尚未定位；后续系统键盘注入出现焦点干扰，测试字符进入用户聊天框，已停止此测试方法。不能据此认定产品丢字，也不能认定输入通过。只允许不会干扰用户输入的定向控件操作或隔离测试环境。旧布局复测应用17688/回显33464已停止，约11.29MiB自有临时文件回收；非本轮明确创建的小文件未处理。下文历史证据保留原时间与范围。

Herdr参考仅为实际查看官方仓库TUI截图与Code示例截图，并核对桌面README和示例RECIPE；没有安装运行Herdr，不以截图证明其运行行为，也不将Code固定只读编辑区当完整编辑器。

以下B1历史原生包绑定 `6deebcf73f080953387edae4458acea999e0ba13`，完整CI绑定 `e24ecfdd1b329b380436487cabfad0005deb5eff`，对应未合并草稿 PR #28。两提交之间只有两个Windows测试修正，运行时代码相同；后续文档更新不代表重新打包或运行。下方A1保留旧应用 `1e5f20d9475bca1a8aa2a1a11f98075d2ecc623e` 的历史证据。开发与复核由主 Agent 串行完成，未调用子 Agent，属于主 Agent 自检。

| 验证表面             | 证据                                                       | 当前结论                                               | 不能代替                  |
| -------------------- | ---------------------------------------------------------- | ------------------------------------------------------ | ------------------------- |
| 仓库质量门           | run `44664995322`；13项检查，Linux Rust120/前端32/TUI17    | PASS；2个ignored为被父测试调用的子进程入口             | 真实CLI、原生桌面交互     |
| Windows 后端与前端   | 同run的 `windows-frontend`；Rust111/前端32、类型检查与构建 | PASS；后端另有2个ignored子进程入口                     | WebView与原生文件选择     |
| 浏览器交互历史       | A1应用1e5f20d的CI页面，Playwright Extension、模拟IPC，14项 | PASS；仅属于旧前端                                     | B1恢复命令及真实文件系统  |
| Windows 打包/原生    | run `44657066531`；NSIS artifact `14593348909`             | 包/安装成功；B1保存、重启、预留接管/释放通过，见本节末 | 用户验收、完整暂停与恢复  |
| macOS 打包           | 同run desktop/TUI均成功，DMG artifact `14594266165`        | PASS；此前容量阻塞解除，Mac原生NOT RUN                 | 真机交互与权限验收        |
| 原生接力与运行中恢复 | 两种CLI账号/权限、受管按钮、停止与接续                     | NOT RUN / 尚未实现                                     | 离线测试进程不是真实Agent |

Windows NSIS 包名沿用 `AgentTerm_0.2.0-rc.1_x64-setup.exe`；验收必须同时核对run、提交和SHA-256，不能只看版本名。B1当时的包2,365,118 bytes，SHA-256 `ba35c63d81ee2214d63688ef2aa8de9af6caa92a04950dedce331cee0bfbfb98`。A1历史包为2,337,722 bytes，SHA-256 `f5f3e35105405e2065e04f1c423315240b1a1e1f1b371dbb26082e6f565cbbf8`。均未正式发布。

### A1 Windows 原生桌面结果（2026-11-20）

本段A1专属应用代码为 `1e5f20d`，历史原生控件由Windows UI Automation/Win32操作，调用真实原生IPC与文件系统；没有模拟IPC、启动新浏览器/CDP或调用子Agent。主实例PID38224、竞争实例30532、重启实例33872；收尾均已退出。

此前安装被Low完整性标签阻塞；用户处理后，02:15回读安装包标签为Medium（RID8192），SHA-256与上表一致。安装器PID3188退出0，实际安装 `E:\AgentTerm\app\agentterm.exe`（10,345,984 bytes，SHA-256 `3c57eef2c47dd081b30e4601d1ecbf1ce17560570af76f1bd12985f6073f455a`）与卸载程序。应用窗口实际打开；不再把安装权限列为当前阻塞。安装器退出太快，本轮未采集其进程完整性令牌，不将文件标签值冒充进程测量。

| 顺序 | 原生操作与实际证据                                                                                                                                                  | 结果                                         |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| 1    | 原生选择器登记已有 `E:\dw-modeling-template`，显式初始化默认账本；初始化前文件不存在，之后出现24字节头记录                                                          | PASS                                         |
| 2    | 首实例保存目标/负责人/范围/既有产物目录；快速第二次Invoke被拒绝；另一已打开实例为 `E:\AgentTerm` 提交被后端拒绝，两窗口显示同一taskId，账本只有一次reserved         | PASS；顺序竞争，非同时压力测试；没有持久排队 |
| 3    | 引用 `agentterm-native-artifact.md`，原生预览内容、字节数、SHA-256逐项与磁盘核对                                                                                    | PASS                                         |
| 4    | 改为版本二后刷新，旧预览报“文件内容已变化”；登记后两摘要独立显示，新预览正确；`../../AGENTS.md`被“只接受产物目录内的相对文件路径”拒绝；测试文件最后按备份原字节恢复 | PASS                                         |
| 5    | 原进程取消尚未派发预留，界面回读无主任务，账本追加idle；再次创建新预留，账本为generation2/revision3                                                                 | PASS                                         |
| 6a   | 受控终止两个已核验测试PID后重启；新进程显示同一taskId“待核实”，没有取消/登记/预留入口；账本重启前后哈希一致                                                         | PASS（异常重启保护）                         |
| 6b   | 重启后项目页显示“还没有项目”                                                                                                                                        | GAP：#6最小项目持久化并入#12恢复集成         |
| 6c   | 尝试操作托盘退出菜单但未取得可操作控件；本轮退出用受控进程终止                                                                                                      | 正常菜单退出NOT RUN，不能由异常重启替代      |

A1为**部分通过**，不宣布 #9～#11、恢复闭环或M4完成。真实CLI、Mac原生、正常退出、历史任务浏览和运行中恢复仍未验；未派发预留恢复见B1新证据。原生冲突提示“须排队”不表示已实现队列。

#### 本机证据与保留占用

证据均位于既有 `E:\dw-modeling-template\.trash\2026-11-19`；`agentterm-native-acceptance-session.json`汇总检查与各证据SHA-256，原生UI快照分别为claim、preview-v1、stale、preview-v2、boundary、cancel、restart-before、restarted、projects-after-restart的 `agentterm-native-*-uia.json`。两次任务文档与最终账本快照为 `agentterm-native-evidence-*.txt`；重启界面截图为 `agentterm-native-recovery.png`。

| 声明           | 出处                                     | 回读原文或结果                                                 |
| -------------- | ---------------------------------------- | -------------------------------------------------------------- |
| 仅一个占用     | claim UI快照与最终账本快照               | “已有主任务占用执行位置，当前任务须排队”；第一次只一条reserved |
| 版本失效拒绝   | stale UI快照                             | “文件内容已变化，当前内容不再属于此版本；请登记新版本”         |
| 越界拒绝       | boundary UI快照                          | “只接受产物目录内的相对文件路径”                               |
| 异常重启保留   | restarted UI快照与session中的账本SHA-256 | “待核实”；前后账本字节一致                                     |
| 项目持久化缺口 | projects-after-restart UI快照            | “还没有项目。点击「添加项目」，选择一个本地文件夹即可开始。”   |

A1当时保留task `task-38224-1795112161279866200-1795112776004138000`，项目 `E:\dw-modeling-template`，generation2/revision3、reserved、旧owner。默认账本为 `%LOCALAPPDATA%\agentterm-main-task-ledger.log`，文档位于同一既有目录。B1已通过新入口接管并取消该预留，再验证另一预留重启后释放；当前最终generation3/revision7、idle，原文档保留。没有删除或改用另一账本，详细新证据见B1。

#### 本轮缓存与收尾

预算：运行/临时缓存512 MiB，E盘低于5 GiB停止。实际WebView用户数据目录设在 `E:\AgentTerm\runtime\webview`，该目录实际产生文件；本轮未在D盘构建或下载运行时。原生界面截图与证据保留在上述已有目录。

收尾核对应用及其已识别WebView进程全部退出后，逐级审查专用目录无链接，再将空temp与runtime（285个文件，72,695,077 bytes，约69.33 MiB）送系统回收站；路径已不存在。最终磁盘整体余量C约58.54/D41.82/E37.96 GiB。回收不等于物理空间释放，不把其他进程的磁盘变化归因本任务；账本、测试文档、安装文件和证据保留。`agentterm-native-cleanup.json`为回读记录。

A1发现的项目持久化与未派发恢复缺口已由下方B1切片补验；下一实现为#12受管尝试的持久启动身份与停止核实。正常退出、运行中合法接续、残留进程和完成释放分别留证。主Agent完成声明-证据与机械检查；按用户禁止子Agent要求不调用独立评审，不称cc独立门禁PASS。

### 后续验收进入条件

- A2：B1打包44657066531已成功取得arm64 DMG/TUI；仍需实际Mac验证原生流程，缺运行环境时保留NOT RUN。
- A3：已完成路径/文件格式静态核查，见下方；Windows `.cmd/.ps1` 包装不符合当前适配器限制。实际启动须有明确测试入口和范围，不把产品测试当成开发委派，也不修改用户全局配置。
- B1/B2：按 [台账](tasks.md) 的 #12/#13 覆盖暂停、启动前后崩溃、残留、重复和迟到事件、证据失效、合法完成释放；离线子进程测试与真实 Agent 验收分开。
- B3/B4：安全停止、恢复、证据和工具权限通过后，才验证受管派发按钮、排队与两种真实 CLI 全流程。M4 仍须满足 #9～#14 原验收条件；当前报告不关闭这些功能。
- M5/M6：每项记录实际授权环境、业务对象与当前数据结果；本地 SQL、静态检查、草稿回读、发布态、实例完成和数据正确分别验收。

### A2 macOS 构建重试记录

首轮 job `135708662000`（desktop）与 `135708662319`（TUI）无 runner、无执行步骤；GitHub check annotations 明确报告多次未取得 hosted runner，并指出 macOS arm64 容量不足。这是调度资源失败，不是该次执行中的代码编译失败。

已执行一次 `gh run rerun 44566248894 --repo SophiaData/AgentTerm --failed`，应用提交仍为`1e5f20d`，Windows成功结果沿用。第2次desktop `135742304005`、TUI `135742303296`同样取消；annotations仍为未取得hosted runner与arm64容量不足，无构建步骤，整体failure。当前不再连续重试，不提高付费runner配置，也不重复触发Windows构建；等待runner资源可用后继续A2。

后续B1候选的44598629148及修正包44657066531均四项打包成功，macOS runner构建阻塞已解除。当前应用包以6deebcf/44657066531为准；未运行Mac桌面，旧包失败记录保留。

### A3 CLI 静态前置结果

| CLI    | 已核对入口                                                                        | 文件证据                                                                                  | 当前结论                                                                                 |
| ------ | --------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| Claude | `%USERPROFILE%\.local\bin\claude.exe`                                             | 普通 EXE，文件版本/产品版本 `2.1.326.0`，378,067,616 bytes                                | 符合原生文件格式前置；未运行 `--version`、未启动 Agent，账号、输出协议、hooks 与权限未验 |
| Cursor | `%LOCALAPPDATA%\cursor-agent\cursor-agent.ps1`，同目录 `.cmd` 存在、`.exe` 不存在 | `Get-Command cursor-agent` 命中 PowerShell 包装；仅检查路径与存在性，未读取包装正文或配置 | 不满足当前 Windows 适配器仅接受 EXE 的限制；不自动改用 shell 或扩大权限                  |

文件存在和 PE 版本不证明适配器协议兼容，也不证明具备只读隔离。两种 CLI 的真实接力仍为 NOT RUN；当前记录没有调用子 Agent。

## Single-main-task ledger (feature 9)

The ledger core uses only the Rust standard library. It can be tested independently without installing the desktop dependencies:

```text
rustc --edition 2024 --test src-tauri/src/task_ledger.rs --crate-name task_ledger -o <existing-directory>/task-ledger-tests
<existing-directory>/task-ledger-tests --test-threads=1
```

On Windows use an `.exe` output name. Set `AGENTTERM_LEDGER_TEST_DIR` to an existing disposable directory when desired; the tests otherwise use the existing OS temporary directory. They create uniquely named test files and do not create directories or delete files. Test artifacts are not credentials and are kept for inspection; use the established recoverable cleanup process.

Coverage includes competing threads and independent processes, duplicate and cross-project claims, retained occupancy after process exit, waiting/review/recovery states, stale generation/revision, cancellation limited to undispatched reservations, unknown schema, damaged/truncated records, invalid paths and capacity limits. The ignored `process_claim_helper` is a subprocess entry point used by the process tests, not a skipped acceptance check.

Windows standalone execution: 18 tests passed, with the helper invoked by process tests. A temporary mutation removing the lock acquisition made the OS-lock test fail (exit 101); the mutation is not committed. Independent review identified a first-initialization race; a deterministic test freezes initialization after file creation and proves other operations cannot observe the empty file. This test failed before the fix (exit 101). A stable companion lock now serializes initialization and all reads/writes before touching the data file; simultaneous first initialization and recovery from an empty companion lock are also covered. The normal source is recompiled and all 18 tests pass after the fix. Full desktop command integration and repository checks are run by PR CI; record their actual results in the PR.

This does not verify a task page, CLI dispatch, Teambition integration, safe stopping of external jobs, or real macOS/Windows desktop interaction. The native command wrapper additionally tests unregistered projects, immutable ledger binding, missing directories and restart ownership. Manual terminal sessions remain outside the ledger until dispatcher integration.

- `python scripts/verify_project.py` validates project instructions, skill metadata and local Markdown links.
- `python scripts/run_quality.py` runs configured commands in `project-policy.json` **without a shell**.
- An empty codebase can pass governance checks; when code appears, empty quality commands become an error.
- Add success, failure and relevant edge/regression cases; do not delete tests to silence failures.
- Treat `NOT RUN`, skipped tests and blocked integration tests honestly; never claim an installed runtime was validated by static checks alone.
- Record command, status, environment and evidence in every PR.

Stack examples (only use when applicable and installed): Python `python -m pytest -q`, Node `pnpm test`, Java `mvn test`, DBT `dbt test`. SQL engine tests require an appropriate isolated environment.

## Sequential CLI relay (feature 10 backend slice)

Run `cargo test --manifest-path src-tauri/Cargo.toml task_relay` and
`cargo test --manifest-path src-tauri/Cargo.toml task_process`, followed by `pnpm check`.
The process layer also supports a standalone Windows test build:

```text
rustc --edition 2024 --test src-tauri/src/task_process.rs --crate-name task_process -o <existing-directory>/task-process-tests.exe
<existing-directory>/task-process-tests.exe --test-threads=1
```

The seven Windows process checks cover actual stdin/output and process termination,
missing/invalid executables, nonzero exit, timeout, bounded output and a surviving
descendant. The ignored `process_fixture` is invoked as a real child by these tests.
Relay tests validate both adapter protocols with a compiled native CLI fixture,
including actual argument/stdin handling, structured result output, process exit
and sequential launch. Further tests cover competing dispatches, stale tickets,
changed ledger revision, wrong project/owner, missing CLIs, all process failure
classes, report identity/role/status/schema, blocked worker, reused review session
and a failed review retaining occupancy. Test files use existing OS temp directories;
no new directory or deletion is needed.

Fixtures do not authenticate or call an AI service. Record full CI and actual test
counts in the PR. Not verified: real Claude/Cursor accounts and installed versions,
macOS desktop interaction, CLI-side permission enforcement, escaped Unix processes,
Windows batch wrappers (deliberately rejected), running-attempt recovery/release or production
tools. Task UI and persisted report artifacts now have the separate feature 11
evidence below. The native API is not exposed to the
renderer until those runtime permissions are accepted; #10 remains pending acceptance.

## Task workbench and artifacts (feature 11)

Run the repository `pnpm check`. Focused checks are
`cargo test --manifest-path src-tauri/Cargo.toml task_document`,
`cargo test --manifest-path src-tauri/Cargo.toml task_commands`,
`cargo test --manifest-path src-tauri/Cargo.toml task_relay` and
`pnpm test -- src/taskWorkbench.test.tsx src/App.test.tsx`.

Backend coverage includes persistent plan reopen, immutable metadata, current
task/owner/version checks, invalid input before occupancy, duplicate/conflicting/
out-of-order events, checksum/truncation/schema failure, locked journals and explicit unlock with duplicated handles, existing
directories only, relative artifact boundaries, symlink rejection, byte/content
limits, version changes and stale preview refusal. Recorded relay tests persist
both stages and prove document failure before spawn or before handoff prevents
the next process. No real AI service, production SQL or project file write is used.

Frontend tests cover distinct main-task/attempt states, goal/owner/scope/version
display, duplicate/out-of-order event reduction, late snapshot/request rejection,
recovery-only viewing, missing-plan state, reservation-only cancellation, and
escaping report/file text. Static React rendering is not desktop interaction.
Record actual complete test counts and CI commit in the PR.

The `windows-frontend` PR job runs `pnpm test` and `pnpm build` on Windows.
This reproduces the case-insensitive module resolution used by the Windows
desktop build: `TaskWorkbench.tsx` and the old `taskWorkbench.ts` helper caused
TS1192/TS1261/TS2305/TS1149 in package run `44565167859`, although Linux quality
checks passed. The unchanged helper is now `workbenchState.ts`. A successful job
also retains the exact compiled frontend for seven days for browser interaction
acceptance. Browser checks with simulated IPC validate UI behavior only; they do
not validate the native ledger, file picker, CLI, or desktop WebView.

Manual acceptance using a package from the exact PR head (merge is not required):
open the task workbench, add
an existing project through the projects tab, explicitly initialize the local
ledger, reserve one task with an existing output directory, reference a small
existing SQL/report file and preview it. Change that disposable file externally,
refresh, and confirm the old version is refused before registering the new one.
Try another task while occupied; it must not claim a second slot. Cancel only an
undispatched reservation. No real CLI launch is available yet.

The 2026-11-20 browser pass covers 14 DOM interaction checks with simulated IPC:
initialization, simulated project selection, required fields, invalid-directory
error feedback, duplicate-submit suppression, hiding creation while occupied,
plan readback, plain-text preview, stale-version refusal, version listing,
refresh-failure disabled actions, recovery-only viewing and reservation cancel.
This is frontend evidence only; backend behavior is simulated and cannot be
counted as a native filesystem or permission test.

Unit/CI tests alone do not verify live desktop interaction; Windows component evidence is recorded in A1 above. Still unverified: macOS live WebView and file picker interaction,
real CLI permissions and accounts, historical-task browsing, automatic refresh,
running-attempt recovery/release or tool-side production boundaries. B1 separately verifies undispatched reservation recovery below.

## V2 gate

- `TEMPLATE_VERSION` and `project-policy.json.template_version` must agree.
- Once application source exists, `quality_checks` must include at least one
  test check, with an argv array. `lint`/`build` alone do not replace tests.
- The runner rejects obvious placeholder commands such as `echo` or `true`.
  It cannot determine whether arbitrary test suites are meaningful; code review
  and branch policy must still verify changes and enforcement.
- Template contract tests run in CI before repository-specific checks.
- Repository owners must configure real dependency installation and any secret
  fixtures through appropriately restricted GitHub Actions environments.

## B1 项目持久化与未派发预留恢复（2026-11-20）

本节为新实现的验收记录，不能沿用上方旧包A1结果。主Agent单线程实施与自检，不调用子Agent。

- 本地Windows独立账本：`rustc --edition 2024 --test src-tauri/src/task_ledger.rs --crate-name task_ledger -o <existing-directory>/ledger-tests.exe`；`ledger-tests.exe --test-threads=1` → 24 passed，1 ignored helper。真实子进程退出、旧票据/owner、并发接管、未知/存活/复用PID、运行中拒绝恢复、损坏及截断均覆盖。
- 完整检查须执行 `pnpm check`；项目存储覆盖重开、多实例读取、连续追加与重开、重复项/未知schema/损坏/超限、截断尾部、摘要/版本不匹配、旧pending残留、锁冲突、非法目录与软链接。命令层覆盖文档保留、缺项目拒绝接管、释放保留记录、已有执行事件拒绝恢复；前端覆盖允许的按钮和运行中禁止按钮。首候选de4a28f的CI44596777931与打包44598629148全部通过；本机安装后的项目存储rename失败，见下方原生修正记录，不能据此判新功能通过。
- 新包6deebcf原生通过：项目保存及重启重载；旧预留接管后计划哈希不变且可取消；第二实例接管存活owner被拒、账本字节不变；取消后新建预留、异常重启仍占用、核实释放后回到可创建任务表单且文档不变。缺项目拒绝来自de4a28f原生记录；损坏/截断等由自动测试覆盖，未改真实项目文件做原生破坏试验。真实默认账本只通过产品入口操作。
- 接管写入新阶段recoveredReserved，旧版本遇到该历史记录失败关闭；应用回滚不能直接继续使用已接管账本，需保留原件并核实。未接管的schema1历史正常读取，不静默重写。
- 当前恢复仅限从未派发状态。正在执行/验收/等待/暂停/未知状态均不由该入口释放；没有停止运行进程、撤回外部作业或自动重派的能力。正常托盘退出和最后工具记忆仍NOT RUN/未实现。

### 原生发现与修正：项目存储重命名失败

2026-11-20首候选de4a28f安装并打开后，旧任务读取及“缺项目拒绝接管”通过，账本不变；添加项目生成137字节pending，但目标项目文件未生成。独立原生Rust探针在同一实际目录验证create/write/sync成功、rename返回 `CrossesDevices / os error 17`。未确定底层文件系统为何报告跨设备，不归因于特定软件。失败pending已按内容/哈希备份，任务计划和账本未修改。

修正为同一路径受锁追加schema2完整快照，连续revision与SHA-256校验；不依赖rename。半条追加、未知版本、错摘要/重复revision一律停止且保留原文件，不静默回滚旧快照。保留1 MiB总日志/200项目上限。旧schema1单文件兼容读取并保留原字节；旧pending仍需先核实、备份，不能自动导入或清空。#6原验收标准不修改，当前替换策略与完整记忆功能的验收限制明确记录。

该修正的CI与Windows原生结果如下，de4a28f失败不被覆盖为PASS。

### B1 修正包与原生结果

应用6deebcf的打包run44657066531全部成功。Windows archive artifact14593348909共2,365,300 bytes，SHA-256 `3b10e71dd5d507e1263c51c7c740dca413459ea44ed4f99044366b3b73d413bc`；单一NSIS文件2,365,118 bytes，摘要见本页首段。静默安装退出0，`E:\AgentTerm\app\agentterm.exe`为10,458,624 bytes，SHA-256 `191a5c846226bd43f696c4cf9d7e03917e73ad20b98e5fc53e8486c8de52cf63`。安装前应用、默认账本及旧任务文档已备份。

CI44664995322绑定e24ecfdd：13项检查通过，Linux后端120/Windows后端111/前端32/TUI17；两端各2个ignored为父测试实际调用的子进程入口。6deebcf→e24ecfdd仅补测试FakeChild的Windows句柄方法与规范根路径期望，不改运行时代码。首次新增Windows后端检查发现的编译/断言失败保留为失败记录，未跳过测试。

| 原生步骤           | 实际结果                                                                                                                                  | 证据文件（统一前缀 `agentterm-recovery-journal-`）   |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| 原生选择已有项目   | schema2/revision1快照落盘，无pending；页面显示项目                                                                                        | `project-saved-uia.json`                             |
| 接管A1旧预留       | taskId/generation2不变，revision3→4、recoveredReserved；计划SHA-256仍为`376bfcd884d8d6483101a0981dad1b5710f3f32a1ad614591292b4c51afcfffb` | `takeover-uia.json`、`acceptance.json`               |
| 存活进程阻止抢占   | PID21188仍在时，PID35028请求接管被后端拒绝；账本SHA-256仍为`e2186f6848c597811beefd2e864b099438da3ba90b57276c033446d02e3fd3fa`             | `live-owner-blocked-uia.json`、`second-session.json` |
| 取消后新建预留     | revision5为idle，随后generation3/revision6为reserved                                                                                      | `cancel-uia.json`、`release-plan-uia.json`           |
| 受控异常终止后重开 | PID38764显示同一taskId待核实；项目列表重载成功                                                                                            | `restarted-uia.json`、`restarted-projects-uia.json`  |
| 明确核实释放       | generation3/revision7为idle；任务文档和项目文件哈希不变，创建表单重新可用                                                                 | `released-uia.json`、`restart-evidence.json`         |

第二实例接管被拒后，错误态按现有规则禁用任务操作，需刷新再重试；后续“释放”点击因按钮禁用未实际提交，不记为另一项原生拒绝测试。选择项目时先确认应用窗口在前台，再核对实际选中值；只给控件焦点而未激活窗口的键盘操作未算通过。未启动真实CLI，未写生产或Teambition。

证据目录仍为既有 `E:\dw-modeling-template\.trash\2026-11-19`。当前账本idle、文档保留；受控终止测试应用后，其7个已识别应用/WebView进程均退出。将72,777,786 bytes（约69.41 MiB）专用WebView缓存和空temp送回收站；11:10磁盘回读C57.46/D41.82/E35.46 GiB。回收不算物理空间释放。汇总为 `agentterm-recovery-journal-acceptance.json`、`agentterm-recovery-journal-cleanup.json`，CI为 `agentterm-recovery-ci-evidence.json`。

此切片不关闭#6/#12或M4。正常托盘退出、Mac原生、真实CLI权限、运行中停止/接续与外部作业撤销仍NOT RUN/未实现。下一项先实现启动意图、实际启动身份和停止确认，并测试崩溃窗口、残留、重复请求和迟到回报；未知状态继续占用。

## 任务页交互优化（2026-11-20）

本轮只改前端现有流程，后端协议及授权边界不变。检查命令仍为`pnpm check`；新增回归覆盖唯一项目选择、已移除选择不自动替换、空白计划拒绝、读失败禁用与操作失败可纠正、预览摘要匹配、错误执行状态文案与取消确认初始不可直接提交。

应用提交 `cc9250dd2017744ba1aa5001a4d72cfe8e09e743` 的CI44684766030两个job成功：13项检查、Linux后端120/Windows后端111/前端37/TUI17；两端后端各2个ignored为子进程测试入口。这个run早于新增“测试单worker”的明确要求，不追记为串行。提交ce3dd400设置Vitest单worker/禁止文件并行、Rust测试与构建各1线程/任务、Windows job依赖Linux；串行run44687258466两个job均通过，数量同上；Linux于06:27:49Z结束，Windows于06:27:50Z开始，实际顺序已核对，日志确认单worker与Rust单线程设置。它仅改测试配置，不改变应用包运行时代码。

使用CI前端artifact14606191496（168,914 bytes，SHA-256 `b3cc6fc364feecd3b82a8627216cc86f223989cbdda741593327e2d9d16d1808`），在已连接Playwright Extension的本地专用标签执行18项交互验证，全部PASS。后端为明确模拟IPC，没有访问真实项目、执行CLI或生产调用。

| 交互组     | 已核对的结果                                                                                      |
| ---------- | ------------------------------------------------------------------------------------------------- |
| 首次使用   | 启用任务记录；在工作台添加项目并自动选中                                                          |
| 计划输入   | 切页往返保留目标/范围；错误目录保存失败后输入保留且可修正；双击只提交一次；保存后显示真实能力限制 |
| 文件与错误 | 错路径可纠正；预览按纯文本显示；旧版拒绝后其他任务操作仍可用；读取失败禁用，重新读取成功后恢复    |
| 取消与恢复 | 取消需确认并聚焦；返回不取消；窗口focus核对归属；拒绝接管后可重试；释放需确认，确认后回到空表单   |
| 布局       | 390px任务页和计划表单无横向溢出；1100px截图已视觉检查                                             |

一次长evaluate工具调用超时，未按超时结果宣告通过；连接恢复后读取实际已完成条目，并逐步补齐剩余检查。证据在既有 `E:\dw-modeling-template\.trash\2026-11-19`：`agentterm-ux-browser-results.txt`、`agentterm-ux-task-desktop.png`、`agentterm-ux-plan-desktop.png`，前端包绑定见 `agentterm-ux-frontend-evidence.json`。

Windows/macOS桌面与TUI打包run44684947681四项成功。Windows archive artifact14605584167为2,370,499 bytes，SHA-256 `56f9f70510993f958f35373cb577d1cd54266520182aeef0a04bb658c021bb13`；单一NSIS为2,370,317 bytes，SHA-256 `7e1de9721e15bdf9cbc9cfa73bbd6d1408985c7e04501cc0bb1f58d99def156c`。安装在既有 `E:\AgentTerm\app`，新EXE为10,462,720 bytes，SHA-256 `588da88257c9cf33260cbc42b14277d5ffb68bd3bd3de41685b41abfb06fd2da`。安装器超出首次20秒等待后退出，退出码未捕获；安装成功依据为实际字节变更及新版UI运行，不伪造exit0。

**本机状态变化与原生限制：** UX安装前默认 `agentterm-main-task-ledger.log`、`agentterm-projects.json`及对应锁文件均不存在，原因未查明；B1旧应用EXE哈希仍正确。未搜索其他私人目录、重建数据或用旧备份替换当前状态。原生PID19632打开后显示首次启用引导，工作台→独立终端→工作台切换通过，4个数据路径依然不存在。完整保存/文件/恢复操作未在新包重新原生验收，不能将18项模拟IPC作为替代。证据 `agentterm-ux-native-package.json`、`agentterm-ux-native-acceptance.json`及3份原生UI快照。

本轮应用和专用WebView退出后，9,377,443 bytes（约8.94 MiB）缓存与空temp移回收站；14:27磁盘D41.82/E35.00 GiB。预览服务18429与本轮标签已关闭。回收不算物理空间释放，未清默认任务数据。正常托盘退出、Mac原生、真实CLI、完整运行中恢复仍未验；用户验收、合并和正式发布未进行。

## CLI交互修复验收（2026-11-20，自动检查通过、交互待验）

代码提交 `3b40d8e109b72164c2b5dffb4e3c4b9dcbc9a38f`，草稿PR30，基于PR29。主Agent串行实施与自检，无子Agent。

| 表面             | 结果与证据                                                                                                                    | 限制                                                 |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| Linux质量门      | CI44701366234，pnpm check全部13项通过，前端50、Rust120、TUI17                                                                 | 2个ignored为既有子进程测试入口                       |
| Windows          | 同run，前端50通过、类型检查与构建通过、Rust111通过                                                                            | 2个ignored为既有子进程入口；不是桌面验收             |
| 新增回归         | 11项通信测试：快照重叠、提前退出、迟到订阅/快照、订阅失败、快照失败、顺序输入、失败不重发、输入/输出上限、超时；2项路径测试   | 合计前端由37增至50；未新增DOM依赖                    |
| 前端产物         | artifact14614110431，170466 bytes，SHA256 6fc6711f3a2f88bb6c191f1e936107090e471c89a3de3cacb190e4140fdc9c8d                    | 原始CI产物，不是桌面安装包                           |
| 实际页面         | BLOCKED，0项通过；后台预览服务Start-Process被自动审批拒绝，未返回具体原因；file协议与本地文件选择也被Playwright Extension拒绝 | 没有绕过限制；自有空白验收标签已关闭                 |
| 真实CLI/原生桌面 | NOT RUN；未构建新包、未改本机已安装应用或CLI账号配置                                                                          | 输入/输出/关闭的真实PTY行为、Mac桌面和用户验收均未验 |

前4次候选检查分别暴露格式、lint、格式和测试mock泛型问题，均已修正；最终候选两端通过，不改写历史失败记录。改动只有13个文件，2个新增文件均位于既有src/terminal，无新依赖、后端修改或新目录。

尚需实际页面检查切页后同一终端保持、导航状态、中文输入、输入失败重连不重发、停止确认/失败保留/自然退出竞争、重复点击及窄屏布局；没有证据不宣布CLI体验验收完成。受管派发、进程树恢复和工具权限沿原计划继续。

## 原生补验与用户截图纠正（2026-11-20）

PR30最终文档CI44704655402成功；ace5f3ba仅更改串行/可选平台打包，CI44712895553成功，Windows-only打包44714802656成功，artifact14620180026。安装器2371061 bytes、SHA256 1af6d275016d81d4932ff49f46bdb9bc3f9de7de590c3ef17fe6174a905934ce，退出0；实际应用10464256 bytes。本轮旧包原生测试PID29752、回显PID24476，输入仅为固定测试文本，无模型调用。项目通过原生选择器登记E:/AgentTerm，任务账本未初始化。

回显首屏、中文输入、切回任务页再返回后PID和输出保留、结束确认及停止后PID消失有原生UIA/截图证据。两次ASCII输入分别预期CLI_TEST_123、ASCII_1234567890，但截图分别显示CLI_TEST_23、ASCII_234567890：存在缺字符，原因尚未定位，不能将全部输入验收标PASS；也没有真实AI CLI审批/快捷键/恢复证据。用户纠正产品方向后停止测试应用和回显进程。

新的项目/CLI会话工作区修改默认入口，任务记录改辅助抽屉；本轮默认入口与外部会话边界回归已通过，合计前端51项；CI44719966384两端成功，实际包和真实CLI仍待验收。旧布局截图和回显不冒充新工作区验收。全程主Agent顺序实施与自检，无子Agent；已知ASCII问题和已有会话接管仍明确未解决。
