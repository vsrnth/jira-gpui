use std::sync::Arc;

use jira_domain::{AccountId, Issue, JiraSiteId, Timestamp};

use crate::{
    ApplicationError, CancellationToken, JiraIssueSearchPort, issue_fetch_scope::IssueFetchScope,
    issue_pagination::IssuePagination,
};

/// Safety limits for a manual issue pull.
///
/// The service validates these values before making a request. A pull that reaches
/// `max_pages` without receiving a terminal cursor fails as an upstream safety
/// error rather than silently returning a partial result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IssuePullConfig {
    pub page_size: usize,
    pub max_pages: usize,
}

impl Default for IssuePullConfig {
    fn default() -> Self {
        Self {
            page_size: 100,
            max_pages: 1_000,
        }
    }
}

/// A read-only issue pull for one Jira site with optional remote assignee and watcher
/// restrictions. When both are present, Jira returns their union.
#[derive(Clone, Debug)]
pub struct IssuePullRequest {
    pub site_id: JiraSiteId,
    /// Optional remote restriction. `None` fetches all issues in the configured Jira scope.
    pub assignees: Option<Vec<AccountId>>,
    pub watchers: Option<Vec<AccountId>>,
    pub jql_scope: Option<String>,
    pub updated_since: Option<Timestamp>,
}

/// The complete result of a successful bounded issue pull.
#[derive(Clone, Debug)]
pub struct IssuePullOutcome {
    pub issues: Vec<Issue>,
    pub pages_fetched: usize,
    /// The greatest server boundary returned by any page, if a page supplied one.
    pub server_time: Option<Timestamp>,
}

/// Application orchestration for manually pulling Jira issues.
///
/// This type owns pagination policy, input validation, cancellation checks, and
/// response de-duplication. It deliberately knows nothing about HTTP, executors,
/// persistence, or presentation frameworks.
#[derive(Clone)]
pub struct IssuePullService {
    jira: Arc<dyn JiraIssueSearchPort>,
    config: IssuePullConfig,
}

impl IssuePullService {
    pub fn new(jira: Arc<dyn JiraIssueSearchPort>, config: IssuePullConfig) -> Self {
        Self { jira, config }
    }

    pub async fn pull(
        &self,
        request: IssuePullRequest,
        cancellation: &CancellationToken,
    ) -> Result<IssuePullOutcome, ApplicationError> {
        let fetch_scope = self.validate(&request)?;
        cancellation.check()?;

        let mut pagination = IssuePagination::new(
            self.config.page_size,
            self.config.max_pages,
            "issue pull pagination configuration is invalid",
        )?;

        loop {
            let page_cursor = pagination.prepare_request(cancellation)?;
            let page = self
                .jira
                .fetch_issue_page(
                    &fetch_scope.issue_fetch_request(
                        request.updated_since,
                        page_cursor,
                        self.config.page_size,
                    ),
                    cancellation,
                )
                .await?;
            pagination.accept_page(page, cancellation)?;

            if !pagination.has_next_page() {
                break;
            }
        }

        let outcome = pagination.finish();
        Ok(IssuePullOutcome {
            issues: outcome.issues,
            pages_fetched: outcome.pages_fetched,
            server_time: outcome.server_time,
        })
    }

    fn validate(&self, request: &IssuePullRequest) -> Result<IssueFetchScope, ApplicationError> {
        let fetch_scope = IssueFetchScope::new(
            request.site_id.clone(),
            request.assignees.clone(),
            request.watchers.clone(),
            request.jql_scope.clone(),
            "issue pull assignees must be unique",
            "issue pull watchers must be unique",
        )?;
        crate::issue_pagination::validate_pagination_config(
            self.config.page_size,
            self.config.max_pages,
            "issue pull pagination configuration is invalid",
        )?;
        Ok(fetch_scope)
    }
}

#[cfg(test)]
#[path = "issue_pull_tests.rs"]
mod tests;
