//! Headless composition for a live Jira workspace.
//!
//! This module deliberately has no GPUI types. It owns the presentation-adapter
//! wiring needed by a shell, while the application services and storage ports
//! remain reusable by a future Tauri frontend.

use std::sync::Arc;

use jira_application::{
    AddCommentRequest, ApplicationError, CancellationToken, Clock, CommentService,
    DefaultDesktopNotificationPolicy, DefaultIssueDiffer, IssueCachePort, IssueCatalogService,
    IssueDetailConfig, IssueDetailRequest, IssueDetailService, IssueListQuery, IssueLocator,
    JiraCommentWritePort, JiraReadPort, NoopEventSink, SyncConfig, SyncMode, SyncOutcome,
    SyncRequest, SyncService, UpdateFeedQuery, UpdateFeedService, UserSetDraft, UserSetPort,
    UserSetService,
};
use jira_desktop_notifications::FreedesktopNotificationPort;
use jira_domain::{
    AccountId, EventId, Issue, IssueDetail, IssueKey, JiraSiteId, Timestamp, UpdateEvent, UserSetId,
};
use jira_storage::SqliteStore;

const WORKSPACE_NAME: &str = "Jira Desk workspace";
const ISSUE_PAGE_SIZE: usize = 1_000;
const MAX_CACHED_ISSUES: usize = 10_000;
const MAX_FEED_EVENTS: usize = 500;

/// The cache and update feed displayed by a workspace shell.
#[derive(Clone, Debug)]
pub struct CachedWorkspace {
    pub issues: Vec<Issue>,
    pub events: Vec<UpdateEvent>,
}

/// Result returned after a live synchronization and cache reload.
#[derive(Clone, Debug)]
pub struct RefreshResult {
    pub cached: CachedWorkspace,
    pub outcome: SyncOutcome,
}

/// Result returned after a local update-feed action and cache reload.
#[derive(Clone, Debug)]
pub struct FeedActionResult {
    pub cached: CachedWorkspace,
    pub changed: usize,
}

/// Presentation-independent live workspace coordinator.
pub struct LiveWorkspace {
    site_id: JiraSiteId,
    authenticated_account: Option<AccountId>,
    user_set_id: UserSetId,
    catalog: IssueCatalogService,
    feed: UpdateFeedService,
    detail: IssueDetailService,
    comments: CommentService,
    cache: Arc<SqliteStore>,
    sync: SyncService,
}

impl LiveWorkspace {
    /// Open the configured workspace, reusing its local user set when present.
    pub async fn initialize(
        site_id: JiraSiteId,
        authenticated_account: Option<AccountId>,
        jira: Arc<dyn JiraReadPort>,
        cache: Arc<SqliteStore>,
    ) -> Result<Self, ApplicationError> {
        Self::initialize_with_comment_writer(
            site_id,
            authenticated_account,
            jira,
            Arc::new(UnsupportedCommentWriter),
            cache,
        )
        .await
    }

    pub async fn initialize_with_comment_writer(
        site_id: JiraSiteId,
        authenticated_account: Option<AccountId>,
        jira: Arc<dyn JiraReadPort>,
        comment_writer: Arc<dyn JiraCommentWritePort>,
        cache: Arc<SqliteStore>,
    ) -> Result<Self, ApplicationError> {
        let members = authenticated_account.iter().cloned().collect::<Vec<_>>();
        let workspace_name = workspace_name();

        let user_sets = UserSetService::new(cache.clone() as Arc<dyn UserSetPort>);
        let existing_user_set = user_sets
            .list(&site_id)
            .await?
            .into_iter()
            .find(|user_set| {
                user_set.site_id == site_id
                    && user_set.name == workspace_name
                    && user_set.members == members
            });
        let user_set_id = match existing_user_set {
            Some(user_set) => user_set.id,
            None if members.is_empty() => {
                // UserSetService models nonempty user sets; save the deliberate
                // empty project-wide cache partition directly through the port.
                cache
                    .save(UserSetDraft {
                        site_id: site_id.clone(),
                        name: workspace_name.clone(),
                        members: Vec::new(),
                    })
                    .await?
                    .id
            }
            None => {
                user_sets
                    .save(UserSetDraft {
                        site_id: site_id.clone(),
                        name: workspace_name.clone(),
                        members,
                    })
                    .await?
                    .id
            }
        };

        let cache_port: Arc<dyn IssueCachePort> = cache.clone();
        let catalog = IssueCatalogService::new(jira.clone(), cache_port.clone());
        let detail = IssueDetailService::new(jira.clone(), IssueDetailConfig::default());
        let comments = CommentService::new(comment_writer);
        let events = Arc::new(NoopEventSink);
        let feed = UpdateFeedService::new(
            cache.clone() as Arc<dyn jira_application::UpdateFeedPort>,
            events.clone(),
        );
        let sync = SyncService::new(
            jira,
            cache_port,
            Arc::new(DefaultIssueDiffer),
            Arc::new(FreedesktopNotificationPort),
            Arc::new(DefaultDesktopNotificationPolicy),
            Arc::new(SystemClock),
            events,
            SyncConfig::default(),
        );

        Ok(Self {
            site_id,
            authenticated_account,
            user_set_id,
            catalog,
            feed,
            detail,
            comments,
            cache,
            sync,
        })
    }

    pub fn site_id(&self) -> &JiraSiteId {
        &self.site_id
    }

    pub fn authenticated_account(&self) -> Option<&AccountId> {
        self.authenticated_account.as_ref()
    }

    pub fn user_set_id(&self) -> &UserSetId {
        &self.user_set_id
    }

    /// Fetch one issue's complete read-only detail through the application service.
    pub async fn fetch_issue_detail(
        &self,
        locator: IssueLocator,
        cancellation: &CancellationToken,
    ) -> Result<IssueDetail, ApplicationError> {
        self.detail
            .fetch(
                IssueDetailRequest {
                    site_id: self.site_id.clone(),
                    locator,
                },
                cancellation,
            )
            .await
    }

    /// Create exactly one explicitly confirmed Jira comment. The application
    /// service validates the plain-text body and deliberately performs no
    /// retry after dispatch.
    pub async fn create_comment(
        &self,
        locator: IssueLocator,
        body: String,
        cancellation: &CancellationToken,
    ) -> Result<jira_domain::IssueComment, ApplicationError> {
        self.comments
            .create(
                AddCommentRequest {
                    site_id: self.site_id.clone(),
                    locator,
                    body,
                },
                cancellation,
            )
            .await
    }

    /// Look up one exact Jira key without adding it to the local cache.
    pub async fn lookup_issue(
        &self,
        key: IssueKey,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<IssueDetail, ApplicationError> {
        self.fetch_issue_detail(IssueLocator::Key(key), cancellation)
            .await
    }

    /// Load bounded cached data without contacting Jira.
    pub async fn load_cached(&self) -> Result<CachedWorkspace, ApplicationError> {
        self.load_cached_with_assignees(Vec::new()).await
    }

    /// Load the authenticated account's issues from the local cache without
    /// contacting Jira. This is a presentation filter over the project-wide
    /// cache, not a second remote synchronization.
    pub async fn load_cached_for_assignee(
        &self,
        account_id: AccountId,
    ) -> Result<CachedWorkspace, ApplicationError> {
        self.load_cached_with_assignees(vec![account_id]).await
    }

    /// Load only the authenticated user's local view. A missing identity is
    /// never treated as permission to show the project-wide cache.
    pub async fn load_cached_for_authenticated_account(
        &self,
    ) -> Result<CachedWorkspace, ApplicationError> {
        let Some(account_id) = self.authenticated_account.clone() else {
            return Err(ApplicationError::invalid_input(
                "authenticated Jira identity is required",
            ));
        };
        self.load_cached_for_assignee(account_id).await
    }

    async fn load_cached_with_assignees(
        &self,
        assignees: Vec<AccountId>,
    ) -> Result<CachedWorkspace, ApplicationError> {
        let mut issues = Vec::new();
        for offset in (0..MAX_CACHED_ISSUES).step_by(ISSUE_PAGE_SIZE) {
            let page = self
                .catalog
                .list_cached(&IssueListQuery {
                    site_id: self.site_id.clone(),
                    user_set_id: self.user_set_id.clone(),
                    text: None,
                    assignees: assignees.clone(),
                    limit: ISSUE_PAGE_SIZE,
                    offset,
                })
                .await?;
            let page_len = page.len();
            issues.extend(page);
            if page_len < ISSUE_PAGE_SIZE {
                break;
            }
        }

        let displayed_issue_ids = issues
            .iter()
            .map(|issue| issue.id.clone())
            .collect::<std::collections::HashSet<_>>();
        let events = self
            .feed
            .list(&UpdateFeedQuery {
                site_id: self.site_id.clone(),
                unread_only: false,
                kinds: Vec::new(),
                before: None,
                limit: MAX_FEED_EVENTS,
            })
            .await?
            .into_iter()
            .filter(|event| displayed_issue_ids.contains(&event.issue_id))
            .collect();
        Ok(CachedWorkspace { issues, events })
    }

    /// Synchronize Jira and reload bounded local data.
    pub async fn refresh(
        &self,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let mode = self.next_mode(SyncMode::Reconciliation).await?;
        self.refresh_with_mode(mode, cancellation).await
    }

    /// Synchronize Jira automatically, using the incremental cursor after the
    /// first successful baseline while preserving the local membership view.
    pub async fn refresh_automatically(
        &self,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let mode = self.next_mode(SyncMode::Incremental).await?;
        self.refresh_with_mode(mode, cancellation).await
    }

    async fn next_mode(&self, subsequent_mode: SyncMode) -> Result<SyncMode, ApplicationError> {
        if self
            .cache
            .sync_state(&self.site_id, &self.user_set_id)
            .await?
            .is_some_and(|state| state.last_incremental_succeeded_at.is_some())
        {
            Ok(subsequent_mode)
        } else {
            Ok(SyncMode::Baseline)
        }
    }

    async fn refresh_with_mode(
        &self,
        mode: SyncMode,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let outcome = self
            .sync
            .run(
                SyncRequest {
                    site_id: self.site_id.clone(),
                    user_set_id: self.user_set_id.clone(),
                    assignees: None,
                    notification_assignees: self
                        .authenticated_account
                        .clone()
                        .map(|account_id| vec![account_id]),
                    mode,
                },
                cancellation,
            )
            .await?;
        let cached = self.load_cached_for_authenticated_account().await?;
        Ok(RefreshResult { cached, outcome })
    }

    /// Mark every currently displayed update as read and reload local data.
    ///
    /// This action only updates the local cache; it never contacts Jira.
    pub async fn mark_all_read(&self) -> Result<FeedActionResult, ApplicationError> {
        let displayed = self.load_cached_for_authenticated_account().await?;
        let event_ids = displayed
            .events
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>();
        let changed = if event_ids.is_empty() {
            0
        } else {
            self.feed.mark_read(&self.site_id, &event_ids, true).await?
        };
        let cached = self.load_cached_for_authenticated_account().await?;
        Ok(FeedActionResult { cached, changed })
    }

    /// Set the read state for selected updates and reload local data.
    ///
    /// An empty selection is a harmless local no-op and avoids a feed mutation
    /// call while still reloading cached data. This action never contacts Jira.
    pub async fn mark_read(
        &self,
        event_ids: &[EventId],
        read: bool,
    ) -> Result<FeedActionResult, ApplicationError> {
        let displayed = self.load_cached_for_authenticated_account().await?;
        let displayed_ids = displayed
            .events
            .iter()
            .map(|event| event.id.clone())
            .collect::<std::collections::HashSet<_>>();
        if event_ids
            .iter()
            .any(|event_id| !displayed_ids.contains(event_id))
        {
            return Err(ApplicationError::invalid_input(
                "update is outside the authenticated issue view",
            ));
        }
        let changed = if event_ids.is_empty() {
            0
        } else {
            self.feed.mark_read(&self.site_id, event_ids, read).await?
        };
        let cached = self.load_cached_for_authenticated_account().await?;
        Ok(FeedActionResult { cached, changed })
    }
}

#[derive(Debug)]
struct UnsupportedCommentWriter;

impl JiraCommentWritePort for UnsupportedCommentWriter {
    fn create_comment<'a>(
        &'a self,
        _request: &'a AddCommentRequest,
        _cancellation: &'a CancellationToken,
    ) -> jira_application::PortFuture<'a, jira_domain::IssueComment> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            jira_application::ErrorKind::Internal,
            "comment creation is unavailable in this workspace",
        ))))
    }
}

fn workspace_name() -> String {
    format!("{WORKSPACE_NAME} · Jira Project")
}

#[derive(Debug)]
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now_utc()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use futures_lite::future::block_on;
    use jira_application::{
        ApplicationError, CancellationToken, ErrorKind, IssueCommentsPage,
        IssueCommentsPageRequest, IssueDetailRequest, IssueFetchRequest, IssuePage, PortFuture,
        UserSearchRequest,
    };
    use jira_domain::{
        AttachmentMetadata, IssueComment, IssueCommentAuthor, IssueDetailCore, IssueId, IssueKey,
        IssueType, JiraSiteId, NotificationDelivery, Priority, Project, Status, UpdateReadState,
        User,
    };
    use time::macros::datetime;

    use super::*;

    #[derive(Default)]
    struct FakeJira {
        pages: Mutex<VecDeque<IssuePage>>,
        detail_pages: Mutex<VecDeque<IssueDetailCore>>,
        comment_pages: Mutex<VecDeque<IssueCommentsPage>>,
        request_count: Mutex<usize>,
        assignee_filters: Mutex<Vec<Option<Vec<AccountId>>>>,
    }

    impl FakeJira {
        fn push_page(&self, page: IssuePage) {
            self.pages.lock().expect("pages lock").push_back(page);
        }

        fn push_detail(&self, detail: IssueDetailCore) {
            self.detail_pages
                .lock()
                .expect("detail pages lock")
                .push_back(detail);
        }

        fn push_comment_page(&self, page: IssueCommentsPage) {
            self.comment_pages
                .lock()
                .expect("comment pages lock")
                .push_back(page);
        }

        fn request_count(&self) -> usize {
            *self.request_count.lock().expect("request count lock")
        }

        fn assignee_filters(&self) -> Vec<Option<Vec<AccountId>>> {
            self.assignee_filters
                .lock()
                .expect("assignee filters lock")
                .clone()
        }
    }

    impl JiraReadPort for FakeJira {
        fn fetch_issue_detail<'a>(
            &'a self,
            _request: &'a IssueDetailRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssueDetailCore> {
            let detail = self
                .detail_pages
                .lock()
                .expect("detail pages lock")
                .pop_front()
                .ok_or_else(|| ApplicationError::new(ErrorKind::Upstream, "missing detail"));
            Box::pin(async move { detail })
        }

        fn fetch_issue_comments_page<'a>(
            &'a self,
            _request: &'a IssueCommentsPageRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssueCommentsPage> {
            let page = self
                .comment_pages
                .lock()
                .expect("comment pages lock")
                .pop_front()
                .ok_or_else(|| ApplicationError::new(ErrorKind::Upstream, "missing comments"));
            Box::pin(async move { page })
        }

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
            *self.request_count.lock().expect("request count lock") += 1;
            self.assignee_filters
                .lock()
                .expect("assignee filters lock")
                .push(request.assignees.clone());
            let result = self
                .pages
                .lock()
                .expect("pages lock")
                .pop_front()
                .ok_or_else(|| ApplicationError::new(ErrorKind::Upstream, "missing fake page"));
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

    fn account(value: &str) -> AccountId {
        AccountId::new(value).expect("valid account")
    }

    fn issue(summary: &str) -> Issue {
        issue_for(summary, "account-a")
    }

    fn issue_for(summary: &str, account_id: &str) -> Issue {
        let (issue_id, issue_key) = if account_id == "account-a" {
            ("10001", "APP-1")
        } else {
            ("10002", "APP-2")
        };
        Issue::new(
            JiraSiteId::new("site").expect("valid site"),
            IssueId::new(issue_id).expect("valid issue"),
            IssueKey::new(issue_key).expect("valid key"),
            Project {
                id: "10".into(),
                key: "APP".into(),
                name: "App".into(),
            },
            IssueType {
                id: "1".into(),
                name: "Task".into(),
                icon_url: None,
            },
            summary,
            Status {
                id: "1".into(),
                name: "Open".into(),
                category: None,
            },
            Priority {
                id: None,
                name: None,
                icon_url: None,
            },
            Some(account(account_id)),
            None,
            None,
            Vec::new(),
            datetime!(2026-01-01 00:00 UTC),
            datetime!(2026-01-02 00:00 UTC),
            None,
        )
    }

    fn page(issue: Issue) -> IssuePage {
        IssuePage {
            issues: vec![issue],
            next_cursor: None,
            server_time: Some(datetime!(2026-01-03 00:00 UTC)),
        }
    }

    fn make_workspace(jira: Arc<FakeJira>, cache: Arc<SqliteStore>) -> LiveWorkspace {
        block_on(LiveWorkspace::initialize(
            JiraSiteId::new("site").expect("valid site"),
            Some(account("account-a")),
            jira,
            cache,
        ))
        .expect("workspace initializes")
    }

    #[test]
    fn initialization_reuses_exact_user_set() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("memory store"));
        let first = make_workspace(jira.clone(), cache.clone());
        let second = block_on(LiveWorkspace::initialize(
            JiraSiteId::new("site").expect("valid site"),
            Some(account("account-a")),
            jira,
            cache.clone(),
        ))
        .expect("workspace initializes");
        assert_eq!(first.user_set_id(), second.user_set_id());
        assert_eq!(second.authenticated_account(), Some(&account("account-a")));
        let sets =
            block_on(cache.list(&JiraSiteId::new("site").expect("valid site"))).expect("list sets");
        assert_eq!(sets.len(), 1);
    }

    #[test]
    fn initialization_does_not_reuse_legacy_assignee_only_user_set() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("memory store"));
        let site_id = JiraSiteId::new("site").expect("valid site");
        let legacy = block_on(cache.save(UserSetDraft {
            site_id: site_id.clone(),
            name: WORKSPACE_NAME.to_owned(),
            members: vec![account("account-a"), account("account-b")],
        }))
        .expect("save legacy user set");

        let current = make_workspace(jira, cache.clone());
        assert_ne!(current.user_set_id(), &legacy.id);
        let sets = block_on(cache.list(&site_id)).expect("list sets");
        assert_eq!(sets.len(), 2);
        assert!(sets.iter().any(|set| set.name.contains("Jira Project")));
    }

    #[test]
    fn baseline_persists_issues_without_events_and_reconciliation_derives_changes() {
        let jira = Arc::new(FakeJira::default());
        jira.push_page(page(issue("Initial summary")));
        jira.push_page(page(issue("Changed summary")));
        let workspace = make_workspace(jira, Arc::new(SqliteStore::in_memory().expect("store")));
        let cancellation = CancellationToken::new();

        let baseline = block_on(workspace.refresh(&cancellation)).expect("baseline refresh");
        assert_eq!(baseline.outcome.mode, SyncMode::Baseline);
        assert_eq!(baseline.outcome.events_inserted, 0);
        assert_eq!(baseline.cached.issues.len(), 1);
        assert!(baseline.cached.events.is_empty());

        let reconciliation = block_on(workspace.refresh(&cancellation)).expect("reconciliation");
        assert_eq!(reconciliation.outcome.mode, SyncMode::Reconciliation);
        assert_eq!(reconciliation.outcome.events_inserted, 1);
        assert_eq!(reconciliation.cached.events.len(), 1);
        assert_eq!(
            reconciliation.cached.events[0].notification_delivery,
            NotificationDelivery::SuppressedByPolicy
        );
    }

    #[test]
    fn automatic_refresh_uses_incremental_cursor_without_removing_cached_issues() {
        let jira = Arc::new(FakeJira::default());
        jira.push_page(page(issue("Initial summary")));
        jira.push_page(IssuePage {
            issues: Vec::new(),
            next_cursor: None,
            server_time: Some(datetime!(2026-01-04 00:00 UTC)),
        });
        let workspace = make_workspace(jira, Arc::new(SqliteStore::in_memory().expect("store")));
        let cancellation = CancellationToken::new();

        let baseline =
            block_on(workspace.refresh_automatically(&cancellation)).expect("automatic baseline");
        assert_eq!(baseline.outcome.mode, SyncMode::Baseline);
        assert_eq!(baseline.outcome.events_inserted, 0);
        assert!(baseline.cached.events.is_empty());

        let incremental = block_on(workspace.refresh_automatically(&cancellation))
            .expect("automatic incremental refresh");
        assert_eq!(incremental.outcome.mode, SyncMode::Incremental);
        assert_eq!(incremental.outcome.events_inserted, 0);
        assert_eq!(incremental.cached.issues.len(), 1);
        assert_eq!(incremental.cached.issues[0].summary, "Initial summary");
    }

    #[test]
    fn feed_actions_are_local_and_persist_read_state() {
        let jira = Arc::new(FakeJira::default());
        jira.push_page(page(issue("Initial summary")));
        jira.push_page(page(issue("Changed summary")));
        let cache = Arc::new(SqliteStore::in_memory().expect("store"));
        let workspace = make_workspace(jira.clone(), cache);
        let cancellation = CancellationToken::new();

        block_on(workspace.refresh(&cancellation)).expect("baseline refresh");
        let reconciliation = block_on(workspace.refresh(&cancellation)).expect("reconciliation");
        let event_id = reconciliation.cached.events[0].id.clone();
        assert_eq!(
            reconciliation.cached.events[0].read_state,
            UpdateReadState::Unread
        );
        let requests_after_refresh = jira.request_count();

        let marked_read = block_on(workspace.mark_read(std::slice::from_ref(&event_id), true))
            .expect("mark read");
        assert_eq!(marked_read.changed, 1);
        assert_eq!(
            marked_read.cached.events[0].read_state,
            UpdateReadState::Read
        );

        let marked_read_again =
            block_on(workspace.mark_read(std::slice::from_ref(&event_id), true)).expect("no-op");
        assert_eq!(marked_read_again.changed, 0);
        assert_eq!(
            marked_read_again.cached.events[0].read_state,
            UpdateReadState::Read
        );

        let marked_unread = block_on(workspace.mark_read(std::slice::from_ref(&event_id), false))
            .expect("mark unread");
        assert_eq!(marked_unread.changed, 1);
        assert_eq!(
            marked_unread.cached.events[0].read_state,
            UpdateReadState::Unread
        );

        let marked_all_read = block_on(workspace.mark_all_read()).expect("mark all read");
        assert_eq!(marked_all_read.changed, 1);
        assert_eq!(
            marked_all_read.cached.events[0].read_state,
            UpdateReadState::Read
        );

        let empty = block_on(workspace.mark_read(&[], false)).expect("empty selection");
        assert_eq!(empty.changed, 0);
        assert_eq!(empty.cached.events[0].read_state, UpdateReadState::Read);
        assert_eq!(
            block_on(workspace.load_cached())
                .expect("reload persisted feed")
                .events[0]
                .read_state,
            UpdateReadState::Read
        );
        assert_eq!(jira.request_count(), requests_after_refresh);
    }

    #[test]
    fn mark_read_rejects_events_outside_authenticated_issue_view() {
        let jira = Arc::new(FakeJira::default());
        let workspace = make_workspace(jira, Arc::new(SqliteStore::in_memory().expect("store")));
        let event_id = EventId::new("not-displayed").expect("event");

        let error = block_on(workspace.mark_read(&[event_id], true)).expect_err("scope error");

        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn cached_loading_does_not_contact_jira() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("store"));
        let workspace = make_workspace(jira.clone(), cache);
        let cached = block_on(workspace.load_cached()).expect("cached load");
        assert!(cached.issues.is_empty());
        assert!(cached.events.is_empty());
        assert_eq!(jira.request_count(), 0);
    }

    #[test]
    fn my_issue_cache_filter_does_not_contact_jira() {
        let jira = Arc::new(FakeJira::default());
        jira.push_page(IssuePage {
            issues: vec![
                issue("Initial summary"),
                issue_for("Other account summary", "account-b"),
            ],
            next_cursor: None,
            server_time: Some(datetime!(2026-01-03 00:00 UTC)),
        });
        jira.push_page(IssuePage {
            issues: vec![
                issue("Changed account-a summary"),
                issue_for("Changed account-b summary", "account-b"),
            ],
            next_cursor: None,
            server_time: Some(datetime!(2026-01-04 00:00 UTC)),
        });
        let workspace = make_workspace(
            jira.clone(),
            Arc::new(SqliteStore::in_memory().expect("store")),
        );
        let cancellation = CancellationToken::new();
        block_on(workspace.refresh(&cancellation)).expect("baseline refresh");
        let _reconciliation =
            block_on(workspace.refresh(&cancellation)).expect("reconciliation refresh");
        let requests_after_sync = jira.request_count();
        assert_eq!(jira.assignee_filters(), vec![None, None]);

        let all = block_on(workspace.load_cached()).expect("load all local issues");
        assert_eq!(all.issues.len(), 2);

        let mine = block_on(workspace.load_cached_for_assignee(account("account-a")))
            .expect("load local my filter");
        assert_eq!(mine.issues.len(), 1);
        assert!(
            mine.events
                .iter()
                .all(|event| event.issue_id == mine.issues[0].id)
        );
        assert!(
            all.events
                .iter()
                .any(|event| event.issue_id != mine.issues[0].id)
        );
        assert_eq!(jira.request_count(), requests_after_sync);
    }

    #[test]
    fn issue_detail_fetches_core_comments_and_attachment_metadata_through_workspace() {
        let jira = Arc::new(FakeJira::default());
        let mut detailed_issue = issue("Detailed summary");
        detailed_issue.description_text = Some("Detailed description".to_owned());
        jira.push_detail(
            IssueDetailCore::new(
                detailed_issue,
                vec![
                    AttachmentMetadata::new("attachment-1", "notes.txt", 42, Some("text/plain"))
                        .expect("attachment"),
                ],
            )
            .expect("detail core"),
        );
        jira.push_comment_page(IssueCommentsPage {
            comments: vec![
                IssueComment::new(
                    "comment-1",
                    Some(
                        IssueCommentAuthor::new(account("account-a"), None::<String>)
                            .expect("author"),
                    ),
                    "A complete comment",
                    datetime!(2026-01-03 00:00 UTC),
                    None,
                    Vec::new(),
                )
                .expect("comment"),
            ],
            start_at: 0,
            next_start_at: None,
            next_cursor: None,
            total: Some(1),
        });
        let workspace = make_workspace(jira, Arc::new(SqliteStore::in_memory().expect("store")));

        let detail = block_on(workspace.fetch_issue_detail(
            IssueLocator::Id(IssueId::new("10001").expect("issue")),
            &CancellationToken::new(),
        ))
        .expect("issue detail");

        assert_eq!(
            detail.core.issue.description_text.as_deref(),
            Some("Detailed description")
        );
        assert_eq!(detail.comments.len(), 1);
        assert_eq!(detail.comments[0].body, "A complete comment");
        assert_eq!(detail.core.attachments[0].filename, "notes.txt");
    }

    #[test]
    fn project_wide_workspace_has_no_authenticated_account() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("store"));
        let result = block_on(LiveWorkspace::initialize(
            JiraSiteId::new("site").expect("valid site"),
            None,
            jira,
            cache.clone(),
        ));
        let workspace = result.expect("project-wide workspace initializes");
        assert!(workspace.authenticated_account().is_none());
        assert_eq!(
            block_on(cache.list(&JiraSiteId::new("site").expect("valid site")))
                .expect("list sets")
                .len(),
            1
        );
    }

    #[test]
    fn failed_first_refresh_records_state_but_next_success_is_baseline() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("store"));
        let workspace = make_workspace(jira.clone(), cache);
        let cancellation = CancellationToken::new();

        let first = block_on(workspace.refresh(&cancellation));
        assert!(matches!(
            first,
            Err(error) if error.kind() == ErrorKind::Upstream
        ));

        jira.push_page(page(issue("Recovered summary")));
        let second = block_on(workspace.refresh(&cancellation)).expect("baseline retry");
        assert_eq!(second.outcome.mode, SyncMode::Baseline);
        assert_eq!(second.outcome.events_inserted, 0);
        assert!(second.cached.events.is_empty());
    }
}
