# Architecture

Jira Desk is split so the core behavior can be reused by another shell later.
The dependency direction is inward: presentation and adapters depend on
application contracts; domain and application do not depend on GPUI, HTTP,
Tokio, or SQLite.

## Workspace layout

| Path | Responsibility |
| --- | --- |
| `apps/gpui` | Cross-platform native GPUI desktop shell, onboarding, dashboard, detail views, and platform-specific window/file integrations |
| `crates/domain` | Jira-independent identifiers, issues, comments, users, and update values |
| `crates/application` | Use cases, ports, cancellation, sync, detail/media loading, comments, and polling policy |
| `crates/jira` | Pure Jira JSON/JQL mapping and bounded request construction |
| `crates/jira-http` | Jira Cloud transport, pagination, cancellation, response limits, and safe error mapping |
| `crates/storage` | Worker-thread SQLite adapter, migrations, cache, membership, cursors, and local feed state |
| `crates/desktop-notifications` | Linux-only best-effort Freedesktop notification adapter; unavailable on macOS |
| `packaging/appimage` | Linux x86_64 native Wayland AppImage build and inspection scripts |
| `packaging/macos` | Native macOS arm64/x86_64 DMG build and inspection procedures |

The GPUI shell uses native platform support on both targets. Linux enables
native Wayland controls, Freedesktop notifications, and XDG portals; macOS
uses the native keyring feature, native file picker, and macOS data locations:
`~/Library/Application Support/dev.jiradesk.JiraDesk` for application data and
`~/Library/Logs/dev.jiradesk.JiraDesk` for logs. X11 and Windows are unsupported.

## Runtime flow

```text
GPUI shell
  -> application services and ports
      -> Jira HTTP adapter       (remote reads/media plus confirmed bounded writes)
      -> SQLite storage adapter  (local cache and update state)
      -> Linux Freedesktop notification adapter (best effort; unavailable on macOS)
```

Synchronous environment bootstrap validates configuration, constructs the Jira
client, and opens SQLite without claiming an authenticated user. During
Dashboard initialization, the existing client resolves the authenticated
`/myself` user; only then does the shell create the account-scoped workspace and
load its account-scoped cache before the first refresh. The sync coordinator
applies the user-editable JQL scope and a remote `(assignee OR watcher)` filter
for the authenticated account; the dashboard trusts that user-set membership
and infers project labels from returned issue snapshots. A successful first
refresh is a quiet baseline. Later polls compare normalized snapshots and
persist deterministic update events. On Linux, desktop alerts retain the
narrower assigned-only policy. The issue list follows Jira `updated_at` newest
first, while issue-detail comments render newest first. A direct ADF mention of
the authenticated account also emits a local comment update and, on Linux, an
alert for a watcher-only ticket.

## Dashboard behavior

The dashboard has one Assigned-or-watched list. Status categories and text search are
local intersections over the retained domain issue list; they do not trigger
Jira or SQLite requests. The status control is a multi-select: selecting any
combination of To Do, In Progress, Done, and Uncategorized applies an OR
filter, while an empty selection means All statuses. Exact-key submission first
selects a local match. If the key is absent, Jira Desk performs a bounded,
cancellable lookup and shows a transient result without adding it to cache
membership.

Settings is a live-workspace-only surface for editing the JQL scope expression.
The editor validates a nonblank expression up to 2,000 bytes and rejects
`ORDER BY`; Jira Desk appends authenticated assigned-or-watched membership,
incremental `updated` overlap, and stable `ORDER BY updated DESC`. Saving first
switches to the scope-fingerprinted user set and runs a refresh. The preference
is atomically persisted only after that refresh commits. A Jira or local-write
failure rolls back the active scope and reloads the previous membership; the
editor retains the attempted text for correction.

Selecting an issue starts a separate cancellable detail request. The request
loads the description, all bounded comment pages, and attachment metadata.
Comments are ordered newest first for display. Full comment bodies remain in
memory only; local update metadata stores only a bounded display excerpt.
Description media is resolved conservatively: only a unique alt/filename
match, or the one-media/one-image case, can trigger an authenticated Jira
thumbnail read. Jira's documented REST surface does not provide a supported
conversion from an ADF Media Services UUID to an attachment ID, so exact
attachment mappings remain preferred. When no exact mapping exists, the mapper
retains at most 16 allowlisted image attachments as metadata-only fallback
candidates. The UI presents them in a clearly labeled bounded gallery and never
claims an ADF position for them. Each thumbnail is capped at 8 MiB, with at
most 16 references and 32 MiB aggregate per detail load. A thumbnail 404, or the
specific bounded unknown-MIME/unrecognized-signature thumbnail-unavailable
result, permits one bounded authenticated original-content fallback. Non-404
status, authentication, transport, malformed-MIME, empty, oversize, and other
errors remain errors. Cached attachment metadata must remain an allowlisted image
MIME. Authenticated thumbnail responses may use `application/octet-stream` or
Jira's `image/jpg`, but MIME preflight still requires a strict payload byte
signature before GPUI chooses a format. Results are applied only when the selected issue
and request generation still match; detail and thumbnail bytes are memory-only.
Configured-origin, redirect, and size protections remain enforced, and arbitrary
Media Services URLs are never followed.

An explicit attachment download is a separate user action. It reads the
configured Jira origin with authentication, caps the response at 64 MiB, and
writes only to the destination selected through a platform-native picker. On
Linux, that picker is the XDG document portal; macOS uses its native file
picker. The local write runs in the background after selection; it is not
automatic, is not retried automatically, and does not mutate Jira. Comment creation, assignment,
and status transition each use a dedicated confirmed request path and do not
share retry behavior with reads.

Issue-scoped edit metadata is persisted in SQLite per site and stable issue
locator. Available transitions and the bounded assignable-user candidate set
are fresh for 24 hours according to the injected application clock. The first
assignable-user read populates that cache through one bounded empty Jira query;
subsequent non-empty searches filter the cached candidates locally by display
name (and stable account ID for matching). A definite successful transition
invalidates the transition choices before the next picker read, while the
confirmed Jira write remains exactly-once and is never automatically retried.

When a sync has both cached and incoming snapshots for an issue whose
`updated_at` changed, it makes a bounded Jira Cloud bulk-changelog read (at
most 1,000 issue IDs per request). Each request is capped at eight cancellable
pages (at most 8,000 histories per issue-ID chunk) and bounded response bytes;
histories are restricted to `(old.updated_at, new.updated_at]`. Usable items
become bounded `FieldChanged` events with display-safe values, and corresponding
snapshot status/assignee/priority/due-date/summary/parent events are
deduplicated per issue. Changelog failures or unsupported gateways are
best-effort: the sync still succeeds with one honest generic `IssueUpdated`
fallback for that issue.

For those same update-emitting snapshots, mention detection is read-only and
examines only the newest 100 comments. On both supported platforms, a direct
ADF mention of the authenticated account creates a stable, locally deduplicated
`CommentAdded`/update event for the local feed; on Linux, it also creates a
desktop alert, including when the issue is watcher-only. Mention detection
begins on later syncs after the quiet baseline; it adds no Jira writes.

On Linux, Settings also exposes a test Freedesktop desktop notification. It
makes no Jira call or database event, uses the production app identity, and
reports the daemon-assigned ID or error category with a timestamp. Fixed-schema,
privacy-safe start/result records go to bounded `diagnostics.jsonl`; API
acceptance is not proof that GNOME rendered a banner. The Freedesktop adapter
and test are unavailable on macOS, where in-app feed and feedback remain
available.

The local update feed is derived from cache transitions. It is Jira Desk's
view of detected changes, not Jira's bell or inbox notification stream. Events
for the same issue are grouped into one ticket card; marking that card read
updates every event in the group in local storage only. The shell also shows
component-level in-app notifications for refresh and explicit write outcomes.
Manual refresh always produces one in-app summary, including when no new
updates were found. Feed navigation offers Unread and All filters. Generic
activity without an exact field uses compact fallback wording and progressive
disclosure rather than exposing raw internal event names. Event timestamps are
rendered in the system's local timezone with an explicit UTC offset. On Linux,
these are additive feedback: the Freedesktop desktop notification adapter
remains enabled for update alerts; its counts mean notifications accepted by
the desktop service, not guaranteed shell display, and delivery is best effort
and never makes a sync fail. On macOS, the in-app feed and feedback are
available, but the Freedesktop adapter and test are unavailable.

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
- In-app outcome messages use the component notification layer. On Linux they
  do not replace Freedesktop desktop alerts. The application registers the
  `gpui-component-assets` bundle so TitleBar and semantic icon assets render;
  idle minimize, maximize, and close controls stay discoverable, with hover
  styling as an enhancement rather than the only cue.

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

Assignee and status controls are issue-scoped. The shell reads the cached
assignable users and currently available workflow transitions, presents the
exact target, and requires a separate confirmation before calling the dedicated
issue-edit port. A confirmed write is dispatched once. Definite rejection is
safe to correct; an unknown outcome requires a Jira refresh before another
attempt.

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
- Dedicated Jira write ports accept only explicitly confirmed comment,
  assignment, and status-transition requests. Automatic writes and retries,
  arbitrary issue edits, deletions, and attachment mutations remain
  prohibited. Attachment reads are authenticated and bounded; local download
  destinations are selected explicitly by the user.

For the component inventory and upgrade decisions, see
[`ui-component-audit.md`](ui-component-audit.md).
