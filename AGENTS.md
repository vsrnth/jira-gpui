# Agent Collaboration Policy

## Model roles

- Use `gpt-5.6-sol` as the primary/root orchestrator.
- Use `gpt-5.6-luna` subagents for implementation and code-writing tasks.
- The Sol orchestrator owns task decomposition, architectural decisions, assignment boundaries, integration review, validation, and commits.
- Give each Luna subagent a concrete, bounded task with explicit files or module ownership. Avoid overlapping write scopes between agents.
- If `gpt-5.6-luna` is unavailable, tell the user before substituting another model. Do not silently change the requested model.

## Working agreement

1. Inspect the relevant code and repository instructions before assigning work.
2. Keep the domain and application crates independent of GPUI, Tauri, HTTP clients, and persistence frameworks.
3. Delegate independent implementation slices to Luna subagents where parallel work is useful.
4. Have Sol review all resulting diffs for architecture, correctness, security, and unintended changes.
5. Run formatting, focused tests, workspace tests, and Clippy as appropriate before committing.
6. Keep Linux Wayland as the only Phase 1 runtime target. macOS remains Phase 2.
7. Keep Jira operations read-only except for explicit, user-confirmed comment creation through the dedicated comment-write port. Prohibit automatic Jira writes, retries, edits, deletes, transitions, assignments, and attachment mutations; local cache, preferences, notification state, and sync cursors may be written locally.
8. Keep commits granular and Mitchell-style: each commit should contain one coherent, independently reviewable and validated change where practical, use an imperative conventional-style subject, avoid unrelated cleanup or mixed milestones, and separate policy or documentation-only changes when sensible. Sol is the only agent that creates commits; Luna workers must not commit.
