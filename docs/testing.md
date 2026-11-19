# Testing policy

## 验收现状与下一项（2026-11-20）

应用代码证据绑定 `1e5f20d9475bca1a8aa2a1a11f98075d2ecc623e`，对应未合并草稿 PR #27；后续文档更新不代表安装包已按新文档提交重新运行。开发与复核由主 Agent 串行完成，未调用子 Agent；本轮结论为主 Agent 自检。

| 验证表面       | 证据                                                                                | 当前结论                                                            | 不能代替                               |
| -------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------- | -------------------------------------- |
| 仓库质量门     | run `44566178018`；13项配置检查，Rust102/前端30/TUI17；2个 ignored 为子进程测试入口 | PASS                                                                | 真实 CLI、原生桌面交互                 |
| Windows 前端   | 同 run 的 `windows-frontend`；30项测试、类型检查与构建                              | PASS                                                                | Windows WebView、原生文件选择          |
| 浏览器交互     | 同提交的 CI 页面构建；现有 Playwright Extension、DOM 事件、模拟 IPC；14项           | PASS（模拟后端）                                                    | 原生账本、文件系统、真实权限与生产边界 |
| Windows 打包   | run `44566248894`；NSIS artifact `14549924822` 与 TUI artifact `14551448972`        | 构建/安装成功；Windows原生主要流程通过，重启项目列表有缺口          | 原生功能验收、用户验收                 |
| macOS 打包     | 同 run 两次尝试的 desktop/TUI jobs；check annotations                               | 两次均CANCELLED：arm64 runner容量不足，未执行构建步骤；停止连续重试 | 没有已验证成功包，也没有真机验收       |
| 原生接力与恢复 | 两种 CLI 原生账号/权限、受管按钮、暂停/恢复                                         | NOT RUN / 尚未实现的部分见台账                                      | 离线测试进程不是真实 Agent             |

Windows NSIS 包名沿用 `AgentTerm_0.2.0-rc.1_x64-setup.exe`；验收必须同时核对 run、提交和 SHA-256，不能只看版本名。文件为 2,337,722 bytes，SHA-256 `f5f3e35105405e2065e04f1c423315240b1a1e1f1b371dbb26082e6f565cbbf8`。未正式发布。

### A1 Windows 原生桌面结果（2026-11-20）

应用代码仍为上表 `1e5f20d`，本轮原生控件由Windows UI Automation/Win32操作，调用真实原生IPC与文件系统；没有模拟IPC、启动新浏览器/CDP或调用子Agent。主实例PID38224、竞争实例30532、重启实例33872；收尾均已退出。

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

A1为**部分通过**，不宣布 #9～#11、恢复闭环或M4完成。真实CLI、Mac原生、正常退出、历史任务浏览和安全恢复释放仍未验；原生冲突提示“须排队”不表示已实现队列。

#### 本机证据与保留占用

证据均位于既有 `E:\dw-modeling-template\.trash\2026-11-19`；`agentterm-native-acceptance-session.json`汇总检查与各证据SHA-256，原生UI快照分别为claim、preview-v1、stale、preview-v2、boundary、cancel、restart-before、restarted、projects-after-restart的 `agentterm-native-*-uia.json`。两次任务文档与最终账本快照为 `agentterm-native-evidence-*.txt`；重启界面截图为 `agentterm-native-recovery.png`。

| 声明           | 出处                                     | 回读原文或结果                                                 |
| -------------- | ---------------------------------------- | -------------------------------------------------------------- |
| 仅一个占用     | claim UI快照与最终账本快照               | “已有主任务占用执行位置，当前任务须排队”；第一次只一条reserved |
| 版本失效拒绝   | stale UI快照                             | “文件内容已变化，当前内容不再属于此版本；请登记新版本”         |
| 越界拒绝       | boundary UI快照                          | “只接受产物目录内的相对文件路径”                               |
| 异常重启保留   | restarted UI快照与session中的账本SHA-256 | “待核实”；前后账本字节一致                                     |
| 项目持久化缺口 | projects-after-restart UI快照            | “还没有项目。点击「添加项目」，选择一个本地文件夹即可开始。”   |

最终保留task `task-38224-1795112161279866200-1795112776004138000`，项目 `E:\dw-modeling-template`，generation2/revision3，状态reserved、旧owner，尚未派发。默认账本为 `%LOCALAPPDATA%\agentterm-main-task-ledger.log`；两份任务文档位于同一既有目录。当前应用进程均退出，后续打开应只读待核实；只通过后续经验证的#12流程核实并恢复/释放，不删除或改用另一账本。

#### 本轮缓存与收尾

预算：运行/临时缓存512 MiB，E盘低于5 GiB停止。实际WebView用户数据目录设在 `E:\AgentTerm\runtime\webview`，该目录实际产生文件；本轮未在D盘构建或下载运行时。原生界面截图与证据保留在上述已有目录。

收尾核对应用及其已识别WebView进程全部退出后，逐级审查专用目录无链接，再将空temp与runtime（285个文件，72,695,077 bytes，约69.33 MiB）送系统回收站；路径已不存在。最终磁盘整体余量C约58.54/D41.82/E37.96 GiB。回收不等于物理空间释放，不把其他进程的磁盘变化归因本任务；账本、测试文档、安装文件和证据保留。`agentterm-native-cleanup.json`为回读记录。

下一实现任务为B1 #12与#6最小项目持久化；正常退出需补验，异常终止、合法接续、安全释放与残留进程判定分别留证。主Agent完成声明-证据与机械检查；按用户禁止子Agent要求不调用独立评审，不称cc独立门禁PASS。

### 后续验收进入条件

- A2：已查明首轮取消原因并发起一次重试，见下方记录；成功后使用实际 Mac 验证 A1 的对应流程。缺 Mac 运行环境时明确保留原生未验。
- A3：已完成路径/文件格式静态核查，见下方；Windows `.cmd/.ps1` 包装不符合当前适配器限制。实际启动须有明确测试入口和范围，不把产品测试当成开发委派，也不修改用户全局配置。
- B1/B2：按 [台账](tasks.md) 的 #12/#13 覆盖暂停、启动前后崩溃、残留、重复和迟到事件、证据失效、合法完成释放；离线子进程测试与真实 Agent 验收分开。
- B3/B4：安全停止、恢复、证据和工具权限通过后，才验证受管派发按钮、排队与两种真实 CLI 全流程。M4 仍须满足 #9～#14 原验收条件；当前报告不关闭这些功能。
- M5/M6：每项记录实际授权环境、业务对象与当前数据结果；本地 SQL、静态检查、草稿回读、发布态、实例完成和数据正确分别验收。

### A2 macOS 构建重试记录

首轮 job `135708662000`（desktop）与 `135708662319`（TUI）无 runner、无执行步骤；GitHub check annotations 明确报告多次未取得 hosted runner，并指出 macOS arm64 容量不足。这是调度资源失败，不是该次执行中的代码编译失败。

已执行一次 `gh run rerun 44566248894 --repo SophiaData/AgentTerm --failed`，应用提交仍为`1e5f20d`，Windows成功结果沿用。第2次desktop `135742304005`、TUI `135742303296`同样取消；annotations仍为未取得hosted runner与arm64容量不足，无构建步骤，整体failure。当前不再连续重试，不提高付费runner配置，也不重复触发Windows构建；等待runner资源可用后继续A2。

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
Windows batch wrappers (deliberately rejected), safe recovery/release or production
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
safe recovery/release or tool-side production boundaries.

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
- 完整检查须执行 `pnpm check`；项目存储覆盖重开、多实例读取、原子替换已有文件、重复项/未知schema/损坏/超限、pending中断残留、锁冲突、非法目录与软链接。命令层覆盖文档保留、缺项目拒绝接管、释放保留记录、已有执行事件拒绝恢复；前端覆盖允许的按钮和运行中禁止按钮。当前完整CI结果以本切片PR回读为准，尚未取得时记NOT RUN。
- 新包原生待验：重启后项目仍在；旧未派发任务接管后计划/产物不变且可取消；缺项目先重新登记；第二个仍在运行的实例不能抢占；明确释放后可建新任务；损坏项目文件应显示错误且不覆盖。真实OS默认账本只通过新入口操作，不删除或换路径。
- 接管写入新阶段recoveredReserved，旧版本遇到该历史记录失败关闭；应用回滚不能直接继续使用已接管账本，需保留原件并核实。未接管的schema1历史正常读取，不静默重写。
- 当前恢复仅限从未派发状态。正在执行/验收/等待/暂停/未知状态均不由该入口释放；没有停止运行进程、撤回外部作业或自动重派的能力。正常托盘退出和最后工具记忆仍NOT RUN/未实现。
