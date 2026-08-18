# Implementation plan

This is the current, concise roadmap. It replaces the former root-level
`IMPLEMENTATION_PLAN.md`, whose early design notes described superseded
assignee-set onboarding and a fully read-only Jira policy.

## Current milestone

The durable Linux Wayland vertical slice is implemented: GPUI onboarding,
authenticated `/myself` identity, account-scoped assigned-or-watched read sync,
user-editable cross-project JQL scope, and inferred project labels,
SQLite caching, local update events, incremental polling, lazy issue detail,
exact Jira-key lookup, ticket-grouped activity, confirmed comment creation,
confirmed assignment and status-transition actions, and AppImage packaging.

The application core remains independent of GPUI, HTTP, and SQLite. The current
milestone is an internal/local build, not a public release. See
[architecture](architecture.md), [operations](operations.md), and
[release validation](release.md) for behavior and limits.

## Deliberate product decisions

- Linux Wayland is the only Phase 1 runtime; macOS is Phase 2.
- Jira synchronization and detail reads are read-only.
- Confirmed comment creation, assignment changes, and status transitions are
  the only Jira writes; each is dispatched once with no automatic retry.
- The remote sync uses the configured scope and returns the authenticated
  user's assigned-or-watched issues across projects.
- Scope changes use a fingerprinted user-set/cache identity and begin with a
  quiet baseline; desktop alerts remain assigned-only.
- Settings persist a validated scope atomically only after Jira accepts and
  commits the corresponding refresh; failed changes roll back the active
  scope and leave the prior preference intact.
- Local cache, update read state, and sync cursors may be written locally.
- The first successful sync is quiet; later changes produce durable local
  events and best-effort desktop notifications.
- Credentials are session-only in the current API-token path. Public OAuth and
  secret-store integration remain release work.

## Remaining release work

1. Broaden Wayland runtime testing across supported compositors and
   distributions.
2. Validate real Jira permission, authentication-expiry, rate-limit, and
   reconnect scenarios.
3. Complete production OAuth 2.0 3LO and platform secret-storage design before
   any public distribution.
4. Expand release automation and artifact promotion criteria after the runtime
   matrix is established.
5. Revisit macOS support as a separate Phase 2 effort.

Changes should keep domain/application ports independent from presentation,
transport, and persistence implementations, and should preserve bounded,
cancellable requests plus explicit error outcomes.
