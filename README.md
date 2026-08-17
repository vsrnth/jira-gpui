# Jira Desk

Jira Desk is a small Jira Cloud desktop client for keeping an authenticated
user's assigned issues visible. It keeps a local SQLite cache, detects changes,
and presents a local update feed without turning Jira into a second system of
record.

Phase 1 targets Linux on native Wayland and ships as an x86_64 AppImage.
macOS is planned for Phase 2; X11 and Windows are out of scope for Phase 1.

## What it does

- Authenticates with a Jira Cloud site and derives the user from Jira's
  `/myself` endpoint; onboarding never asks for an account ID.
- Synchronizes the Jira Project project, then shows only the authenticated
  user's issues in the dashboard.
- Filters status, searches issue keys/summaries locally, and can perform a
  cancellable exact-key Jira lookup for an issue outside the local cache.
- Loads selected-issue descriptions, paginated comments, and attachment
  metadata lazily. Attachment content is never downloaded or opened.
- Keeps a durable local update feed and best-effort desktop notifications.
- Provides client-side Wayland title-bar controls and a local SQLite cache.

Jira operations are read-only except for one deliberate action: creating a
comment after the user explicitly confirms the exact issue and body. Comment
creation is sent once, with no automatic retry; an uncertain result requires
refreshing comments before retrying.

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
Jira Desk validates them through `/myself`, discards the token input before
connection work begins, and keeps credentials only in the in-memory client.
Environment bootstrap first constructs the client and opens SQLite; Dashboard
initialization then resolves `/myself` before creating the authenticated
workspace and loading its scoped cache. See [operations and
security](docs/operations.md).

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
