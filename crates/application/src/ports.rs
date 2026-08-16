use std::{future::Future, pin::Pin};

use jira_domain::{
    EventId, Issue, IssueId, JiraSiteId, NotificationDelivery, Timestamp, UpdateEvent, User,
    UserSet, UserSetId,
};

use crate::{
    ApplicationError, ApplicationEvent, CancellationToken, ChangeSet, CommitOutcome,
    IssueFetchRequest, IssueListQuery, IssuePage, NotificationRequest, SyncCommit, SyncState,
    UpdateFeedQuery, UserSearchRequest, UserSetDraft,
};

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// Read-only Jira gateway. Its implementation may own any async runtime it needs.
pub trait JiraReadPort: Send + Sync {
    fn search_users<'a>(
        &'a self,
        request: &'a UserSearchRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<User>>;

    fn fetch_issue_page<'a>(
        &'a self,
        request: &'a IssueFetchRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssuePage>;

    fn fetch_issues_by_id<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_ids: &'a [IssueId],
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<Issue>>;
}

/// Cache operations required by issue browsing and synchronization.
pub trait IssueCachePort: Send + Sync {
    fn list_issues<'a>(&'a self, query: &'a IssueListQuery) -> PortFuture<'a, Vec<Issue>>;

    fn get_issue<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_id: &'a IssueId,
    ) -> PortFuture<'a, Option<Issue>>;

    fn issues_for_user_set<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
    ) -> PortFuture<'a, Vec<Issue>>;

    fn sync_state<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
    ) -> PortFuture<'a, Option<SyncState>>;

    /// Must atomically upsert snapshots, update membership, insert deduplicated
    /// events, and persist the successful cursor.
    fn commit_sync<'a>(&'a self, commit: SyncCommit) -> PortFuture<'a, CommitOutcome>;

    fn record_sync_failure<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
        kind: crate::ErrorKind,
        at: Timestamp,
    ) -> PortFuture<'a, ()>;

    fn record_notification_delivery<'a>(
        &'a self,
        event_id: &'a EventId,
        delivery: NotificationDelivery,
        at: Timestamp,
    ) -> PortFuture<'a, ()>;
}

pub trait IssueDiffer: Send + Sync {
    fn diff(&self, change_set: ChangeSet) -> Result<Vec<UpdateEvent>, ApplicationError>;
}

pub trait UpdateFeedPort: Send + Sync {
    fn list<'a>(&'a self, query: &'a UpdateFeedQuery) -> PortFuture<'a, Vec<UpdateEvent>>;
    fn unread_count<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize>;
    fn mark_read<'a>(&'a self, event_ids: &'a [EventId], read: bool) -> PortFuture<'a, usize>;
    fn mark_all_read<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize>;
}

pub trait UserSetPort: Send + Sync {
    fn list<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, Vec<UserSet>>;
    fn save<'a>(&'a self, draft: UserSetDraft) -> PortFuture<'a, UserSet>;
    fn delete<'a>(&'a self, user_set_id: &'a UserSetId) -> PortFuture<'a, ()>;
}

pub trait NotificationPort: Send + Sync {
    fn deliver<'a>(&'a self, request: NotificationRequest) -> PortFuture<'a, ()>;
}

pub trait NotificationPolicy: Send + Sync {
    fn should_notify(&self, event: &UpdateEvent) -> bool;
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// Bridges background use cases to a GPUI/Tauri foreground dispatcher.
pub trait ApplicationEventSink: Send + Sync {
    fn publish(&self, event: ApplicationEvent);
}

#[derive(Debug, Default)]
pub struct NoopEventSink;

impl ApplicationEventSink for NoopEventSink {
    fn publish(&self, _event: ApplicationEvent) {}
}
