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

The project is fixed to Jira Project. `JIRA_ASSIGNEE_ACCOUNT_IDS` is not
required and is not treated as authenticated identity. Environment bootstrap
constructs the client and opens SQLite but does not set an authenticated user.
Dashboard initialization resolves `/myself` through that client, then creates
the authenticated workspace and loads its scoped cache before enabling the
My-issues view.

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
are validated and styled but remain inert, and media nodes are placeholders:
the client does not download or open media or attachments.

Issue snapshots, including display metadata and rich descriptions, are retained
in the local SQLite cache. Selected issue details and comments are fetched
remotely and held in memory; comment bodies are not persisted across restart.

The dashboard is responsive: below 720 px it shows one mobile pane at a time;
720–959 px uses a compact navigation rail; 960–1,199 px uses the standard
desktop columns; and 1,200 px or wider uses the expanded desktop layout.

## Comment policy

Creating a comment is the sole Jira write. The composer is memory-only and
validates a nonblank body of at most 10,000 Unicode scalar characters and 64
KiB UTF-8. The user must confirm the exact issue, body, and sizes before the
single dispatch. A confirmed request is never automatically retried. If the
outcome is unknown, the draft is retained and the UI asks the user to refresh
comments before deciding whether to retry.

No Jira issue edits, deletions, transitions, assignments, worklogs, attachment
uploads, or background writes are supported.
