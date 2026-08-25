use std::fmt;

use jira_domain::IssueId;

/// Jira's enhanced search endpoint accepts a bounded collection of issue IDs.
///
/// The limit is deliberately conservative: it keeps generated JQL bodies small and gives the
/// caller a clear failure mode instead of relying on an endpoint-specific request-size limit.
pub const MAX_ISSUE_IDS: usize = 1_000;

pub use jira_application::{DEFAULT_JQL_SCOPE, MAX_JQL_SCOPE_LENGTH};

/// A Jira Cloud account identifier safe to interpolate in a quoted JQL literal.
///
/// Atlassian currently uses opaque account IDs. We intentionally accept their common
/// punctuation but reject control characters, quotes and backslashes instead of attempting
/// to "fix" untrusted input. That keeps a list of account IDs from changing the query's
/// meaning.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AccountId(String);

impl AccountId {
    pub fn parse(value: impl Into<String>) -> Result<Self, JqlError> {
        let value = value.into();
        if value.is_empty() {
            return Err(JqlError::EmptyAccountId);
        }
        if value.len() > 256 {
            return Err(JqlError::AccountIdTooLong);
        }
        if value
            .chars()
            .any(|character| character.is_control() || matches!(character, '"' | '\\'))
        {
            return Err(JqlError::UnsafeAccountId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Builds the JQL used for the primary read-only view.
///
/// Builds a Jira scope query with optional assignee and watcher restrictions. An empty pair means
/// unrestricted results within the supplied scope. IDs are emitted as quoted literals and each
/// collection is sorted/deduplicated independently for deterministic request bodies.
pub fn scoped_issues_jql(
    scope: Option<&str>,
    assignee_ids: impl IntoIterator<Item = AccountId>,
    watcher_ids: impl IntoIterator<Item = AccountId>,
) -> Result<String, JqlError> {
    let scope = validated_scope(scope)?;
    let mut assignee_ids = assignee_ids.into_iter().collect::<Vec<_>>();
    let mut watcher_ids = watcher_ids.into_iter().collect::<Vec<_>>();
    assignee_ids.sort();
    assignee_ids.dedup();
    watcher_ids.sort();
    watcher_ids.dedup();

    let assignee_clause = account_clause("assignee", &assignee_ids);
    let watcher_clause = account_clause("watcher", &watcher_ids);
    let account_clause = match (assignee_clause, watcher_clause) {
        (Some(assignee), Some(watcher)) => Some(format!("({assignee} OR {watcher})")),
        (Some(assignee), None) => Some(assignee),
        (None, Some(watcher)) => Some(watcher),
        (None, None) => None,
    };

    let mut clauses = vec![format!("({scope})")];
    if let Some(account_clause) = account_clause {
        clauses.push(account_clause);
    }
    Ok(format!("{} ORDER BY updated DESC", clauses.join(" AND ")))
}

/// Alias with product-facing terminology for callers constructing the authenticated view.
pub fn assigned_or_watched_issues_jql(
    assignee_ids: impl IntoIterator<Item = AccountId>,
    watcher_ids: impl IntoIterator<Item = AccountId>,
) -> Result<String, JqlError> {
    scoped_issues_jql(None, assignee_ids, watcher_ids)
}

fn account_clause(field: &str, account_ids: &[AccountId]) -> Option<String> {
    (!account_ids.is_empty()).then(|| {
        let literals = account_ids
            .iter()
            .map(|account_id| format!("\"{}\"", account_id.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{field} IN ({literals})")
    })
}

fn validated_scope(scope: Option<&str>) -> Result<String, JqlError> {
    let scope = scope.unwrap_or(DEFAULT_JQL_SCOPE).trim();
    jira_application::validate_jql_scope(Some(scope)).map_err(|error| match error {
        "Jira scope cannot be empty" => JqlError::EmptyScope,
        "Jira scope is too long" => JqlError::ScopeTooLong,
        "Jira scope must not contain ORDER BY" => JqlError::ScopeContainsOrderBy,
        _ => JqlError::EmptyScope,
    })?;
    Ok(scope.to_owned())
}

fn validated_issue_ids(issue_ids: &[IssueId]) -> Result<Vec<String>, JqlError> {
    if issue_ids.is_empty() {
        return Err(JqlError::NoIssueIds);
    }
    if issue_ids.len() > MAX_ISSUE_IDS {
        return Err(JqlError::TooManyIssueIds {
            maximum: MAX_ISSUE_IDS,
            received: issue_ids.len(),
        });
    }

    let mut issue_ids = issue_ids
        .iter()
        .map(|issue_id| {
            let value = issue_id.as_str();
            if value.trim().is_empty() {
                return Err(JqlError::EmptyIssueId);
            }
            if value.len() > 255 {
                return Err(JqlError::IssueIdTooLong);
            }
            if value
                .chars()
                .any(|character| character.is_control() || matches!(character, '"' | '\\'))
            {
                return Err(JqlError::UnsafeIssueId);
            }
            Ok(value.to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;

    issue_ids.sort();
    issue_ids.dedup();
    Ok(issue_ids)
}

/// Builds the default-scope assignee-only JQL retained for callers that do not need watchers.
pub fn assigned_issues_jql(
    account_ids: impl IntoIterator<Item = AccountId>,
) -> Result<String, JqlError> {
    scoped_issues_jql(None, account_ids, Vec::<AccountId>::new())
}

/// Builds the assigned-issues JQL directly from stable domain account IDs.
///
/// Domain IDs are intentionally broad because they are also used for storage. Re-validating at
/// this query boundary avoids giving a persisted or externally supplied value JQL syntax.
pub fn assigned_issues_for_account_ids(
    account_ids: impl IntoIterator<Item = jira_domain::AccountId>,
) -> Result<String, JqlError> {
    account_ids
        .into_iter()
        .map(|account_id| AccountId::parse(account_id.into_inner()))
        .collect::<Result<Vec<_>, _>>()
        .and_then(assigned_issues_jql)
}

pub fn scoped_issues_for_account_ids(
    scope: Option<&str>,
    assignee_ids: impl IntoIterator<Item = jira_domain::AccountId>,
    watcher_ids: impl IntoIterator<Item = jira_domain::AccountId>,
) -> Result<String, JqlError> {
    let assignee_ids = assignee_ids
        .into_iter()
        .map(|account_id| AccountId::parse(account_id.into_inner()))
        .collect::<Result<Vec<_>, _>>()?;
    let watcher_ids = watcher_ids
        .into_iter()
        .map(|account_id| AccountId::parse(account_id.into_inner()))
        .collect::<Result<Vec<_>, _>>()?;
    scoped_issues_jql(scope, assignee_ids, watcher_ids)
}

pub fn assigned_or_watched_issues_for_account_ids(
    assignee_ids: impl IntoIterator<Item = jira_domain::AccountId>,
    watcher_ids: impl IntoIterator<Item = jira_domain::AccountId>,
) -> Result<String, JqlError> {
    scoped_issues_for_account_ids(None, assignee_ids, watcher_ids)
}

/// Builds a read-only enhanced-search request for a known set of Jira issue IDs.
///
/// Issue IDs are quoted JQL literals and are validated at this adapter boundary because domain
/// IDs may come from persisted data or a JSON payload that bypassed a constructor. IDs are sorted
/// and deduplicated so equivalent calls produce identical request bodies. No cursor is included:
/// this helper is intended for bounded refreshes of already-known issues.
pub fn enhanced_search_request_for_issue_ids(
    issue_ids: &[IssueId],
) -> Result<crate::EnhancedSearchRequest, JqlError> {
    let issue_ids = validated_issue_ids(issue_ids)?;

    let literals = issue_ids
        .into_iter()
        .map(|issue_id| format!("\"{issue_id}\""))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(crate::EnhancedSearchRequest {
        jql: format!("id IN ({literals}) ORDER BY updated DESC"),
        next_page_token: None,
        max_results: Some(100),
        fields: crate::ASSIGNED_ISSUE_FIELDS
            .iter()
            .map(ToString::to_string)
            .collect(),
        expand: Vec::new(),
    })
}

/// Builds one bounded bulk changelog request. The HTTP adapter owns pagination
/// by replacing the opaque token on each subsequent request.
pub fn bulk_changelog_request(
    request: &jira_application::IssueChangelogRequest,
    next_page_token: Option<String>,
) -> Result<crate::JiraBulkChangelogRequest, JqlError> {
    let issue_ids = validated_issue_ids(&request.issue_ids)?;
    Ok(crate::JiraBulkChangelogRequest {
        issue_ids_or_keys: issue_ids,
        max_results: 1_000,
        next_page_token,
    })
}

/// Converts the application-layer fetch command into Jira's enhanced-search request body.
///
/// This is intentionally pure: it adds no HTTP client or credentials dependency. The cursor is
/// Jira's opaque `nextPageToken`, so it is forwarded unchanged and never interpolated into JQL.
pub fn enhanced_search_request(
    request: &jira_application::IssueFetchRequest,
) -> Result<crate::EnhancedSearchRequest, JqlError> {
    if !(1..=1_000).contains(&request.page_size) {
        return Err(JqlError::InvalidPageSize(request.page_size));
    }

    let mut jql = scoped_issues_for_account_ids(
        request.jql_scope.as_deref(),
        request.assignees.clone().unwrap_or_default(),
        request.watchers.clone().unwrap_or_default(),
    )?;
    if let Some(updated_since) = request.updated_since {
        // Jira Cloud accepts this explicit UTC literal for JQL date comparisons. It is generated
        // from a typed timestamp, rather than copied from any user-entered query text.
        let timestamp = updated_since
            .to_offset(time::UtcOffset::UTC)
            .format(time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]"
            ))
            .expect("the static JQL timestamp format is valid");
        jql = jql.replacen(
            " ORDER BY updated DESC",
            &format!(" AND updated >= \"{timestamp}\" ORDER BY updated DESC"),
            1,
        );
    }

    Ok(crate::EnhancedSearchRequest {
        jql,
        next_page_token: request.page_cursor.as_ref().map(|cursor| cursor.0.clone()),
        max_results: Some(request.page_size as u16),
        fields: crate::ASSIGNED_ISSUE_FIELDS
            .iter()
            .map(ToString::to_string)
            .collect(),
        expand: Vec::new(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum JqlError {
    #[error("a Jira account ID cannot be empty")]
    EmptyAccountId,
    #[error("a Jira account ID is longer than 256 bytes")]
    AccountIdTooLong,
    #[error("a Jira account ID contains unsafe JQL characters")]
    UnsafeAccountId,
    #[error("Jira page size must be between 1 and 1000, received {0}")]
    InvalidPageSize(usize),
    #[error("a Jira JQL scope cannot be empty")]
    EmptyScope,
    #[error("a Jira JQL scope is longer than {MAX_JQL_SCOPE_LENGTH} bytes")]
    ScopeTooLong,
    #[error("a Jira JQL scope must not contain ORDER BY")]
    ScopeContainsOrderBy,
    #[error("at least one Jira issue ID is required")]
    NoIssueIds,
    #[error("a Jira issue ID cannot be empty")]
    EmptyIssueId,
    #[error("a Jira issue ID is longer than 255 bytes")]
    IssueIdTooLong,
    #[error("a Jira issue ID contains unsafe JQL characters")]
    UnsafeIssueId,
    #[error("at most {maximum} Jira issue IDs may be requested, received {received}")]
    TooManyIssueIds { maximum: usize, received: usize },
}

#[cfg(test)]
#[path = "jql_tests.rs"]
mod tests;
