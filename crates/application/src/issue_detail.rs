use std::sync::Arc;

use jira_domain::IssueDetail;

use crate::{
    ApplicationError, CancellationToken, IssueCommentsPageRequest, IssueDetailRequest,
    JiraIssueDetailReadPort,
    comment_pagination::{CommentPageDecision, CommentPagination},
};

const MAX_PAGE_SIZE: usize = 1_000;

/// Bounds for the read-only comment aggregation performed by `IssueDetailService`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IssueDetailConfig {
    pub comment_page_size: usize,
    pub max_comment_pages: usize,
    pub max_comments: usize,
}

impl Default for IssueDetailConfig {
    fn default() -> Self {
        Self {
            comment_page_size: 100,
            max_comment_pages: 1_000,
            max_comments: 10_000,
        }
    }
}

/// Application orchestration for one issue's core data and all paginated comments.
#[derive(Clone)]
pub struct IssueDetailService {
    jira: Arc<dyn JiraIssueDetailReadPort>,
    config: IssueDetailConfig,
}

impl IssueDetailService {
    pub fn new(jira: Arc<dyn JiraIssueDetailReadPort>, config: IssueDetailConfig) -> Self {
        Self { jira, config }
    }

    pub async fn fetch(
        &self,
        request: IssueDetailRequest,
        cancellation: &CancellationToken,
    ) -> Result<IssueDetail, ApplicationError> {
        self.validate_config()?;
        cancellation.check()?;

        let core = self.jira.fetch_issue_detail(&request, cancellation).await?;
        cancellation.check()?;
        if core.issue.site_id != request.site_id || !locator_matches(&request.locator, &core) {
            return Err(upstream("Jira returned detail for a different issue"));
        }
        let canonical_issue_id = core.issue.id.clone();

        let mut pagination = CommentPagination::new(
            self.config.comment_page_size,
            self.config.max_comment_pages,
            self.config.max_comments,
        );

        let comments = loop {
            cancellation.check()?;
            let page = self
                .jira
                .fetch_issue_comments_page(
                    &IssueCommentsPageRequest {
                        site_id: request.site_id.clone(),
                        issue_id: canonical_issue_id.clone(),
                        start_at: pagination.start_at(),
                        page_cursor: pagination.page_cursor(),
                        page_size: self.config.comment_page_size,
                    },
                    cancellation,
                )
                .await?;
            cancellation.check()?;

            if pagination.accept_page(page)? == CommentPageDecision::Complete {
                break pagination.finish();
            }
        };

        Ok(IssueDetail::new(core, comments))
    }

    pub async fn load(
        &self,
        request: IssueDetailRequest,
        cancellation: &CancellationToken,
    ) -> Result<IssueDetail, ApplicationError> {
        self.fetch(request, cancellation).await
    }

    fn validate_config(&self) -> Result<(), ApplicationError> {
        if !(1..=MAX_PAGE_SIZE).contains(&self.config.comment_page_size)
            || self.config.max_comment_pages == 0
            || self.config.max_comments == 0
        {
            return Err(ApplicationError::invalid_input(
                "issue detail pagination configuration is invalid",
            ));
        }
        Ok(())
    }
}

fn upstream(message: &'static str) -> ApplicationError {
    ApplicationError::new(crate::ErrorKind::Upstream, message)
}

fn locator_matches(locator: &crate::IssueLocator, core: &jira_domain::IssueDetailCore) -> bool {
    match locator {
        crate::IssueLocator::Id(issue_id) => &core.issue.id == issue_id,
        crate::IssueLocator::Key(issue_key) => core
            .issue
            .key
            .as_str()
            .eq_ignore_ascii_case(issue_key.as_str()),
    }
}

#[cfg(test)]
#[path = "issue_detail_tests.rs"]
mod tests;
