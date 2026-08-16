use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use jira_domain::{AccountId, Issue, JiraSiteId, Timestamp};

use crate::{ApplicationError, CancellationToken, IssueFetchRequest, JiraReadPort, PageCursor};

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

/// A read-only issue pull for one Jira site and a set of assignees.
#[derive(Clone, Debug)]
pub struct IssuePullRequest {
    pub site_id: JiraSiteId,
    pub assignees: Vec<AccountId>,
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
    jira: Arc<dyn JiraReadPort>,
    config: IssuePullConfig,
}

impl IssuePullService {
    pub fn new(jira: Arc<dyn JiraReadPort>, config: IssuePullConfig) -> Self {
        Self { jira, config }
    }

    pub async fn pull(
        &self,
        request: IssuePullRequest,
        cancellation: &CancellationToken,
    ) -> Result<IssuePullOutcome, ApplicationError> {
        self.validate(&request)?;
        cancellation.check()?;

        let mut page_cursor: Option<PageCursor> = None;
        let mut requested_cursors = HashSet::new();
        let mut issues = Vec::new();
        let mut server_time = None;
        let mut pages_fetched = 0;

        loop {
            cancellation.check()?;
            if pages_fetched >= self.config.max_pages {
                return Err(ApplicationError::new(
                    crate::ErrorKind::Upstream,
                    "Jira pagination exceeded the configured safety limit",
                ));
            }

            if let Some(cursor) = &page_cursor {
                if cursor.0.trim().is_empty() {
                    return Err(ApplicationError::new(
                        crate::ErrorKind::Upstream,
                        "Jira returned an empty pagination cursor",
                    ));
                }
                if !requested_cursors.insert(cursor.0.clone()) {
                    return Err(ApplicationError::new(
                        crate::ErrorKind::Upstream,
                        "Jira returned a pagination cursor cycle",
                    ));
                }
            }

            let page = self
                .jira
                .fetch_issue_page(
                    &IssueFetchRequest {
                        site_id: request.site_id.clone(),
                        assignees: request.assignees.clone(),
                        updated_since: request.updated_since,
                        page_cursor: page_cursor.clone(),
                        page_size: self.config.page_size,
                    },
                    cancellation,
                )
                .await?;
            cancellation.check()?;

            pages_fetched += 1;
            server_time = max_timestamp(server_time, page.server_time);
            page_cursor = page.next_cursor;
            issues.extend(page.issues);

            if page_cursor.is_none() {
                break;
            }
        }

        cancellation.check()?;
        Ok(IssuePullOutcome {
            issues: deduplicate_issues(issues),
            pages_fetched,
            server_time,
        })
    }

    fn validate(&self, request: &IssuePullRequest) -> Result<(), ApplicationError> {
        if request.assignees.is_empty() {
            return Err(ApplicationError::invalid_input(
                "issue pull requires at least one assignee",
            ));
        }
        if request.assignees.iter().collect::<HashSet<_>>().len() != request.assignees.len() {
            return Err(ApplicationError::invalid_input(
                "issue pull assignees must be unique",
            ));
        }
        if !(1..=1_000).contains(&self.config.page_size) || self.config.max_pages == 0 {
            return Err(ApplicationError::new(
                crate::ErrorKind::Internal,
                "issue pull pagination configuration is invalid",
            ));
        }
        Ok(())
    }
}

fn max_timestamp(current: Option<Timestamp>, candidate: Option<Timestamp>) -> Option<Timestamp> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (None, candidate) => candidate,
        (current, None) => current,
    }
}

fn deduplicate_issues(issues: Vec<Issue>) -> Vec<Issue> {
    // Pages can overlap while Jira's search index catches up. Replacing in place
    // preserves the first-seen order while retaining the last snapshot received.
    let mut unique = Vec::with_capacity(issues.len());
    let mut positions = HashMap::with_capacity(issues.len());
    for issue in issues {
        let id = issue.id.clone();
        if let Some(position) = positions.get(&id).copied() {
            unique[position] = issue;
        } else {
            positions.insert(id, unique.len());
            unique.push(issue);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        future::Future,
        sync::{Arc, Mutex},
        task::{Context, Poll, Wake, Waker},
    };

    use jira_domain::{IssueId, IssueKey, IssueType, Priority, Project, Status, User};
    use time::macros::datetime;

    use super::*;
    use crate::{ErrorKind, IssuePage, PortFuture, UserSearchRequest};

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

    impl JiraReadPort for FakeJira {
        fn fetch_current_user<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, User> {
            Box::pin(async {
                Err(ApplicationError::new(
                    ErrorKind::Internal,
                    "fake does not implement current user",
                ))
            })
        }

        fn search_users<'a>(
            &'a self,
            _request: &'a UserSearchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<User>> {
            Box::pin(async { Ok(Vec::new()) })
        }

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
            assignees: vec![assignee()],
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

    fn block_on<F: Future>(future: F) -> F::Output {
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }

        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
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
    }

    #[test]
    fn rejects_empty_duplicate_assignees_and_invalid_configuration() {
        let jira = Arc::new(FakeJira::new(Vec::new()));
        let service = IssuePullService::new(jira.clone(), IssuePullConfig::default());

        let mut empty = request();
        empty.assignees.clear();
        assert_eq!(
            block_on(service.pull(empty, &CancellationToken::new()))
                .expect_err("empty assignees")
                .kind(),
            ErrorKind::InvalidInput
        );

        let mut duplicate = request();
        duplicate.assignees.push(assignee());
        assert_eq!(
            block_on(service.pull(duplicate, &CancellationToken::new()))
                .expect_err("duplicate assignees")
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
