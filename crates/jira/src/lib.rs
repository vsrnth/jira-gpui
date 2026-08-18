//! Read-only Jira Cloud adapter.
//!
//! This crate deliberately contains no UI state, persistence, credentials, or HTTP client.
//! It owns the boundary between Jira's enhanced-search JSON representation and a stable,
//! UI-independent set of records used by the application layer.

mod jql;
mod mapping;
mod models;

pub use jql::{
    AccountId as JqlAccountId, DEFAULT_JQL_SCOPE, JqlError, MAX_ISSUE_IDS, MAX_JQL_SCOPE_LENGTH,
    assigned_issues_for_account_ids, assigned_issues_jql,
    assigned_or_watched_issues_for_account_ids, assigned_or_watched_issues_jql,
    bulk_changelog_request, enhanced_search_request, enhanced_search_request_for_issue_ids,
    scoped_issues_for_account_ids, scoped_issues_jql,
};
pub use mapping::{
    DomainIssuePage, IssueMapper, MappingError, RemoteIssue, RemoteIssuePage, RemoteNamedEntity,
    RemoteProject, RemoteUser, adf_to_plain_text,
};
pub use models::{
    EnhancedSearchPage, EnhancedSearchRequest, JiraAttachment, JiraBulkChangelogRequest,
    JiraBulkChangelogResponse, JiraChangeHistory, JiraChangeItem, JiraComment, JiraCommentPage,
    JiraIssue, JiraIssueChangeLog, JiraIssueFields, JiraNamedEntity, JiraParentIssue, JiraProject,
    JiraUser,
};

/// The fields requested by the initial assigned-issues sync.
///
/// Keeping reporter here ensures list/cached issues retain the identity metadata needed for
/// presentation. More detail fields can be requested by a separate issue-detail operation later.
pub const ASSIGNED_ISSUE_FIELDS: &[&str] = &[
    "summary",
    "issuetype",
    "project",
    "status",
    "priority",
    "assignee",
    "reporter",
    "parent",
    "labels",
    "created",
    "updated",
    "duedate",
    "resolution",
];

/// Fields that are only needed in addition to the baseline fields for an issue detail fetch.
pub const ISSUE_DETAIL_ONLY_FIELDS: &[&str] = &["description", "attachment"];

/// Returns the complete field set required to construct the core domain issue plus detail data.
/// Building this from the baseline prevents the two request shapes from silently drifting.
pub fn issue_detail_fields() -> Vec<&'static str> {
    ASSIGNED_ISSUE_FIELDS
        .iter()
        .copied()
        .chain(ISSUE_DETAIL_ONLY_FIELDS.iter().copied())
        .collect()
}

pub fn issue_detail_fields_query() -> String {
    issue_detail_fields().join(",")
}

#[cfg(test)]
mod detail_field_tests {
    use super::{
        ASSIGNED_ISSUE_FIELDS, ISSUE_DETAIL_ONLY_FIELDS, issue_detail_fields,
        issue_detail_fields_query,
    };

    #[test]
    fn detail_fields_cover_every_core_mapper_field_and_detail_payload() {
        for field in [
            "summary",
            "issuetype",
            "project",
            "status",
            "priority",
            "assignee",
            "reporter",
            "parent",
            "labels",
            "created",
            "updated",
            "duedate",
            "resolution",
            "description",
            "attachment",
        ] {
            assert!(issue_detail_fields().contains(&field), "missing {field}");
        }
        assert!(ASSIGNED_ISSUE_FIELDS.contains(&"reporter"));
        assert_eq!(issue_detail_fields_query(), issue_detail_fields().join(","));
        assert_eq!(ISSUE_DETAIL_ONLY_FIELDS, &["description", "attachment"]);
        assert_eq!(
            issue_detail_fields().len(),
            issue_detail_fields()
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        );
    }
}

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
