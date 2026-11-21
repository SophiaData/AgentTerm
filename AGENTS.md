# SophiaData Project Agent Rules

Applies to GPT, Claude Code, Codex, DSH and other coding assistants working on this repository.

- Read `docs/architecture.md`, `docs/development.md` and `docs/testing.md` before substantive changes.
- Follow the project plan and acceptance criteria; inspect source and tests before editing.
- Current user direction (2026-11-20): the lead performs planning, implementation, verification and self-review serially. Do not invoke subagents for development or review. Label this as lead self-review, never independent review; follow `docs/workflow.md`.
- The target product has an application-wide single-main-task limit across projects and windows. The occupancy ledger, native relay core, task page and artifact references are implemented; managed CLI dispatch, queue, safe recovery and evidence-based completion remain pending. See `docs/tasks.md` and `docs/architecture.md`.
- Prefer existing components and public interfaces. Keep changes scoped; avoid speculative refactors.
- Do not record credentials, tokens, cookies, or local environment secrets in code, logs or PRs.
- Work on a topic branch/worktree. Preserve unrelated changes.
- Add regression tests for behavior changes. Never invent results or downgrade a check to pass.
- Review actual diffs and test evidence. Under the current no-subagent direction, perform and record lead self-review; do not fabricate independent approval. Product runtime reviewer roles remain a separate planned capability.
- Do not auto-merge, deploy, release, publish, or modify production without explicit authorization.
- For DataWorks and similar production systems, modify drafts only when authorized; never submit/publish production nodes automatically.
- CI is the executable quality gate; instructions and Skills alone do not enforce behavior.
- Use `.agents/skills/` instructions for planning, implementation, review and PR preparation.

## AgentTerm stack rules

- Keep the renderer sandboxed: no Node.js integration and no arbitrary shell execution from the frontend.
- Validate tool IDs and project directories in Rust before starting a PTY; removing a project must never remove user files.
- Run `pnpm check` before delivery. It covers frontend and Rust checks as well as the template contract.
- Build only macOS arm64 and Windows x64 packages. Package generation is separate from an authorized formal release.
- Update `docs/tasks.md` and `docs/testing.md` whenever status, blockers or verification results change. Keep one explicit next task and distinguish implementation, simulated tests, native operation, human acceptance, merge and release.

## V2 policy

Before shipping application code, run `python scripts/configure_project.py`
with the real project name/stack, configure non-placeholder tests in the
`quality_checks` array, and run the template contract tests. Template version
is tracked in `TEMPLATE_VERSION`; updates require a reviewed PR following
[upgrade guidance](docs/template-upgrades.md). The declared command categories
are an audit signal, not evidence that an arbitrary test command is meaningful.
