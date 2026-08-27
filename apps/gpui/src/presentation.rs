//! Maps framework-independent domain objects into display-ready data.
//!
//! Keeping this mapping free of GPUI types means a future Tauri presentation
//! adapter can reuse the same decisions without depending on the native UI.

mod format;
mod identity;
mod issues;
mod outcomes;
mod updates;

pub use identity::IdentityDirectory;
pub(crate) use issues::issue_views_for_filter_with_offset;
#[allow(unused_imports)]
pub use issues::{
    AttachmentViewModel, CommentViewModel, IssueDetailViewModel, IssueStatusFilter,
    IssueStatusSelection, IssueViewModel, issue_views_for_filter, normalized_issue_key,
};
pub(crate) use updates::update_groups_for_events_with_offset;
#[allow(unused_imports)]
pub use updates::{UpdateGroupViewModel, update_groups_for_events};

pub(crate) use outcomes::{
    CommentOutcomeKind, FeedbackSeverity, IssueEditPhase, OutcomeCopy, ReadSurface,
    RecoveryDirective, SavedLoginOutcomeKind, ScopeOutcomeKind, TeamInvalidInputKind,
    TeamOutcomeKind, comment_outcome_copy, comment_validation_kind, issue_edit_error_copy,
    issue_edit_workspace_unavailable_copy, lookup_workspace_unavailable_copy, read_error_copy,
    saved_login_outcome_copy, scope_outcome_copy, team_outcome_copy,
};

#[cfg(test)]
pub(crate) use outcomes::FeedbackCertainty;
#[cfg(test)]
pub(crate) use updates::UpdateViewModel;

pub(crate) use updates::{
    CompactedUpdateRow, UPDATE_PREVIEW_LIMIT, UpdateFilter, compact_update_rows,
    filtered_update_group_indices, generic_summary_label, hidden_update_row_count,
    update_group_event_ids, visible_update_row_count,
};

#[allow(unused_imports)]
pub(crate) use format::{format_timestamp, format_timestamp_for};
pub(crate) use updates::describe_update_with_directory;

#[cfg(test)]
use format::format_timestamp_with_offset;
#[cfg(test)]
use jira_domain::{ChangeValue, Issue, IssueKey, UpdateEvent, UpdateKind, User};
#[cfg(test)]
use time::UtcOffset;

#[cfg(test)]
mod tests;
