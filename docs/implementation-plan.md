# Implementation plan

This is the current, concise roadmap. It replaces the former root-level
`IMPLEMENTATION_PLAN.md`, whose early design notes described superseded
assignee-set onboarding and a fully read-only Jira policy.

## Current milestone

The durable cross-platform desktop slice is implemented: native GPUI
onboarding, authenticated `/myself` identity, account-scoped
assigned-or-watched read sync, user-editable cross-project JQL scope, inferred
project labels, SQLite caching, local update events, incremental polling, lazy
issue detail, exact Jira-key lookup, ticket-grouped activity, confirmed comment
creation, confirmed assignment and status-transition actions, Linux Wayland
AppImage packaging, and native macOS DMG packaging. Linux is x86_64; macOS is
native arm64 or x86_64.

The application core remains independent of GPUI, HTTP, and SQLite. The current
milestone is an internal/local build, not a public release. See
[architecture](architecture.md), [operations](operations.md), and
[release validation](release.md) for behavior and limits.

## Deliberate product decisions

- Linux x86_64 native Wayland and native macOS arm64/x86_64 are the supported
  desktop targets. Linux ships as an AppImage and macOS as a DMG; X11 and
  Windows remain unsupported. Each artifact is built and validated natively
  on its own host and does not validate the other platform.
- Linux uses Wayland controls, Freedesktop notifications, and XDG portals.
  macOS uses native GPUI support, the native keyring feature, native file
  picker behavior, and its Application Support/Logs locations; the Freedesktop
  notification adapter and test are unavailable there. In-app feed and
  feedback remain available on both targets.
- Jira synchronization and detail reads are read-only.
- Confirmed comment creation, assignment changes, and status transitions are
  the only Jira writes; each is dispatched once with no automatic retry.
- The remote sync uses the configured scope and returns the authenticated
  user's assigned-or-watched issues across projects, ordered by Jira
  `updated_at` newest first; issue-detail comments render newest first.
- Scope changes use a fingerprinted user-set/cache identity and begin with a
  quiet baseline; on Linux, ordinary desktop alerts remain assigned-only.
  Direct ADF mentions of the authenticated account are local comment updates
  and, on Linux, alerts, including on watcher-only tickets.
- Settings persist a validated scope atomically only after Jira accepts and
  commits the corresponding refresh; failed changes roll back the active
  scope and leave the prior preference intact.
- Local cache, update read state, and sync cursors may be written locally.
- The first successful sync is quiet; later update-emitting syncs inspect only
  the newest 100 comments for direct ADF mentions. Mention events use stable
  local identity/deduplication, bounded local excerpts, memory-only full
  bodies, and no Jira writes; on Linux, desktop delivery counts mean accepted
  by the desktop notification API, not guaranteed banner display.
- On Linux, Settings provides a test Freedesktop desktop notification that
  makes no Jira call or database event. It uses the production app identity,
  reports the daemon-assigned ID/error category and timestamp, and writes
  privacy-safe fixed-schema start/result entries to bounded `diagnostics.jsonl`;
  API acceptance does not prove GNOME rendered a banner. The adapter and test
  are unavailable on macOS, where in-app feed and feedback remain available.
- Credentials remain in memory for the session unless the user leaves **Remember
  securely in system keyring** enabled (the default); after successful
  authentication, the validated URL, email, and scoped API token are stored only
  in the system keyring. They are never written to SQLite, preferences, or logs.
  Public OAuth 2.0 3LO and broader credential lifecycle controls remain release
  work.

## Remaining release work

1. Broaden Linux Wayland runtime testing across supported compositors and
   distributions, and add native macOS arm64/x86_64 smoke coverage.
2. Validate real Jira permission, authentication-expiry, rate-limit, and
   reconnect scenarios on both supported hosts.
3. Complete production OAuth 2.0 3LO and platform secret-storage design before
   any public distribution; current native keyring support does not imply
   public OAuth readiness.
4. Expand release automation and artifact promotion criteria after the
   platform-specific runtime matrix is established.
5. Keep native AppImage and DMG build/validation procedures separate; do not
   cross-compile or infer one platform's artifact from the other.

Changes should keep domain/application ports independent from presentation,
transport, and persistence implementations, and should preserve bounded,
cancellable requests plus explicit error outcomes.
