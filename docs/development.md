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

Test execution stays serial to keep results deterministic: `pnpm test` uses one
worker without file parallelism, and CI keeps `RUST_TEST_THREADS=1` (use the
same setting or `-- --test-threads=1` locally). Per user direction on
2026-11-24, GitHub Actions allows multi-core compilation, caches Cargo/pnpm
dependencies via `Swatinem/rust-cache` and `setup-node`, and runs independent
Linux/Windows/macOS runner jobs concurrently so feedback and packaging do not
wait serially across VMs. Concurrency regression fixtures still exercise
competing processes inside one test; they are not separate development agents
or parallel test workers.

For a new stack, configure real test/typecheck/lint commands in `project-policy.json` before shipping source.

## Packaging

The package workflow builds the desktop and TUI artifacts independently after a
quality-gated commit. The Windows desktop job also runs:

```bash
node scripts/package-npm.mjs src-tauri/target/release/agentterm.exe dist-npm
```

This stages the real desktop executable under `vendor/` and creates a Windows
x64 npm tarball. Release publication remains manual. After downloading the
builder artifacts, assemble normalized names and hashes with:

```bash
node scripts/package-release.mjs \
  --installer AgentTerm_0.2.0-rc.13_x64-setup.exe \
  --desktop agentterm.exe \
  --tui agentterm-tui.exe \
  --npm agentterm-0.2.0-rc.13.tgz \
  --output dist-release
```

The script writes `SHA256SUMS.txt` and `release-manifest.json`; creating the
GitHub pre-release remains an explicit authorized action.

## Project initialization and template upgrades

The template is versioned in `TEMPLATE_VERSION`. Run
`python scripts/configure_project.py --name <project> --stack auto` for a
read-only proposal, then `--apply` once the proposed metadata is correct.
Document stack-specific quality commands and dependency setup before editing
business code. Existing projects upgrade via a reviewed diff, not blind overwrite;
see [template-upgrades.md](template-upgrades.md).
