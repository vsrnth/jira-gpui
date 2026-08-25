//! Headless composition for a live Jira workspace.
//!
//! This module deliberately has no GPUI types. It owns the presentation-adapter
//! wiring needed by a shell, while the application services and storage ports
//! remain reusable by a future Tauri frontend.

use std::sync::{Arc, RwLock};

use jira_application::{
    AddCommentRequest, ApplicationError, AssignIssueRequest, AssignableUserSearchRequest,
    AttachmentContent, AttachmentDownloadRequest, AttachmentImage, AttachmentImageRequest,
    CancellationToken, Clock, CommentService, DEFAULT_JQL_SCOPE, DefaultDesktopNotificationPolicy,
    DefaultIssueDiffer, IssueCachePort, IssueCatalogService, IssueDetailConfig, IssueDetailRequest,
    IssueDetailService, IssueEditCachePort, IssueEditService, IssueListQuery, IssueLocator,
    IssueMediaConfig, IssueMediaService, IssueTransitionsRequest, JiraCommentWritePort,
    JiraIssueEditPort, JiraReadPort, NoopEventSink, SyncConfig, SyncMode, SyncOutcome, SyncRequest,
    SyncService, TransitionIssueRequest, UpdateFeedQuery, UpdateFeedService, UserSearchRequest,
    UserSetDraft, UserSetPort, UserSetService, validate_jql_scope,
};
use jira_desktop_notifications::FreedesktopNotificationPort;
use jira_domain::{
    AccountId, EventId, Issue, IssueDetail, IssueKey, JiraSiteId, Timestamp, UpdateEvent, User,
    UserSetId,
};
use jira_storage::SqliteStore;

const WORKSPACE_NAME: &str = "Jira Desk workspace";
const TEAM_WORKSPACE_NAME: &str = "Jira Desk workspace · configured team view";
const TEAM_STATUS_SCOPE: &str = "statusCategory = \"In Progress\"";
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
    scope_state: Arc<RwLock<ScopeState>>,
    team_state: Arc<RwLock<TeamState>>,
    catalog: IssueCatalogService,
    feed: UpdateFeedService,
    detail: IssueDetailService,
    media: IssueMediaService,
    comments: CommentService,
    issue_editor: IssueEditService,
    cache: Arc<SqliteStore>,
    sync: SyncService,
    notification_port: Arc<FreedesktopNotificationPort>,
}

#[derive(Clone, Debug)]
struct ScopeState {
    user_set_id: UserSetId,
    normalized_scope: String,
}

#[derive(Clone, Debug)]
struct TeamState {
    user_set_id: UserSetId,
    members: Vec<AccountId>,
}

impl LiveWorkspace {
    /// Send a fixed local desktop-notification diagnostic. This intentionally
    /// bypasses Jira, synchronization, and the local update feed.
    pub async fn test_desktop_notification(
        &self,
    ) -> Result<jira_desktop_notifications::DesktopNotificationReceipt, ApplicationError> {
        self.notification_port.test_notification().await
    }

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
        Self::initialize_with_writers(
            site_id,
            authenticated_account,
            jira,
            comment_writer,
            Arc::new(UnsupportedIssueEditor),
            cache,
        )
        .await
    }

    pub async fn initialize_with_writers(
        site_id: JiraSiteId,
        authenticated_account: Option<AccountId>,
        jira: Arc<dyn JiraReadPort>,
        comment_writer: Arc<dyn JiraCommentWritePort>,
        issue_editor: Arc<dyn JiraIssueEditPort>,
        cache: Arc<SqliteStore>,
    ) -> Result<Self, ApplicationError> {
        Self::initialize_with_writers_and_scope(
            site_id,
            authenticated_account,
            jira,
            comment_writer,
            issue_editor,
            cache,
            None,
        )
        .await
    }

    pub async fn initialize_with_writers_and_scope(
        site_id: JiraSiteId,
        authenticated_account: Option<AccountId>,
        jira: Arc<dyn JiraReadPort>,
        comment_writer: Arc<dyn JiraCommentWritePort>,
        issue_editor: Arc<dyn JiraIssueEditPort>,
        cache: Arc<SqliteStore>,
        requested_scope: Option<String>,
    ) -> Result<Self, ApplicationError> {
        let members = authenticated_account.iter().cloned().collect::<Vec<_>>();
        let normalized_scope = normalize_scope(requested_scope.as_deref())?;
        let workspace_name = workspace_name(&normalized_scope);

        let user_sets = UserSetService::new(cache.clone() as Arc<dyn UserSetPort>);
        let existing_user_sets = user_sets.list(&site_id).await?;
        let existing_user_set = existing_user_sets.iter().find(|user_set| {
            user_set.site_id == site_id
                && user_set.name == workspace_name
                && user_set.members == members
        });
        let user_set_id = match existing_user_set {
            Some(user_set) => user_set.id.clone(),
            None if members.is_empty() => {
                // UserSetService models nonempty user sets; save the deliberate
                // empty unrestricted cache partition directly through the port.
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
        // Keep the unconfigured team entirely local. A persisted user-set partition is created
        // only once members exist, which makes the no-members state both empty and side-effect
        // free while retaining a deterministic in-memory identity for presentation adapters.
        let team_members = Vec::new();
        let team_user_set_id = empty_team_user_set_id();

        let cache_port: Arc<dyn IssueCachePort> = cache.clone();
        let catalog = IssueCatalogService::new(jira.clone(), cache_port.clone());
        let detail = IssueDetailService::new(jira.clone(), IssueDetailConfig::default());
        let media = IssueMediaService::new(jira.clone(), IssueMediaConfig::default());
        let comments = CommentService::new(comment_writer);
        let edit_cache: Arc<dyn IssueEditCachePort> = cache.clone();
        let issue_editor =
            IssueEditService::new_with_cache(issue_editor, edit_cache, Arc::new(SystemClock));
        let events = Arc::new(NoopEventSink);
        let feed = UpdateFeedService::new(
            cache.clone() as Arc<dyn jira_application::UpdateFeedPort>,
            events.clone(),
        );
        let notification_port = Arc::new(FreedesktopNotificationPort::new());
        let sync = SyncService::new(
            jira,
            cache_port,
            Arc::new(DefaultIssueDiffer),
            notification_port.clone() as Arc<dyn jira_application::NotificationPort>,
            Arc::new(DefaultDesktopNotificationPolicy),
            Arc::new(SystemClock),
            events,
            SyncConfig::default(),
        );

        Ok(Self {
            site_id,
            authenticated_account,
            scope_state: Arc::new(RwLock::new(ScopeState {
                user_set_id,
                normalized_scope,
            })),
            team_state: Arc::new(RwLock::new(TeamState {
                user_set_id: team_user_set_id,
                members: team_members,
            })),
            catalog,
            feed,
            detail,
            media,
            comments,
            issue_editor,
            cache,
            sync,
            notification_port,
        })
    }

    pub fn site_id(&self) -> &JiraSiteId {
        &self.site_id
    }

    pub fn authenticated_account(&self) -> Option<&AccountId> {
        self.authenticated_account.as_ref()
    }

    /// Resolve a user-entered Jira query through the read-only user-search port. The caller owns
    /// exact-match policy and must not infer an account when Jira returns zero or many candidates.
    pub async fn search_users(
        &self,
        query: String,
        limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<User>, ApplicationError> {
        self.catalog
            .search_users(
                &UserSearchRequest {
                    site_id: self.site_id.clone(),
                    query,
                    limit,
                },
                cancellation,
            )
            .await
    }

    /// Return the configured team members in deterministic order.
    pub fn team_members(&self) -> Vec<AccountId> {
        self.team_state
            .read()
            .expect("team state lock")
            .members
            .clone()
    }

    pub fn team_user_set_id(&self) -> UserSetId {
        self.team_state
            .read()
            .expect("team state lock")
            .user_set_id
            .clone()
    }

    /// Replace the configured team membership and select its isolated local cache partition.
    /// Account IDs are sorted and deduplicated, so equivalent configurations reuse the same
    /// user-set identity. An empty configuration is valid and remains a local no-op on refresh.
    pub async fn configure_team_members(
        &self,
        mut members: Vec<AccountId>,
    ) -> Result<(), ApplicationError> {
        members.sort();
        members.dedup();
        let existing = self
            .cache
            .list(&self.site_id)
            .await?
            .into_iter()
            .find(|user_set| user_set.name == TEAM_WORKSPACE_NAME && user_set.members == members);
        let user_set_id = match existing {
            Some(user_set) => user_set.id,
            None if members.is_empty() => empty_team_user_set_id(),
            None => {
                self.cache
                    .save(UserSetDraft {
                        site_id: self.site_id.clone(),
                        name: TEAM_WORKSPACE_NAME.to_owned(),
                        members: members.clone(),
                    })
                    .await?
                    .id
            }
        };
        *self.team_state.write().expect("team state lock") = TeamState {
            user_set_id,
            members,
        };
        Ok(())
    }

    /// Product-facing alias for replacing configured team members.
    pub async fn replace_team_members(
        &self,
        members: Vec<AccountId>,
    ) -> Result<(), ApplicationError> {
        self.configure_team_members(members).await
    }

    pub fn user_set_id(&self) -> UserSetId {
        self.scope_state
            .read()
            .expect("scope state lock")
            .user_set_id
            .clone()
    }

    /// Return the active user-editable Jira scope. `None` means the default scope is active.
    pub fn jql_scope(&self) -> Option<String> {
        let scope = self
            .scope_state
            .read()
            .expect("scope state lock")
            .normalized_scope
            .clone();
        (scope != DEFAULT_JQL_SCOPE).then_some(scope)
    }

    fn scope_state(&self) -> ScopeState {
        self.scope_state.read().expect("scope state lock").clone()
    }

    /// Validate and stage a scope change. The new scope has a distinct user-set/cache identity;
    /// the next refresh therefore starts a quiet baseline instead of reusing another scope's
    /// cursor or membership.
    pub async fn set_jql_scope(&self, scope: Option<String>) -> Result<(), ApplicationError> {
        let normalized_scope = normalize_scope(scope.as_deref())?;
        let members = self
            .authenticated_account
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let name = workspace_name(&normalized_scope);
        let user_sets = UserSetService::new(self.cache.clone() as Arc<dyn UserSetPort>);
        let existing = user_sets
            .list(&self.site_id)
            .await?
            .into_iter()
            .find(|user_set| user_set.name == name && user_set.members == members);
        let user_set_id = if let Some(existing) = existing {
            existing.id
        } else {
            self.cache
                .save(UserSetDraft {
                    site_id: self.site_id.clone(),
                    name,
                    members,
                })
                .await?
                .id
        };
        *self.scope_state.write().expect("scope state lock") = ScopeState {
            user_set_id,
            normalized_scope,
        };
        Ok(())
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

    /// Fetch one bounded authenticated thumbnail through the application media service.
    pub async fn fetch_attachment_image(
        &self,
        request: AttachmentImageRequest,
        cancellation: &CancellationToken,
    ) -> Result<AttachmentImage, ApplicationError> {
        self.media.fetch(request, cancellation).await
    }

    /// Fetch one bounded authenticated original attachment for an explicit download.
    pub async fn download_attachment(
        &self,
        request: AttachmentDownloadRequest,
        cancellation: &CancellationToken,
    ) -> Result<AttachmentContent, ApplicationError> {
        self.media.download(request, cancellation).await
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

    /// Search the bounded set of users that Jira permits for assignment. An
    /// empty query is allowed for initial picker candidates.
    pub async fn search_assignable_users(
        &self,
        locator: IssueLocator,
        query: String,
        limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<jira_domain::User>, ApplicationError> {
        self.issue_editor
            .search_assignable_users(
                AssignableUserSearchRequest {
                    site_id: self.site_id.clone(),
                    locator,
                    query,
                    limit,
                },
                cancellation,
            )
            .await
    }

    /// Load the workflow transitions currently available for an issue.
    pub async fn available_transitions(
        &self,
        locator: IssueLocator,
        cancellation: &CancellationToken,
    ) -> Result<Vec<jira_application::IssueTransition>, ApplicationError> {
        self.issue_editor
            .available_transitions(
                IssueTransitionsRequest {
                    site_id: self.site_id.clone(),
                    locator,
                },
                cancellation,
            )
            .await
    }

    /// Dispatch one assignment after the presentation layer has obtained
    /// explicit user confirmation. The application service dispatches once and
    /// never retries an uncertain Jira response.
    pub async fn assign_issue(
        &self,
        locator: IssueLocator,
        assignee: Option<AccountId>,
        cancellation: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        self.issue_editor
            .assign(
                AssignIssueRequest {
                    site_id: self.site_id.clone(),
                    locator,
                    assignee,
                },
                cancellation,
            )
            .await
    }

    /// Dispatch one workflow transition after the presentation layer has
    /// obtained explicit user confirmation. The application service dispatches
    /// once and never retries an uncertain Jira response.
    pub async fn transition_issue(
        &self,
        locator: IssueLocator,
        transition_id: String,
        cancellation: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        self.issue_editor
            .transition(
                TransitionIssueRequest {
                    site_id: self.site_id.clone(),
                    locator,
                    transition_id,
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

    /// Load only the authenticated user's local view. A missing identity is
    /// never treated as permission to show an unrestricted cache.
    pub async fn load_cached_for_authenticated_account(
        &self,
    ) -> Result<CachedWorkspace, ApplicationError> {
        if self.authenticated_account.is_none() {
            return Err(ApplicationError::invalid_input(
                "authenticated Jira identity is required",
            ));
        }
        self.load_cached().await
    }

    /// Load bounded cached data without contacting Jira.
    pub async fn load_cached(&self) -> Result<CachedWorkspace, ApplicationError> {
        let scope_state = self.scope_state();
        self.load_cached_for_scope(&scope_state).await
    }

    async fn load_cached_for_scope(
        &self,
        scope_state: &ScopeState,
    ) -> Result<CachedWorkspace, ApplicationError> {
        self.load_cached_for_partition(&scope_state.user_set_id, Some(&scope_state.user_set_id))
            .await
    }

    async fn load_cached_for_partition(
        &self,
        user_set_id: &UserSetId,
        required_event_user_set_id: Option<&UserSetId>,
    ) -> Result<CachedWorkspace, ApplicationError> {
        let mut issues = Vec::new();
        for offset in (0..MAX_CACHED_ISSUES).step_by(ISSUE_PAGE_SIZE) {
            let page = self
                .catalog
                .list_cached(&IssueListQuery {
                    site_id: self.site_id.clone(),
                    user_set_id: user_set_id.clone(),
                    text: None,
                    // Membership is already account-scoped by the remote JQL and user-set
                    // identity. Re-filtering by assignee would hide watched issues.
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
            .filter(|event| {
                required_event_user_set_id.is_none_or(|user_set_id| {
                    event
                        .matching_user_set_ids
                        .iter()
                        .any(|id| id == user_set_id)
                })
            })
            .collect();
        Ok(CachedWorkspace { issues, events })
    }

    /// Load only the configured team's local cache and update feed. This never contacts Jira.
    pub async fn load_cached_team(&self) -> Result<CachedWorkspace, ApplicationError> {
        let state = self.team_state.read().expect("team state lock").clone();
        if state.members.is_empty() {
            return Ok(CachedWorkspace {
                issues: Vec::new(),
                events: Vec::new(),
            });
        }
        self.load_cached_for_partition(&state.user_set_id, Some(&state.user_set_id))
            .await
    }

    /// Synchronize Jira and reload bounded local data.
    pub async fn refresh(
        &self,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let scope_state = self.scope_state();
        let mode = self
            .next_mode(&scope_state, SyncMode::Reconciliation)
            .await?;
        self.refresh_with_mode(&scope_state, mode, cancellation)
            .await
    }

    /// Synchronize Jira automatically, using the incremental cursor after the
    /// first successful baseline while preserving the local membership view.
    pub async fn refresh_automatically(
        &self,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let scope_state = self.scope_state();
        let mode = self.next_mode(&scope_state, SyncMode::Incremental).await?;
        self.refresh_with_mode(&scope_state, mode, cancellation)
            .await
    }

    async fn next_mode(
        &self,
        scope_state: &ScopeState,
        subsequent_mode: SyncMode,
    ) -> Result<SyncMode, ApplicationError> {
        self.next_mode_for_user_set(&scope_state.user_set_id, subsequent_mode)
            .await
    }

    async fn next_mode_for_user_set(
        &self,
        user_set_id: &UserSetId,
        subsequent_mode: SyncMode,
    ) -> Result<SyncMode, ApplicationError> {
        if self
            .cache
            .sync_state(&self.site_id, user_set_id)
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
        scope_state: &ScopeState,
        mode: SyncMode,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let outcome = self
            .sync
            .run(
                SyncRequest {
                    site_id: self.site_id.clone(),
                    user_set_id: scope_state.user_set_id.clone(),
                    assignees: self
                        .authenticated_account
                        .clone()
                        .map(|account_id| vec![account_id]),
                    watchers: self
                        .authenticated_account
                        .clone()
                        .map(|account_id| vec![account_id]),
                    jql_scope: Some(scope_state.normalized_scope.clone()),
                    notification_assignees: self
                        .authenticated_account
                        .clone()
                        .map(|account_id| vec![account_id]),
                    mode,
                },
                cancellation,
            )
            .await?;
        let cached = self.load_cached_for_scope(scope_state).await?;
        Ok(RefreshResult { cached, outcome })
    }

    /// Synchronize the configured team's in-progress issues and reload its isolated local data.
    /// No Jira request is made when the team has no configured members.
    pub async fn refresh_team(
        &self,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let team_state = self.team_state.read().expect("team state lock").clone();
        if team_state.members.is_empty() {
            cancellation.check()?;
            return Ok(RefreshResult {
                cached: self.load_cached_team().await?,
                outcome: empty_team_sync_outcome(),
            });
        }
        let mode = self
            .next_mode_for_user_set(&team_state.user_set_id, SyncMode::Reconciliation)
            .await?;
        self.refresh_team_with_mode(&team_state, mode, cancellation)
            .await
    }

    /// Synchronize the configured team, using its incremental cursor after a successful
    /// baseline while preserving the local team membership view.
    pub async fn refresh_team_automatically(
        &self,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let team_state = self.team_state.read().expect("team state lock").clone();
        if team_state.members.is_empty() {
            cancellation.check()?;
            return Ok(RefreshResult {
                cached: self.load_cached_team().await?,
                outcome: empty_team_sync_outcome(),
            });
        }
        let mode = self
            .next_mode_for_user_set(&team_state.user_set_id, SyncMode::Incremental)
            .await?;
        self.refresh_team_with_mode(&team_state, mode, cancellation)
            .await
    }

    async fn refresh_team_with_mode(
        &self,
        team_state: &TeamState,
        mode: SyncMode,
        cancellation: &jira_application::CancellationToken,
    ) -> Result<RefreshResult, ApplicationError> {
        let outcome = self
            .sync
            .run(
                SyncRequest {
                    site_id: self.site_id.clone(),
                    user_set_id: team_state.user_set_id.clone(),
                    assignees: Some(team_state.members.clone()),
                    watchers: None,
                    jql_scope: Some(team_jql_scope()),
                    // Team membership controls fetches only. Desktop notifications remain
                    // restricted to the authenticated account, matching the primary view.
                    notification_assignees: self
                        .authenticated_account
                        .clone()
                        .map(|account_id| vec![account_id]),
                    mode,
                },
                cancellation,
            )
            .await?;
        let cached = self.load_cached_team().await?;
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

#[derive(Debug)]
struct UnsupportedIssueEditor;

impl JiraIssueEditPort for UnsupportedIssueEditor {
    fn search_assignable_users<'a>(
        &'a self,
        _request: &'a AssignableUserSearchRequest,
        _cancellation: &'a CancellationToken,
    ) -> jira_application::PortFuture<'a, Vec<jira_domain::User>> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            jira_application::ErrorKind::Internal,
            "issue assignment is unavailable in this workspace",
        ))))
    }

    fn fetch_issue_transitions<'a>(
        &'a self,
        _request: &'a IssueTransitionsRequest,
        _cancellation: &'a CancellationToken,
    ) -> jira_application::PortFuture<'a, Vec<jira_application::IssueTransition>> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            jira_application::ErrorKind::Internal,
            "issue transitions are unavailable in this workspace",
        ))))
    }

    fn assign_issue<'a>(
        &'a self,
        _request: &'a AssignIssueRequest,
        _cancellation: &'a CancellationToken,
    ) -> jira_application::PortFuture<'a, ()> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            jira_application::ErrorKind::Internal,
            "issue assignment is unavailable in this workspace",
        ))))
    }

    fn transition_issue<'a>(
        &'a self,
        _request: &'a TransitionIssueRequest,
        _cancellation: &'a CancellationToken,
    ) -> jira_application::PortFuture<'a, ()> {
        Box::pin(std::future::ready(Err(ApplicationError::new(
            jira_application::ErrorKind::Internal,
            "issue transitions are unavailable in this workspace",
        ))))
    }
}

fn workspace_name(scope: &str) -> String {
    format!(
        "{WORKSPACE_NAME} · account view · scope-{}",
        scope_fingerprint(scope)
    )
}

fn team_jql_scope() -> String {
    TEAM_STATUS_SCOPE.to_owned()
}

fn empty_team_user_set_id() -> UserSetId {
    UserSetId::new("configured-team-empty").expect("static team user set ID is valid")
}

fn empty_team_sync_outcome() -> SyncOutcome {
    SyncOutcome {
        mode: SyncMode::Baseline,
        pages_fetched: 0,
        issues_fetched: 0,
        events_inserted: 0,
        notifications_delivered: 0,
        notification_failures: 0,
        cursor: Timestamp::now_utc(),
    }
}

fn normalize_scope(scope: Option<&str>) -> Result<String, ApplicationError> {
    validate_jql_scope(scope).map_err(ApplicationError::invalid_input)?;
    Ok(scope.unwrap_or(DEFAULT_JQL_SCOPE).trim().to_owned())
}

fn scope_fingerprint(scope: &str) -> String {
    // FNV-1a is small, deterministic across restarts, and sufficient for a bounded cache key.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in scope.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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
        IssueCommentsPageRequest, IssueDetailRequest, IssueFetchRequest, IssuePage,
        IssueTransition, IssueTransitionsRequest, JiraAttachmentReadPort, JiraIssueActivityPort,
        JiraIssueDetailReadPort, JiraIssueSearchPort, JiraReadPort, JiraSyncReadPort,
        JiraUserReadPort, PortFuture, TransitionIssueRequest, UserSearchRequest,
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
        watcher_filters: Mutex<Vec<Option<Vec<AccountId>>>>,
        jql_scopes: Mutex<Vec<Option<String>>>,
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

        fn watcher_filters(&self) -> Vec<Option<Vec<AccountId>>> {
            self.watcher_filters
                .lock()
                .expect("watcher filters lock")
                .clone()
        }

        fn jql_scopes(&self) -> Vec<Option<String>> {
            self.jql_scopes.lock().expect("JQL scopes lock").clone()
        }
    }

    impl JiraIssueDetailReadPort for FakeJira {
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
    }

    impl JiraUserReadPort for FakeJira {
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
    }

    impl JiraIssueSearchPort for FakeJira {
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
            self.watcher_filters
                .lock()
                .expect("watcher filters lock")
                .push(request.watchers.clone());
            self.jql_scopes
                .lock()
                .expect("JQL scopes lock")
                .push(request.jql_scope.clone());
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

    impl JiraIssueActivityPort for FakeJira {}
    impl JiraAttachmentReadPort for FakeJira {}
    impl JiraSyncReadPort for FakeJira {}
    impl JiraReadPort for FakeJira {}

    #[derive(Clone)]
    struct FakeIssueEditor {
        users: Vec<User>,
        transitions: Vec<IssueTransition>,
        user_queries: Arc<Mutex<Vec<String>>>,
        transition_reads: Arc<Mutex<usize>>,
        transition_writes: Arc<Mutex<usize>>,
    }

    impl FakeIssueEditor {
        fn new() -> Self {
            let site_id = JiraSiteId::new("site").expect("site");
            Self {
                users: vec![
                    User::new(
                        site_id.clone(),
                        account("alice-id"),
                        "Alice Example",
                        None,
                        true,
                    ),
                    User::new(site_id, account("bob-id"), "Bob Example", None, true),
                ],
                transitions: vec![IssueTransition {
                    id: "31".into(),
                    name: "In progress".into(),
                    to: Status {
                        id: "3".into(),
                        name: "In Progress".into(),
                        category: None,
                    },
                }],
                user_queries: Arc::new(Mutex::new(Vec::new())),
                transition_reads: Arc::new(Mutex::new(0)),
                transition_writes: Arc::new(Mutex::new(0)),
            }
        }

        fn user_queries(&self) -> Vec<String> {
            self.user_queries.lock().expect("queries lock").clone()
        }

        fn transition_reads(&self) -> usize {
            *self.transition_reads.lock().expect("transition reads lock")
        }

        fn transition_writes(&self) -> usize {
            *self
                .transition_writes
                .lock()
                .expect("transition writes lock")
        }
    }

    impl JiraIssueEditPort for FakeIssueEditor {
        fn search_assignable_users<'a>(
            &'a self,
            request: &'a AssignableUserSearchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<User>> {
            self.user_queries
                .lock()
                .expect("queries lock")
                .push(request.query.clone());
            let users = self.users.clone();
            Box::pin(async move { Ok(users) })
        }

        fn fetch_issue_transitions<'a>(
            &'a self,
            _request: &'a IssueTransitionsRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<IssueTransition>> {
            *self.transition_reads.lock().expect("transition reads lock") += 1;
            let transitions = self.transitions.clone();
            Box::pin(async move { Ok(transitions) })
        }

        fn assign_issue<'a>(
            &'a self,
            _request: &'a AssignIssueRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }

        fn transition_issue<'a>(
            &'a self,
            _request: &'a TransitionIssueRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, ()> {
            *self
                .transition_writes
                .lock()
                .expect("transition writes lock") += 1;
            Box::pin(async { Ok(()) })
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

    fn make_edit_workspace(
        jira: Arc<FakeJira>,
        cache: Arc<SqliteStore>,
        editor: Arc<FakeIssueEditor>,
    ) -> LiveWorkspace {
        block_on(LiveWorkspace::initialize_with_writers(
            JiraSiteId::new("site").expect("valid site"),
            Some(account("account-a")),
            jira,
            Arc::new(UnsupportedCommentWriter),
            editor,
            cache,
        ))
        .expect("workspace initializes")
    }

    fn edit_locator() -> IssueLocator {
        IssueLocator::Key(IssueKey::new("APP-1").expect("valid key"))
    }

    #[test]
    fn issue_editor_cache_fetches_empty_users_once_and_filters_follow_up_queries_locally() {
        let editor = Arc::new(FakeIssueEditor::new());
        let workspace = make_edit_workspace(
            Arc::new(FakeJira::default()),
            Arc::new(SqliteStore::in_memory().expect("store")),
            editor.clone(),
        );
        let cancellation = CancellationToken::new();

        let all = block_on(workspace.search_assignable_users(
            edit_locator(),
            String::new(),
            100,
            &cancellation,
        ))
        .expect("initial candidates");
        let alice = block_on(workspace.search_assignable_users(
            edit_locator(),
            "ALICE".into(),
            100,
            &cancellation,
        ))
        .expect("local filtered candidates");
        let bob = block_on(workspace.search_assignable_users(
            edit_locator(),
            "bob-id".into(),
            100,
            &cancellation,
        ))
        .expect("local account-id filtered candidates");

        assert_eq!(all.len(), 2);
        assert_eq!(
            alice
                .iter()
                .map(|user| user.display_name.as_str())
                .collect::<Vec<_>>(),
            ["Alice Example"]
        );
        assert_eq!(
            bob.iter()
                .map(|user| user.display_name.as_str())
                .collect::<Vec<_>>(),
            ["Bob Example"]
        );
        assert_eq!(editor.user_queries(), vec![String::new()]);
    }

    #[test]
    fn issue_editor_transition_cache_reuses_reads_and_refreshes_after_successful_write() {
        let editor = Arc::new(FakeIssueEditor::new());
        let workspace = make_edit_workspace(
            Arc::new(FakeJira::default()),
            Arc::new(SqliteStore::in_memory().expect("store")),
            editor.clone(),
        );
        let cancellation = CancellationToken::new();

        let first = block_on(workspace.available_transitions(edit_locator(), &cancellation))
            .expect("initial transitions");
        let second = block_on(workspace.available_transitions(edit_locator(), &cancellation))
            .expect("cached transitions");
        block_on(workspace.transition_issue(edit_locator(), "31".into(), &cancellation))
            .expect("confirmed transition");
        let refreshed = block_on(workspace.available_transitions(edit_locator(), &cancellation))
            .expect("refreshed transitions");

        assert_eq!(first, second);
        assert_eq!(second, refreshed);
        assert_eq!(editor.transition_reads(), 2);
        assert_eq!(editor.transition_writes(), 1);
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
    fn team_members_are_deduplicated_sorted_and_use_an_isolated_identity() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("memory store"));
        let workspace = make_workspace(jira, cache.clone());
        let primary_id = workspace.user_set_id();

        block_on(workspace.configure_team_members(vec![
            account("team-b"),
            account("team-a"),
            account("team-b"),
        ]))
        .expect("configure team");
        let first_id = workspace.team_user_set_id();
        assert_ne!(first_id, primary_id);
        assert_eq!(
            workspace.team_members(),
            vec![account("team-a"), account("team-b")]
        );

        block_on(workspace.replace_team_members(vec![account("team-a"), account("team-b")]))
            .expect("replace equivalent team");
        assert_eq!(workspace.team_user_set_id(), first_id);
    }

    #[test]
    fn empty_team_refresh_is_a_safe_local_no_op() {
        let jira = Arc::new(FakeJira::default());
        let workspace = make_workspace(
            jira.clone(),
            Arc::new(SqliteStore::in_memory().expect("store")),
        );
        let result = block_on(workspace.refresh_team(&CancellationToken::new()))
            .expect("empty team refresh");
        assert_eq!(result.outcome.mode, SyncMode::Baseline);
        assert_eq!(result.outcome.pages_fetched, 0);
        assert!(result.cached.issues.is_empty());
        assert_eq!(jira.request_count(), 0);
    }

    #[test]
    fn team_refresh_is_assignee_only_and_keeps_primary_view_separate() {
        let jira = Arc::new(FakeJira::default());
        jira.push_page(page(issue("Primary issue")));
        jira.push_page(page(issue_for("Team issue", "team-a")));
        let workspace = make_workspace(
            jira.clone(),
            Arc::new(SqliteStore::in_memory().expect("store")),
        );
        let cancellation = CancellationToken::new();

        let primary = block_on(workspace.refresh(&cancellation)).expect("primary baseline");
        block_on(workspace.configure_team_members(vec![account("team-a")]))
            .expect("configure team");
        let team = block_on(workspace.refresh_team(&cancellation)).expect("team baseline");

        assert_eq!(primary.cached.issues[0].summary, "Primary issue");
        assert_eq!(team.outcome.mode, SyncMode::Baseline);
        assert_eq!(team.cached.issues[0].summary, "Team issue");
        assert_eq!(
            jira.assignee_filters(),
            vec![
                Some(vec![account("account-a")]),
                Some(vec![account("team-a")])
            ]
        );
        assert_eq!(
            jira.watcher_filters(),
            vec![Some(vec![account("account-a")]), None]
        );
        assert_eq!(
            jira.jql_scopes(),
            vec![Some(DEFAULT_JQL_SCOPE.to_owned()), Some(team_jql_scope())]
        );
    }

    #[test]
    fn initialization_uses_the_requested_normalized_scope_identity() {
        let jira = Arc::new(FakeJira::default());
        let cache = Arc::new(SqliteStore::in_memory().expect("memory store"));
        let workspace = block_on(LiveWorkspace::initialize_with_writers_and_scope(
            JiraSiteId::new("site").expect("site"),
            Some(account("account-a")),
            jira,
            Arc::new(UnsupportedCommentWriter),
            Arc::new(UnsupportedIssueEditor),
            cache,
            Some("  project = APP  ".to_owned()),
        ))
        .expect("workspace initializes");
        assert_eq!(workspace.jql_scope().as_deref(), Some("project = APP"));
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
        assert_ne!(current.user_set_id(), legacy.id);
        let sets = block_on(cache.list(&site_id)).expect("list sets");
        assert_eq!(sets.len(), 2);
        let current_set = sets
            .iter()
            .find(|set| set.id == current.user_set_id())
            .expect("current account-view user set persists");
        assert_eq!(current_set.name, workspace_name(DEFAULT_JQL_SCOPE));
        assert_eq!(current_set.members, vec![account("account-a")]);
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
        assert_eq!(
            jira.assignee_filters(),
            vec![
                Some(vec![account("account-a")]),
                Some(vec![account("account-a")])
            ]
        );
        assert_eq!(
            jira.watcher_filters(),
            vec![
                Some(vec![account("account-a")]),
                Some(vec![account("account-a")])
            ]
        );

        let all = block_on(workspace.load_cached()).expect("load all local issues");
        assert_eq!(all.issues.len(), 2);

        let mine = block_on(workspace.load_cached_for_authenticated_account())
            .expect("load local account view");
        assert_eq!(mine.issues.len(), 2);
        assert!(
            mine.events
                .iter()
                .all(|event| mine.issues.iter().any(|issue| issue.id == event.issue_id))
        );
        assert_eq!(all.events.len(), mine.events.len());
        assert_eq!(jira.request_count(), requests_after_sync);
    }

    #[test]
    fn issue_detail_fetches_core_comments_and_attachment_metadata_through_workspace() {
        let jira = Arc::new(FakeJira::default());
        let mut detailed_issue = issue("Detailed summary");
        detailed_issue.description_text = Some("Detailed description".to_owned());
        jira.push_detail(IssueDetailCore::new(
            detailed_issue,
            vec![
                AttachmentMetadata::new("attachment-1", "notes.txt", 42, Some("text/plain"))
                    .expect("attachment"),
            ],
        ));
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
