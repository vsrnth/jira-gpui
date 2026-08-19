//! GPUI presentation adapter for Jira Desk.
//!
//! The binary entry point only performs native window composition. Keeping the
//! view in this library lets its mapping and render code compile independently
//! of a platform renderer, which is useful while Linux is the only release
//! target and macOS remains a later phase.

mod app_shell;
mod config;
#[cfg(target_os = "linux")]
pub(crate) mod credential_store;
mod dashboard;
mod diagnostics;
mod live_workspace;
mod local_data;
mod presentation;
mod responsive;
mod rich_text_view;
mod sample_data;
mod semantic_icons;
mod team_table;

pub use app_shell::AppShell;
pub use config::{StartupSelection, startup_from_environment};
pub use dashboard::Dashboard;
pub use live_workspace::{CachedWorkspace, FeedActionResult, LiveWorkspace, RefreshResult};
