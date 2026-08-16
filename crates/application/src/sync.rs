use std::{collections::HashSet, sync::Arc};

use jira_domain::{Issue, NotificationDelivery};
use time::Duration;

use crate::{
    ApplicationError, ApplicationEvent, ApplicationEventSink, CancellationToken, ChangeSet, Clock,
    IssueCachePort, IssueDiffer, IssueFetchRequest, JiraReadPort, NotificationPolicy,
    NotificationPort, NotificationRequest, SyncCommit, SyncMode, SyncOutcome, SyncRequest,
    SyncState,
};

#[derive(Clone, Copy, Debug)]
pub struct SyncConfig {
    pub page_size: usize,
    pub max_pages: usize,
    pub overlap: Duration,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            page_size: 100,
            max_pages: 1_000,
            overlap: Duration::minutes(5),
        }
    }
}

#[derive(Clone)]
pub struct SyncService {
    jira: Arc<dyn JiraReadPort>,
    cache: Arc<dyn IssueCachePort>,
    differ: Arc<dyn IssueDiffer>,
    notifications: Arc<dyn NotificationPort>,
    notification_policy: Arc<dyn NotificationPolicy>,
    clock: Arc<dyn Clock>,
    events: Arc<dyn ApplicationEventSink>,
    config: SyncConfig,
}

impl SyncService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        jira: Arc<dyn JiraReadPort>,
        cache: Arc<dyn IssueCachePort>,
        differ: Arc<dyn IssueDiffer>,
        notifications: Arc<dyn NotificationPort>,
        notification_policy: Arc<dyn NotificationPolicy>,
        clock: Arc<dyn Clock>,
        events: Arc<dyn ApplicationEventSink>,
        config: SyncConfig,
    ) -> Self {
        Self {
            jira,
            cache,
            differ,
            notifications,
            notification_policy,
            clock,
            events,
            config,
        }
    }

    pub async fn run(
        &self,
        request: SyncRequest,
        cancellation: &CancellationToken,
    ) -> Result<SyncOutcome, ApplicationError> {
        self.validate(&request)?;
        cancellation.check()?;
        self.events.publish(ApplicationEvent::SyncStarted {
            site_id: request.site_id.clone(),
            user_set_id: request.user_set_id.clone(),
            mode: request.mode,
        });

        let result = self.run_inner(&request, cancellation).await;
        match result {
            Ok(outcome) => {
                self.events.publish(ApplicationEvent::SyncCompleted {
                    user_set_id: request.user_set_id,
                    outcome: outcome.clone(),
                });
                Ok(outcome)
            }
            Err(error) => {
                let now = self.clock.now();
                // Recording diagnostics is best effort and must not hide the root failure.
                let _ = self
                    .cache
                    .record_sync_failure(&request.site_id, &request.user_set_id, error.kind(), now)
                    .await;
                self.events.publish(ApplicationEvent::SyncFailed {
                    user_set_id: request.user_set_id,
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }

    async fn run_inner(
        &self,
        request: &SyncRequest,
        cancellation: &CancellationToken,
    ) -> Result<SyncOutcome, ApplicationError> {
        let started_at = self.clock.now();
        let previous_state = self
            .cache
            .sync_state(&request.site_id, &request.user_set_id)
            .await?
            .unwrap_or_else(|| {
                SyncState::new(request.site_id.clone(), request.user_set_id.clone())
            });
        let updated_since = match request.mode {
            SyncMode::Incremental => previous_state
                .last_incremental_succeeded_at
                .map(|cursor| cursor - self.config.overlap),
            SyncMode::Baseline | SyncMode::Reconciliation => None,
        };

        let mut page_cursor = None;
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
            let page = self
                .jira
                .fetch_issue_page(
                    &IssueFetchRequest {
                        site_id: request.site_id.clone(),
                        assignees: request.assignees.clone(),
                        updated_since,
                        page_cursor,
                        page_size: self.config.page_size,
                    },
                    cancellation,
                )
                .await?;
            pages_fetched += 1;
            server_time = page.server_time.or(server_time);
            page_cursor = page.next_cursor;
            let page_issue_count = page.issues.len();
            issues.extend(page.issues);
            self.events.publish(ApplicationEvent::SyncPageFetched {
                user_set_id: request.user_set_id.clone(),
                page: pages_fetched,
                issue_count: page_issue_count,
                total_issue_count: issues.len(),
            });
            if page_cursor.is_none() {
                break;
            }
        }
        cancellation.check()?;

        let issues = deduplicate_issues(issues);
        let cursor = server_time.unwrap_or_else(|| self.clock.now());
        let existing = if request.mode.emits_updates() {
            self.cache
                .issues_for_user_set(&request.site_id, &request.user_set_id)
                .await?
        } else {
            Vec::new()
        };
        let update_events = if request.mode.emits_updates() {
            self.differ.diff(ChangeSet {
                existing,
                incoming: issues.clone(),
                site_id: request.site_id.clone(),
                user_set_id: request.user_set_id.clone(),
                detected_at: cursor,
                include_removed_from_view: request.mode.replaces_membership(),
            })?
        } else {
            Vec::new()
        };

        let mut state = previous_state;
        state.last_incremental_started_at = Some(started_at);
        state.last_incremental_succeeded_at = Some(cursor);
        if request.mode.replaces_membership() {
            state.last_full_sync_at = Some(cursor);
        }
        state.consecutive_failures = 0;
        state.last_error_kind = None;
        let issue_count = issues.len();
        let committed = self
            .cache
            .commit_sync(SyncCommit {
                site_id: request.site_id.clone(),
                user_set_id: request.user_set_id.clone(),
                issues,
                update_events,
                replace_membership: request.mode.replaces_membership(),
                state,
            })
            .await?;

        let mut notifications_delivered = 0;
        let mut notification_failures = 0;
        if request.mode.emits_updates() {
            for event in &committed.inserted_events {
                if self.notification_policy.should_notify(event) {
                    match self
                        .notifications
                        .deliver(NotificationRequest {
                            event: event.clone(),
                        })
                        .await
                    {
                        Ok(()) => {
                            notifications_delivered += 1;
                            let _ = self
                                .cache
                                .record_notification_delivery(
                                    &event.id,
                                    NotificationDelivery::Delivered,
                                    self.clock.now(),
                                )
                                .await;
                        }
                        Err(_) => {
                            notification_failures += 1;
                            let _ = self
                                .cache
                                .record_notification_delivery(
                                    &event.id,
                                    NotificationDelivery::Unavailable,
                                    self.clock.now(),
                                )
                                .await;
                        }
                    }
                } else {
                    let _ = self
                        .cache
                        .record_notification_delivery(
                            &event.id,
                            NotificationDelivery::SuppressedByPolicy,
                            self.clock.now(),
                        )
                        .await;
                }
            }
        }

        Ok(SyncOutcome {
            mode: request.mode,
            pages_fetched,
            issues_fetched: issue_count,
            events_inserted: committed.inserted_events.len(),
            notifications_delivered,
            notification_failures,
            cursor,
        })
    }

    fn validate(&self, request: &SyncRequest) -> Result<(), ApplicationError> {
        if request.assignees.is_empty() {
            return Err(ApplicationError::invalid_input(
                "sync requires at least one assignee",
            ));
        }
        if request.assignees.iter().collect::<HashSet<_>>().len() != request.assignees.len() {
            return Err(ApplicationError::invalid_input(
                "sync assignees must be unique",
            ));
        }
        if self.config.page_size == 0 || self.config.max_pages == 0 {
            return Err(ApplicationError::new(
                crate::ErrorKind::Internal,
                "invalid sync pagination configuration",
            ));
        }
        Ok(())
    }
}

fn deduplicate_issues(issues: Vec<Issue>) -> Vec<Issue> {
    // Jira pages should not overlap, but an eventually-consistent search can return
    // duplicates. Keeping the last snapshot gives the cache the freshest value.
    let mut unique = Vec::with_capacity(issues.len());
    let mut positions = std::collections::HashMap::new();
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

    use jira_domain::{
        AccountId, EventId, IssueId, IssueKey, IssueType, JiraSiteId, NotificationDelivery,
        Priority, Project, Status, UpdateEvent, UpdateKind, User, UserSetId,
    };
    use time::macros::datetime;

    use super::*;
    use crate::{
        CommitOutcome, ErrorKind, IssueListQuery, IssuePage, PageCursor, PortFuture, SyncCommit,
        UserSearchRequest,
    };

    #[derive(Default)]
    struct FakeJira {
        pages: Mutex<VecDeque<IssuePage>>,
        requests: Mutex<Vec<IssueFetchRequest>>,
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
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            let result = self
                .pages
                .lock()
                .expect("pages lock")
                .pop_front()
                .ok_or_else(|| ApplicationError::new(ErrorKind::Upstream, "missing fake page"));
            self.requests
                .lock()
                .expect("requests lock")
                .push(request.clone());
            Box::pin(async move { result })
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

    #[derive(Default)]
    struct FakeCache {
        state: Mutex<Option<SyncState>>,
        existing: Mutex<Vec<Issue>>,
        commits: Mutex<Vec<SyncCommit>>,
        inserted_events: Mutex<Vec<UpdateEvent>>,
        failures: Mutex<Vec<ErrorKind>>,
        deliveries: Mutex<Vec<(EventId, NotificationDelivery)>>,
    }

    impl IssueCachePort for FakeCache {
        fn list_issues<'a>(&'a self, _query: &'a IssueListQuery) -> PortFuture<'a, Vec<Issue>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn get_issue<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _issue_id: &'a IssueId,
        ) -> PortFuture<'a, Option<Issue>> {
            Box::pin(async { Ok(None) })
        }

        fn issues_for_user_set<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _user_set_id: &'a UserSetId,
        ) -> PortFuture<'a, Vec<Issue>> {
            let issues = self.existing.lock().expect("existing lock").clone();
            Box::pin(async move { Ok(issues) })
        }

        fn sync_state<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _user_set_id: &'a UserSetId,
        ) -> PortFuture<'a, Option<SyncState>> {
            let state = self.state.lock().expect("state lock").clone();
            Box::pin(async move { Ok(state) })
        }

        fn commit_sync<'a>(&'a self, commit: SyncCommit) -> PortFuture<'a, CommitOutcome> {
            self.commits.lock().expect("commits lock").push(commit);
            let events = self
                .inserted_events
                .lock()
                .expect("inserted events lock")
                .clone();
            Box::pin(async move {
                Ok(CommitOutcome {
                    inserted_events: events,
                })
            })
        }

        fn record_sync_failure<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _user_set_id: &'a UserSetId,
            kind: ErrorKind,
            _at: jira_domain::Timestamp,
        ) -> PortFuture<'a, ()> {
            self.failures.lock().expect("failures lock").push(kind);
            Box::pin(async { Ok(()) })
        }

        fn record_notification_delivery<'a>(
            &'a self,
            event_id: &'a EventId,
            delivery: NotificationDelivery,
            _at: jira_domain::Timestamp,
        ) -> PortFuture<'a, ()> {
            self.deliveries
                .lock()
                .expect("deliveries lock")
                .push((event_id.clone(), delivery));
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Default)]
    struct FakeDiffer {
        calls: Mutex<usize>,
        events: Mutex<Vec<UpdateEvent>>,
    }

    impl IssueDiffer for FakeDiffer {
        fn diff(&self, _change_set: ChangeSet) -> Result<Vec<UpdateEvent>, ApplicationError> {
            *self.calls.lock().expect("differ calls lock") += 1;
            Ok(self.events.lock().expect("differ events lock").clone())
        }
    }

    #[derive(Default)]
    struct FakeNotifications {
        calls: Mutex<usize>,
        fail: bool,
    }

    impl NotificationPort for FakeNotifications {
        fn deliver<'a>(&'a self, _request: NotificationRequest) -> PortFuture<'a, ()> {
            *self.calls.lock().expect("notification calls lock") += 1;
            let fail = self.fail;
            Box::pin(async move {
                if fail {
                    Err(ApplicationError::new(
                        ErrorKind::Notification,
                        "desktop service unavailable",
                    ))
                } else {
                    Ok(())
                }
            })
        }
    }

    struct AlwaysNotify;

    impl NotificationPolicy for AlwaysNotify {
        fn should_notify(&self, _event: &UpdateEvent) -> bool {
            true
        }
    }

    struct FixedClock(jira_domain::Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> jira_domain::Timestamp {
            self.0
        }
    }

    fn fixture_ids() -> (JiraSiteId, UserSetId, AccountId) {
        (
            JiraSiteId::new("cloud-1").expect("site id"),
            UserSetId::new("team-1").expect("user set id"),
            AccountId::new("account-1").expect("account id"),
        )
    }

    fn fixture_issue(site_id: JiraSiteId) -> Issue {
        Issue::new(
            site_id,
            IssueId::new("10001").expect("issue id"),
            IssueKey::new("APP-1").expect("issue key"),
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
            "Build the cache boundary",
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
            None,
            None,
            None,
            Vec::new(),
            datetime!(2026-08-16 10:00 UTC),
            datetime!(2026-08-16 10:00 UTC),
            None,
        )
    }

    fn fixture_event(site_id: JiraSiteId, user_set_id: UserSetId) -> UpdateEvent {
        UpdateEvent::new(
            EventId::new("event-1").expect("event id"),
            site_id,
            IssueId::new("10001").expect("issue id"),
            IssueKey::new("APP-1").expect("issue key"),
            UpdateKind::IssueAddedToView,
            datetime!(2026-08-16 13:01 UTC),
            vec![user_set_id],
        )
    }

    fn service(
        jira: Arc<FakeJira>,
        cache: Arc<FakeCache>,
        differ: Arc<FakeDiffer>,
        notifications: Arc<FakeNotifications>,
        now: jira_domain::Timestamp,
    ) -> SyncService {
        SyncService::new(
            jira,
            cache,
            differ,
            notifications,
            Arc::new(AlwaysNotify),
            Arc::new(FixedClock(now)),
            Arc::new(crate::NoopEventSink),
            SyncConfig::default(),
        )
    }

    #[test]
    fn baseline_is_committed_without_diffing_or_notifications() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let issue = fixture_issue(site_id.clone());
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([IssuePage {
                issues: vec![issue.clone(), issue],
                next_cursor: None,
                server_time: Some(datetime!(2026-08-16 12:01 UTC)),
            }])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        let differ = Arc::new(FakeDiffer::default());
        let notifications = Arc::new(FakeNotifications::default());
        let service = service(
            jira,
            cache.clone(),
            differ.clone(),
            notifications.clone(),
            datetime!(2026-08-16 12:00 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: vec![account_id],
                mode: SyncMode::Baseline,
            },
            &CancellationToken::new(),
        ))
        .expect("baseline sync");

        assert_eq!(
            outcome.issues_fetched, 1,
            "duplicate pages are deduplicated"
        );
        assert_eq!(outcome.events_inserted, 0);
        assert_eq!(*differ.calls.lock().expect("differ calls lock"), 0);
        assert_eq!(
            *notifications.calls.lock().expect("notification calls lock"),
            0
        );
        let commits = cache.commits.lock().expect("commits lock");
        assert_eq!(commits.len(), 1);
        assert!(commits[0].update_events.is_empty());
        assert!(commits[0].replace_membership);
        assert_eq!(
            commits[0].state.last_full_sync_at,
            Some(datetime!(2026-08-16 12:01 UTC))
        );
    }

    #[test]
    fn incremental_sync_overlaps_cursor_and_notification_failure_is_non_fatal() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let event = fixture_event(site_id.clone(), user_set_id.clone());
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([
                IssuePage {
                    issues: vec![fixture_issue(site_id.clone())],
                    next_cursor: Some(PageCursor("unused-in-single-test".into())),
                    server_time: Some(datetime!(2026-08-16 13:01 UTC)),
                },
                IssuePage {
                    issues: Vec::new(),
                    next_cursor: None,
                    server_time: None,
                },
            ])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.state.lock().expect("state lock") = Some(SyncState {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            last_incremental_started_at: None,
            last_incremental_succeeded_at: Some(datetime!(2026-08-16 12:00 UTC)),
            last_full_sync_at: None,
            consecutive_failures: 0,
            last_error_kind: None,
        });
        *cache.inserted_events.lock().expect("inserted events lock") = vec![event.clone()];
        let differ = Arc::new(FakeDiffer::default());
        *differ.events.lock().expect("differ events lock") = vec![event];
        let notifications = Arc::new(FakeNotifications {
            fail: true,
            ..FakeNotifications::default()
        });
        let service = service(
            jira.clone(),
            cache.clone(),
            differ,
            notifications,
            datetime!(2026-08-16 13:00 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: vec![account_id],
                mode: SyncMode::Incremental,
            },
            &CancellationToken::new(),
        ))
        .expect("incremental sync survives desktop notification failure");

        assert_eq!(outcome.pages_fetched, 2);
        assert_eq!(outcome.events_inserted, 1);
        assert_eq!(outcome.notification_failures, 1);
        let requests = jira.requests.lock().expect("requests lock");
        assert_eq!(
            requests[0].updated_since,
            Some(datetime!(2026-08-16 11:55 UTC))
        );
        assert_eq!(
            cache.deliveries.lock().expect("deliveries lock").as_slice(),
            &[(
                EventId::new("event-1").expect("event id"),
                NotificationDelivery::Unavailable
            )]
        );
    }

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }
}
