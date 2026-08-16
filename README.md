# Jira GPUI

A read-only Jira Cloud desktop client built with GPUI and `gpui-component`.

Phase 1 targets Linux on Wayland and will be distributed as an AppImage. The
application core is kept independent from GPUI so another presentation adapter,
such as Tauri, can be added later without replacing Jira, synchronization, or
storage code.

## Workspace layout

- `apps/gpui`: GPUI presentation adapter and desktop entry point.
- `crates/domain`: UI-independent domain types.
- `crates/application`: use cases and ports implemented by adapters.
- `crates/jira`: framework-independent Jira request/response mapping.
- `crates/jira-http`: read-only Jira Cloud HTTP transport behind the application port.
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

The desktop opens a deterministic preview when Jira is not configured. For an
internal development build, setting all five variables below enables a manual,
read-only issue pull for the configured assignee account IDs. The UI starts
empty in live mode and reports pull progress and safe categorized failures; it
does not yet persist snapshots or generate update diffs.

The prototype reads these environment variables as an all-or-none set:

- `JIRA_BASE_URL`: an HTTPS `*.atlassian.net` site URL.
- `JIRA_SITE_ID`: the Atlassian cloud/site identifier.
- `JIRA_EMAIL`: the Atlassian account email used for the token.
- `JIRA_API_TOKEN`: an API token consumed into the in-memory HTTP client; the
  app does not persist or log this environment value.
- `JIRA_ASSIGNEE_ACCOUNT_IDS`: comma-separated stable Atlassian account IDs.

This API-token flow is intended only for internal development and local
testing; never commit or log these values. The application does not erase or
modify the process environment, so the environment remains the caller's
responsibility. Production/public distribution requires an interactive
Atlassian 3LO/OAuth flow with scoped, revocable credentials and a
platform-appropriate secret store. That is an independent release milestone,
separate from the planned macOS Phase 2 work.

The Linux release build will enable GPUI's Wayland backend only. X11 is not a
supported runtime target.
