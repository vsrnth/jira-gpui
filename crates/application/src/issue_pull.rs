use std::{collections::HashSet, sync::Arc};

use jira_domain::{AccountId, Issue, JiraSiteId, Timestamp};

use crate::{
    ApplicationError, CancellationToken, IssueFetchRequest, JiraIssueSearchPort,
    issue_pagination::IssuePagination, validate_jql_scope,
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
        self.validate(&request)?;
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
                    &IssueFetchRequest {
                        site_id: request.site_id.clone(),
                        assignees: request.assignees.clone(),
                        watchers: request.watchers.clone(),
                        jql_scope: request.jql_scope.clone(),
                        updated_since: request.updated_since,
                        page_cursor,
                        page_size: self.config.page_size,
                    },
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

    fn validate(&self, request: &IssuePullRequest) -> Result<(), ApplicationError> {
        validate_jql_scope(request.jql_scope.as_deref())
            .map_err(ApplicationError::invalid_input)?;
        if let Some(assignees) = &request.assignees
            && assignees.iter().collect::<HashSet<_>>().len() != assignees.len()
        {
            return Err(ApplicationError::invalid_input(
                "issue pull assignees must be unique",
            ));
        }
        if let Some(watchers) = &request.watchers
            && watchers.iter().collect::<HashSet<_>>().len() != watchers.len()
        {
            return Err(ApplicationError::invalid_input(
                "issue pull watchers must be unique",
            ));
        }
        crate::issue_pagination::validate_pagination_config(
            self.config.page_size,
            self.config.max_pages,
            "issue pull pagination configuration is invalid",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use jira_domain::{IssueId, IssueKey, IssueType, Priority, Project, Status};
    use time::macros::datetime;

    use super::*;
    use crate::{ErrorKind, IssuePage, PageCursor, PortFuture, test_support::block_on};

    struct FakeJira {
        pages: Mutex<VecDeque<Result<IssuePage, ApplicationError>>>,
        requests: Mutex<Vec<IssueFetchRequest>>,
        cancel_after_first_page: bool,
    }

    impl FakeJira {
        fn new(pages: Vec<Result<IssuePage, ApplicationError>>) -> Self {
            Self {
                pages: Mutex::new(pages.into()),
                requests: Mutex::new(Vec::new()),
                cancel_after_first_page: false,
            }
        }
    }

    impl JiraIssueSearchPort for FakeJira {
        fn fetch_issue_page<'a>(
            &'a self,
            request: &'a IssueFetchRequest,
            cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            let result = self
                .pages
                .lock()
                .expect("pages lock")
                .pop_front()
                .expect("fake page available");
            let cancel = self.cancel_after_first_page && request.page_cursor.is_none();
            self.requests
                .lock()
                .expect("requests lock")
                .push(request.clone());
            let cancellation = cancellation.clone();
            Box::pin(async move {
                if cancel {
                    cancellation.cancel();
                }
                result
            })
        }

        fn fetch_issues_by_id<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _issue_ids: &'a [IssueId],
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<Issue>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn site() -> JiraSiteId {
        JiraSiteId::new("cloud-1").expect("site")
    }

    fn assignee() -> AccountId {
        AccountId::new("account-1").expect("account")
    }

    fn request() -> IssuePullRequest {
        IssuePullRequest {
            site_id: site(),
            assignees: Some(vec![assignee()]),
            watchers: None,
            jql_scope: None,
            updated_since: Some(datetime!(2026-08-16 10:00 UTC)),
        }
    }

    fn issue(id: &str, summary: &str, updated_at: Timestamp) -> Issue {
        Issue::new(
            site(),
            IssueId::new(id).expect("issue id"),
            IssueKey::new(format!("APP-{id}")).expect("issue key"),
            Project {
                id: "10".into(),
                key: "APP".into(),
                name: "Application".into(),
            },
            IssueType {
                id: "1".into(),
                name: "Task".into(),
                icon_url: None,
            },
            summary,
            Status {
                id: "open".into(),
                name: "Open".into(),
                category: None,
            },
            Priority {
                id: None,
                name: Some("Medium".into()),
                icon_url: None,
            },
            Some(assignee()),
            None,
            None,
            Vec::new(),
            datetime!(2026-08-16 09:00 UTC),
            updated_at,
            None,
        )
    }

    fn page(
        issues: Vec<Issue>,
        next_cursor: Option<&str>,
        server_time: Option<Timestamp>,
    ) -> Result<IssuePage, ApplicationError> {
        Ok(IssuePage {
            issues,
            next_cursor: next_cursor.map(|cursor| PageCursor(cursor.into())),
            server_time,
        })
    }

    #[test]
    fn pulls_all_pages_deduplicates_and_keeps_first_seen_order() {
        let old = issue("1", "old", datetime!(2026-08-16 10:01 UTC));
        let replacement = issue("1", "fresh", datetime!(2026-08-16 10:03 UTC));
        let jira = Arc::new(FakeJira::new(vec![
            page(
                vec![old, issue("2", "second", datetime!(2026-08-16 10:02 UTC))],
                Some("next"),
                Some(datetime!(2026-08-16 10:02 UTC)),
            ),
            page(
                vec![
                    replacement,
                    issue("3", "third", datetime!(2026-08-16 10:04 UTC)),
                ],
                None,
                Some(datetime!(2026-08-16 10:05 UTC)),
            ),
        ]));
        let service = IssuePullService::new(jira.clone(), IssuePullConfig::default());

        let outcome =
            block_on(service.pull(request(), &CancellationToken::new())).expect("pull succeeds");

        assert_eq!(outcome.pages_fetched, 2);
        assert_eq!(
            outcome
                .issues
                .iter()
                .map(|issue| issue.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2", "3"]
        );
        assert_eq!(outcome.issues[0].summary, "fresh");
        assert_eq!(outcome.server_time, Some(datetime!(2026-08-16 10:05 UTC)));
        let requests = jira.requests.lock().expect("requests lock");
        assert_eq!(requests[0].page_cursor, None);
        assert_eq!(requests[1].page_cursor, Some(PageCursor("next".into())));
        assert_eq!(requests[0].updated_since, request().updated_since);
        assert_eq!(requests[0].assignees, Some(vec![assignee()]));
        assert_eq!(requests[0].watchers, None);
    }

    #[test]
    fn allows_empty_remote_assignee_restrictions_and_rejects_duplicates() {
        let jira = Arc::new(FakeJira::new(vec![
            page(Vec::new(), None, None),
            page(Vec::new(), None, None),
        ]));
        let service = IssuePullService::new(jira.clone(), IssuePullConfig::default());

        let mut empty = request();
        empty.assignees = None;
        assert!(block_on(service.pull(empty, &CancellationToken::new())).is_ok());

        let mut explicitly_empty = request();
        explicitly_empty.assignees = Some(Vec::new());
        explicitly_empty.watchers = Some(Vec::new());
        assert!(block_on(service.pull(explicitly_empty, &CancellationToken::new())).is_ok());

        let mut duplicate = request();
        duplicate
            .assignees
            .as_mut()
            .expect("assignee restriction")
            .push(assignee());
        assert_eq!(
            block_on(service.pull(duplicate, &CancellationToken::new()))
                .expect_err("duplicate assignees")
                .kind(),
            ErrorKind::InvalidInput
        );

        let mut duplicate_watchers = request();
        duplicate_watchers.watchers = Some(vec![assignee(), assignee()]);
        assert_eq!(
            block_on(service.pull(duplicate_watchers, &CancellationToken::new()))
                .expect_err("duplicate watchers")
                .kind(),
            ErrorKind::InvalidInput
        );

        for config in [
            IssuePullConfig {
                page_size: 0,
                max_pages: 1,
            },
            IssuePullConfig {
                page_size: 1,
                max_pages: 0,
            },
            IssuePullConfig {
                page_size: 1_001,
                max_pages: 1,
            },
        ] {
            let invalid = IssuePullService::new(jira.clone(), config);
            assert_eq!(
                block_on(invalid.pull(request(), &CancellationToken::new()))
                    .expect_err("invalid configuration")
                    .kind(),
                ErrorKind::Internal
            );
        }
    }

    #[test]
    fn stops_on_cursor_cycle_and_page_limit_as_upstream_safety_errors() {
        let cycle_jira = Arc::new(FakeJira::new(vec![
            page(vec![], Some("same"), None),
            page(vec![], Some("same"), None),
        ]));
        let cycle_service = IssuePullService::new(cycle_jira, IssuePullConfig::default());
        assert_eq!(
            block_on(cycle_service.pull(request(), &CancellationToken::new()))
                .expect_err("cycle")
                .kind(),
            ErrorKind::Upstream
        );

        let limit_jira = Arc::new(FakeJira::new(vec![page(vec![], Some("next"), None)]));
        let limit_service = IssuePullService::new(
            limit_jira,
            IssuePullConfig {
                page_size: 1,
                max_pages: 1,
            },
        );
        assert_eq!(
            block_on(limit_service.pull(request(), &CancellationToken::new()))
                .expect_err("page limit")
                .kind(),
            ErrorKind::Upstream
        );
    }

    #[test]
    fn honors_cancellation_before_and_during_pagination() {
        let jira = Arc::new(FakeJira::new(vec![page(vec![], None, None)]));
        let service = IssuePullService::new(jira.clone(), IssuePullConfig::default());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            block_on(service.pull(request(), &cancellation))
                .expect_err("cancelled before pull")
                .kind(),
            ErrorKind::Cancelled
        );
        assert!(jira.requests.lock().expect("requests lock").is_empty());

        let mut fake = FakeJira::new(vec![page(vec![], Some("next"), None)]);
        fake.cancel_after_first_page = true;
        let service = IssuePullService::new(Arc::new(fake), IssuePullConfig::default());
        assert_eq!(
            block_on(service.pull(request(), &CancellationToken::new()))
                .expect_err("cancelled during pull")
                .kind(),
            ErrorKind::Cancelled
        );
    }

    #[test]
    fn propagates_port_errors_unchanged() {
        let expected = ApplicationError::new(ErrorKind::Offline, "Jira is unavailable");
        let jira = Arc::new(FakeJira::new(vec![Err(expected.clone())]));
        let service = IssuePullService::new(jira, IssuePullConfig::default());

        let error =
            block_on(service.pull(request(), &CancellationToken::new())).expect_err("port error");
        assert_eq!(error, expected);
    }
}
