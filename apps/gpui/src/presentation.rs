//! Maps framework-independent domain objects into display-ready data.
//!
//! Keeping this mapping free of GPUI types means a future Tauri presentation
//! adapter can reuse the same decisions without depending on the native UI.

mod format;
mod identity;
mod issues;
mod updates;

pub use identity::IdentityDirectory;
#[allow(unused_imports)]
pub use issues::{
    AttachmentViewModel, CommentViewModel, IssueDetailViewModel, IssueStatusFilter,
    IssueStatusSelection, IssueViewModel, issue_views_for_filter, normalized_issue_key,
};
pub use updates::{UpdateGroupViewModel, UpdateViewModel, update_groups_for_events};

pub(crate) use format::format_timestamp;
pub(crate) use updates::describe_update_with_directory;

#[cfg(test)]
use format::format_timestamp_with_offset;
#[cfg(test)]
use jira_domain::{ChangeValue, Issue, IssueKey, UpdateEvent, UpdateKind, User};
#[cfg(test)]
use time::UtcOffset;

#[cfg(test)]
mod tests;
