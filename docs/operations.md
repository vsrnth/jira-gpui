# Operations, onboarding, and security

## Interactive onboarding

The first-run form asks for exactly three values:

1. Jira Cloud URL.
2. Atlassian account email.
3. An unscoped API token.

Jira Desk trims the snapshots before constructing the session, validates the
credentials through Jira's authenticated `/myself` endpoint, and derives the
account ID and display name from that response. It never asks the user to
discover or enter an account ID. Missing local email/token input is reported
separately from remote `401 Unauthorized` and `403 Forbidden` failures.

The token input control is dropped before asynchronous connection work starts.
The token is held only by the in-memory Jira client; it is never logged or
written to SQLite. Error messages are safe and actionable without echoing
credentials.

## Environment startup

For internal development, startup can use this all-or-none set:

```text
JIRA_BASE_URL=https://your-site.atlassian.net
JIRA_SITE_ID=your-atlassian-cloud-id
JIRA_EMAIL=you@example.com
JIRA_API_TOKEN=your-unscoped-api-token
```

No project is fixed during onboarding. `JIRA_ASSIGNEE_ACCOUNT_IDS` is not
required and is not treated as authenticated identity. Environment bootstrap
constructs the client and opens SQLite but does not set an authenticated user.
Dashboard initialization resolves `/myself` through that client, then creates
the authenticated assigned-or-watched workspace and loads its scoped cache
before enabling the view. The active JQL scope defaults to Jira's generic
issue-type/status/created-date expression and may be replaced by a validated,
trimmed expression up to 2,000 bytes; `ORDER BY` is reserved for the adapter's
stable pagination ordering. Scope changes use a fingerprinted user-set and
start a quiet baseline rather than reusing another scope's cursor.

The Settings surface is available only after a live Jira workspace is ready.
It stores the exact normalized default as `null` and custom scopes as trimmed
text in the private atomic `preferences.json` file. Missing, unreadable, or
invalid preferences fall back to the documented default with a safe warning;
they never block Jira startup. `Save and refresh` validates locally, switches
to the scope-specific cache partition, waits for a successful Jira sync commit,
then saves the preference. Jira rejection or a local persistence failure rolls
back to the previous scope and preference without applying the new cache to the
dashboard. The editor keeps the failed expression visible for correction.

This API-token path is for internal development and local testing. Public
distribution still needs Atlassian OAuth 2.0 3LO with scoped, revocable
credentials and a platform-appropriate secret store. Jira Desk does not erase
or modify the process environment, so callers remain responsible for its
exposure.

## Local data and sync

The SQLite database is stored at `$XDG_DATA_HOME/jira-desk/jira-desk.sqlite3`
when `XDG_DATA_HOME` is a non-empty absolute path, otherwise at
`$HOME/.local/share/jira-desk/jira-desk.sqlite3`. Relative roots are rejected.
The application directory is created with mode `0700` and a new database with
mode `0600`. Credentials and raw Jira transport payloads are not stored.

The first successful refresh writes a quiet baseline. Manual refresh performs
full reconciliation; automatic polling is incremental with a five-minute
overlap and bounded backoff. Offline/upstream failures back off from 30 seconds
to 15 minutes. Rate limits honor a clamped `Retry-After` from 30 seconds to
one hour. Authentication, authorization, invalid-input, not-found, storage,
notification, and unknown-outcome failures pause automatic polling until a
successful manual refresh restarts it.

Cancellation is propagated through Jira pagination, detail loading, and exact
key lookup. A failed remote refresh leaves the last committed cache available.
Mark-read operations change only local state.

Remote membership uses `(assignee OR watcher)` for the authenticated account.
The cache trusts that user-set membership so watched issues remain visible.
Freedesktop desktop alerts intentionally remain assigned-only; watched issues
are available in the dashboard and local update feed without silently widening
OS alert delivery.

## Image diagnostics

Image decode and rendering diagnostics are best-effort. They are written as
JSON Lines to `$XDG_STATE_HOME/jira-desk/diagnostics.jsonl`
when `XDG_STATE_HOME` is a non-empty absolute path, otherwise to
`$HOME/.local/state/jira-desk/diagnostics.jsonl`. The state directory is mode
`0700`; the active log and its single backup are mode `0600`. Rotation keeps at
most 256 KiB in the active file plus one 256 KiB backup. A missing, unreadable,
or unwritable diagnostics log must not prevent the application from starting or
serving Jira data; diagnostic setup and write failures are isolated from normal
Jira operation.

Each record contains only structured safe enums and integers describing the
diagnostic stage, outcome, bounded image limits, and decode/fallback category.
Diagnostics never include tokens or `Authorization`, URLs or hosts, issue or
attachment IDs, filenames or alt text, descriptions or comments, response
bodies or raw bytes, or raw error text. In particular, a decode fallback
diagnostic identifies the safe `gpui_decode_fallback` category without copying
the remote payload or error into the log.

## Identity and rich content

Jira account IDs are retained internally because they are the stable keys used
for assignment, filtering, and update-event matching. The UI uses display names
from `/myself`, the authenticated user catalog, and embedded issue/comment
metadata. An unknown identity is shown as `Unknown user` or `Unknown author`,
and an empty assignment as `Unassigned`; an account ID is never used as a
display fallback.

Issue descriptions and comment bodies may be Jira ADF. Jira Desk accepts a
bounded safe subset of paragraphs, headings, lists, code blocks, quotes, panels,
text marks, and mentions. Unsupported nodes show a placeholder. HTTP(S) links
are validated and styled but remain inert. Description media is resolved only
when its attachment reference is unambiguous: a unique alt/filename match, or
the one-media/one-image case. Jira's documented REST APIs do not support
converting an ADF Media Services UUID into an attachment ID, so exact mappings
are always preferred. If a mapping remains unresolved, the UI shows a clearly
labeled, bounded gallery of remaining allowlisted Jira image attachments
without claiming ADF placement. Thumbnails are authenticated reads from the
configured Jira origin, capped at 8 MiB each, 16 references, and 32 MiB
aggregate; a 404, or the specific bounded unknown-MIME/unrecognized-signature
thumbnail-unavailable result, permits one bounded authenticated original-content
fallback. Authentication, transport, non-404 status, malformed-MIME, empty,
oversize, and other thumbnail errors do not fall back. Cached attachment metadata
must remain an allowlisted image MIME. Authenticated thumbnail responses may
report `application/octet-stream` or Jira's `image/jpg`; the response is still
preflighted and the payload must carry a strict image signature before selecting
the GPUI image format. Configured
origin, redirect, and size protections remain enforced. Arbitrary Media
Services URLs and redirects are never followed. Thumbnail bytes are memory-only
and are not written to SQLite or another automatic cache.

An attachment download is explicit and separate from description rendering.
It reads the configured authenticated Jira origin, rejects redirect behavior,
and caps the original content at 64 MiB. The user selects the destination via
the XDG document portal; only then does a background local write begin. Cancel
or success is reported to the UI, no automatic download or retry is performed,
and no Jira attachment or issue mutation occurs. The remote Jira state and the
local destination are intentionally separate.

Issue snapshots, including display metadata and rich descriptions, are retained
in the local SQLite cache. Selected issue details and comments are fetched
remotely and held in memory; comment bodies are not persisted across restart.

The dashboard is responsive: below 720 px it shows one mobile pane at a time;
720–959 px uses a compact navigation rail; 960–1,199 px uses the standard
desktop columns; and 1,200 px or wider uses the expanded desktop layout.

The application registers the GPUI component asset bundle at startup, keeping
title-bar and semantic icons rendered at rest. Client-side minimize, maximize,
and close controls remain discoverable when the window is idle; hover styling
adds emphasis but is not required to find the controls.

When started from an AppImage, startup also best-effort installs the current
launcher and icon for the current user before creating the GPUI window. With
an absolute `XDG_DATA_HOME`, the files are
`$XDG_DATA_HOME/applications/dev.jiradesk.JiraDesk.desktop` and
`$XDG_DATA_HOME/icons/hicolor/256x256/apps/dev.jiradesk.JiraDesk.png`, plus
the content-addressed
`$XDG_DATA_HOME/icons/hicolor/256x256/apps/dev.jiradesk.JiraDesk-<fingerprint>.png`;
otherwise the same paths are rooted at `$HOME/.local/share`. The desktop copy
contains the canonical absolute `APPIMAGE` path, is atomically refreshed on
each launch, and is safe to repeat. The stable
`dev.jiradesk.JiraDesk.png` copy supports named notification lookup; the
generated per-user entry points to an absolute
`dev.jiradesk.JiraDesk-<fingerprint>.png` copy, making icon-content changes
produce a new GNOME Shell cache key. Older fingerprint variants are not
automatically deleted. Although the embedded template uses the named
`Icon=dev.jiradesk.JiraDesk`, no cache updater runs. Permission or filesystem
failures are isolated: Jira Desk continues starting. To remove this
integration, delete the desktop entry, stable icon, and only the managed
app-ID-prefixed fingerprint variants in the exact icon directory; do not
broadly delete unrelated icons. This does not remove the AppImage or local
Jira data. Extracted AppDir runs without `APPIMAGE` do not register. On GNOME,
the registered `Name=Jira Desk` and installed icon are then used for Alt-Tab
and taskbar presentation instead of the raw `dev.jiradesk.JiraDesk` app ID.

On desktop, the issue list and selected-issue detail are separated by a
resizable split. On mobile, the shell presents one pane at a time with an
explicit back action. Status filtering accepts multiple categories; no
selected categories means All statuses. The issue list has its own component
scrollbar so long result sets do not expand the surrounding layout.

Refresh uses a loading spinner in the button while work is in progress. Refresh
and confirmed-write outcomes also appear as in-app notifications. These in-app
notifications are additive and do not disable or replace the existing
Freedesktop OS desktop alerts for update delivery.

The local update feed groups detected events by Jira issue. Marking one ticket
read updates all event IDs in that group in SQLite only; Mark all read applies
the same local operation to the entire displayed feed. Neither action contacts
Jira.

## Jira write policy

Creating a comment is one of three allowed Jira writes. The composer is
memory-only and validates a nonblank body of at most 10,000 Unicode scalar
characters and 64 KiB UTF-8. The user must confirm the exact issue, body, and
sizes before the single dispatch. A confirmed request is never automatically
retried. If the
outcome is unknown, the draft is retained and the UI asks the user to refresh
comments before deciding whether to retry.

The composer is a plain Textarea, not a rich ADF editor. Jira Desk wraps the
confirmed plain text in a safe Jira ADF paragraph before sending it. Received
descriptions and comments use the bounded read-only ADF renderer; unsupported
nodes and empty documents use safe fallbacks.

Assignment changes and status transitions follow the same safety boundary. The
UI first reads users assignable to the selected issue or its currently
available transitions, then shows the exact target for a separate confirmation.
Each confirmed request is sent once without automatic retry. An unknown outcome
must be reconciled by refreshing Jira before another attempt.

No other Jira issue edits, deletions, worklogs, attachment uploads, or
background Jira writes are supported. A local attachment download is the sole
exception to the “no background write” wording: it is a user-selected local
file write, never a Jira mutation or an automatic action.

Freedesktop OS alerts remain enabled for update delivery independently of the
in-app notification layer. Media loading, local download cancellation, and
file-write errors must not disable or replace those OS alerts.
