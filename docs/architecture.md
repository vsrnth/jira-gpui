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
| `crates/application` | Use cases, ports, cancellation, sync, detail/media loading, comments, and polling policy |
| `crates/jira` | Pure Jira JSON/JQL mapping and bounded request construction |
| `crates/jira-http` | Jira Cloud transport, pagination, cancellation, response limits, and safe error mapping |
| `crates/storage` | Worker-thread SQLite adapter, migrations, cache, membership, cursors, and local feed state |
| `crates/desktop-notifications` | Best-effort Freedesktop notification adapter |
| `packaging/appimage` | Linux Wayland AppImage build and inspection scripts |

## Runtime flow

```text
GPUI shell
  -> application services and ports
      -> Jira HTTP adapter       (remote read/media, plus confirmed comment creation)
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
Jira or SQLite requests. The status control is a multi-select: selecting any
combination of To Do, In Progress, Done, and Uncategorized applies an OR
filter, while an empty selection means All statuses. Exact-key submission first
selects a local match. If the key is absent, Jira Desk performs a bounded,
cancellable lookup and shows a transient result without adding it to cache
membership.

Selecting an issue starts a separate cancellable detail request. The request
loads the description, all bounded comment pages, and attachment metadata.
Description media is resolved conservatively: only a unique alt/filename
match, or the one-media/one-image case, can trigger an authenticated Jira
thumbnail read. Each thumbnail is capped at 8 MiB, with at most 16 references
and 32 MiB aggregate per detail load. Results are applied only when the
selected issue and request generation still match; detail and thumbnail bytes
are memory-only. Arbitrary Media Services URLs and redirects are never
followed.

An explicit attachment download is a separate user action. It reads the
configured Jira origin with authentication, caps the response at 64 MiB, and
writes only to the destination selected through the XDG portal. The local
write runs in the background after selection; it is not automatic, is not
retried automatically, and does not mutate Jira. Comment creation uses a
separate confirmed request path and does not share retry behavior with reads.

The local update feed is derived from cache transitions. It is Jira Desk's
view of detected changes, not Jira's bell or inbox notification stream. The
shell also shows component-level in-app notifications for refresh and comment
outcomes. These are additive feedback: the Freedesktop desktop notification
adapter remains enabled for update alerts, and desktop delivery is best effort
and never makes a sync fail.

## Dashboard components

The desktop issue workspace uses the `gpui-component` primitives that match
the interaction rather than duplicating them in the shell:

- Compact, standard, and wide layouts use a horizontal resizable split between
  the issue list and selected-issue detail. The mobile layout shows one pane at
  a time and provides an explicit back action.
- The issue list uses the component scrollbar. The refresh button uses the
  component button's loading state, which displays its spinner and disables
  duplicate activation while a refresh is running.
- Status filtering uses the component combobox in multiple-selection mode. It
  is a local presentation filter, not a change to the Jira query.
- In-app outcome messages use the component notification layer. They do not
  replace OS/Freedesktop desktop alerts.

Issues and Updates are navigation controls in the current shell, not tab
panels, so the component Tabs control is not currently used. A true tabbed
detail surface can be introduced when there are separate detail panels that
need tab semantics and keyboard navigation.

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
the current shell. Media references follow the conservative resolution and
thumbnail bounds above; no arbitrary Media Services URL is treated as a
download target. Parser, renderer, and aggregate media bounds apply
independently so cached content cannot force unbounded rich-text work in the
UI.

Issue snapshots are stored in the local SQLite cache, including display metadata
and rich descriptions. Details and comments are fetched remotely on selection
and held in memory; comment bodies are not persisted by the current cache.

Issue-type and priority labels remain the source of truth. The dashboard adds
small embedded `gpui-component` semantic icons as secondary cues: generic icons
cover Story, Initiative, Task, Sub-task, Bug, Epic, and unknown types, while
priority arrows/minus communicate Highest through Lowest with restrained theme
tones.

The comment composer is intentionally a plain multiline Textarea. Jira Desk
does not present it as a rich ADF editor: after confirmation, the plain text is
serialized as one safe Jira ADF paragraph and sent once through the dedicated
comment-write port. Rich authored marks, lists, mentions, and attachments are
not implied by the Textarea.

Descriptions and received comments are rendered from the bounded, supported ADF
subset described above. Empty ADF documents fall back to the normal empty-state
copy, and unsupported or unresolved media-only content remains visible through
safe placeholders rather than producing a blank panel or triggering an
implicit attachment download.

## Boundaries worth preserving

- Jira locators and account IDs are typed in application/domain contracts.
- HTTP status and transport details are mapped before reaching presentation.
- SQLite schema and migrations stay behind storage ports.
- UI state owns cancellation and stale-result guards, not transport code.
- The only Jira write port is dedicated to explicitly confirmed comment
  creation; issue edits, transitions, assignments, attachment mutations, and
  automatic writes remain prohibited. Attachment reads are authenticated and
  bounded; local download destinations are selected explicitly by the user.

For the component inventory and upgrade decisions, see
[`ui-component-audit.md`](ui-component-audit.md).
