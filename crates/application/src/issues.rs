use std::sync::Arc;

use jira_domain::{Issue, IssueId, JiraSiteId, User};

use crate::{
    ApplicationError, CancellationToken, IssueCachePort, IssueListQuery, JiraReadPort,
    UserSearchRequest,
};

#[derive(Clone)]
pub struct IssueCatalogService {
    jira: Arc<dyn JiraReadPort>,
    cache: Arc<dyn IssueCachePort>,
}

impl IssueCatalogService {
    pub fn new(jira: Arc<dyn JiraReadPort>, cache: Arc<dyn IssueCachePort>) -> Self {
        Self { jira, cache }
    }

    pub async fn list_cached(
        &self,
        query: &IssueListQuery,
    ) -> Result<Vec<Issue>, ApplicationError> {
        validate_page_size(query.limit)?;
        self.cache.list_issues(query).await
    }

    pub async fn issue(
        &self,
        site_id: &JiraSiteId,
        issue_id: &IssueId,
    ) -> Result<Option<Issue>, ApplicationError> {
        self.cache.get_issue(site_id, issue_id).await
    }

    pub async fn search_users(
        &self,
        request: &UserSearchRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<User>, ApplicationError> {
        if request.query.trim().is_empty() {
            return Ok(Vec::new());
        }
        validate_page_size(request.limit)?;
        cancellation.check()?;
        self.jira.search_users(request, cancellation).await
    }
}

fn validate_page_size(limit: usize) -> Result<(), ApplicationError> {
    if (1..=1_000).contains(&limit) {
        Ok(())
    } else {
        Err(ApplicationError::invalid_input(
            "page size must be between 1 and 1000",
        ))
    }
}
