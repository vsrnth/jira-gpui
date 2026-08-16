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
- `crates/jira`: read-only Jira Cloud adapter.
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

The desktop currently opens a functional preview backed by deterministic domain
fixtures: issue selection, issue details, the local update inbox, marking local
updates read, and the pull-updates action are wired. Live Atlassian
authentication and HTTP transport are the next vertical slice.

The Linux release build will enable GPUI's Wayland backend only. X11 is not a
supported runtime target.
