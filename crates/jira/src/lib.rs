//! Read-only Jira Cloud adapter.
//!
//! This crate deliberately contains no UI state, persistence, credentials, or HTTP client.
//! It owns the boundary between Jira's enhanced-search JSON representation and a stable,
//! UI-independent set of records used by the application layer.

mod jql;
mod mapping;
mod models;

pub use jql::{
    AccountId as JqlAccountId, JqlError, assigned_issues_for_account_ids, assigned_issues_jql,
    enhanced_search_request,
};
pub use mapping::{
    DomainIssuePage, IssueMapper, MappingError, RemoteIssue, RemoteIssuePage, RemoteNamedEntity,
    RemoteProject, RemoteUser,
};
pub use models::{
    EnhancedSearchPage, EnhancedSearchRequest, JiraIssue, JiraIssueFields, JiraNamedEntity,
    JiraParentIssue, JiraProject, JiraUser,
};

/// The fields requested by the initial assigned-issues sync.
///
/// Keeping this list here prevents the UI from accidentally coupling itself to Jira's field
/// names. More detail fields can be requested by a separate issue-detail operation later.
pub const ASSIGNED_ISSUE_FIELDS: &[&str] = &[
    "summary",
    "issuetype",
    "project",
    "status",
    "priority",
    "assignee",
    "parent",
    "labels",
    "created",
    "updated",
    "duedate",
    "resolution",
];

/// A narrow, read-only request boundary for a Jira Cloud client.
///
/// An HTTP implementation can live in a future crate without exposing `reqwest` (or OAuth)
/// to either the UI or domain/application crates.
pub trait JiraSearchGateway: Send + Sync {
    fn enhanced_search(
        &self,
        request: EnhancedSearchRequest,
    ) -> impl Future<Output = Result<EnhancedSearchPage, JiraAdapterError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum JiraAdapterError {
    #[error("Jira returned malformed search data: {0}")]
    InvalidResponse(#[from] MappingError),
    #[error("the search request is invalid: {0}")]
    InvalidRequest(#[from] JqlError),
    #[error("the Jira transport failed: {0}")]
    Transport(String),
}
