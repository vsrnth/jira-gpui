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

## Responsive presentation

The GPUI shell adapts at the window width rather than assuming a fixed desktop
canvas:

| Width | Layout |
| --- | --- |
| below 720 px | Mobile navigation with one visible pane; selecting an issue opens its detail view and a back action returns to the list |
| 720–959 px | Compact desktop layout with a 64 px navigation rail, issue list, and detail pane |
| 960–1,199 px | Standard desktop layout with full navigation and narrower list/detail columns |
| 1,200 px and wider | Wide desktop layout with full navigation and expanded list/detail columns |

List rows, detail fields, comments, and rich text use constrained flex children
and wrapping/truncation so long Jira content does not determine the window
width.

## Identity and rich content

Account IDs are stable typed identities used for matching, filtering, and local
state. They are not UI labels. The presentation directory resolves display
names from the authenticated user catalog and issue/comment metadata; missing
names use `Unknown user`, `Unknown author`, or `Unassigned` rather than falling
back to an opaque account ID.

Issue descriptions and comments may arrive as Jira ADF. The adapter retains a
bounded, transport-neutral subset: paragraphs, headings, lists, code blocks,
quotes, panels, plain text marks, mentions, and validated HTTP(S) link marks.
Unsupported nodes become an explicit placeholder. Links are styled but inert in
the current shell, and media is never downloaded or opened. Parser and renderer
bounds apply independently so cached content cannot force unbounded rich-text
work in the UI.

Issue snapshots are stored in the local SQLite cache, including display metadata
and rich descriptions. Details and comments are fetched remotely on selection
and held in memory; comment bodies are not persisted by the current cache.

Issue-type and priority labels remain the source of truth. The dashboard adds
small embedded `gpui-component` semantic icons as secondary cues: generic icons
cover Story, Initiative, Task, Sub-task, Bug, Epic, and unknown types, while
priority arrows/minus communicate Highest through Lowest with restrained theme
tones.

## Boundaries worth preserving

- Jira locators and account IDs are typed in application/domain contracts.
- HTTP status and transport details are mapped before reaching presentation.
- SQLite schema and migrations stay behind storage ports.
- UI state owns cancellation and stale-result guards, not transport code.
- The only Jira write port is dedicated to explicitly confirmed comment
  creation; issue edits, transitions, assignments, attachments, and automatic
  writes remain prohibited.
