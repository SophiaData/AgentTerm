# Testing policy

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
Windows batch wrappers (deliberately rejected), task UI, persisted report artifacts,
safe recovery/release or production tools. The native API is not exposed to the
renderer until those runtime permissions are accepted; #10 remains pending acceptance.

## Task workbench and artifacts (feature 11)

Run the repository `pnpm check`. Focused checks are
`cargo test --manifest-path src-tauri/Cargo.toml task_document`,
`cargo test --manifest-path src-tauri/Cargo.toml task_commands`,
`cargo test --manifest-path src-tauri/Cargo.toml task_relay` and
`pnpm test -- src/taskWorkbench.test.tsx src/App.test.tsx`.

Backend coverage includes persistent plan reopen, immutable metadata, current
task/owner/version checks, invalid input before occupancy, duplicate/conflicting/
out-of-order events, checksum/truncation/schema failure, locked journals, existing
directories only, relative artifact boundaries, symlink rejection, byte/content
limits, version changes and stale preview refusal. Recorded relay tests persist
both stages and prove document failure before spawn or before handoff prevents
the next process. No real AI service, production SQL or project file write is used.

Frontend tests cover distinct main-task/attempt states, goal/owner/scope/version
display, duplicate/out-of-order event reduction, late snapshot/request rejection,
recovery-only viewing, missing-plan state, reservation-only cancellation, and
escaping report/file text. Static React rendering is not desktop interaction.
Record actual complete test counts and CI commit in the PR.

Manual acceptance after the stacked PRs are merged: open the task workbench, add
an existing project through the projects tab, explicitly initialize the local
ledger, reserve one task with an existing output directory, reference a small
existing SQL/report file and preview it. Change that disposable file externally,
refresh, and confirm the old version is refused before registering the new one.
Try another task while occupied; it must not claim a second slot. Cancel only an
undispatched reservation. No real CLI launch is available yet.

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
