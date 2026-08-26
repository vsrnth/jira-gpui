# Jira Desk

Jira Desk is a focused Jira Cloud desktop client for keeping an authenticated
user's assigned or watched issues visible. It keeps a local cache, detects changes, and
shows a local update feed without turning Jira into a second system of record.

Supported desktop targets are Linux x86_64 on native Wayland, distributed as an
AppImage, and native macOS on arm64 or x86_64, packaged as a DMG. X11 and
Windows are unsupported.

## What it does

- Authenticates with a Jira Cloud site and derives the user from Jira's
  `/myself` endpoint; onboarding never asks for an account ID.
- Synchronizes the configured Jira scope across projects, returning issues
  assigned to or watched by the authenticated user.
- Infers the project label from returned issues; onboarding does not contain
  project-specific choices.
- Adapts from a mobile single-pane view to compact and full desktop layouts.
- Filters one or more status categories, searches issue keys/summaries locally,
  and can perform a cancellable exact-key Jira lookup for an issue outside the
  local cache.
- Loads selected-issue descriptions, paginated comments, and attachment
  metadata lazily; issue comments are shown newest first. Rich Jira text is
  displayed through a safe subset. A
  description image is fetched only when its attachment reference resolves
  unambiguously (a unique alt/filename, or the one-media/one-image case), as a
  bounded authenticated thumbnail held in memory. If an ADF Media Services UUID
  cannot be converted through Jira's documented REST APIs, the renderer shows a
  clearly labeled, bounded gallery of remaining allowlisted Jira image
  attachments without claiming those candidates occupy the unresolved ADF
  position. Explicit attachment downloads use a user-selected destination and
  are separate from description rendering. Linux uses the XDG document portal;
  macOS uses its native file picker.
- Shows user display names in the interface while retaining stable Jira
  account IDs only for matching and local application state.
- Keeps a durable local update feed grouped by ticket, with local per-ticket
  and global mark-read actions and in-app operation feedback. Linux also has
  best-effort Freedesktop desktop notifications; macOS currently uses the
  in-app feed and feedback only.
- On Linux, desktop alerts remain limited to assigned issues; watched issues
  appear in the list and local feed without expanding ordinary OS alerts.
  Direct ADF mentions of the authenticated account also create local comment
  updates and desktop alerts on watcher-only tickets.
- Lets the user explicitly choose and confirm an assignee change or one of the
  issue's currently available workflow transitions.
- Provides client-side Wayland title-bar controls on Linux and a local SQLite
  cache on both supported targets.
- Keeps bounded, privacy-safe image diagnostics in the local state directory;
  diagnostic setup and write failures never prevent startup, and logging is
  best-effort.

Jira writes are limited to three deliberate actions: creating a comment,
changing an assignee, and applying an available status transition. Each action
shows the exact issue and target for explicit confirmation, is sent once with
no automatic retry, and requires a refresh before retrying an uncertain result.
All other issue edits and attachment mutations remain unsupported.

Media reads do not mutate Jira. Description thumbnails are limited to 8 MiB
each, 16 references, and 32 MiB aggregate, with no arbitrary Media Services
URLs, redirects, or persistence. A thumbnail 404, or the specific bounded
unknown-MIME/unrecognized-signature thumbnail-unavailable result, may fall back
once to bounded authenticated original content. Non-404 status, authentication,
transport, malformed-MIME, empty, oversize, and other thumbnail errors do not.
Cached attachment metadata must remain an allowlisted image MIME; authenticated
thumbnail responses may use `application/octet-stream` or Jira's `image/jpg`,
but the payload must carry a strict image signature before GPUI chooses the decoder. Origin,
redirect, and size protections remain enforced. An explicit attachment download
is limited to 64 MiB, reads only from the configured authenticated Jira origin,
writes in the background only after the user selects a destination, and never
starts automatically or retries itself.

The GPUI asset bundle is registered at application startup so title-bar and
semantic icons render without a hover-only discovery dependency. On Linux,
window minimize, maximize, and close controls are client-side Wayland controls;
hover may provide emphasis, but idle controls remain discoverable. Image
diagnostics use only safe structured enums and integers, rotate within 256 KiB
plus one backup, and exclude credentials, URLs, Jira identifiers, user content,
payloads, and raw errors.

On Linux, Settings can send a test Freedesktop desktop notification without
making a Jira call or creating a database event. The diagnostic uses the
production app identity, shows the daemon-assigned ID/error category and
timestamp, and writes privacy-safe fixed-schema start/result entries to bounded
`diagnostics.jsonl`. API acceptance does not prove that GNOME or another shell
rendered a banner. The Freedesktop adapter and test are unavailable on macOS;
its in-app feed and feedback remain available.

## Prerequisites

- Linux x86_64 with a Wayland compositor and the build dependencies listed in
  [`packaging/appimage/README.md`](packaging/appimage/README.md), or native
  macOS arm64/x86_64 with the tools listed in
  [`packaging/macos/README.md`](packaging/macos/README.md).
- Rust 1.95 or newer, installed through [rustup](https://rustup.rs/). GPUI is
  native on both supported targets.
- A Jira Cloud site and a scoped Atlassian API token for local development. Tokens
  are secrets: do not commit, log, or paste them into issue reports.

## Quick start

```bash
cargo run -p jira-gpui
```

On first launch, enter the Jira URL and Atlassian email, then create an **API
token with scopes** and select exactly these classic scopes:

```text
read:jira-user
read:jira-work
write:jira-work
```

Jira Desk discovers the Cloud ID automatically from the site URL, and Jira
permissions still apply. “Remember securely in system keyring” is enabled by
default; when selected, the URL, email, and token are stored only in the
system keyring after successful authentication. The macOS build uses its native
keyring feature. Uncheck it for a session-only login. See [operations and
security](docs/operations.md) for credential handling and local data.

## Development commands

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --lib --bins --locked -- -D warnings
cargo run -p jira-gpui
```

For the Linux AppImage and macOS DMG workflows and current validation
boundaries, see [release and validation](docs/release.md),
[Linux packaging](packaging/appimage/README.md), and
[macOS packaging](packaging/macos/README.md). For system boundaries and data
flow, see [architecture](docs/architecture.md). The current roadmap is in
[the implementation plan](docs/implementation-plan.md).

Run the validation commands in [release and validation](docs/release.md)
against the current tree; test totals and smoke-artifact versions are expected
to change as the project evolves.
