# Refactoring roadmap

This living roadmap tracks behavior-preserving, independently reviewable refactors—not feature work. Baseline: `6f3eb55`.

## Status legend

- **Completed** — merged and recorded here.
- **Ready** — audited and bounded for implementation.
- **Blocked** — scoped, but validation is currently unavailable.
- **Deferred** — intentionally held for later review.

Items 3–4 are audited queued items.

## Ranked work

| Rank / status | Ownership / files | Intent | Scope guardrails | Validation | Commit when completed |
| --- | --- | --- | --- | --- | --- |
| 1 · **Completed** | Application: `crates/application/src/issue_pagination.rs`, `crates/application/src/issue_pull.rs`, `crates/application/src/sync.rs`, `crates/application/src/lib.rs` | Share issue pagination, cursor safety, and response deduplication policy between pulls and sync. | Preserve first-seen ordering, latest snapshots, cancellation, cursor-cycle checks, page limits, and server-time behavior; keep application independent of adapters. | Boundary: focused application pagination, pull, and sync tests. | `b16b18e` |
| 2 · **Completed** | Application: `crates/application/src/comment_pagination.rs`, `crates/application/src/issue_detail.rs`, `crates/application/src/lib.rs` | Extract comment pagination state from issue-detail orchestration. | Preserve start-at/cursor handling, totals, limits, cycle/progress checks, and returned comment order; no port or persistence changes. | Boundary: focused comment-pagination and issue-detail tests. | `6f3eb55` |
| 3 · **Blocked** | GPUI presentation: `apps/gpui/src/dashboard.rs` → existing `apps/gpui/src/presentation/updates.rs` (or a private sibling module) | Move pure update-feed policy out of the dashboard. | Move only `UpdateFilter`, `filtered_update_group_indices`, `CompactedUpdateRow`, `compact_update_rows`, `generic_summary_label`, preview-count helpers/constants, and `update_group_event_ids`. Keep rendering and handlers in `dashboard.rs`; preserve ordering, read/unread, and mark-read semantics. | Focused dashboard/presentation tests on Linux Wayland. Local validation is blocked because macOS compilation currently fails on pre-existing unconditionally imported Linux-gated `credential_store` symbols; Linux Wayland remains the Phase 1 target. | — |
| 4 · **Ready** | Application test modules → `crates/application/src/test_support.rs` under `cfg(test)`; current helpers in `crates/application/src/comment.rs`, `crates/application/src/issue_detail.rs`, `crates/application/src/issue_edit.rs`, `crates/application/src/issue_media.rs`, `crates/application/src/issue_pull.rs`, and `crates/application/src/sync.rs` | Consolidate duplicated `block_on`/no-op-waker test helpers. | Test-only module and imports; no runtime dependency, fake/fixture consolidation, or production API. Keep each test’s behavior and ownership intact. | Focused application tests, then relevant application/workspace validation. | — |

Workspace-wide macOS compilation and all-target Clippy blockers are validation context only, not tasks in this roadmap.

## Workflow

Select the highest-ranked **Ready** item, assign one Luna writer with bounded file ownership, preserve behavior, and run focused plus relevant broad validation. Obtain independent review. Sol commits one coherent change, then updates this roadmap in a separate documentation commit when the status changes.
