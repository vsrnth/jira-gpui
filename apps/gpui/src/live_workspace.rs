//! Headless composition for a live Jira workspace.
//!
//! This module deliberately has no GPUI types. It owns the presentation-adapter
//! wiring needed by a shell, while the application services and storage ports
//! remain reusable by a future Tauri frontend.

use std::{collections::HashSet, sync::Arc};

use jira_application::{
    ApplicationError, Clock, DefaultDesktopNotificationPolicy, DefaultIssueDiffer, IssueCachePort,
    IssueCatalogService, IssueListQuery, JiraReadPort, NoopEventSink, SyncConfig, SyncMode,
    SyncOutcome, SyncRequest, SyncService, UpdateFeedQuery, UpdateFeedService, UserSetDraft,
    UserSetPort, UserSetService,
};
use jira_desktop_notifications::FreedesktopNotificationPort;
use jira_domain::{AccountId, EventId, Issue, JiraSiteId, Timestamp, UpdateEvent, UserSetId};
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
    assignees: Vec<AccountId>,
    user_set_id: UserSetId,
    catalog: IssueCatalogService,
    feed: UpdateFeedService,
    cache: Arc<SqliteStore>,
    sync: SyncService,
}

impl LiveWorkspace {
    /// Open the configured workspace, reusing its local user set when present.
    pub async fn initialize(
        site_id: JiraSiteId,
        assignees: Vec<AccountId>,
        jira: Arc<dyn JiraReadPort>,
        cache: Arc<SqliteStore>,
    ) -> Result<Self, ApplicationError> {
        validate_assignees(&assignees)?;
        let mut assignees = assignees;
        assignees.sort();

        let user_sets = UserSetService::new(cache.clone() as Arc<dyn UserSetPort>);
        let user_set_id = match user_sets
            .list(&site_id)
            .await?
            .into_iter()
            .find(|user_set| {
                user_set.site_id == site_id
                    && user_set.name == WORKSPACE_NAME
                    && user_set.members == assignees
            }) {
            Some(user_set) => user_set.id,
            None => {
                user_sets
                    .save(UserSetDraft {
                        site_id: site_id.clone(),
                        name: WORKSPACE_NAME.to_owned(),
                        members: assignees.clone(),
                    })
                    .await?
                    .id
            }
        };

        let cache_port: Arc<dyn IssueCachePort> = cache.clone();
        let catalog = IssueCatalogService::new(jira.clone(), cache_port.clone());
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
            assignees,
            user_set_id,
            catalog,
            feed,
            cache,
            sync,
        })
    }

    pub fn site_id(&self) -> &JiraSiteId {
        &self.site_id
    }

    pub fn assignees(&self) -> &[AccountId] {
        &self.assignees
    }

    pub fn user_set_id(&self) -> &UserSetId {
        &self.user_set_id
    }

    /// Load bounded cached data without contacting Jira.
    pub async fn load_cached(&self) -> Result<CachedWorkspace, ApplicationError> {
        let mut issues = Vec::new();
        for offset in (0..MAX_CACHED_ISSUES).step_by(ISSUE_PAGE_SIZE) {
            let page = self
                .catalog
                .list_cached(&IssueListQuery {
                    site_id: self.site_id.clone(),
                    user_set_id: self.user_set_id.clone(),
                    text: None,
                    assignees: Vec::new(),
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

        let events = self
            .feed
            .list(&UpdateFeedQuery {
                site_id: self.site_id.clone(),
                unread_only: false,
                kinds: Vec::new(),
                before: None,
                limit: MAX_FEED_EVENTS,
            })
            .await?;
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
                    assignees: self.assignees.clone(),
                    mode,
                },
                cancellation,
            )
            .await?;
        let cached = self.load_cached().await?;
        Ok(RefreshResult { cached, outcome })
    }

    /// Mark every update in this workspace's site as read and reload local data.
    ///
    /// This action only updates the local cache; it never contacts Jira.
    pub async fn mark_all_read(&self) -> Result<FeedActionResult, ApplicationError> {
        let changed = self.feed.mark_all_read(&self.site_id).await?;
        let cached = self.load_cached().await?;
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
        let changed = if event_ids.is_empty() {
            0
        } else {
            self.feed.mark_read(&self.site_id, event_ids, read).await?
        };
        let cached = self.load_cached().await?;
        Ok(FeedActionResult { cached, changed })
    }
}

fn validate_assignees(assignees: &[AccountId]) -> Result<(), ApplicationError> {
    if assignees.is_empty() {
        return Err(ApplicationError::invalid_input(
            "workspace requires at least one assignee",
        ));
    }
    if assignees.iter().collect::<HashSet<_>>().len() != assignees.len() {
        return Err(ApplicationError::invalid_input(
            "workspace assignees must be unique",
        ));
    }
    Ok(())
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
        ApplicationError, CancellationToken, ErrorKind, IssueFetchRequest, IssuePage, PortFuture,
        UserSearchRequest,
    };
    use jira_domain::{
        IssueId, IssueKey, IssueType, JiraSiteId, NotificationDelivery, Priority, Project, Status,
        UpdateReadState, User,
    };
    use time::macros::datetime;

    use super::*;

    #[derive(Default)]
    struct FakeJira {
        pages: Mutex<VecDeque<IssuePage>>,
        request_count: Mutex<usize>,
    }

    impl FakeJira {
        fn push_page(&self, page: IssuePage) {
            self.pages.lock().expect("pages lock").push_back(page);
        }

        fn request_count(&self) -> usize {
            *self.request_count.lock().expect("request count lock")
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
            _request: &'a IssueFetchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            *self.request_count.lock().expect("request count lock") += 1;
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
        Issue::new(
            JiraSiteId::new("site").expect("valid site"),
            IssueId::new("10001").expect("valid issue"),
            IssueKey::new("APP-1").expect("valid key"),
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
            Some(account("account-a")),
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
            vec![account("account-a"), account("account-b")],
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
            vec![account("account-b"), account("account-a")],
            jira,
            cache.clone(),
        ))
        .expect("workspace initializes");
        assert_eq!(first.user_set_id(), second.user_set_id());
        assert_eq!(
            second.assignees(),
            &[account("account-a"), account("account-b")]
        );
        let sets =
            block_on(cache.list(&JiraSiteId::new("site").expect("valid site"))).expect("list sets");
        assert_eq!(sets.len(), 1);
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
    fn invalid_assignee_selection_is_rejected_before_persistence() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("store"));
        let result = block_on(LiveWorkspace::initialize(
            JiraSiteId::new("site").expect("valid site"),
            vec![account("account-a"), account("account-a")],
            jira,
            cache.clone(),
        ));
        assert!(matches!(
            result,
            Err(error) if error.kind() == ErrorKind::InvalidInput
        ));
        assert!(
            block_on(cache.list(&JiraSiteId::new("site").expect("valid site")))
                .expect("list sets")
                .is_empty()
        );

        let empty_result = block_on(LiveWorkspace::initialize(
            JiraSiteId::new("site").expect("valid site"),
            Vec::new(),
            Arc::new(FakeJira::default()),
            cache,
        ));
        assert!(matches!(
            empty_result,
            Err(error) if error.kind() == ErrorKind::InvalidInput
        ));
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
