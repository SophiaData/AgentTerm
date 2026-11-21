# Development workflow

1. **Plan**: inspect the repository and task; document scope, non-goals, risks and acceptance criteria.
2. **Implement**: change the smallest possible surface in a feature branch/worktree; add tests and docs.
3. **Verify**: run relevant checks configured in `project-policy.json`; record exact results.
4. **Review**: current user direction (2026-11-20) forbids development/review subagents. The lead checks actual diffs, security, compatibility and evidence serially and labels the result lead self-review, not independent review. Product runtime reviewer roles are a separate capability.
5. **Deliver**: use the PR template; wait for CI and authorized human review before merge.

Do not automatically publish, release, deploy or alter production services. An AI `PASS` is not an authorized merge.

Follow the single next task and phase dependencies in [tasks](tasks.md). Update
that ledger and [testing](testing.md) when evidence, blockers or status change;
update PRD/architecture/workflow when their respective decisions change. Existing
acceptance criteria remain frozen unless a change is explicitly called out.
Documentation-only work must preserve the commit and environment of historical
runtime evidence instead of claiming those checks ran on a new application build.

The user requires serial verification. `pnpm test` uses one worker and disables
file parallelism. CI sets `RUST_TEST_THREADS=1` and `CARGO_BUILD_JOBS=1`, and the
Windows quality job waits for the Linux quality job. For local Rust checks use
the same environment or `-- --test-threads=1`. Concurrency regression fixtures
still exercise competing processes inside one test; they are not separate
development agents or parallel test workers.

For a new stack, configure real test/typecheck/lint commands in `project-policy.json` before shipping source.

## Project initialization and template upgrades

The template is versioned in `TEMPLATE_VERSION`. Run
`python scripts/configure_project.py --name <project> --stack auto` for a
read-only proposal, then `--apply` once the proposed metadata is correct.
Document stack-specific quality commands and dependency setup before editing
business code. Existing projects upgrade via a reviewed diff, not blind overwrite;
see [template-upgrades.md](template-upgrades.md).
