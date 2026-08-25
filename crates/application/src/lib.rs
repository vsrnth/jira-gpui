//! UI- and infrastructure-independent application use cases.
//!
//! The crate owns orchestration and the contracts implemented by Jira, SQLite,
//! desktop-notification, GPUI, and (potentially) Tauri adapters. It deliberately
//! contains no executor, database, HTTP, or UI dependencies.

mod attachment_diagnostics;
mod cancellation;
mod comment;
mod comment_pagination;
mod error;
mod feed;
mod issue_detail;
mod issue_diff;
mod issue_edit;
mod issue_media;
mod issue_pagination;
mod issue_pull;
mod issues;
mod model;
mod notifications;
mod polling;
mod ports;
mod sync;
#[cfg(test)]
mod test_support;
mod user_sets;

pub use attachment_diagnostics::{
    AttachmentBodyClass, AttachmentMimeClass, AttachmentReadAttempt, AttachmentReadDiagnostic,
    AttachmentReadStage, AttachmentTransportClass,
};
pub use cancellation::CancellationToken;
pub use comment::{CommentService, MAX_COMMENT_BYTES, MAX_COMMENT_CHARS};
pub use error::{ApplicationError, ErrorKind};
pub use feed::UpdateFeedService;
pub use issue_detail::{IssueDetailConfig, IssueDetailService};
pub use issue_diff::{DefaultIssueDiffer, enrich_with_changelog};
pub use issue_edit::{
    ISSUE_EDIT_CACHE_TTL, IssueEditService, MAX_ASSIGNABLE_USER_SEARCH_LIMIT, MAX_ISSUE_TRANSITIONS,
};
pub use issue_media::{
    DEFAULT_ATTACHMENT_IMAGE_HEIGHT, DEFAULT_ATTACHMENT_IMAGE_WIDTH,
    DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES, DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES, IssueMediaConfig,
    IssueMediaService,
};
pub use issue_pull::{IssuePullConfig, IssuePullOutcome, IssuePullRequest, IssuePullService};
pub use issues::IssueCatalogService;
pub use model::*;
pub use notifications::DefaultDesktopNotificationPolicy;
pub use polling::DefaultPollingPolicy;
pub use ports::*;
pub use sync::{SyncConfig, SyncService};
pub use user_sets::UserSetService;
