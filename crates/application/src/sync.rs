use std::{collections::HashSet, sync::Arc};

use jira_domain::{NotificationDelivery, UpdateEvent, UpdateKind};
use time::Duration;

use crate::{
    ApplicationError, ApplicationEvent, ApplicationEventSink, CancellationToken, ChangeSet, Clock,
    IssueCachePort, IssueDiffer, JiraSyncReadPort, NotificationPolicy, NotificationPort,
    NotificationRequest, SyncCommit, SyncMode, SyncOutcome, SyncRequest, SyncState,
    issue_fetch_scope::IssueFetchScope,
    issue_pagination::IssuePagination,
    sync_activity::{SyncActivityEnricher, SyncActivityRequest},
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
    jira: Arc<dyn JiraSyncReadPort>,
    cache: Arc<dyn IssueCachePort>,
    differ: Arc<dyn IssueDiffer>,
    notifications: Arc<dyn NotificationPort>,
    notification_policy: Arc<dyn NotificationPolicy>,
    clock: Arc<dyn Clock>,
    events: Arc<dyn ApplicationEventSink>,
    config: SyncConfig,
}

struct NotificationStats {
    delivered: usize,
    failures: usize,
}

fn notification_issue_ids(
    request: &SyncRequest,
    issues: &[jira_domain::Issue],
) -> Option<HashSet<jira_domain::IssueId>> {
    request.notification_assignees.as_deref().map(|assignees| {
        issues
            .iter()
            .filter(|issue| {
                issue
                    .assignee
                    .as_ref()
                    .is_some_and(|assignee| assignees.contains(assignee))
            })
            .map(|issue| issue.id.clone())
            .collect()
    })
}

fn updated_since(
    state: &SyncState,
    mode: SyncMode,
    overlap: Duration,
) -> Option<time::OffsetDateTime> {
    match mode {
        SyncMode::Incremental => state
            .last_incremental_succeeded_at
            .map(|cursor| cursor - overlap),
        SyncMode::Baseline | SyncMode::Reconciliation => None,
    }
}

impl SyncService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        jira: Arc<dyn JiraSyncReadPort>,
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
        let fetch_scope = self.validate(&request)?;
        cancellation.check()?;
        self.events.publish(ApplicationEvent::SyncStarted {
            site_id: request.site_id.clone(),
            user_set_id: request.user_set_id.clone(),
            mode: request.mode,
        });

        let result = self.run_inner(&request, &fetch_scope, cancellation).await;
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
        fetch_scope: &IssueFetchScope,
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
        let updated_since = updated_since(&previous_state, request.mode, self.config.overlap);
        let pagination_outcome = self
            .fetch_issues(request, fetch_scope, updated_since, cancellation)
            .await?;
        let issues = pagination_outcome.issues;
        let pages_fetched = pagination_outcome.pages_fetched;
        let server_time = pagination_outcome.server_time;
        let notification_issue_ids = notification_issue_ids(request, &issues);
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
            let update_events = self.differ.diff(ChangeSet {
                existing,
                incoming: issues.clone(),
                site_id: request.site_id.clone(),
                user_set_id: request.user_set_id.clone(),
                detected_at: cursor,
                include_removed_from_view: request.mode.replaces_membership(),
            })?;
            SyncActivityEnricher::enrich(
                self.jira.as_ref(),
                update_events,
                SyncActivityRequest {
                    existing: &existing_for_enrichment,
                    incoming: &issues,
                    site_id: &request.site_id,
                    user_set_id: &request.user_set_id,
                    notification_assignees: request.notification_assignees.as_deref(),
                    cancellation,
                },
            )
            .await?
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

        let notification_stats = self
            .deliver_notifications(
                request.mode,
                &committed.inserted_events,
                notification_issue_ids.as_ref(),
            )
            .await;

        Ok(SyncOutcome {
            mode: request.mode,
            pages_fetched,
            issues_fetched: issue_count,
            events_inserted: committed.inserted_events.len(),
            notifications_delivered: notification_stats.delivered,
            notification_failures: notification_stats.failures,
            cursor,
        })
    }

    async fn deliver_notifications(
        &self,
        mode: SyncMode,
        inserted_events: &[UpdateEvent],
        notification_issue_ids: Option<&HashSet<jira_domain::IssueId>>,
    ) -> NotificationStats {
        let mut stats = NotificationStats {
            delivered: 0,
            failures: 0,
        };
        if !mode.emits_updates() {
            return stats;
        }
        for event in inserted_events {
            let outcome = self
                .deliver_notification(event, notification_issue_ids)
                .await;
            stats.delivered += outcome.delivered;
            stats.failures += outcome.failures;
        }
        stats
    }

    async fn fetch_issues(
        &self,
        request: &SyncRequest,
        fetch_scope: &IssueFetchScope,
        updated_since: Option<time::OffsetDateTime>,
        cancellation: &CancellationToken,
    ) -> Result<crate::issue_pagination::IssuePaginationOutcome, ApplicationError> {
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
                    &fetch_scope.issue_fetch_request(
                        updated_since,
                        page_cursor,
                        self.config.page_size,
                    ),
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
        Ok(pagination.finish())
    }

    async fn deliver_notification(
        &self,
        event: &UpdateEvent,
        notification_issue_ids: Option<&HashSet<jira_domain::IssueId>>,
    ) -> NotificationStats {
        if !matches!(event.kind, UpdateKind::CommentAdded { .. })
            && notification_issue_ids.is_some_and(|ids| !ids.contains(&event.issue_id))
        {
            self.record_notification(event, NotificationDelivery::SuppressedByPolicy)
                .await;
            return NotificationStats {
                delivered: 0,
                failures: 0,
            };
        }
        if !self.notification_policy.should_notify(event) {
            self.record_notification(event, NotificationDelivery::SuppressedByPolicy)
                .await;
            return NotificationStats {
                delivered: 0,
                failures: 0,
            };
        }

        let delivery = self
            .notifications
            .deliver(NotificationRequest {
                event: event.clone(),
            })
            .await;
        match delivery {
            Ok(()) => {
                self.record_notification(event, NotificationDelivery::Delivered)
                    .await;
                NotificationStats {
                    delivered: 1,
                    failures: 0,
                }
            }
            Err(_) => {
                self.record_notification(event, NotificationDelivery::Unavailable)
                    .await;
                NotificationStats {
                    delivered: 0,
                    failures: 1,
                }
            }
        }
    }

    async fn record_notification(&self, event: &UpdateEvent, delivery: NotificationDelivery) {
        let _ = self
            .cache
            .record_notification_delivery(&event.id, delivery, self.clock.now())
            .await;
    }

    fn validate(&self, request: &SyncRequest) -> Result<IssueFetchScope, ApplicationError> {
        let fetch_scope = IssueFetchScope::new(
            request.site_id.clone(),
            request.assignees.clone(),
            request.watchers.clone(),
            request.jql_scope.clone(),
            "sync assignees must be unique",
            "sync watchers must be unique",
        )?;
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
        Ok(fetch_scope)
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
