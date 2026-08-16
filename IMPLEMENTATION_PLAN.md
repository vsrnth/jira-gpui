# Jira GPUI Desktop Application — Implementation Plan

Status: In progress — durable cache, in-app feed, best-effort Freedesktop notifications, automatic incremental polling, and validated AppImage artifact packaging are implemented; authentication, runtime, and release validation remain.
Last updated: 2026-08-16

## 1. Objective

Build a read-only Jira Cloud desktop application that lets an authenticated user select one or more Jira users and monitor the issues, epics, and related hierarchy assigned to them.

The first release targets Linux on Wayland and is distributed as a single AppImage. macOS is explicitly deferred to Phase 2.

The application is read-only with respect to Jira. It may write local preferences, cached issue data, sync cursors, and read/unread notification state, but it will not create, edit, assign, transition, comment on, or otherwise modify Jira issues.

## 2. Decisions already made

| Area | Decision |
|---|---|
| Jira product | Jira Cloud first |
| Jira access | Read-only |
| Primary subject | Issues assigned to one user or a saved set of users |
| Updates | Maintain an authoritative in-app feed with best-effort actionable desktop notifications |
| Linux display system | Native Wayland only; no X11 support |
| Linux packaging | AppImage only |
| Initial CPU architecture | `x86_64`; add `aarch64` only when requested |
| Phase 1 platform | Linux |
| Phase 2 platform | macOS |
| UI framework | GPUI with `gpui-component` |
| Initial product mode | Read-only table/detail application, not a full Jira replacement |
| Expected implementer | Experienced programmer with little or no Rust experience |

## 3. Assumptions requiring confirmation

These assumptions allow implementation to begin without expanding the first release:

1. “Issues for a user” means issues where that user is the current assignee.
2. Reporter and watcher-based views are later filters, not part of the first vertical slice.
3. The initial AppImage is for internal/private use. If it will be publicly distributed, production OAuth and an authentication broker become release requirements.
4. The application may show notifications while it is running or minimized. Notifications while the process is completely stopped require a later hosted webhook service or an auto-start background process.
5. Issue updates include assignment, status, priority, due date, summary, parent/epic, and new-comment changes.
6. The initial supported scale is up to 50 selected users, 10,000 cached issues per Jira site, and 90 days of local update history.
7. One Jira Cloud site is active at a time. The data model will allow additional sites later.

### Implementation checkpoint — 2026-08-16

The current foundation and live read-only vertical slice include:

- Separate domain, application, Jira mapping, storage, and GPUI presentation crates. Domain and application APIs remain independent of GPUI, HTTP, Tokio, and SQLite.
- `crates/jira` remains a pure Jira JSON/JQL and domain-mapping crate. It has a bounded deterministic issue-ID query helper for bulk lookup and no HTTP client or UI dependency.
- `crates/jira-http` owns `reqwest`, a dedicated Tokio runtime, read-only enhanced-search/user requests, pagination, cancellation, bounded responses, status/error mapping, and redacted API-token credentials. It accepts only HTTPS Jira Cloud sites under the validated `*.atlassian.net` boundary and binds requests to the configured site ID.
- The application layer provides cancellation, safe assignee validation, baseline/reconciliation synchronization, deterministic normalized-issue diffing, and application ports that a future Tauri shell can reuse.
- `crates/storage` now contains a dedicated-worker SQLite adapter with migration v1, WAL for file-backed stores, foreign keys, secure file opening, normalized searchable projections plus serialized normalized domain snapshots, saved user sets and ordered membership, sync cursors/failure state, durable update events, event-to-user-set associations, and local read/notification-delivery state. Jira transport payloads and credentials are not stored.
- The GPUI shell has an explicitly internal environment/API-token bootstrap (`JIRA_BASE_URL`, `JIRA_SITE_ID`, `JIRA_EMAIL`, `JIRA_API_TOKEN`, and `JIRA_ASSIGNEE_ACCOUNT_IDS`). The live Dashboard owns one cancellable automatic-poll task, opens SQLite only after Jira configuration and client validation, reuses a saved user set whose canonical member list matches the configured accounts, loads cached issues/events without Jira, and exposes manual refresh and local mark-all-read behavior.
- The first successful refresh is a quiet baseline. A later manual refresh performs reconciliation, including membership removals, and persists deterministic update-feed events. A failed refresh records local failure state while preserving the last committed cache.

Validation: 88 Linux-target workspace tests passed in an x86_64 Ubuntu 22.04 container with Rust 1.95.0 (89 on the macOS host due to the non-Linux adapter fallback test); rustfmt, warning-denied production Clippy, metadata checks, and Cargo feature and packaged-link no-X11 checks passed. The AppImage was rebuilt with checksum, extraction, and payload checks. Wayland GUI launch, FUSE execution, real Jira/notification-daemon delivery, hosted CI execution, public release, and multi-distribution coverage remain unvalidated. The Linux runtime target remains Wayland only; X11 is unsupported, macOS remains Phase 2, and all Jira operations remain read-only.

The repository includes an AppImage build and CI validation flow under
`packaging/appimage/`. Next milestones are production OAuth 2.0 3LO, broader UI
wiring, and Linux runtime/release validation. The local macOS host cannot run
the Linux GUI, AppImage, or Wayland runtime checks; hosted CI execution,
public release, and the multi-distribution matrix remain unvalidated.

## 4. Phase 1 user outcomes

The following are the Phase 1 target outcomes. The current implementation
includes the read-only pull, durable cache, in-app feed, local read state,
manual refresh, automatic incremental polling, best-effort notifications, and
AppImage artifact packaging; onboarding, full issue browsing, periodic
automatic full reconciliation, and runtime/release validation remain.

A successful Phase 1 user can:

1. Connect the application to a Jira Cloud site.
2. Search for Jira users by display name and select one or more of them.
3. Save the selection as a named user set, such as “Backend team.”
4. View the current issues assigned to those users.
5. See each issue’s project, type, status, priority, assignee, update time, and parent or epic.
6. Filter and sort the issue list locally.
7. Select an issue and inspect its read-only details.
8. Open the issue in the system browser.
9. Refresh manually or let the application refresh in the background.
10. Use previously synchronized data while temporarily offline.
11. See an in-app feed of issue updates since the last visit.
12. Receive Wayland desktop notifications for configured update types.
13. Mark local update events read or unread without changing Jira.

## 5. Phase 1 scope

### 5.1 Included

- Jira Cloud connection and site selection.
- Read-only OAuth scopes, or an explicitly internal-only API-token bootstrap mode.
- Jira user search using stable Atlassian `accountId` values.
- Saved user sets.
- Assigned-issue queries using JQL enhanced search.
- Issue and parent/epic hydration without one request per issue.
- A virtualized issue table.
- A read-only issue-detail panel.
- Project, user, issue type, status, priority, and date filters.
- Local text filtering across issue key and summary.
- SQLite caching and database migrations.
- Incremental polling and manual full reconciliation now, with periodic automatic full reconciliation before release.
- An update-event inbox.
- Native Linux desktop notifications over the Freedesktop notification interface.
- Offline, loading, stale-data, authentication, rate-limit, and partial-error states.
- Linux Wayland AppImage packaging.
- Automated unit, integration, UI-state, and packaging smoke tests.

### 5.2 Explicitly excluded

- Creating or editing issues.
- Workflow transitions.
- Changing assignees.
- Adding comments or worklogs.
- Sprint planning or rank changes.
- A full Kanban board.
- Jira notification-scheme administration.
- Reproducing Jira’s email notification inbox exactly.
- Jira Data Center or Server.
- X11 support.
- `.deb`, `.rpm`, Flatpak, or Snap packaging.
- AppImage auto-update support.
- macOS support in Phase 1.
- Windows support.
- Always-on notifications when the application process is stopped.

## 6. Product experience

### 6.1 Onboarding

The first-run flow should contain four short steps:

1. Explain that the app is read-only and that results follow the signed-in user’s Jira permissions.
2. Connect to Atlassian.
3. Select an accessible Jira Cloud site.
4. Search for users and create the first user set.

Authentication failures must explain whether the cause is expired credentials, missing scopes, revoked consent, or unavailable site access.

### 6.2 Main window

The recommended layout is:

```text
+------------------+----------------------------------------+
| Saved user sets  | Toolbar: search, filters, refresh      |
|                  +------------------------+---------------+
| - My issues      | Virtualized issue list | Issue detail  |
| - Backend team   |                        |               |
| - Design team    |                        |               |
|                  |                        |               |
| Updates (12)     |                        |               |
+------------------+------------------------+---------------+
```

The issue table initially contains:

- Issue key.
- Summary.
- Type.
- Parent or epic.
- Assignee.
- Status.
- Priority.
- Updated time.
- Due date when present.

The detail panel contains:

- Summary and issue key.
- Project, type, status, priority, and assignee.
- Parent/epic breadcrumb.
- Description rendered from Atlassian Document Format into safe read-only content.
- Labels and dates.
- Recent comments when loaded.
- “Open in Jira” action.

### 6.3 Update inbox

Each locally derived update event contains:

- Jira site and issue key.
- Event type.
- Old and new value where applicable.
- Event time.
- Matching saved user sets.
- Read/unread state.
- Whether a desktop notification was emitted.

Initial event types are:

- `issue_added_to_view`
- `issue_removed_from_view`
- `status_changed`
- `assignee_changed`
- `priority_changed`
- `due_date_changed`
- `summary_changed`
- `parent_changed`
- `comment_added`

The first synchronization establishes a baseline and must not generate a notification storm.

## 7. High-level architecture

```text
                         +----------------------+
                         | Atlassian Jira Cloud |
                         +----------+-----------+
                                    |
                              HTTPS REST API
                                    |
+------------------+       +--------v---------+
| GPUI application |<----->| Jira read client |
| state and views  |       +--------+---------+
+--------+---------+                |
         |                    sync commands/events
         |                          |
         |                 +--------v---------+
         +---------------->| Sync coordinator |
         |                 +---+-----------+--+
         |                     |           |
         |                 SQLite       credential
         |                  cache          store
         |                     |           |
         +---------------------+-----------+
                               |
                    Freedesktop notifications
```

The UI must never perform blocking HTTP or SQLite work on GPUI’s foreground thread.

## 8. Repository structure

The framework boundary is explicit from the start so a Tauri presentation can be added without changing the domain, Jira adapter, synchronization, or persistence logic. The number of crates is intentionally small, and each crate has one role.

```text
jira_gpui/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── apps/
│   ├── gpui/                    # Phase 1 presentation and native entry point
│   └── tauri/                   # optional future presentation adapter
├── crates/
│   ├── domain/                  # entities, value objects, invariants
│   ├── application/             # use cases and adapter ports
│   ├── jira/                    # pure Jira JSON/JQL models, validation, and mapping
│   ├── jira-http/               # read-only reqwest/Tokio Jira Cloud transport
│   └── storage/                 # worker-thread SQLite and in-memory adapters
├── assets/
│   ├── icons/
│   ├── app-icon/
│   └── fonts/                   # only if licensing and UI require bundled fonts
├── migrations/
├── tests/
│   ├── fixtures/
│   ├── jira_contract.rs
│   ├── sync_scenarios.rs
│   └── migration_tests.rs
├── packaging/appimage/
└── .github/workflows/
```

Dependency direction is one-way: presentations and adapters depend on `application` and `domain`; `application` depends only on `domain`; `domain` depends on no UI, HTTP, or persistence framework. GPUI types must not appear in public application/domain APIs.

## 9. Technology choices

### 9.1 Core

- Latest stable Rust, pinned through `rust-toolchain.toml` after the first successful build.
- GPUI for application/window/state infrastructure.
- `gpui_platform` with the `wayland` feature only on Linux.
- `gpui-component` for styled controls, virtualized tables/lists, panels, Markdown-like content, themes, and common interaction behavior.
- Exact compatible Git revisions or exact crate versions for GPUI and `gpui-component`; never use wildcard versions.
- `serde` and `serde_json` for Jira transport data.
- `url` for safe URL construction.
- `thiserror` for typed library errors and a small application-facing error type.
- `tracing` for structured logs with explicit secret redaction.

### 9.2 HTTP and asynchronous execution

GPUI has its own executor, while `reqwest` uses Tokio for asynchronous I/O. The current implementation resolves this boundary in `crates/jira-http`: it owns one dedicated Tokio runtime and exposes the executor-independent `JiraReadPort` from `crates/application`. GPUI submits work through the application service and applies results back on its foreground context.

- Use `reqwest` with JSON and Rustls TLS features in the transport crate.
- Construct endpoints with `url::Url`; require HTTPS and validate the Jira Cloud host before sending requests.
- Keep request timeouts, response-size limits, cancellation checks, and status classification in the transport boundary.
- Keep Jira JSON/JQL construction and mapping in the pure `crates/jira` crate.

Do not mix runtimes implicitly or call a blocking HTTP client from the UI thread. A future Tauri presentation can consume the same application port without taking a dependency on GPUI or Tokio details.

### 9.3 Storage

- SQLite using `rusqlite` initially, because its synchronous and explicit API is easier for a new Rust developer to reason about.
- Run database work on a dedicated worker thread.
- Use numbered SQL migrations committed to the repository.
- Enable WAL mode and foreign keys.
- Keep network transport JSON out of the primary schema. The current v1 adapter stores normalized searchable columns plus a serialized normalized domain snapshot; it does not persist raw Jira transport payloads.

### 9.4 Credentials and notifications

Current behavior is intentionally narrow: the internal API-token bootstrap keeps credentials in the Jira HTTP client only, never in SQLite, and the in-app update feed is durable and authoritative. The best-effort Freedesktop adapter uses the default policy for issue-added, status, assignee, priority, due-date, and comment events; removal, summary, and parent events remain in-app only. Delivery failures are nonfatal.

Future production work should store OAuth credentials in the desktop secret service rather than SQLite or plain configuration files, detect missing Secret Service support, and add notification coalescing, mute, and quiet-hours controls.

## 10. Jira Cloud integration

### 10.1 Read-only boundary

The Jira client will expose only read operations. Some Jira query endpoints use HTTP `POST`, but these calls remain semantically read-only.

The application must not request `write:jira-work` or any granular write scopes. An automated test should assert the exact scope list.

Recommended classic OAuth scopes:

- `read:jira-work`
- `read:jira-user`
- `offline_access`
- `read:me` only if the authenticated profile is displayed

### 10.2 Authentication modes

#### Production/distributable mode

Atlassian OAuth 2.0 3LO requires a client secret for authorization-code and rotating refresh-token exchanges. A client secret cannot be protected inside an AppImage, so a distributable build requires a small authentication broker.

The broker should:

1. Keep the Atlassian client secret outside the desktop application.
2. Start an OAuth flow bound to a high-entropy state value.
3. Receive the Atlassian callback over HTTPS.
4. Exchange the code and retain the rotating refresh token securely.
5. Return a one-time, short-lived application code to the desktop app.
6. Provide short-lived Jira access tokens to the authenticated desktop instance.
7. Rotate refresh tokens atomically so concurrent refresh attempts cannot invalidate the credential chain.
8. Store no Jira issue content.

This broker is a separate deployable service and adds approximately one to two weeks plus operational work.

#### Internal prototype mode

An internal build may accept an Atlassian email and API token and store them in the operating-system credential store. This mode must be visibly marked for internal development and must not become the public distribution design.

The chosen authentication mode is a release-blocking decision before the authentication milestone begins.

### 10.3 Accessible site discovery

For OAuth:

1. Retrieve accessible Atlassian resources.
2. Let the user choose a Jira site when more than one is available.
3. Store the selected `cloudId`, display URL, and site name.
4. Construct API URLs through `https://api.atlassian.com/ex/jira/{cloudId}/...`.

### 10.4 User search

Use Jira’s user-search APIs and persist `accountId` as the identity key. Display name and avatar are presentation data and may change. Email may be hidden by privacy settings and cannot be treated as an identifier.

User search should be debounced, cancellable, and limited to a small result page. Provide a direct `accountId` entry fallback for large directories where Jira search cannot return a desired user.

### 10.5 Issue search

Use enhanced JQL search:

```http
POST /rest/api/3/search/jql
```

Logical initial query:

```jql
assignee IN ("account-id-1", "account-id-2")
ORDER BY updated DESC
```

Incremental query:

```jql
assignee IN ("account-id-1", "account-id-2")
AND updated >= "overlap-window-start"
ORDER BY updated ASC
```

The JQL builder must quote and escape values centrally. UI strings must never be interpolated directly into JQL.

Large user sets should be chunked by serialized query length, with results merged and deduplicated by Jira issue ID.

Request only these initial fields:

- `summary`
- `issuetype`
- `project`
- `status`
- `priority`
- `assignee`
- `reporter`
- `parent`
- `labels`
- `description` when the detail view requests it
- `created`
- `updated`
- `duedate`
- `resolution`
- comment metadata only when update detection requires it

Follow `nextPageToken` until the response is complete. Apply cancellation when the active site or selected user set changes.

### 10.6 Epics and hierarchy

Treat Jira hierarchy generically:

1. Read the issue’s `parent` reference.
2. Collect missing parent IDs or keys after each issue page.
3. Fetch parents in batches using bulk issue fetch where available.
4. Repeat for higher hierarchy levels with a small maximum-depth guard.
5. Store parent relationships independently of labels such as “Epic.”

Do not depend on the legacy “Epic Link” custom field or issue-type names that administrators may rename.

### 10.7 Descriptions and comments

Jira REST API v3 returns rich text using Atlassian Document Format. Implement a safe read-only subset first:

- Paragraphs and line breaks.
- Plain text and emphasis.
- Headings.
- Bullet and numbered lists.
- Inline code and code blocks.
- Links with safe URL validation.

Unsupported nodes should render their child text rather than disappearing or causing a crash.

For comment notifications, fetch new comment pages only for issues whose comment metadata or update time indicates possible comment activity. Store comment IDs and timestamps to avoid duplicate events. Do not fetch every issue’s complete comment history during normal polling.

## 11. Local data model

Migration `0001_initial.sql` is the current v1 schema. It is deliberately
smaller than the eventual product model:

The Phase 1 file is `jira-desk/jira-desk.sqlite3` under the absolute
`XDG_DATA_HOME` root, or under `$HOME/.local/share` when XDG is unset/empty.
The final app directory is created with Unix mode `0700`, and a newly created
database file uses mode `0600`; relative roots and a final app-directory
symlink are rejected. The storage layer returns redacted errors without
exposing paths or SQLite details.

### Current v1 tables

- `user_sets`: site-scoped set ID, deterministic/local name, and created/updated timestamps. IDs are globally unique so dependent local state can be removed safely.
- `user_set_members`: ordered account IDs with uniqueness constraints and a composite foreign key back to `user_sets`.
- `issues`: site and Jira issue ID, issue key, summary, assignee ID, normalized updated timestamp, and a serialized normalized domain snapshot. The searchable columns support local filtering; the snapshot contains the rest of the normalized issue fields.
- `issue_membership`: the current membership of an issue in a site/user-set view, replaced atomically by baseline and reconciliation commits.
- `sync_states`: last started/succeeded cursor timestamps, last full-sync timestamp, failure count, and categorized last error.
- `update_events`: immutable event identity, site/issue identity, event kind, occurrence time, local read state, local notification-delivery state, and a serialized normalized event snapshot.
- `event_user_sets`: the many-to-many association between durable events and matching configured user sets.

The SQLite worker enables foreign keys, uses WAL for file-backed databases,
and applies migrations transactionally. A successful sync atomically updates
issue snapshots, membership, deduplicated events and associations, and the
cursor. Local read/unread and delivery state are never sent to Jira.

### Future extensions

The following remain design targets rather than current tables: Jira-site and
cached-user profiles, comment history/deduplication, raw ADF storage, user
preferences/notification settings, retention policy, and a richer issue search
projection. Add them through new numbered migrations only after their use cases
and privacy behavior are implemented.

## 12. Synchronization design

The implemented coordinator supports a live workspace, a quiet baseline,
automatic incremental polling, and explicit manual reconciliation. The live
Dashboard owns one cancellable GPUI task; polling exists only while the app
runs.

### 12.1 Baseline synchronization

On the first refresh for a configured user set:

1. Reuse or create the deterministic local user set.
2. Fetch all matching issues through enhanced JQL pagination.
3. Normalize and upsert issue snapshots.
4. Replace view membership and record the successful cursor atomically.
5. Mark this as a baseline and emit no update events.
6. Reload bounded cached pages for the UI.

### 12.2 Incremental polling

After a quiet baseline, automatic polls use incremental mode and preserve
membership. Manual refresh remains full reconciliation.

The implemented incremental flow is:

1. Starts from the last successful poll minus a five-minute overlap window.
2. Fetches issues matching the selected assignees and update window.
3. Compares the new normalized snapshot with the existing snapshot.
4. Creates deduplicated update events for relevant differences.
5. Advances the cursor only after the transaction commits.
6. Applies the notification policy after durable event creation.

The overlap prevents missed records due to clock skew and eventual search consistency. Deduplication prevents repeat notifications.

### 12.3 Full reconciliation

Manual refresh is the implemented full reconciliation path. It is selected
only when a prior successful cursor exists; a failed first attempt records
failure state but the next successful attempt remains a quiet baseline.

The current path compares the complete returned set with
`issue_membership`. Issues no longer returned become
`issue_removed_from_view` events after the full page set succeeds. The commit
replaces membership, persists deduplicated events and associations, and
advances the cursor atomically. The UI reload is capped at 10,000 issues and
500 feed events.

### 12.4 Scheduling and cancellation

The first automatic tick occurs after five minutes. `operation_in_progress`
prevents overlap with manual refresh and feed actions. Offline and upstream
failures use 30-second exponential backoff capped at 15 minutes; rate limits
honor a clamped `Retry-After` from 30 seconds to one hour. Nontransient errors
pause automatic polling, and a successful manual refresh restarts it.
Cancellation propagates through Jira pagination, failures record categorized
local state, and cache loading does not contact Jira. Polling is not available
when the process is stopped; configurable intervals, jitter, and a richer
stale-state UI remain future work. Periodic automatic full reconciliation is
also future work; manual full reconciliation is the current safety path.

## 13. Notification policy

The in-app feed is implemented and authoritative. Baseline synchronization is
quiet; later deterministic events are persisted with local read state and a
notification-delivery state. The desktop adapter is best-effort and uses the
default actionable policy; delivery failures do not interrupt synchronization.

Future notification work should add coalescing, mute and quiet-hours settings,
and focus/select behavior. The
initial target event types remain assignment, status, priority, due-date, and
comment changes; summary and parent changes are currently durable in-app
events but should not automatically notify by default.

## 14. GPUI application state

Use a small number of explicit entities rather than a deep framework abstraction:

- `AppModel`: active site, route, theme, and global status.
- `ConnectionModel`: authentication and site-selection state.
- `UserSetModel`: saved sets and the active selection.
- `IssueListModel`: cached rows, filters, sort order, selection, and loading state.
- `IssueDetailModel`: lazy-loaded issue detail and recent comments.
- `UpdatesModel`: unread count, update rows, and notification preferences.
- `SyncModel`: current operation, progress, last success, and recoverable error.

UI event handlers send commands to models. Models request work from the sync/database services. Completed work is sent back as typed events and applied on the GPUI foreground context.

Avoid shared mutable state protected by broad mutexes. Prefer message passing and GPUI entities.

## 15. Delivery phases and estimates

The estimates assume one full-time experienced programmer who is new to Rust. They include learning and integration uncertainty, not just typing code.

### Phase 0 — Rust, GPUI, and packaging spike

Estimate: 1 week.

Work:

- Install and pin stable Rust.
- Build the GPUI hello-world example on a Wayland Linux environment.
- Render a `gpui-component` table with at least 10,000 synthetic rows.
- Prove selection, scrolling, keyboard focus, and a detail panel.
- Validate background HTTP without blocking the UI.
- Validate SQLite work on a worker thread.
- Send one Freedesktop desktop notification.
- Build a minimal Wayland AppImage and run it on a second Linux distribution.
- Record the exact GPUI and `gpui-component` revisions that work together.

Exit criteria:

- A minimal AppImage opens natively on Wayland.
- The UI remains responsive during an HTTP request and database write.
- The chosen async runtime approach is documented.

### Phase 1 — Jira vertical slice (foundation delivered; UI completion remains)

Estimate: 1 to 1.5 weeks.

Work:

- Define domain and Jira transport models.
- Implement the selected prototype authentication mode.
- Discover/select a Jira site.
- Implement user search.
- Implement safe JQL construction.
- Fetch one user’s assigned issues with pagination.
- Display cached and live issues in the virtualized table.
- Open the selected issue in Jira.

Exit criteria:

- A user can connect, select one Jira user, and view that user’s assigned issues.
- Tests cover pagination, malformed responses, authentication errors, and JQL escaping.

### Phase 2 — Multi-user views and hierarchy

Estimate: 1 week.

Work:

- Add saved user sets.
- Query multiple assignees with query chunking.
- Merge and deduplicate pages.
- Hydrate parent and epic relationships in batches.
- Add local filters and sorting.
- Add the detail panel and ADF renderer subset.

Exit criteria:

- A 50-user set can be synchronized without N+1 parent requests.
- Issue hierarchy and detail content remain correct across cached restarts.

### Phase 3 — Durable cache and sync engine (implemented v1 slice)

Estimate: 1 to 1.5 weeks.

Delivered:

- Add schema migrations and repositories.
- Implement baseline, incremental polling, and reconciliation sync modes.
- Add durable sync cursors and local failure state.
- Add deterministic snapshot diffing, event associations, and event deduplication.
- Add cancellation and atomic issue/membership/event/cursor commits.
- Add offline startup cache loading and bounded cache reads.

Remaining:

- Add periodic automatic full reconciliation, stale-cache presentation, and broader offline states; polling and retry/rate-limit policy are delivered.

Delivered/remaining exit criteria:

- Killing the application during a sync cannot advance the cursor past committed data.
- Repeated overlapping polls do not create duplicate update events.
- Offline startup displays cached data; accurate stale-status presentation remains.

### Phase 4 — Updates and notifications (best-effort delivery implemented)

Estimate: 1 week.

Delivered:

- Build the in-app update inbox.
- Detect configured issue field changes.
- Add read/unread state.
- Suppress baseline notifications.
- Deliver actionable events through the Freedesktop adapter.
- Preserve the durable in-app feed when notification delivery fails.

Remaining:

- Detect newly visible comments without downloading full histories repeatedly.
- Validate real notification-daemon delivery and runtime behavior.
- Add coalescing, mute and quiet-hours settings, and focus/select behavior.

Remaining exit criteria:

- A controlled Jira change produces one durable in-app event and at most one desktop notification.
- Notification failure does not lose the in-app event or break synchronization.

### Phase 5 — Product hardening

Estimate: 1 to 1.5 weeks.

Work:

- Finish onboarding and error recovery.
- Add keyboard navigation and focus states.
- Test large lists and high-DPI rendering.
- Add local structured logs and a redacted support bundle.
- Add database backup/recovery behavior for migration failure.
- Audit scopes, logs, persisted data, and URL handling.
- Test GNOME Wayland, KDE Plasma Wayland, and Sway.

Exit criteria:

- All P0 acceptance tests pass.
- No token, authorization header, comment body, or description appears in normal logs.
- The app recovers cleanly from network loss, `429`, invalid credentials, and a corrupt cache copy.

### Phase 6 — AppImage release (artifact build validated; runtime/release validation outstanding)

Estimate: 0.5 to 1 week.

The repository now has the AppDir metadata, desktop entry, icon, `AppRun`,
license inclusion, and a guarded `linuxdeploy`/`appimagetool` build script.
The 0.1.0 artifact was built with checksum-verified pinned tools/runtime and
passed checksum, extraction, required-file, and `ldd` no-missing/X11-link
checks. Runtime and release validation remain outstanding.

Work:

- Create the AppDir layout, desktop entry, icon, assets, and `AppRun` entry point.
- Produce the AppImage in Linux CI from the oldest supported build environment.
- Bundle eligible libraries while leaving host GPU, Wayland, D-Bus, and glibc responsibilities explicit.
- Smoke-test the extracted AppImage under a headless Wayland compositor in CI.
- Test the actual AppImage with FUSE on representative user systems.
- Generate checksums and release notes.

Exit criteria:

- A clean supported Wayland system can download, mark executable, and launch the AppImage.
- The release artifact connects to Jira, loads cached data, and sends a desktop notification.

### Total Phase 1 estimate

- Internal authentication prototype: approximately 7 to 10 weeks.
- Production OAuth broker and distributable authentication: approximately 9 to 12 weeks including broker implementation, deployment, and integration.

## 16. Testing strategy

### 16.1 Unit tests

- JQL escaping, chunking, and ordering.
- Jira transport-to-domain mapping.
- ADF safe rendering.
- Snapshot diff behavior for every event type.
- Dedupe-key stability.
- Notification policy and coalescing.
- Retry classification and backoff bounds.
- Local filtering and sorting.

### 16.2 HTTP contract tests

Use a local mock HTTP server with sanitized fixtures for:

- Accessible resource discovery.
- User search and privacy-restricted users.
- Single and multiple issue pages.
- `nextPageToken` pagination.
- Parent bulk hydration.
- Missing/deleted issues.
- Comments and restricted comments.
- `401`, `403`, `404`, `429`, and `5xx` responses.
- Malformed or partially missing optional fields.

No test fixture may contain a real Jira token, user email, confidential summary, comment, or description.

### 16.3 Database tests

- Every migration from an empty database.
- Upgrade from each previously released schema.
- Transaction rollback during interrupted sync.
- Concurrent UI read and worker write under WAL mode.
- Dedupe constraints.
- Cache replacement/recovery after corruption detection.

### 16.4 Sync scenario tests

- First baseline creates no events.
- New assignment creates one event.
- Status changes within overlapping polls create one event.
- Issue moves away from selected users during reconciliation.
- Issue becomes inaccessible.
- New comment detection.
- Authentication expires during pagination.
- Rate limiting occurs halfway through a sync.
- User changes active user set while a sync is running.
- Application exits after issue upsert but before cursor advancement.

### 16.5 UI tests

- First-run and reconnect flows.
- Loading, empty, stale, offline, partial, and fatal states.
- Keyboard traversal and issue selection.
- Virtualized scrolling with 10,000 synthetic issues.
- Filters and sorting.
- Update inbox read/unread behavior.
- Theme contrast and 1x/2x scale factors.

### 16.6 Manual release matrix

| Environment | Required |
|---|---|
| GNOME Wayland | Yes |
| KDE Plasma Wayland | Yes |
| Sway or another wlroots compositor | Yes |
| X11 session | Confirm clear unsupported behavior only |
| Offline start | Yes |
| No Secret Service provider | Yes, actionable error |
| No desktop notification service | Yes, in-app fallback |
| High-DPI display | Yes |

## 17. CI and release pipeline

Every pull request should run:

- `cargo fmt --check`
- `cargo clippy` with warnings denied for project code
- Unit and integration tests
- Migration tests
- Dependency/license audit
- Release-mode Linux compile

Release workflow:

1. Build on Linux in a pinned container or runner.
2. Run tests.
3. Create the AppDir.
4. Generate the AppImage.
5. Run extract-and-execute smoke tests under headless Wayland.
6. Upload the AppImage and SHA-256 checksum.
7. Perform a manual FUSE/desktop integration check before promoting the release.

Do not attempt to produce the release AppImage from macOS through ad-hoc cross-compilation.

## 18. Security and privacy

- Use TLS validation without insecure overrides.
- Current internal mode consumes API-token credentials into the Jira HTTP client and never writes them to SQLite. Production mode must store credentials only through the secret-storage abstraction.
- Never persist the Atlassian OAuth client secret in the desktop repository or binary.
- Never log authorization headers, API tokens, access tokens, refresh tokens, OAuth codes, or broker session credentials.
- Redact Jira description and comment bodies from normal logs.
- Validate browser URLs against the selected Jira site before opening them.
- Keep OAuth state single-use and high entropy.
- Use least-privilege, read-only scopes.
- Apply Jira permissions naturally; never attempt to bypass missing issues or fields.
- Provide “Disconnect and erase local Jira data” as an explicit local operation.
- Document what is cached and how long update history is retained.
- Make telemetry opt-in if telemetry is added at all.

## 19. Reliability and performance targets

These are engineering targets, not external service guarantees:

- Cached launch to visible issue list: under 1 second on the reference machine.
- Smooth scrolling with 10,000 cached rows.
- No UI-thread network or database blocking longer than one frame.
- Incremental sync starts within 10 seconds of its scheduled interval while online.
- No duplicate update events after retries or restart.
- All Jira pagination is bounded, cancellable, and progress-reporting.
- Memory remains bounded by virtualization and paged/lazy detail content.
- `429` responses never cause an immediate retry loop.

## 20. Observability and supportability

Provide local structured logs containing:

- Application version and platform.
- Sync start/end, mode, duration, and counts.
- Page counts, not issue content.
- Retry reason and delay.
- Database migration version.
- Notification delivery outcome.
- Typed error categories.

Add a user-triggered support bundle containing redacted logs, schema version, feature flags, and non-sensitive environment information. It must exclude the database and credentials by default.

## 21. Primary risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| GPUI is pre-1.0 and changes quickly | Broken builds and API churn | Pin exact revisions; update only in isolated dependency PRs |
| `gpui-component` and GPUI revisions diverge | Compile or runtime incompatibility | Pin a known compatible pair established in Phase 0 |
| Developer is new to Rust | Schedule and ownership-model friction | One crate, explicit types, minimal generics, small vertical slices, frequent tests |
| GPUI and HTTP runtime mismatch | Deadlocks or stalled requests | Resolve in Phase 0; use a dedicated runtime and channels if needed |
| OAuth needs a client secret | Unsafe distributable desktop auth | Use a hosted authentication broker for production |
| Jira enhanced search is eventually consistent | Missed or delayed updates | Overlap window, dedupe, manual full reconciliation now, periodic full reconciliation later |
| Issue moves out of selected assignees | Incremental query no longer sees it | Full reconciliation and neutral removal events |
| Large Jira directories | User search may omit accounts | Search debounce, account ID fallback, recently seen users |
| Rate limiting | Delayed sync | Request minimal fields, batch, cap concurrency, honor headers and backoff |
| Wayland desktop differences | Notification/window behavior varies | Test GNOME, KDE, and wlroots; use protocol capability detection |
| AppImage host-library compatibility | Artifact fails on some distributions | Build on oldest supported base; test multiple distributions; document runtime requirements |
| Secret Service is absent | Cannot safely retain credentials | Actionable error and optional session-only credential mode |

## 22. Phase 1 definition of done

Phase 1 is complete only when:

- The application runs natively on supported Wayland environments without X11 features.
- A user can authenticate and select an accessible Jira Cloud site.
- A user can create and reopen saved multi-user sets.
- The application fetches all matching assigned issues using enhanced JQL pagination.
- Parent and epic relationships are resolved in batches.
- The issue table remains responsive with 10,000 cached rows.
- Issue detail and supported ADF content render safely.
- Baseline, incremental, manual, and periodic full sync modes work.
- Update events survive restart and do not duplicate across overlapping polls.
- Desktop notification failure falls back to the in-app inbox.
- Offline startup shows cached data and accurate staleness.
- Jira remains unmodified and the app requests no write scopes.
- Tokens and sensitive issue content are absent from logs.
- The AppImage passes the supported Wayland release matrix.
- Installation and first-run documentation are complete.

## 23. Phase 2: macOS

Phase 2 reuses the Jira, domain, storage, sync, and most UI code. It adds:

- macOS-specific `gpui_platform` configuration with `font-kit`.
- Keychain credential backend validation.
- macOS notification integration and behavior testing.
- Menu bar and standard application-menu behavior.
- macOS window conventions and keyboard shortcuts.
- Universal or explicitly selected CPU architecture builds.
- Application bundle creation.
- Developer ID signing.
- Hardened runtime configuration.
- Notarization and stapling.
- macOS release CI and update strategy.

No macOS-specific work should be allowed to complicate the Phase 1 Linux release, but platform interfaces for credentials, notifications, URL opening, and packaging should remain narrow enough to implement a second backend.

## 24. First implementation backlog

Execute these tasks in order:

1. Create the Cargo application and pin the Rust toolchain. (Done.)
2. Pin a compatible GPUI/`gpui-component` pair. (Done.)
3. Open a Wayland window containing a static issue table and detail pane. (Preview path done; Linux validation remains.)
4. Prove async HTTP and database communication without blocking GPUI. (Done in adapters; runtime validation remains.)
5. Produce the minimal AppImage spike. (Artifact build and CI checks done; runtime execution validation remains.)
6. Define Jira transport fixtures and domain models. (Done.)
7. Implement the read-only Jira client and JQL builder. (Done.)
8. Fetch and render one configured account’s issues. (Read-only live pull foundation done; broader UI wiring remains.)
9. Add user search and user-set persistence. (Persistence done; user-search UI remains.)
10. Add pagination and parent hydration. (Bounded pagination done; parent hydration remains.)
11. Add durable caching. (SQLite v1 done.)
12. Add baseline and incremental synchronization. (Baseline, incremental polling, and reconciliation done; richer stale-state UX remains.)
13. Add snapshot diff events. (Done for normalized issue fields and membership.)
14. Add the update inbox. (Done, including local read state.)
15. Add desktop notifications. (Best-effort Freedesktop adapter and default policy done; daemon/runtime validation remains.)
16. Finish error recovery, release testing, and AppImage automation. (Build/CI automation and artifact checks done; runtime/release validation remains.)

## 25. Decisions to close before coding reaches authentication

1. Is the first AppImage strictly internal, or will it be distributed to unrelated users?
2. If distributed, where will the OAuth authentication broker be hosted and operated?
3. Which Linux distribution and glibc baseline define the minimum supported system?
4. Is `x86_64` sufficient for Phase 1?
5. Is the default five-minute polling interval acceptable for the expected number of users and issues?
6. Should new comments be a mandatory Phase 1 notification, or may they land immediately after the field-change notifications?

## 26. References

- [GPUI examples and overview](https://gpui.rs/#examples)
- [GPUI README and platform features](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)
- [gpui-component repository](https://github.com/longbridge/gpui-component)
- [Jira Cloud OAuth 2.0 3LO](https://developer.atlassian.com/cloud/jira/platform/oauth-2-3lo-apps/)
- [Jira Cloud enhanced JQL issue search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-search/)
- [Jira Cloud user search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-user-search/)
- [Jira Cloud issue and changelog APIs](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/)
- [Jira Cloud rate limiting](https://developer.atlassian.com/cloud/jira/platform/rate-limiting/)
- [Jira Cloud webhook behavior](https://developer.atlassian.com/cloud/jira/software/webhooks/)
