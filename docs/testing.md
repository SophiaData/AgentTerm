# Testing policy

## 验收现状与下一项（2026-11-20）

应用代码证据绑定 `1e5f20d9475bca1a8aa2a1a11f98075d2ecc623e`，对应未合并草稿 PR #27；后续文档更新不代表安装包已按新文档提交重新运行。开发与复核由主 Agent 串行完成，未调用子 Agent；本轮结论为主 Agent 自检。

| 验证表面       | 证据                                                                                | 当前结论                                       | 不能代替                               |
| -------------- | ----------------------------------------------------------------------------------- | ---------------------------------------------- | -------------------------------------- |
| 仓库质量门     | run `44566178018`；13项配置检查，Rust102/前端30/TUI17；2个 ignored 为子进程测试入口 | PASS                                           | 真实 CLI、原生桌面交互                 |
| Windows 前端   | 同 run 的 `windows-frontend`；30项测试、类型检查与构建                              | PASS                                           | Windows WebView、原生文件选择          |
| 浏览器交互     | 同提交的 CI 页面构建；现有 Playwright Extension、DOM 事件、模拟 IPC；14项           | PASS（模拟后端）                               | 原生账本、文件系统、真实权限与生产边界 |
| Windows 打包   | run `44566248894`；NSIS artifact `14549924822` 与 TUI artifact `14551448972`        | SUCCESS，尚未安装运行                          | 原生功能验收、用户验收                 |
| macOS 打包     | 同 run 的 desktop/TUI jobs                                                          | CANCELLED；整体 run 为 failure；取消原因未核实 | 没有可用成功包，也没有真机验收         |
| 原生接力与恢复 | 两种 CLI 原生账号/权限、受管按钮、暂停/恢复                                         | NOT RUN / 尚未实现的部分见台账                 | 离线测试进程不是真实 Agent             |

Windows NSIS 包名沿用 `AgentTerm_0.2.0-rc.1_x64-setup.exe`；验收必须同时核对 run、提交和 SHA-256，不能只看版本名。文件为 2,337,722 bytes，SHA-256 `f5f3e35105405e2065e04f1c423315240b1a1e1f1b371dbb26082e6f565cbbf8`。未正式发布。

### A1 Windows 原生桌面清单（唯一下一项）

使用上述包和已有的授权项目、已有输出目录、可恢复测试文件。操作前记录环境、应用来源、现有账本是否有占用；有未知占用先核实，不能清空文件强行继续。安装目标不得违反当前目录约束；若安装器必须新建目录且没有合规的既有运行环境，明确记录阻塞，不静默执行安装。所需桌面通道不可用时记 NOT RUN。

| 顺序 | 操作                                                                      | 必须看到的真实结果                                                                     | 当前状态 |
| ---- | ------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- | -------- |
| 1    | 进入工作台，通过原生选择器添加已有项目，显式初始化账本                    | 项目归属正确；没有创建项目/产物目录；初始化不覆盖现有占用                              | NOT RUN  |
| 2    | 填目标、负责人、范围与已有产物目录，连续提交预留；另一个窗口/实例尝试竞争 | 只一次成功；真实后端拒绝第二次占用，目标与步骤回读一致；当前仅冲突反馈，尚无队列       | NOT RUN  |
| 3    | 登记一个已有 SQL/Markdown 测试文件，预览并与文件核对                      | 相对路径、内容、字节数及 SHA-256 对应；不执行 SQL，正文作为文本显示                    | NOT RUN  |
| 4    | 变更可恢复测试文件后查看旧版本，再登记新版本；尝试越界路径                | 旧版本拒绝冒充当前文件，新版本单独列出；越界由原生入口拒绝；测试文件按原字节恢复       | NOT RUN  |
| 5    | 取消同一进程中尚未派发的预留，再新建一项测试预留                          | 只有未派发且版本/归属匹配时释放；不误删文档或项目文件                                  | NOT RUN  |
| 6    | 保留这项预留并退出重启，检查原生回读及项目列表                            | 旧归属显示只读待核实；不能自动重派、取消或释放；如项目列表未恢复，登记 #6/#12 集成缺口 | NOT RUN  |

第6步是最后一步，会按当前实现留下待核实占用；它用于验证保护，不代表恢复已经完成。后续只能通过经验证的 #12 流程处理，不删除/另开账本绕过。逐步记录提交、OS、步骤、期望、实际、证据位置、PASS/FAIL/NOT RUN及遗留占用；只有全部真实执行才报 A1 通过。

### 后续验收进入条件

- A2：先查明 macOS 取消原因再重跑包；成功后使用实际 Mac 验证 A1 的对应流程。缺 Mac 运行环境时明确保留原生未验。
- A3：登记两种 CLI 的版本、路径、可执行格式及权限差距；Windows `.cmd` 等包装不符合当前适配器限制。实际启动须有明确测试入口和范围，不把产品测试当成开发委派，也不修改用户全局配置。
- B1/B2：按 [台账](tasks.md) 的 #12/#13 覆盖暂停、启动前后崩溃、残留、重复和迟到事件、证据失效、合法完成释放；离线子进程测试与真实 Agent 验收分开。
- B3/B4：安全停止、恢复、证据和工具权限通过后，才验证受管派发按钮、排队与两种真实 CLI 全流程。M4 仍须满足 #9～#14 原验收条件；当前报告不关闭这些功能。
- M5/M6：每项记录实际授权环境、业务对象与当前数据结果；本地 SQL、静态检查、草稿回读、发布态、实例完成和数据正确分别验收。

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

Not verified by unit/CI tests: macOS/Windows live WebView and file picker interaction,
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
