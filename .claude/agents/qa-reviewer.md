---
name: qa-reviewer
description: Disabled legacy definition. Current project policy requires lead-only self-review and forbids development or review subagents.
tools: Read, Grep, Glob
---

# Disabled

This legacy QA subagent definition is retained only so older installations do
not fail to parse it. It is disabled by `AGENTS.md`, `CLAUDE.md` and
`docs/workflow.md`.

Do not dispatch this agent. The lead performs implementation, verification and
self-review serially, and must label the result as lead self-review rather than
independent review.
