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
    IssueTransition, IssueTransitionsRequest, NotificationRequest, RecentIssueCommentsRequest,
    SyncCommit, SyncState, TransitionIssueRequest, UpdateFeedQuery, UserSearchRequest,
    UserSetDraft,
};

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// Read-only Jira issue search capability. Its implementation may own any async runtime it needs.
pub trait JiraIssueSearchPort: Send + Sync {
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

/// Read-only Jira issue activity capability.
pub trait JiraIssueActivityPort: Send + Sync {
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

    /// Fetches the newest bounded comments for sync-time mention enrichment.
    /// Older gateways may leave this unsupported; sync treats unsupported
    /// enrichment as a best-effort miss.
    fn fetch_recent_issue_comments<'a>(
        &'a self,
        _request: &'a RecentIssueCommentsRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<jira_domain::IssueComment>> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            crate::ErrorKind::Internal,
            "recent issue comments are not supported by this Jira gateway",
        ))))
    }
}

/// Combined read capability required by synchronization.
pub trait JiraSyncReadPort: JiraIssueSearchPort + JiraIssueActivityPort {}

/// Read-only Jira issue-detail capability.
pub trait JiraIssueDetailReadPort: Send + Sync {
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
}

/// Read-only Jira attachment capability.
pub trait JiraAttachmentReadPort: Send + Sync {
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
}

/// Read-only Jira user capability.
pub trait JiraUserReadPort: Send + Sync {
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
}

/// Aggregate read-only Jira gateway retained to preserve consumer construction and trait-object
/// upcast compatibility within this unpublished workspace. Legacy implementors must migrate to
/// capability impls; this aggregate does not preserve implementor source compatibility.
pub trait JiraReadPort:
    JiraSyncReadPort + JiraIssueDetailReadPort + JiraAttachmentReadPort + JiraUserReadPort
{
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
    /// Replace one existing cached issue with its detail-enriched snapshot.
    /// This never changes memberships, events, or synchronization cursors.
    fn cache_detail_issue<'a>(&'a self, issue: &'a Issue) -> PortFuture<'a, bool>;

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{ErrorKind, IssueLocator, test_support::block_on};

    struct ContractRead;

    impl JiraIssueSearchPort for ContractRead {
        fn fetch_issue_page<'a>(
            &'a self,
            _request: &'a IssueFetchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            Box::pin(std::future::ready(Err(ApplicationError::new(
                ErrorKind::Internal,
                "unused search capability",
            ))))
        }

        fn fetch_issues_by_id<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _issue_ids: &'a [IssueId],
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<Issue>> {
            Box::pin(std::future::ready(Err(ApplicationError::new(
                ErrorKind::Internal,
                "unused search capability",
            ))))
        }
    }

    impl JiraIssueActivityPort for ContractRead {}
    impl JiraSyncReadPort for ContractRead {}
    impl JiraIssueDetailReadPort for ContractRead {}
    impl JiraAttachmentReadPort for ContractRead {}

    impl JiraUserReadPort for ContractRead {
        fn fetch_current_user<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, User> {
            Box::pin(std::future::ready(Err(ApplicationError::new(
                ErrorKind::Internal,
                "unused user capability",
            ))))
        }

        fn search_users<'a>(
            &'a self,
            _request: &'a UserSearchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<User>> {
            Box::pin(std::future::ready(Err(ApplicationError::new(
                ErrorKind::Internal,
                "unused user capability",
            ))))
        }
    }

    impl JiraReadPort for ContractRead {}

    fn site() -> JiraSiteId {
        JiraSiteId::new("site").expect("site")
    }

    fn issue_id() -> IssueId {
        IssueId::new("100").expect("issue")
    }

    fn assert_unsupported<T: std::fmt::Debug>(result: Result<T, ApplicationError>, message: &str) {
        let error = result.expect_err("capability should retain its unsupported default");
        assert_eq!(error.kind(), ErrorKind::Internal);
        assert_eq!(error.message(), message);
    }

    #[test]
    fn optional_capability_defaults_preserve_current_errors() {
        let port = ContractRead;
        let cancellation = CancellationToken::new();
        let site_id = site();
        let issue_id = issue_id();

        assert_unsupported(
            block_on(port.fetch_issue_changelog(
                &IssueChangelogRequest {
                    site_id: site_id.clone(),
                    issue_ids: vec![issue_id.clone()],
                },
                &cancellation,
            )),
            "issue changelog is not supported by this Jira gateway",
        );
        assert_unsupported(
            block_on(port.fetch_recent_issue_comments(
                &RecentIssueCommentsRequest {
                    site_id: site_id.clone(),
                    issue_id: issue_id.clone(),
                    limit: 1,
                },
                &cancellation,
            )),
            "recent issue comments are not supported by this Jira gateway",
        );
        assert_unsupported(
            block_on(port.fetch_issue_detail(
                &IssueDetailRequest {
                    site_id: site_id.clone(),
                    locator: IssueLocator::Id(issue_id.clone()),
                },
                &cancellation,
            )),
            "issue detail is not supported by this Jira gateway",
        );
        assert_unsupported(
            block_on(port.fetch_issue_comments_page(
                &IssueCommentsPageRequest {
                    site_id: site_id.clone(),
                    issue_id: issue_id.clone(),
                    start_at: 0,
                    page_cursor: None,
                    page_size: 1,
                },
                &cancellation,
            )),
            "issue comments are not supported by this Jira gateway",
        );
        assert_unsupported(
            block_on(port.fetch_attachment_content(
                &AttachmentDownloadRequest {
                    site_id: site_id.clone(),
                    issue_id: issue_id.clone(),
                    attachment_id: "attachment".to_owned(),
                    max_bytes: 1,
                },
                &cancellation,
            )),
            "attachment downloads are not supported by this Jira gateway",
        );
        assert_unsupported(
            block_on(port.fetch_attachment_image(
                &AttachmentImageRequest {
                    site_id,
                    issue_id,
                    attachment_id: "attachment".to_owned(),
                    width: 1,
                    height: 1,
                    max_bytes: 1,
                },
                &cancellation,
            )),
            "attachment images are not supported by this Jira gateway",
        );
    }

    #[test]
    fn aggregate_upcasts_to_each_capability() {
        let aggregate: Arc<dyn JiraReadPort> = Arc::new(ContractRead);
        let _: Arc<dyn JiraSyncReadPort> = aggregate.clone();
        let _: Arc<dyn JiraIssueSearchPort> = aggregate.clone();
        let _: Arc<dyn JiraIssueActivityPort> = aggregate.clone();
        let _: Arc<dyn JiraIssueDetailReadPort> = aggregate.clone();
        let _: Arc<dyn JiraAttachmentReadPort> = aggregate.clone();
        let _: Arc<dyn JiraUserReadPort> = aggregate;
    }
}
