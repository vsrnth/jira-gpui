//! UI- and infrastructure-independent application use cases.
//!
//! The crate owns orchestration and the contracts implemented by Jira, SQLite,
//! desktop-notification, GPUI, and (potentially) Tauri adapters. It deliberately
//! contains no executor, database, HTTP, or UI dependencies.

mod cancellation;
mod comment;
mod error;
mod feed;
mod issue_detail;
mod issue_diff;
mod issue_pull;
mod issues;
mod model;
mod notifications;
mod polling;
mod ports;
mod sync;
mod user_sets;

pub use cancellation::CancellationToken;
pub use comment::{CommentService, MAX_COMMENT_BYTES, MAX_COMMENT_CHARS};
pub use error::{ApplicationError, ErrorKind};
pub use feed::UpdateFeedService;
pub use issue_detail::{IssueDetailConfig, IssueDetailService};
pub use issue_diff::DefaultIssueDiffer;
pub use issue_pull::{IssuePullConfig, IssuePullOutcome, IssuePullRequest, IssuePullService};
pub use issues::IssueCatalogService;
pub use model::*;
pub use notifications::DefaultDesktopNotificationPolicy;
pub use polling::DefaultPollingPolicy;
pub use ports::*;
pub use sync::{SyncConfig, SyncService};
pub use user_sets::UserSetService;
