use std::collections::HashSet;

use jira_domain::{AccountId, JiraSiteId, Timestamp};

use crate::{ApplicationError, IssueFetchRequest, PageCursor, validate_jql_scope};

/// Validated remote restrictions shared by manual pulls and synchronization.
///
/// Notification restrictions deliberately do not belong here: they are local
/// delivery policy owned by `SyncService`, not part of the Jira fetch scope.
#[derive(Clone, Debug)]
pub(crate) struct IssueFetchScope {
    site_id: JiraSiteId,
    assignees: Option<Vec<AccountId>>,
    watchers: Option<Vec<AccountId>>,
    jql_scope: Option<String>,
}

impl IssueFetchScope {
    /// Builds a validated remote scope while allowing each caller to retain its
    /// established duplicate-validation wording.
    pub(crate) fn new(
        site_id: JiraSiteId,
        assignees: Option<Vec<AccountId>>,
        watchers: Option<Vec<AccountId>>,
        jql_scope: Option<String>,
        duplicate_assignees_message: &'static str,
        duplicate_watchers_message: &'static str,
    ) -> Result<Self, ApplicationError> {
        validate_jql_scope(jql_scope.as_deref()).map_err(ApplicationError::invalid_input)?;
        validate_unique_accounts(assignees.as_deref(), duplicate_assignees_message)?;
        validate_unique_accounts(watchers.as_deref(), duplicate_watchers_message)?;

        Ok(Self {
            site_id,
            assignees,
            watchers,
            jql_scope,
        })
    }

    /// Creates the transport-neutral port request for one page without
    /// taking ownership of pagination policy or cursor state.
    pub(crate) fn issue_fetch_request(
        &self,
        updated_since: Option<Timestamp>,
        page_cursor: Option<PageCursor>,
        page_size: usize,
    ) -> IssueFetchRequest {
        IssueFetchRequest {
            site_id: self.site_id.clone(),
            assignees: self.assignees.clone(),
            watchers: self.watchers.clone(),
            jql_scope: self.jql_scope.clone(),
            updated_since,
            page_cursor,
            page_size,
        }
    }
}

fn validate_unique_accounts(
    accounts: Option<&[AccountId]>,
    error_message: &'static str,
) -> Result<(), ApplicationError> {
    if let Some(accounts) = accounts
        && accounts.iter().collect::<HashSet<_>>().len() != accounts.len()
    {
        return Err(ApplicationError::invalid_input(error_message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use jira_domain::{AccountId, JiraSiteId};
    use time::macros::datetime;

    use super::*;
    use crate::{ErrorKind, PageCursor};

    fn site() -> JiraSiteId {
        JiraSiteId::new("cloud-1").expect("site")
    }

    fn account(value: &str) -> AccountId {
        AccountId::new(value).expect("account")
    }

    fn scope(
        assignees: Option<Vec<AccountId>>,
        watchers: Option<Vec<AccountId>>,
        jql_scope: Option<String>,
    ) -> Result<IssueFetchScope, ApplicationError> {
        IssueFetchScope::new(
            site(),
            assignees,
            watchers,
            jql_scope,
            "pull assignees must be unique",
            "pull watchers must be unique",
        )
    }

    #[test]
    fn preserves_optional_scope_values_and_order_when_building_page_request() {
        let fetch_scope = scope(
            Some(vec![account("assignee-2"), account("assignee-1")]),
            Some(vec![account("watcher-2"), account("watcher-1")]),
            Some("project = APP".into()),
        )
        .expect("valid scope");

        let request = fetch_scope.issue_fetch_request(
            Some(datetime!(2026-08-16 10:00 UTC)),
            Some(PageCursor("opaque-token".into())),
            37,
        );

        assert_eq!(request.site_id, site());
        assert_eq!(
            request.assignees,
            Some(vec![account("assignee-2"), account("assignee-1")])
        );
        assert_eq!(
            request.watchers,
            Some(vec![account("watcher-2"), account("watcher-1")])
        );
        assert_eq!(request.jql_scope, Some("project = APP".into()));
        assert_eq!(request.updated_since, Some(datetime!(2026-08-16 10:00 UTC)));
        assert_eq!(request.page_cursor, Some(PageCursor("opaque-token".into())));
        assert_eq!(request.page_size, 37);
    }

    #[test]
    fn preserves_none_distinct_from_explicitly_empty_restrictions() {
        let unrestricted = scope(None, None, None)
            .expect("valid unrestricted scope")
            .issue_fetch_request(None, None, 100);
        let explicitly_empty = scope(Some(Vec::new()), Some(Vec::new()), None)
            .expect("valid empty scope")
            .issue_fetch_request(None, None, 100);

        assert_eq!(unrestricted.assignees, None);
        assert_eq!(unrestricted.watchers, None);
        assert_eq!(explicitly_empty.assignees, Some(Vec::new()));
        assert_eq!(explicitly_empty.watchers, Some(Vec::new()));
    }

    #[test]
    fn preserves_jql_validation_messages_and_invalid_input_kind() {
        for (jql_scope, expected) in [
            (Some("   ".into()), "Jira scope cannot be empty"),
            (
                Some("x".repeat(crate::MAX_JQL_SCOPE_LENGTH + 1)),
                "Jira scope is too long",
            ),
            (
                Some("project = APP ORDER BY updated".into()),
                "Jira scope must not contain ORDER BY",
            ),
        ] {
            let error = scope(None, None, jql_scope).expect_err("invalid JQL scope");
            assert_eq!(error.kind(), ErrorKind::InvalidInput);
            assert_eq!(error.message(), expected);
        }
    }

    #[test]
    fn preserves_caller_specific_duplicate_messages() {
        let assignee_error = IssueFetchScope::new(
            site(),
            Some(vec![account("same"), account("same")]),
            None,
            None,
            "issue pull assignees must be unique",
            "issue pull watchers must be unique",
        )
        .expect_err("duplicate assignees");
        assert_eq!(
            assignee_error.message(),
            "issue pull assignees must be unique"
        );

        let watcher_error = IssueFetchScope::new(
            site(),
            None,
            Some(vec![account("same"), account("same")]),
            None,
            "sync assignees must be unique",
            "sync watchers must be unique",
        )
        .expect_err("duplicate watchers");
        assert_eq!(watcher_error.message(), "sync watchers must be unique");
    }

    #[test]
    fn validates_duplicate_assignees_before_duplicate_watchers() {
        let error = scope(
            Some(vec![account("same"), account("same")]),
            Some(vec![account("same"), account("same")]),
            None,
        )
        .expect_err("both restrictions contain duplicates");

        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert_eq!(error.message(), "pull assignees must be unique");
    }
}
