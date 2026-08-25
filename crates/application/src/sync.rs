use std::{collections::HashSet, sync::Arc};

use jira_domain::{Issue, IssueComment, NotificationDelivery, Timestamp, UpdateEvent, UpdateKind};
use time::Duration;

use crate::{
    ApplicationError, ApplicationEvent, ApplicationEventSink, CancellationToken, ChangeSet, Clock,
    IssueCachePort, IssueDiffer, IssueFetchRequest, JiraReadPort, NotificationPolicy,
    NotificationPort, NotificationRequest, SyncCommit, SyncMode, SyncOutcome, SyncRequest,
    SyncState, enrich_with_changelog, issue_pagination::IssuePagination, validate_jql_scope,
};

const MAX_CHANGELOG_ISSUES_PER_REQUEST: usize = 1_000;
const MAX_RECENT_ISSUE_COMMENTS: usize = 100;
const MAX_COMMENT_EXCERPT_BYTES: usize = 280;

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

        let mut pagination = IssuePagination::new(
            self.config.page_size,
            self.config.max_pages,
            "invalid sync pagination configuration",
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
                        updated_since,
                        page_cursor,
                        page_size: self.config.page_size,
                    },
                    cancellation,
                )
                .await?;
            let page_stats = pagination.accept_page(page, cancellation)?;
            self.events.publish(ApplicationEvent::SyncPageFetched {
                user_set_id: request.user_set_id.clone(),
                page: page_stats.page,
                issue_count: page_stats.issue_count,
                total_issue_count: page_stats.total_issue_count,
            });
            if !pagination.has_next_page() {
                break;
            }
        }

        let pagination_outcome = pagination.finish();
        let issues = pagination_outcome.issues;
        let pages_fetched = pagination_outcome.pages_fetched;
        let server_time = pagination_outcome.server_time;
        let notification_issue_ids = request.notification_assignees.as_deref().map(|assignees| {
            issues
                .iter()
                .filter(|issue| {
                    issue
                        .assignee
                        .as_ref()
                        .is_some_and(|assignee| assignees.contains(assignee))
                })
                .map(|issue| issue.id.clone())
                .collect::<HashSet<_>>()
        });
        let cursor = server_time.unwrap_or_else(|| self.clock.now());
        let existing = if request.mode.emits_updates() {
            self.cache
                .issues_for_user_set(&request.site_id, &request.user_set_id)
                .await?
        } else {
            Vec::new()
        };
        let existing_for_enrichment = existing.clone();
        let update_events = if request.mode.emits_updates() {
            let mut update_events = self.differ.diff(ChangeSet {
                existing,
                incoming: issues.clone(),
                site_id: request.site_id.clone(),
                user_set_id: request.user_set_id.clone(),
                detected_at: cursor,
                include_removed_from_view: request.mode.replaces_membership(),
            })?;
            let changelog_issue_ids = changed_issue_ids(&existing_for_enrichment, &issues);
            if !changelog_issue_ids.is_empty() {
                for issue_ids in changelog_issue_ids.chunks(MAX_CHANGELOG_ISSUES_PER_REQUEST) {
                    cancellation.check()?;
                    match self
                        .jira
                        .fetch_issue_changelog(
                            &crate::IssueChangelogRequest {
                                site_id: request.site_id.clone(),
                                issue_ids: issue_ids.to_vec(),
                            },
                            cancellation,
                        )
                        .await
                    {
                        Ok(page) => {
                            update_events = enrich_with_changelog(
                                update_events,
                                &existing_for_enrichment,
                                &issues,
                                &page,
                                &request.site_id,
                                &request.user_set_id,
                            );
                        }
                        Err(error) if error.kind() == crate::ErrorKind::Cancelled => {
                            return Err(error);
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }
            }
            let mention_events = self
                .mention_events(&existing_for_enrichment, &issues, request, cancellation)
                .await?;
            let mentioned_issue_ids = mention_events
                .iter()
                .map(|event| event.issue_id.clone())
                .collect::<HashSet<_>>();
            // A generic snapshot fallback represents the same activity as a
            // direct mention. Remove only that fallback for the affected issue;
            // specific field/changelog events remain independently useful.
            update_events.retain(|event| {
                !(mentioned_issue_ids.contains(&event.issue_id)
                    && matches!(event.kind, UpdateKind::IssueUpdated))
            });
            update_events.extend(mention_events);
            update_events
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
                if !matches!(event.kind, UpdateKind::CommentAdded { .. })
                    && notification_issue_ids
                        .as_ref()
                        .is_some_and(|issue_ids| !issue_ids.contains(&event.issue_id))
                {
                    let _ = self
                        .cache
                        .record_notification_delivery(
                            &event.id,
                            NotificationDelivery::SuppressedByPolicy,
                            self.clock.now(),
                        )
                        .await;
                    continue;
                }
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

    async fn mention_events(
        &self,
        existing: &[Issue],
        incoming: &[Issue],
        request: &SyncRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<UpdateEvent>, ApplicationError> {
        let Some(notification_assignees) = request.notification_assignees.as_deref() else {
            return Ok(Vec::new());
        };
        if notification_assignees.is_empty() {
            return Ok(Vec::new());
        }

        let mut events = Vec::new();
        for (old_issue, new_issue) in changed_issue_pairs(existing, incoming) {
            cancellation.check()?;
            let comments = match self
                .jira
                .fetch_recent_issue_comments(
                    &crate::RecentIssueCommentsRequest {
                        site_id: request.site_id.clone(),
                        issue_id: new_issue.id.clone(),
                        limit: MAX_RECENT_ISSUE_COMMENTS,
                    },
                    cancellation,
                )
                .await
            {
                Ok(comments) => comments,
                Err(error) if error.kind() == crate::ErrorKind::Cancelled => {
                    return Err(error);
                }
                // A gateway that predates the optional read, or a restricted/deleted
                // issue, must not prevent the rest of the sync from committing.
                Err(error)
                    if matches!(
                        error.kind(),
                        crate::ErrorKind::Authorization
                            | crate::ErrorKind::NotFound
                            | crate::ErrorKind::Internal
                    ) =>
                {
                    continue;
                }
                // Authentication, transport, rate-limit, and upstream failures are
                // retryable. Do not advance the sync cursor after one of these.
                Err(error) => return Err(error),
            };

            let mut seen_comments = HashSet::new();
            for comment in comments {
                cancellation.check()?;
                if !seen_comments.insert(comment.id.clone()) {
                    continue;
                }
                let Some(activity_at) =
                    comment_activity(&comment, old_issue.updated_at, new_issue.updated_at)
                else {
                    continue;
                };
                let Some(rich_body) = comment.rich_body.as_ref() else {
                    continue;
                };
                if !notification_assignees
                    .iter()
                    .any(|account| rich_body.mentions_account(account))
                {
                    continue;
                }
                let kind = UpdateKind::CommentAdded {
                    comment_id: comment.id.clone(),
                    author: comment
                        .author
                        .as_ref()
                        .map(|author| author.account_id.clone()),
                    excerpt: comment_excerpt(&comment),
                };
                events.push(UpdateEvent::new(
                    comment_event_id(&request.site_id, new_issue, &comment, activity_at),
                    request.site_id.clone(),
                    new_issue.id.clone(),
                    new_issue.key.clone(),
                    kind,
                    activity_at,
                    vec![request.user_set_id.clone()],
                ));
            }
        }
        Ok(events)
    }

    fn validate(&self, request: &SyncRequest) -> Result<(), ApplicationError> {
        validate_jql_scope(request.jql_scope.as_deref())
            .map_err(ApplicationError::invalid_input)?;
        if let Some(assignees) = &request.assignees
            && assignees.iter().collect::<HashSet<_>>().len() != assignees.len()
        {
            return Err(ApplicationError::invalid_input(
                "sync assignees must be unique",
            ));
        }
        if let Some(watchers) = &request.watchers
            && watchers.iter().collect::<HashSet<_>>().len() != watchers.len()
        {
            return Err(ApplicationError::invalid_input(
                "sync watchers must be unique",
            ));
        }
        if let Some(assignees) = &request.notification_assignees
            && assignees.iter().collect::<HashSet<_>>().len() != assignees.len()
        {
            return Err(ApplicationError::invalid_input(
                "notification assignees must be unique",
            ));
        }
        crate::issue_pagination::validate_pagination_config(
            self.config.page_size,
            self.config.max_pages,
            "invalid sync pagination configuration",
        )?;
        Ok(())
    }
}

fn changed_issue_ids(existing: &[Issue], incoming: &[Issue]) -> Vec<jira_domain::IssueId> {
    let mut ids = changed_issue_pairs(existing, incoming)
        .into_iter()
        .map(|(_, issue)| issue.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

fn changed_issue_pairs<'a>(
    existing: &'a [Issue],
    incoming: &'a [Issue],
) -> Vec<(&'a Issue, &'a Issue)> {
    let old = existing
        .iter()
        .map(|issue| (&issue.id, issue))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut pairs = incoming
        .iter()
        .filter_map(|issue| {
            let previous = old.get(&issue.id).copied()?;
            (previous.lifecycle == jira_domain::IssueLifecycle::Present
                && issue.lifecycle == jira_domain::IssueLifecycle::Present
                && previous.updated_at != issue.updated_at)
                .then_some((previous, issue))
        })
        .collect::<Vec<_>>();
    pairs.sort_by(|(_, left), (_, right)| left.id.cmp(&right.id));
    pairs
}

fn comment_activity(
    comment: &IssueComment,
    old_updated_at: Timestamp,
    new_updated_at: Timestamp,
) -> Option<Timestamp> {
    let in_window =
        |timestamp: Timestamp| timestamp > old_updated_at && timestamp <= new_updated_at;
    comment
        .updated_at
        .filter(|timestamp| in_window(*timestamp))
        .or_else(|| in_window(comment.created_at).then_some(comment.created_at))
}

fn comment_excerpt(comment: &IssueComment) -> String {
    let source = comment
        .rich_body
        .as_ref()
        .map(|body| body.plain_text())
        .unwrap_or_else(|| comment.body.clone());
    let mut excerpt = String::with_capacity(source.len().min(MAX_COMMENT_EXCERPT_BYTES));
    for character in source.chars() {
        if character.is_control() {
            if (character == '\n' || character == '\r' || character == '\t')
                && !excerpt.ends_with(' ')
            {
                excerpt.push(' ');
            }
        } else {
            excerpt.push(character);
        }
        if excerpt.len() >= MAX_COMMENT_EXCERPT_BYTES {
            break;
        }
    }
    excerpt.truncate(excerpt.floor_char_boundary(MAX_COMMENT_EXCERPT_BYTES));
    excerpt.trim().to_owned()
}

fn comment_event_id(
    site_id: &jira_domain::JiraSiteId,
    issue: &Issue,
    comment: &IssueComment,
    activity_at: Timestamp,
) -> jira_domain::EventId {
    let activity = activity_at.unix_timestamp_nanos().to_string();
    let parts = [
        site_id.as_str(),
        issue.id.as_str(),
        comment.id.as_str(),
        &activity,
    ];
    let left = stable_digest(&parts, 0xcbf29ce484222325);
    let right = stable_digest(&parts, 0x84222325cbf29ce4);
    jira_domain::EventId::new(format!("v1-comment-{left:016x}{right:016x}"))
        .expect("event ID length")
}

fn stable_digest(parts: &[&str], mut hash: u64) -> u64 {
    for part in parts {
        for byte in (part.len() as u64)
            .to_le_bytes()
            .into_iter()
            .chain(part.as_bytes().iter().copied())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use jira_domain::{
        AccountId, EventId, IssueId, IssueKey, IssueType, JiraSiteId, NotificationDelivery,
        Priority, Project, RichBlock, RichInline, RichTextDocument, Status, UpdateEvent,
        UpdateKind, User, UserSetId,
    };
    use time::macros::datetime;

    use super::*;
    use crate::{
        CommitOutcome, DefaultDesktopNotificationPolicy, ErrorKind, IssueListQuery, IssuePage,
        PageCursor, PortFuture, SyncCommit, UserSearchRequest, test_support::block_on,
    };

    #[derive(Default)]
    struct FakeJira {
        pages: Mutex<VecDeque<IssuePage>>,
        requests: Mutex<Vec<IssueFetchRequest>>,
        recent_comments: Mutex<VecDeque<Result<Vec<jira_domain::IssueComment>, ApplicationError>>>,
        comment_requests: Mutex<Vec<crate::RecentIssueCommentsRequest>>,
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

        fn fetch_recent_issue_comments<'a>(
            &'a self,
            request: &'a crate::RecentIssueCommentsRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<jira_domain::IssueComment>> {
            self.comment_requests
                .lock()
                .expect("comment requests lock")
                .push(request.clone());
            let result = self
                .recent_comments
                .lock()
                .expect("recent comments lock")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(ApplicationError::new(
                        ErrorKind::Internal,
                        "missing fake comments",
                    ))
                });
            Box::pin(async move { result })
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
            let committed_events = commit.update_events.clone();
            self.commits.lock().expect("commits lock").push(commit);
            let events = if committed_events.is_empty() {
                self.inserted_events
                    .lock()
                    .expect("inserted events lock")
                    .clone()
            } else {
                committed_events
            };
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

    fn mention_comment(
        id: &str,
        account_id: &AccountId,
        created_at: jira_domain::Timestamp,
        updated_at: Option<jira_domain::Timestamp>,
    ) -> jira_domain::IssueComment {
        let mut comment = jira_domain::IssueComment::new(
            id,
            None,
            "@Asha was mentioned",
            created_at,
            updated_at,
            Vec::new(),
        )
        .expect("comment");
        comment.rich_body = Some(RichTextDocument::new(
            vec![RichBlock::BulletList(vec![jira_domain::RichListItem {
                blocks: vec![RichBlock::Paragraph(vec![RichInline::Mention {
                    account_id: Some(account_id.clone()),
                    label: "@Asha".into(),
                }])],
            }])],
            false,
        ));
        comment
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

    fn service_with_policy(
        jira: Arc<FakeJira>,
        cache: Arc<FakeCache>,
        differ: Arc<FakeDiffer>,
        notifications: Arc<FakeNotifications>,
        policy: Arc<dyn NotificationPolicy>,
        now: jira_domain::Timestamp,
    ) -> SyncService {
        SyncService::new(
            jira,
            cache,
            differ,
            notifications,
            policy,
            Arc::new(FixedClock(now)),
            Arc::new(crate::NoopEventSink),
            SyncConfig::default(),
        )
    }

    #[test]
    fn baseline_is_committed_without_diffing_or_notifications() {
        let (site_id, user_set_id, _account_id) = fixture_ids();
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
            jira.clone(),
            cache.clone(),
            differ.clone(),
            notifications.clone(),
            datetime!(2026-08-16 12:00 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: None,
                jql_scope: None,
                notification_assignees: None,
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
        assert_eq!(
            jira.requests.lock().expect("requests lock")[0].assignees,
            None
        );
        assert!(
            jira.comment_requests
                .lock()
                .expect("comment requests lock")
                .is_empty()
        );
    }

    #[test]
    fn rejects_cursor_cycles_before_commit() {
        let (site_id, user_set_id, _account_id) = fixture_ids();
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([
                IssuePage {
                    issues: Vec::new(),
                    next_cursor: Some(PageCursor("same".into())),
                    server_time: None,
                },
                IssuePage {
                    issues: Vec::new(),
                    next_cursor: Some(PageCursor("same".into())),
                    server_time: None,
                },
            ])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        let service = service(
            jira.clone(),
            cache.clone(),
            Arc::new(FakeDiffer::default()),
            Arc::new(FakeNotifications::default()),
            datetime!(2026-08-16 12:00 UTC),
        );

        let error = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: None,
                jql_scope: None,
                notification_assignees: None,
                mode: SyncMode::Baseline,
            },
            &CancellationToken::new(),
        ))
        .expect_err("cursor cycle");

        assert_eq!(error.kind(), ErrorKind::Upstream);
        assert!(cache.commits.lock().expect("commits lock").is_empty());
        assert_eq!(jira.requests.lock().expect("requests lock").len(), 2);
    }

    #[test]
    fn uses_greatest_server_timestamp_when_pages_are_non_monotonic() {
        let (site_id, user_set_id, _account_id) = fixture_ids();
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([
                IssuePage {
                    issues: Vec::new(),
                    next_cursor: Some(PageCursor("older".into())),
                    server_time: Some(datetime!(2026-08-16 12:00 UTC)),
                },
                IssuePage {
                    issues: Vec::new(),
                    next_cursor: None,
                    server_time: Some(datetime!(2026-08-16 11:00 UTC)),
                },
            ])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        let service = service(
            jira,
            cache.clone(),
            Arc::new(FakeDiffer::default()),
            Arc::new(FakeNotifications::default()),
            datetime!(2026-08-16 10:00 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: None,
                jql_scope: None,
                notification_assignees: None,
                mode: SyncMode::Baseline,
            },
            &CancellationToken::new(),
        ))
        .expect("baseline sync");

        let greatest = datetime!(2026-08-16 12:00 UTC);
        assert_eq!(outcome.cursor, greatest);
        assert_eq!(
            cache.commits.lock().expect("commits lock")[0]
                .state
                .last_full_sync_at,
            Some(greatest)
        );
    }

    #[test]
    fn direct_mention_delivers_for_watcher_only_changed_ticket_and_filters_activity() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let old_updated_at = datetime!(2026-08-16 10:00 UTC);
        let new_updated_at = datetime!(2026-08-16 12:00 UTC);
        let mut old_issue = fixture_issue(site_id.clone());
        old_issue.updated_at = old_updated_at;
        let mut incoming_issue = old_issue.clone();
        incoming_issue.updated_at = new_updated_at;
        let unrelated_account = AccountId::new("account-2").expect("account");
        let direct = mention_comment(
            "comment-1",
            &account_id,
            datetime!(2026-08-16 11:00 UTC),
            None,
        );
        let unrelated = mention_comment(
            "comment-2",
            &unrelated_account,
            datetime!(2026-08-16 11:10 UTC),
            None,
        );
        let outside_window = mention_comment(
            "comment-3",
            &account_id,
            datetime!(2026-08-16 09:00 UTC),
            Some(datetime!(2026-08-16 09:30 UTC)),
        );
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([IssuePage {
                issues: vec![incoming_issue],
                next_cursor: None,
                server_time: Some(new_updated_at),
            }])),
            recent_comments: Mutex::new(VecDeque::from([Ok(vec![
                direct.clone(),
                direct,
                unrelated,
                outside_window,
            ])])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.existing.lock().expect("existing lock") = vec![old_issue];
        let notifications = Arc::new(FakeNotifications::default());
        let service = service(
            jira.clone(),
            cache.clone(),
            Arc::new(FakeDiffer::default()),
            notifications.clone(),
            datetime!(2026-08-16 12:01 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: Some(vec![account_id.clone()]),
                jql_scope: None,
                notification_assignees: Some(vec![account_id]),
                mode: SyncMode::Incremental,
            },
            &CancellationToken::new(),
        ))
        .expect("mention sync");

        assert_eq!(outcome.events_inserted, 1);
        assert_eq!(outcome.notifications_delivered, 1);
        assert_eq!(*notifications.calls.lock().expect("notification calls"), 1);
        let commits = cache.commits.lock().expect("commits lock");
        assert_eq!(commits[0].update_events.len(), 1);
        assert!(matches!(
            commits[0].update_events[0].kind,
            UpdateKind::CommentAdded { .. }
        ));
        let requests = jira.comment_requests.lock().expect("comment requests lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].limit, MAX_RECENT_ISSUE_COMMENTS);
    }

    #[test]
    fn direct_mention_replaces_only_generic_fallback_for_assigned_issue() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let mut old_issue = fixture_issue(site_id.clone());
        old_issue.assignee = Some(account_id.clone());
        old_issue.updated_at = datetime!(2026-08-16 10:00 UTC);
        let mut incoming_issue = old_issue.clone();
        incoming_issue.updated_at = datetime!(2026-08-16 12:00 UTC);
        let fallback = UpdateEvent::new(
            EventId::new("generic-fallback").expect("event id"),
            site_id.clone(),
            incoming_issue.id.clone(),
            incoming_issue.key.clone(),
            UpdateKind::IssueUpdated,
            incoming_issue.updated_at,
            vec![user_set_id.clone()],
        );
        let comment = mention_comment(
            "comment-1",
            &account_id,
            datetime!(2026-08-16 11:00 UTC),
            None,
        );
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([IssuePage {
                issues: vec![incoming_issue],
                next_cursor: None,
                server_time: Some(datetime!(2026-08-16 12:00 UTC)),
            }])),
            recent_comments: Mutex::new(VecDeque::from([Ok(vec![comment])])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.existing.lock().expect("existing lock") = vec![old_issue];
        let differ = Arc::new(FakeDiffer::default());
        *differ.events.lock().expect("differ events lock") = vec![fallback];
        let notifications = Arc::new(FakeNotifications::default());
        let service = service(
            jira,
            cache.clone(),
            differ,
            notifications.clone(),
            datetime!(2026-08-16 12:01 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: None,
                jql_scope: None,
                notification_assignees: Some(vec![account_id]),
                mode: SyncMode::Incremental,
            },
            &CancellationToken::new(),
        ))
        .expect("mention sync");

        assert_eq!(outcome.events_inserted, 1);
        assert_eq!(outcome.notifications_delivered, 1);
        assert_eq!(*notifications.calls.lock().expect("notification calls"), 1);
        let commits = cache.commits.lock().expect("commits lock");
        assert_eq!(commits[0].update_events.len(), 1);
        assert!(matches!(
            &commits[0].update_events[0].kind,
            UpdateKind::CommentAdded { .. }
        ));
    }

    #[test]
    fn assignment_to_authenticated_account_is_delivered() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let old_issue = fixture_issue(site_id.clone());
        let mut incoming_issue = old_issue.clone();
        incoming_issue.assignee = Some(account_id.clone());
        incoming_issue.updated_at = datetime!(2026-08-16 12:00 UTC);
        let event = UpdateEvent::new(
            EventId::new("assigned-to-me").expect("event id"),
            site_id.clone(),
            incoming_issue.id.clone(),
            incoming_issue.key.clone(),
            UpdateKind::AssigneeChanged {
                old: jira_domain::ChangeValue::Empty,
                new: jira_domain::ChangeValue::Account(account_id.clone()),
            },
            incoming_issue.updated_at,
            vec![user_set_id.clone()],
        );
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([IssuePage {
                issues: vec![incoming_issue],
                next_cursor: None,
                server_time: Some(datetime!(2026-08-16 12:00 UTC)),
            }])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.existing.lock().expect("existing lock") = vec![old_issue];
        let differ = Arc::new(FakeDiffer::default());
        *differ.events.lock().expect("differ events lock") = vec![event.clone()];
        let notifications = Arc::new(FakeNotifications::default());
        let service = service(
            jira,
            cache.clone(),
            differ,
            notifications.clone(),
            datetime!(2026-08-16 12:01 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: None,
                jql_scope: None,
                notification_assignees: Some(vec![account_id]),
                mode: SyncMode::Incremental,
            },
            &CancellationToken::new(),
        ))
        .expect("assignment sync");

        assert_eq!(outcome.notifications_delivered, 1);
        assert_eq!(
            cache.deliveries.lock().expect("deliveries lock").as_slice(),
            &[(event.id, NotificationDelivery::Delivered)]
        );
    }

    #[test]
    fn assignment_away_from_watcher_only_ticket_is_suppressed() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let mut old_issue = fixture_issue(site_id.clone());
        old_issue.assignee = Some(account_id.clone());
        let mut incoming_issue = old_issue.clone();
        incoming_issue.assignee = None;
        incoming_issue.updated_at = datetime!(2026-08-16 12:00 UTC);
        let event = UpdateEvent::new(
            EventId::new("assigned-away").expect("event id"),
            site_id.clone(),
            incoming_issue.id.clone(),
            incoming_issue.key.clone(),
            UpdateKind::AssigneeChanged {
                old: jira_domain::ChangeValue::Account(account_id.clone()),
                new: jira_domain::ChangeValue::Empty,
            },
            incoming_issue.updated_at,
            vec![user_set_id.clone()],
        );
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([IssuePage {
                issues: vec![incoming_issue],
                next_cursor: None,
                server_time: Some(datetime!(2026-08-16 12:00 UTC)),
            }])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.existing.lock().expect("existing lock") = vec![old_issue];
        let differ = Arc::new(FakeDiffer::default());
        *differ.events.lock().expect("differ events lock") = vec![event.clone()];
        let notifications = Arc::new(FakeNotifications::default());
        let service = service(
            jira,
            cache.clone(),
            differ,
            notifications.clone(),
            datetime!(2026-08-16 12:01 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: Some(vec![account_id.clone()]),
                jql_scope: None,
                notification_assignees: Some(vec![account_id]),
                mode: SyncMode::Incremental,
            },
            &CancellationToken::new(),
        ))
        .expect("assignment sync");

        assert_eq!(outcome.notifications_delivered, 0);
        assert_eq!(*notifications.calls.lock().expect("notification calls"), 0);
        assert_eq!(
            cache.deliveries.lock().expect("deliveries lock").as_slice(),
            &[(event.id, NotificationDelivery::SuppressedByPolicy)]
        );
    }

    #[test]
    fn transient_recent_comment_error_aborts_before_commit_and_retry_reuses_cursor() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let mut old_issue = fixture_issue(site_id.clone());
        old_issue.updated_at = datetime!(2026-08-16 10:00 UTC);
        let mut incoming_issue = old_issue.clone();
        incoming_issue.updated_at = datetime!(2026-08-16 12:00 UTC);
        let previous_cursor = datetime!(2026-08-16 10:00 UTC);
        let next_page = || IssuePage {
            issues: vec![incoming_issue.clone()],
            next_cursor: None,
            server_time: Some(datetime!(2026-08-16 12:00 UTC)),
        };
        let comment = mention_comment(
            "comment-1",
            &account_id,
            datetime!(2026-08-16 11:00 UTC),
            None,
        );
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([next_page(), next_page()])),
            recent_comments: Mutex::new(VecDeque::from([
                Err(ApplicationError::new(ErrorKind::Offline, "offline")),
                Ok(vec![comment]),
            ])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.existing.lock().expect("existing lock") = vec![old_issue];
        *cache.state.lock().expect("state lock") = Some(SyncState {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            last_incremental_started_at: None,
            last_incremental_succeeded_at: Some(previous_cursor),
            last_full_sync_at: None,
            consecutive_failures: 0,
            last_error_kind: None,
        });
        let notifications = Arc::new(FakeNotifications::default());
        let service = service(
            jira.clone(),
            cache.clone(),
            Arc::new(FakeDiffer::default()),
            notifications,
            datetime!(2026-08-16 12:01 UTC),
        );
        let request = SyncRequest {
            site_id,
            user_set_id,
            assignees: None,
            watchers: None,
            jql_scope: None,
            notification_assignees: Some(vec![account_id]),
            mode: SyncMode::Incremental,
        };

        let first = block_on(service.run(request.clone(), &CancellationToken::new()))
            .expect_err("offline comment read must abort before commit");
        assert_eq!(first.kind(), ErrorKind::Offline);
        assert!(cache.commits.lock().expect("commits lock").is_empty());

        let second = block_on(service.run(request, &CancellationToken::new()))
            .expect("retry after comment read recovers");
        assert_eq!(second.notifications_delivered, 1);
        assert_eq!(cache.commits.lock().expect("commits lock").len(), 1);
        let requests = jira.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].updated_since,
            Some(previous_cursor - Duration::minutes(5))
        );
        assert_eq!(requests[0].updated_since, requests[1].updated_since);
    }

    #[test]
    fn comment_event_id_is_stable_for_retries() {
        let (site_id, _user_set_id, _account_id) = fixture_ids();
        let issue = fixture_issue(site_id.clone());
        let comment = jira_domain::IssueComment::new(
            "comment-1",
            None,
            "body",
            datetime!(2026-08-16 11:00 UTC),
            None,
            Vec::new(),
        )
        .expect("comment");
        let first = comment_event_id(&site_id, &issue, &comment, comment.created_at);
        let retry = comment_event_id(&site_id, &issue, &comment, comment.created_at);
        assert_eq!(first, retry);
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
                assignees: Some(vec![account_id.clone()]),
                watchers: Some(vec![account_id.clone()]),
                jql_scope: None,
                notification_assignees: None,
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
        assert_eq!(requests[0].assignees, Some(vec![account_id.clone()]));
        assert_eq!(requests[0].watchers, Some(vec![account_id.clone()]));
        assert_eq!(
            cache.deliveries.lock().expect("deliveries lock").as_slice(),
            &[(
                EventId::new("event-1").expect("event id"),
                NotificationDelivery::Unavailable
            )]
        );
    }

    #[test]
    fn persists_and_delivers_in_scope_generic_issue_update() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let mut issue = fixture_issue(site_id.clone());
        issue.assignee = Some(account_id.clone());
        let event = UpdateEvent::new(
            EventId::new("event-generic-update").expect("event id"),
            site_id.clone(),
            issue.id.clone(),
            issue.key.clone(),
            UpdateKind::IssueUpdated,
            datetime!(2026-08-16 13:01 UTC),
            vec![user_set_id.clone()],
        );
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([IssuePage {
                issues: vec![issue],
                next_cursor: None,
                server_time: Some(datetime!(2026-08-16 13:01 UTC)),
            }])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.inserted_events.lock().expect("inserted events lock") = vec![event.clone()];
        let differ = Arc::new(FakeDiffer::default());
        *differ.events.lock().expect("differ events lock") = vec![event.clone()];
        let notifications = Arc::new(FakeNotifications::default());
        let service = service_with_policy(
            jira,
            cache.clone(),
            differ,
            notifications.clone(),
            Arc::new(DefaultDesktopNotificationPolicy),
            datetime!(2026-08-16 13:00 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: None,
                jql_scope: None,
                notification_assignees: Some(vec![account_id]),
                mode: SyncMode::Incremental,
            },
            &CancellationToken::new(),
        ))
        .expect("incremental sync");

        assert_eq!(outcome.events_inserted, 1);
        assert_eq!(outcome.notifications_delivered, 1);
        assert_eq!(outcome.notification_failures, 0);
        let commits = cache.commits.lock().expect("commits lock");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].update_events, vec![event.clone()]);
        assert_eq!(
            *notifications.calls.lock().expect("notification calls lock"),
            1
        );
        assert_eq!(
            cache.deliveries.lock().expect("deliveries lock").as_slice(),
            &[(event.id, NotificationDelivery::Delivered)]
        );
    }

    #[test]
    fn project_wide_sync_scopes_notifications_to_incoming_my_issues() {
        let (site_id, user_set_id, account_id) = fixture_ids();
        let mut my_issue = fixture_issue(site_id.clone());
        my_issue.assignee = Some(account_id.clone());
        let mut other_issue = fixture_issue(site_id.clone());
        other_issue.id = IssueId::new("10002").expect("issue id");
        other_issue.key = IssueKey::new("APP-2").expect("issue key");
        other_issue.assignee = Some(AccountId::new("account-2").expect("account id"));

        let my_event = fixture_event(site_id.clone(), user_set_id.clone());
        let other_event = UpdateEvent::new(
            EventId::new("event-2").expect("event id"),
            site_id.clone(),
            other_issue.id.clone(),
            other_issue.key.clone(),
            UpdateKind::StatusChanged {
                old: jira_domain::ChangeValue::Text("Open".into()),
                new: jira_domain::ChangeValue::Text("Done".into()),
            },
            datetime!(2026-08-16 13:01 UTC),
            vec![user_set_id.clone()],
        );
        let jira = Arc::new(FakeJira {
            pages: Mutex::new(VecDeque::from([IssuePage {
                issues: vec![my_issue, other_issue],
                next_cursor: None,
                server_time: Some(datetime!(2026-08-16 13:01 UTC)),
            }])),
            ..FakeJira::default()
        });
        let cache = Arc::new(FakeCache::default());
        *cache.inserted_events.lock().expect("inserted events lock") =
            vec![my_event.clone(), other_event.clone()];
        let differ = Arc::new(FakeDiffer::default());
        *differ.events.lock().expect("differ events lock") =
            vec![my_event.clone(), other_event.clone()];
        let notifications = Arc::new(FakeNotifications::default());
        let service = service(
            jira.clone(),
            cache.clone(),
            differ,
            notifications.clone(),
            datetime!(2026-08-16 13:00 UTC),
        );

        let outcome = block_on(service.run(
            SyncRequest {
                site_id,
                user_set_id,
                assignees: None,
                watchers: None,
                jql_scope: None,
                notification_assignees: Some(vec![account_id]),
                mode: SyncMode::Incremental,
            },
            &CancellationToken::new(),
        ))
        .expect("project-wide sync");

        assert_eq!(outcome.notifications_delivered, 1);
        assert_eq!(outcome.notification_failures, 0);
        assert_eq!(
            *notifications.calls.lock().expect("notification calls lock"),
            1
        );
        assert_eq!(
            cache.deliveries.lock().expect("deliveries lock").as_slice(),
            &[
                (my_event.id, NotificationDelivery::Delivered,),
                (other_event.id, NotificationDelivery::SuppressedByPolicy,),
            ]
        );
        assert_eq!(
            jira.requests.lock().expect("requests lock")[0].assignees,
            None,
            "notification scope must not narrow the project-wide Jira fetch"
        );
    }
}
