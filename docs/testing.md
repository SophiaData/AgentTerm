# Testing policy

## Single-main-task ledger (feature 9)

The ledger core uses only the Rust standard library. It can be tested independently without installing the desktop dependencies:

```text
rustc --edition 2024 --test src-tauri/src/task_ledger.rs --crate-name task_ledger -o <existing-directory>/task-ledger-tests
<existing-directory>/task-ledger-tests --test-threads=1
```

On Windows use an `.exe` output name. Set `AGENTTERM_LEDGER_TEST_DIR` to an existing disposable directory when desired; the tests otherwise use the existing OS temporary directory. They create uniquely named test files and do not create directories or delete files. Test artifacts are not credentials and are kept for inspection; use the established recoverable cleanup process.

Coverage includes competing threads and independent processes, duplicate and cross-project claims, retained occupancy after process exit, waiting/review/recovery states, stale generation/revision, cancellation limited to undispatched reservations, unknown schema, damaged/truncated records, invalid paths and capacity limits. The ignored `process_claim_helper` is a subprocess entry point used by the process tests, not a skipped acceptance check.

Windows standalone execution: 15 tests passed, with the helper invoked by process tests. A temporary mutation removing the lock acquisition made the OS-lock test fail (exit 101); the mutation is not committed. The normal source is recompiled and tested after this negative control. Full desktop command integration and repository checks are run by PR CI; record their actual results in the PR.

This does not verify a task page, CLI dispatch, Teambition integration, safe stopping of external jobs, or real macOS/Windows desktop interaction. The native command wrapper additionally tests unregistered projects, immutable ledger binding, missing directories and restart ownership. Manual terminal sessions remain outside the ledger until dispatcher integration.

- `python scripts/verify_project.py` validates project instructions, skill metadata and local Markdown links.
- `python scripts/run_quality.py` runs configured commands in `project-policy.json` **without a shell**.
- An empty codebase can pass governance checks; when code appears, empty quality commands become an error.
- Add success, failure and relevant edge/regression cases; do not delete tests to silence failures.
- Treat `NOT RUN`, skipped tests and blocked integration tests honestly; never claim an installed runtime was validated by static checks alone.
- Record command, status, environment and evidence in every PR.

Stack examples (only use when applicable and installed): Python `python -m pytest -q`, Node `pnpm test`, Java `mvn test`, DBT `dbt test`. SQL engine tests require an appropriate isolated environment.

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
