# Jira Desk

Jira Desk is a focused Jira Cloud desktop client for keeping an authenticated
user's assigned issues visible. It keeps a local cache, detects changes, and
shows a local update feed without turning Jira into a second system of record.

Phase 1 targets Linux on native Wayland and ships as an x86_64 AppImage.
macOS is planned for Phase 2; X11 and Windows are out of scope for Phase 1.

## What it does

- Authenticates with a Jira Cloud site and derives the user from Jira's
  `/myself` endpoint; onboarding never asks for an account ID.
- Synchronizes the Jira Project project, then shows only the authenticated
  user's issues in the dashboard.
- Adapts from a mobile single-pane view to compact and full desktop layouts.
- Filters one or more status categories, searches issue keys/summaries locally,
  and can perform a cancellable exact-key Jira lookup for an issue outside the
  local cache.
- Loads selected-issue descriptions, paginated comments, and attachment
  metadata lazily. Rich Jira text is displayed through a safe subset. A
  description image is fetched only when its attachment reference resolves
  unambiguously (a unique alt/filename, or the one-media/one-image case), as a
  bounded authenticated thumbnail held in memory. If an ADF Media Services UUID
  cannot be converted through Jira's documented REST APIs, the renderer shows a
  clearly labeled, bounded gallery of remaining allowlisted Jira image
  attachments without claiming those candidates occupy the unresolved ADF
  position. Explicit attachment downloads use a user-selected XDG portal
  destination and are separate from description rendering.
- Shows user display names in the interface while retaining stable Jira
  account IDs only for matching and local application state.
- Keeps a durable local update feed, in-app refresh/comment feedback, and
  best-effort OS desktop notifications.
- Provides client-side Wayland title-bar controls and a local SQLite cache.
- Keeps bounded, privacy-safe image diagnostics in the local state directory;
  diagnostic setup and write failures never prevent startup, and logging is
  best-effort.

Jira operations are read-only except for one deliberate action: creating a
comment after the user explicitly confirms the exact issue and body. Comment
creation is sent once, with no automatic retry; an uncertain result requires
refreshing comments before retrying.

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
semantic icons render without a hover-only discovery dependency. Window
minimize, maximize, and close controls remain client-side Wayland controls;
hover may provide emphasis, but idle controls remain discoverable. OS alerts
remain unchanged and independent of in-app notifications. Image diagnostics use
only safe structured enums and integers, rotate within 256 KiB plus one backup,
and exclude credentials, URLs, Jira identifiers, user content, payloads, and
raw errors.

## Prerequisites

- Linux with a Wayland compositor and the build dependencies listed in
  [`packaging/appimage/README.md`](packaging/appimage/README.md).
- Rust 1.95 or newer, installed through [rustup](https://rustup.rs/).
- A Jira Cloud site and an Atlassian API token for local development. Tokens
  are secrets: do not commit, log, or paste them into issue reports.

## Quick start

```bash
cargo run -p jira-gpui
```

On first launch, enter the Jira URL, Atlassian email, and unscoped API token.
Jira Desk derives your Jira identity from the authenticated connection; you do
not need to find or enter an account ID. See [operations and
security](docs/operations.md) for credential handling and local data.

## Development commands

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --lib --bins --locked -- -D warnings
cargo run -p jira-gpui
```

For the AppImage workflow and current validation boundaries, see
[release and validation](docs/release.md). For system boundaries and data
flow, see [architecture](docs/architecture.md). The current roadmap is in
[the implementation plan](docs/implementation-plan.md).

Run the validation commands in [release and validation](docs/release.md)
against the current tree; test totals and smoke-artifact versions are expected
to change as the project evolves.
