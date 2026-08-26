//! GPUI presentation adapter for Jira Desk.
//!
//! The binary entry point only performs native window composition. Keeping the
//! view in this library lets its mapping and render code compile independently
//! of a platform renderer, which keeps the application logic independent from
//! the native window entry points.

mod app_shell;
mod config;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) mod credential_store;
mod dashboard;
mod diagnostics;
mod live_workspace;
mod local_data;
mod presentation;
mod responsive;
mod rich_text_view;
#[cfg(test)]
mod sample_data;
mod semantic_icons;
mod team_table;

pub use app_shell::AppShell;
pub use config::{StartupSelection, startup_from_environment};
