use jira_domain::{AccountId, Issue, JiraSiteId, Timestamp, UpdateEvent, UpdateKind, UserSetId};

#[derive(Clone, Debug)]
pub struct UserSearchRequest {
    pub site_id: JiraSiteId,
    pub query: String,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct IssueFetchRequest {
    pub site_id: JiraSiteId,
    /// Optional remote restriction. `None` fetches all issues in the configured Jira scope.
    pub assignees: Option<Vec<AccountId>>,
    pub updated_since: Option<Timestamp>,
    pub page_cursor: Option<PageCursor>,
    pub page_size: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageCursor(pub String);

#[derive(Clone, Debug)]
pub struct IssuePage {
    pub issues: Vec<Issue>,
    pub next_cursor: Option<PageCursor>,
    /// A server-derived boundary is preferable to the local clock for the next poll.
    pub server_time: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncMode {
    Baseline,
    Incremental,
    Reconciliation,
}

impl SyncMode {
    pub fn replaces_membership(self) -> bool {
        matches!(self, Self::Baseline | Self::Reconciliation)
    }

    pub fn emits_updates(self) -> bool {
        !matches!(self, Self::Baseline)
    }
}

#[derive(Clone, Debug)]
pub struct SyncRequest {
    pub site_id: JiraSiteId,
    pub user_set_id: UserSetId,
    /// Optional remote restriction. `None` fetches all issues in the configured Jira scope.
    pub assignees: Option<Vec<AccountId>>,
    pub mode: SyncMode,
}

#[derive(Clone, Debug)]
pub struct SyncState {
    pub site_id: JiraSiteId,
    pub user_set_id: UserSetId,
    pub last_incremental_started_at: Option<Timestamp>,
    pub last_incremental_succeeded_at: Option<Timestamp>,
    pub last_full_sync_at: Option<Timestamp>,
    pub consecutive_failures: u32,
    pub last_error_kind: Option<crate::ErrorKind>,
}

impl SyncState {
    pub fn new(site_id: JiraSiteId, user_set_id: UserSetId) -> Self {
        Self {
            site_id,
            user_set_id,
            last_incremental_started_at: None,
            last_incremental_succeeded_at: None,
            last_full_sync_at: None,
            consecutive_failures: 0,
            last_error_kind: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChangeSet {
    pub existing: Vec<Issue>,
    pub incoming: Vec<Issue>,
    pub site_id: JiraSiteId,
    pub user_set_id: UserSetId,
    pub detected_at: Timestamp,
    pub include_removed_from_view: bool,
}

#[derive(Clone, Debug)]
pub struct SyncCommit {
    pub site_id: JiraSiteId,
    pub user_set_id: UserSetId,
    pub issues: Vec<Issue>,
    pub update_events: Vec<UpdateEvent>,
    pub replace_membership: bool,
    pub state: SyncState,
}

/// The persistence adapter must insert events and advance the cursor atomically.
#[derive(Clone, Debug, Default)]
pub struct CommitOutcome {
    /// Only newly inserted events are returned; deduplicated events are omitted.
    pub inserted_events: Vec<UpdateEvent>,
}

#[derive(Clone, Debug)]
pub struct SyncOutcome {
    pub mode: SyncMode,
    pub pages_fetched: usize,
    pub issues_fetched: usize,
    pub events_inserted: usize,
    pub notifications_delivered: usize,
    pub notification_failures: usize,
    pub cursor: Timestamp,
}

#[derive(Clone, Debug)]
pub enum ApplicationEvent {
    SyncStarted {
        site_id: JiraSiteId,
        user_set_id: UserSetId,
        mode: SyncMode,
    },
    SyncPageFetched {
        user_set_id: UserSetId,
        page: usize,
        issue_count: usize,
        total_issue_count: usize,
    },
    SyncCompleted {
        user_set_id: UserSetId,
        outcome: SyncOutcome,
    },
    SyncFailed {
        user_set_id: UserSetId,
        error: crate::ApplicationError,
    },
    FeedChanged {
        site_id: JiraSiteId,
    },
}

#[derive(Clone, Debug)]
pub struct NotificationRequest {
    pub event: UpdateEvent,
}

#[derive(Clone, Debug)]
pub struct UpdateFeedQuery {
    pub site_id: JiraSiteId,
    pub unread_only: bool,
    pub kinds: Vec<UpdateKind>,
    pub before: Option<Timestamp>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct IssueListQuery {
    pub site_id: JiraSiteId,
    pub user_set_id: UserSetId,
    pub text: Option<String>,
    pub assignees: Vec<AccountId>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Clone, Debug)]
pub struct UserSetDraft {
    pub site_id: JiraSiteId,
    pub name: String,
    pub members: Vec<AccountId>,
}
