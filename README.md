# Jira GPUI

A Jira Cloud desktop client built with GPUI and `gpui-component`. Jira
synchronization remains read-only; the sole remote write is explicit,
user-confirmed comment creation.

Phase 1 targets Linux on Wayland and will be distributed as an AppImage. The
application core is kept independent from GPUI so another presentation adapter,
such as Tauri, can be added later without replacing Jira, synchronization, or
storage code.

## Workspace layout

- `apps/gpui`: GPUI presentation adapter and desktop entry point.
- `crates/domain`: UI-independent domain types.
- `crates/application`: use cases and ports implemented by adapters.
- `crates/jira`: framework-independent Jira request/response mapping.
- `crates/jira-http`: Jira Cloud HTTP transport behind the application ports.
- `crates/storage`: local persistence adapter.

See [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) for scope and milestones.

## Development

`gpui-component` is pinned in `Cargo.toml`; GPUI uses the same Git source URL as
the component and is pinned to its compatible commit by `Cargo.lock`. Update
them together and verify Linux Wayland plus the local development platform
before accepting an upgrade.

Install Rust through [rustup](https://rustup.rs/) and use Rust 1.95 or newer.
The current GPUI revision relies on `std::hint::cold_path`, which became stable
in Rust 1.95. A system package manager may provide an older compiler even when
the package itself is fully up to date.

```bash
cargo --version
cargo test --workspace
cargo run -p jira-gpui
```

The desktop opens a native Jira setup form when no configuration is available.
Enter the Jira Cloud site URL, Atlassian account email, and an unscoped API
token. Jira Desk verifies the credentials with the authenticated-current-user
endpoint, syncs all issues in the Jira Project project, and derives the
authenticated account automatically; no account ID needs to be discovered or
pasted. The token is masked, discarded from the input before connection starts,
held only by the in-memory HTTP client, and never stored locally or logged.
The authenticated `/myself` user is also resolved during environment startup;
interactive onboarding reuses the user it already verified, so neither path
asks for an account ID. Assignee values use that user's Jira display name when
available. The Dashboard displays only My issues for the authenticated account;
the remote sync remains project-wide, while status-category filters (All
statuses, To do, In progress, Done, and Uncategorized) are local over the
loaded cache and never refetch Jira. Search immediately filters cached issue
keys and summaries locally. Pressing Enter or choosing `Search Jira` with a
strict Jira key performs a cancellable exact-key lookup, including for an issue
not present in the local cache; the transient result is not inserted into cache
membership. A client-side Wayland title bar provides minimize,
maximize/restore, and close controls.
Live startup opens the local SQLite cache, uses an Jira Project/account-scoped
workspace identity, and loads cached issues and in-app update events before
contacting Jira. The first successful refresh establishes a
quiet baseline; later automatic polls are incremental and preserve membership,
while manual refresh remains full reconciliation. The live Dashboard owns one
cancellable polling task, starts its first automatic tick after five minutes,
and prevents overlap with manual refresh/feed actions. Offline/upstream errors
back off from 30 seconds to 15 minutes, rate limits honor a clamped 30-second
to one-hour Retry-After, and nontransient errors pause polling until a
successful manual refresh restarts it. Polling exists only while the app runs.
Jira failures leave the last committed cache available, and mark-all-read
changes are local only. The Local updates feed is Jira Desk's durable view of
detected changes, not Jira's bell/inbox notification stream; desktop delivery
is best-effort and the local feed remains authoritative.

Phase 1 local data is stored under `$XDG_DATA_HOME/jira-desk/` when
`XDG_DATA_HOME` is set to a non-empty absolute path, or
`$HOME/.local/share/jira-desk/` when it is unset or empty, in
`jira-desk.sqlite3`. Relative roots are rejected. The app
directory is created with Unix mode `0700` and a newly created database file
with mode `0600`; SQLite uses a worker thread, WAL,
foreign keys, migrations, and protected database-file opening. Credentials are
not stored in SQLite: the current internal API-token flow keeps them only in
the in-memory Jira HTTP client. The Linux Freedesktop notification adapter is
best-effort: the default policy notifies for issue-added, status, assignee,
priority, due-date, and comment events. Removal, summary, and parent events
remain in-app only; delivery failures are nonfatal and the durable in-app feed
is authoritative.

The prototype reads these environment variables as an all-or-none set; the
project is fixed to Jira Project and no assignee variable is required:

- `JIRA_BASE_URL`: an HTTPS `*.atlassian.net` site URL.
- `JIRA_SITE_ID`: the Atlassian cloud/site identifier.
- `JIRA_EMAIL`: the Atlassian account email used for the token.
- `JIRA_API_TOKEN`: an API token consumed into the in-memory HTTP client; the
  app does not persist or log this environment value.

Both interactive and environment startup resolve Jira's authenticated `/myself`
identity, and My issues is enabled after that check succeeds. Selecting an
issue lazily loads its description, paginated comments, and attachment
metadata; these detail requests are memory-only, bounded, cancellable, and
read-only. Attachment content is never downloaded or opened. The comment
composer is memory-only and requires explicit confirmation showing the target
issue and body size. Confirmed comment creation is the sole Jira write; there
are no automatic retries. If Jira may have accepted a comment but the outcome
is unknown, Jira Desk retains the draft and requires Refresh comments before a
retry.

This API-token flow is intended only for internal development and local
testing; never commit or log these values. The current direct-site transport
does not support scoped API tokens; those use
`https://api.atlassian.com/ex/jira/{cloudId}` instead. The application does not
erase or modify the process environment, so the environment remains the
caller's responsibility. Production/public distribution still requires an
interactive Atlassian 3LO/OAuth flow with scoped, revocable credentials and a
platform-appropriate secret store; collected API tokens are not suitable for
public distribution. That is an independent release milestone, separate from
the planned macOS Phase 2 work.

The Linux release build will enable GPUI's Wayland backend only. X11 is not a
supported runtime target. Production OAuth and the Linux runtime matrix remain
outstanding; macOS is Phase 2.

Validation includes 123 Linux-target workspace tests in an x86_64 Ubuntu 22.04
container with Rust 1.95.0 (124 on the macOS host due to the non-Linux adapter
fallback test), rustfmt, warning-denied production Clippy, metadata checks, and
the Cargo feature guard. A 0.1.0 AppImage was built with checksum-verified
pinned tools/runtime; its checksum, extracted contents, required files, and
`ldd` library/X11 checks passed. The GitHub-hosted workflow, AppImage
build/extraction inspection, and artifact upload are validated by CI. Wayland
GUI launch, FUSE execution, real Jira and notification-daemon delivery, public
release, and multi-distribution runtime coverage remain unvalidated.
