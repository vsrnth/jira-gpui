use std::fmt;

use jira_domain::IssueId;

/// Jira's enhanced search endpoint accepts a bounded collection of issue IDs.
///
/// The limit is deliberately conservative: it keeps generated JQL bodies small and gives the
/// caller a clear failure mode instead of relying on an endpoint-specific request-size limit.
pub const MAX_ISSUE_IDS: usize = 1_000;

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
/// Account IDs are emitted as quoted literals. Duplicate IDs are removed so callers can
/// freely combine saved sets without producing unnecessarily long queries.
pub fn assigned_issues_jql(
    account_ids: impl IntoIterator<Item = AccountId>,
) -> Result<String, JqlError> {
    let mut account_ids = account_ids.into_iter().collect::<Vec<_>>();
    account_ids.sort();
    account_ids.dedup();

    if account_ids.is_empty() {
        return Err(JqlError::NoAccountIds);
    }

    let literals = account_ids
        .into_iter()
        .map(|account_id| format!("\"{}\"", account_id.as_str()))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(format!("assignee IN ({literals}) ORDER BY updated DESC"))
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

/// Builds a read-only enhanced-search request for a known set of Jira issue IDs.
///
/// Issue IDs are quoted JQL literals and are validated at this adapter boundary because domain
/// IDs may come from persisted data or a JSON payload that bypassed a constructor. IDs are sorted
/// and deduplicated so equivalent calls produce identical request bodies. No cursor is included:
/// this helper is intended for bounded refreshes of already-known issues.
pub fn enhanced_search_request_for_issue_ids(
    issue_ids: &[IssueId],
) -> Result<crate::EnhancedSearchRequest, JqlError> {
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

    let mut jql = assigned_issues_for_account_ids(request.assignees.clone())?;
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
    #[error("at least one Jira account ID is required")]
    NoAccountIds,
    #[error("a Jira account ID cannot be empty")]
    EmptyAccountId,
    #[error("a Jira account ID is longer than 256 bytes")]
    AccountIdTooLong,
    #[error("a Jira account ID contains unsafe JQL characters")]
    UnsafeAccountId,
    #[error("Jira page size must be between 1 and 1000, received {0}")]
    InvalidPageSize(usize),
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
mod tests {
    use super::*;

    #[test]
    fn builds_a_deterministic_assignee_query() {
        let alpha = AccountId::parse("557058:aaa").unwrap();
        let beta = AccountId::parse("712020:bbb").unwrap();

        assert_eq!(
            assigned_issues_jql([beta, alpha.clone(), alpha]).unwrap(),
            "assignee IN (\"557058:aaa\", \"712020:bbb\") ORDER BY updated DESC"
        );
    }

    #[test]
    fn rejects_values_that_could_change_jql() {
        assert_eq!(
            AccountId::parse("\" OR project = ABC").unwrap_err(),
            JqlError::UnsafeAccountId
        );
        assert_eq!(
            AccountId::parse("abc\\def").unwrap_err(),
            JqlError::UnsafeAccountId
        );
        assert_eq!(
            AccountId::parse("abc\ndef").unwrap_err(),
            JqlError::UnsafeAccountId
        );
    }

    #[test]
    fn accepts_domain_account_ids_at_the_application_boundary() {
        let account_id = jira_domain::AccountId::new("557058:abc-123").unwrap();
        assert_eq!(
            assigned_issues_for_account_ids([account_id]).unwrap(),
            "assignee IN (\"557058:abc-123\") ORDER BY updated DESC"
        );
    }

    #[test]
    fn converts_an_application_fetch_request_to_an_enhanced_search_request() {
        use jira_application::{IssueFetchRequest, PageCursor};
        use time::macros::datetime;

        let request = IssueFetchRequest {
            site_id: jira_domain::JiraSiteId::new("site").unwrap(),
            assignees: vec![jira_domain::AccountId::new("557058:abc-123").unwrap()],
            updated_since: Some(datetime!(2026-08-15 17:20 UTC)),
            page_cursor: Some(PageCursor("opaque-token".into())),
            page_size: 100,
        };

        let enhanced = enhanced_search_request(&request).unwrap();
        assert_eq!(enhanced.next_page_token.as_deref(), Some("opaque-token"));
        assert_eq!(enhanced.max_results, Some(100));
        assert_eq!(
            enhanced.jql,
            "assignee IN (\"557058:abc-123\") AND updated >= \"2026-08-15 17:20\" ORDER BY updated DESC"
        );
    }

    #[test]
    fn builds_a_deterministic_deduplicated_issue_id_request() {
        let first = IssueId::new("1002").unwrap();
        let second = IssueId::new("1001").unwrap();
        let request =
            enhanced_search_request_for_issue_ids(&[first.clone(), second.clone(), first]).unwrap();

        assert_eq!(
            request.jql,
            "id IN (\"1001\", \"1002\") ORDER BY updated DESC"
        );
        assert_eq!(request.max_results, Some(100));
        assert_eq!(
            request.fields,
            crate::ASSIGNED_ISSUE_FIELDS
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rejects_empty_oversized_and_unsafe_issue_id_inputs() {
        assert_eq!(
            enhanced_search_request_for_issue_ids(&[]).unwrap_err(),
            JqlError::NoIssueIds
        );

        let empty: IssueId = serde_json::from_str("\"\"").unwrap();
        assert_eq!(
            enhanced_search_request_for_issue_ids(&[empty]).unwrap_err(),
            JqlError::EmptyIssueId
        );

        let oversized: IssueId = serde_json::from_str(&format!("\"{}\"", "x".repeat(256))).unwrap();
        assert_eq!(
            enhanced_search_request_for_issue_ids(&[oversized]).unwrap_err(),
            JqlError::IssueIdTooLong
        );

        let unsafe_id = IssueId::new("100\" OR project = SECRET").unwrap();
        assert_eq!(
            enhanced_search_request_for_issue_ids(&[unsafe_id]).unwrap_err(),
            JqlError::UnsafeIssueId
        );

        let ids = (0..=MAX_ISSUE_IDS)
            .map(|index| IssueId::new(index.to_string()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            enhanced_search_request_for_issue_ids(&ids).unwrap_err(),
            JqlError::TooManyIssueIds {
                maximum: MAX_ISSUE_IDS,
                received: MAX_ISSUE_IDS + 1,
            }
        );
    }

    #[test]
    fn serializes_the_issue_id_request_using_jira_field_names() {
        let request =
            enhanced_search_request_for_issue_ids(&[IssueId::new("1001").unwrap()]).unwrap();
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(value["jql"], "id IN (\"1001\") ORDER BY updated DESC");
        assert_eq!(value["maxResults"], 100);
        assert_eq!(value["fields"][0], "summary");
        assert!(value.get("nextPageToken").is_none());
    }
}
