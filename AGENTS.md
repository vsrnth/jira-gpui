# Agent Collaboration Policy

## Model roles

- Use `gpt-5.6-sol` as the primary/root orchestrator.
- Use `gpt-5.6-luna` subagents exclusively for all implementation and code-writing tasks. All production code, test code, scripts, migrations, configuration code, and code modifications must be authored by Luna subagents.
- The Sol orchestrator must not write or modify code directly. Sol owns task decomposition, architectural decisions, assignment boundaries, orchestration, integration review, validation, and commits.
- Give each Luna subagent a concrete, bounded task with explicit files or module ownership. Avoid overlapping write scopes between agents.
- If `gpt-5.6-luna` is unavailable, tell the user before substituting another model. Do not silently change the requested model.

## Working agreement

1. Inspect the relevant code and repository instructions before assigning work.
2. Keep the domain and application crates independent of GPUI, Tauri, HTTP clients, and persistence frameworks.
3. Delegate every implementation and code-writing task to Luna subagents, whether or not parallel execution is useful. Sol may inspect code and request revisions, but Luna must author every code change.
4. Have Sol review all resulting diffs for architecture, correctness, security, and unintended changes.
5. Run formatting, focused tests, workspace tests, and Clippy as appropriate before committing.
6. Keep native macOS (arm64/x86_64) and Linux x86_64 Wayland as the supported desktop runtime targets; X11 and Windows remain out of scope.
7. Keep Jira writes limited to explicit, user-confirmed comment creation, assignment changes, and status transitions through dedicated write ports. Dispatch each confirmed write once and prohibit automatic Jira writes, retries, deletes, other issue edits, and attachment mutations; local cache, preferences, notification state, and sync cursors may be written locally.
8. Keep commits granular and Mitchell-style: each commit should contain one coherent, independently reviewable and validated change where practical, use an imperative conventional-style subject, avoid unrelated cleanup or mixed milestones, and separate policy or documentation-only changes when sensible. Sol is the only agent that creates commits; Luna workers must not commit.
9. Every UI regression fix or new UI behavior must add deterministic local macOS automation coverage where feasible. Keep these tests local-only (never CI), fixture-based, free of Jira credentials, network access, and Jira writes, and use semantic accessibility assertions for behavior and identity plus bounded visual assertions for geometry and rendering.

## Agent skills

### Issue tracker

Issues and PRDs are tracked in GitHub Issues. See `docs/agents/issue-tracker.md`.

### Triage labels

Use the default canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. See `docs/agents/domain.md`.
