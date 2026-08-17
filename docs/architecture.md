# Architecture

Jira Desk is split so the core behavior can be reused by another shell later.
The dependency direction is inward: presentation and adapters depend on
application contracts; domain and application do not depend on GPUI, HTTP,
Tokio, or SQLite.

## Workspace layout

| Path | Responsibility |
| --- | --- |
| `apps/gpui` | Linux desktop shell, onboarding, dashboard, detail views, and window controls |
| `crates/domain` | Jira-independent identifiers, issues, comments, users, and update values |
| `crates/application` | Use cases, ports, cancellation, sync, detail loading, comments, and polling policy |
| `crates/jira` | Pure Jira JSON/JQL mapping and bounded request construction |
| `crates/jira-http` | Jira Cloud transport, pagination, cancellation, response limits, and safe error mapping |
| `crates/storage` | Worker-thread SQLite adapter, migrations, cache, membership, cursors, and local feed state |
| `crates/desktop-notifications` | Best-effort Freedesktop notification adapter |
| `packaging/appimage` | Linux Wayland AppImage build and inspection scripts |

## Runtime flow

```text
GPUI shell
  -> application services and ports
      -> Jira HTTP adapter       (remote read, plus confirmed comment creation)
      -> SQLite storage adapter  (local cache and update state)
      -> desktop notification adapter (best effort)
```

Synchronous environment bootstrap validates configuration, constructs the Jira
client, and opens SQLite without claiming an authenticated user. During
Dashboard initialization, the existing client resolves the authenticated
`/myself` user; only then does the shell create the account-scoped workspace and
load its scoped cache before the first project refresh. The sync coordinator
fetches the fixed Jira Project project, while the dashboard locally scopes
the visible list to that authenticated account. A successful first refresh is
a quiet baseline. Later polls compare normalized snapshots and persist
deterministic update events.

## Dashboard behavior

The dashboard has one My-issues list. Status categories and text search are
local intersections over the retained domain issue list; they do not trigger
Jira or SQLite requests. Exact-key submission first selects a local match. If
the key is absent, Jira Desk performs a bounded, cancellable lookup and shows a
transient result without adding it to cache membership.

Selecting an issue starts a separate cancellable detail request. The request
loads the description, all bounded comment pages, and attachment metadata.
Results are applied only when the selected issue and request generation still
match. Detail data is memory-only. Comment creation uses a separate confirmed
request path and does not share retry behavior with reads.

The local update feed is derived from cache transitions. It is Jira Desk's
view of detected changes, not Jira's bell or inbox notification stream.
Desktop delivery is best effort and never makes a sync fail.

## Boundaries worth preserving

- Jira locators and account IDs are typed in application/domain contracts.
- HTTP status and transport details are mapped before reaching presentation.
- SQLite schema and migrations stay behind storage ports.
- UI state owns cancellation and stale-result guards, not transport code.
- The only Jira write port is dedicated to explicitly confirmed comment
  creation; issue edits, transitions, assignments, attachments, and automatic
  writes remain prohibited.
