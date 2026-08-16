//! GPUI presentation adapter for Jira Desk.
//!
//! The binary entry point only performs native window composition. Keeping the
//! view in this library lets its mapping and render code compile independently
//! of a platform renderer, which is useful while Linux is the only release
//! target and macOS remains a later phase.

mod dashboard;
mod presentation;
mod sample_data;

pub use dashboard::Dashboard;
