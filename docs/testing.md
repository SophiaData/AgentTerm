# Testing policy

## 验收现状与下一项（2026-11-20）

当前PR30为CLI交互修复，代码3b40d8e109b72164c2b5dffb4e3c4b9dcbc9a38f；CI44701366234的Linux与Windows两个job顺序通过，前端50项通过。页面实测因预览服务审批与浏览器文件访问限制未完成，原生包未重建/安装，不能宣称用户可见运行版本已更新。以下PR29和B1为历史证据。

当前PR29交互优化应用为cc9250dd，CI44684766030与打包44684947681通过；前端37项、18项构建页面模拟IPC交互通过。新包Windows原生仅验证启动与切页：升级前默认项目/台账及锁文件已不存在，未重建或恢复旧备份，完整写入流程未重验。后续ce3dd400仅变更单worker/串行质量检查配置；见文末本轮证据，不沿用下方B1历史状态冒充当前状态。

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
