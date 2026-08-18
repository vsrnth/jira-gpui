use std::{future::Future, pin::Pin};

use jira_domain::{
    EventId, Issue, IssueId, JiraSiteId, NotificationDelivery, Timestamp, UpdateEvent, User,
    UserSet, UserSetId,
};

use crate::{
    AddCommentRequest, ApplicationError, ApplicationEvent, AssignIssueRequest,
    AssignableUserSearchRequest, AttachmentContent, AttachmentDownloadRequest, AttachmentImage,
    AttachmentImageRequest, CachedAssignableUsers, CachedIssueTransitions, CancellationToken,
    ChangeSet, CommitOutcome, IssueChangelog, IssueChangelogRequest, IssueCommentsPage,
    IssueCommentsPageRequest, IssueDetailRequest, IssueFetchRequest, IssueListQuery, IssuePage,
    IssueTransition, IssueTransitionsRequest, NotificationRequest, SyncCommit, SyncState,
    TransitionIssueRequest, UpdateFeedQuery, UserSearchRequest, UserSetDraft,
};

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// Read-only Jira gateway. Its implementation may own any async runtime it needs.
pub trait JiraReadPort: Send + Sync {
    /// Fetches bounded bulk changelog data for changed issue snapshots. Older
    /// gateways may leave this unsupported; synchronization treats that as a
    /// best-effort enrichment miss and retains its generic fallback event.
    fn fetch_issue_changelog<'a>(
        &'a self,
        _request: &'a IssueChangelogRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<IssueChangelog>> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            crate::ErrorKind::Internal,
            "issue changelog is not supported by this Jira gateway",
        ))))
    }

    /// Fetches the original bytes of one authenticated attachment. The default preserves
    /// compatibility with gateways that do not implement attachment downloads.
    fn fetch_attachment_content<'a>(
        &'a self,
        _request: &'a AttachmentDownloadRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentContent> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            crate::ErrorKind::Internal,
            "attachment downloads are not supported by this Jira gateway",
        ))))
    }

    /// Fetches one authenticated image attachment thumbnail. The default preserves compatibility
    /// with gateways that do not implement issue media reads.
    fn fetch_attachment_image<'a>(
        &'a self,
        _request: &'a AttachmentImageRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentImage> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            crate::ErrorKind::Internal,
            "attachment images are not supported by this Jira gateway",
        ))))
    }

    /// Fetches the typed core payload for one issue. Adapters may override this when issue-detail
    /// support is available; the default preserves compatibility with existing gateways.
    fn fetch_issue_detail<'a>(
        &'a self,
        _request: &'a IssueDetailRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, jira_domain::IssueDetailCore> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            crate::ErrorKind::Internal,
            "issue detail is not supported by this Jira gateway",
        ))))
    }

    /// Fetches one comments page. Adapters may override this when issue-detail support is
    /// available; the default preserves compatibility with existing gateways.
    fn fetch_issue_comments_page<'a>(
        &'a self,
        _request: &'a IssueCommentsPageRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssueCommentsPage> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            crate::ErrorKind::Internal,
            "issue comments are not supported by this Jira gateway",
        ))))
    }

    fn fetch_current_user<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, User>;

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

/// Dedicated confirmed Jira comment-write boundary. Implementations must issue
/// exactly one explicit comment creation request and must not retry it
/// automatically.
pub trait JiraCommentWritePort: Send + Sync {
    fn create_comment<'a>(
        &'a self,
        request: &'a AddCommentRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, jira_domain::IssueComment>;
}

/// Explicit issue-edit boundary. Every write method represents one already
/// confirmed user action and must dispatch exactly once; implementations must
/// not retry after handing a request to Jira.
pub trait JiraIssueEditPort: Send + Sync {
    fn search_assignable_users<'a>(
        &'a self,
        request: &'a AssignableUserSearchRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<jira_domain::User>>;

    fn fetch_issue_transitions<'a>(
        &'a self,
        request: &'a IssueTransitionsRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<IssueTransition>>;

    /// Dispatch one confirmed assignment exactly once. An `UnknownOutcome`
    /// error must be returned unchanged when Jira's response is uncertain.
    fn assign_issue<'a>(
        &'a self,
        request: &'a AssignIssueRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, ()>;

    /// Dispatch one confirmed transition exactly once. An `UnknownOutcome`
    /// error must be returned unchanged when Jira's response is uncertain.
    fn transition_issue<'a>(
        &'a self,
        request: &'a TransitionIssueRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, ()>;
}

/// Persistent cache for issue-scoped edit metadata. Implementations must make
/// each replacement atomic so readers never observe a partially written list.
/// The locator kind is part of the key: an issue ID and an issue key are never
/// allowed to collide accidentally.
pub trait IssueEditCachePort: Send + Sync {
    fn cached_assignable_users<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a crate::IssueLocator,
    ) -> PortFuture<'a, Option<CachedAssignableUsers>>;

    fn replace_assignable_users<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a crate::IssueLocator,
        users: Vec<User>,
        fetched_at: Timestamp,
    ) -> PortFuture<'a, ()>;

    fn cached_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a crate::IssueLocator,
    ) -> PortFuture<'a, Option<CachedIssueTransitions>>;

    fn replace_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a crate::IssueLocator,
        transitions: Vec<IssueTransition>,
        fetched_at: Timestamp,
    ) -> PortFuture<'a, ()>;

    /// Remove only the transition list for this exact site and locator.
    fn invalidate_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a crate::IssueLocator,
    ) -> PortFuture<'a, ()>;
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
