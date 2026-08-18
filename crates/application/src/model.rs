use jira_domain::{
    AccountId, Issue, IssueComment, IssueId, IssueKey, JiraSiteId, Status, Timestamp, UpdateEvent,
    UpdateKind, UserSetId,
};
use serde::{Deserialize, Serialize};

/// Conservative bound for the user-editable Jira scope expression. Account filters and the
/// adapter-owned ordering are always appended outside this expression.
pub const MAX_JQL_SCOPE_LENGTH: usize = 2_000;
pub const DEFAULT_JQL_SCOPE: &str = "issuetype in (Bug, Story, Task, Sub-task) and status not in (Canceled, rejected, Cancelled) and createdDate >= startOfYear()";

pub fn validate_jql_scope(scope: Option<&str>) -> Result<(), &'static str> {
    let Some(scope) = scope else {
        return Ok(());
    };
    let scope = scope.trim();
    if scope.is_empty() {
        return Err("Jira scope cannot be empty");
    }
    if scope.len() > MAX_JQL_SCOPE_LENGTH {
        return Err("Jira scope is too long");
    }
    let words = scope
        .split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if words.windows(2).any(|window| window == ["order", "by"]) {
        return Err("Jira scope must not contain ORDER BY");
    }
    Ok(())
}

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
    /// Optional remote watcher restriction. When both restrictions are present, Jira returns
    /// their union (issues assigned to or watched by the supplied accounts).
    pub watchers: Option<Vec<AccountId>>,
    /// Optional user-editable Jira scope expression. The Jira adapter validates and
    /// parenthesizes it before adding account restrictions and stable ordering.
    pub jql_scope: Option<String>,
    pub updated_since: Option<Timestamp>,
    pub page_cursor: Option<PageCursor>,
    pub page_size: usize,
}

/// Request for bounded changelog enrichment for issue snapshots that changed
/// since the previous synchronized snapshot. The Jira adapter enforces the
/// 1,000-issue bulk endpoint limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueChangelogRequest {
    pub site_id: JiraSiteId,
    pub issue_ids: Vec<IssueId>,
}

/// One safe, transport-neutral changelog item. The application layer bounds
/// and sanitizes the display fields before turning these into update events.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueChangelogItem {
    pub field: Option<String>,
    pub field_id: Option<String>,
    pub from_string: Option<String>,
    pub to_string: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueChangelogHistory {
    pub id: String,
    pub created: Timestamp,
    pub items: Vec<IssueChangelogItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueChangelog {
    pub issue_id: IssueId,
    pub histories: Vec<IssueChangelogHistory>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueChangelogPage {
    pub changelogs: Vec<IssueChangelog>,
    pub next_page_token: Option<String>,
}

/// A user-confirmed Jira comment creation request. The body remains plain text
/// here; the Jira adapter owns conversion to Atlassian Document Format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddCommentRequest {
    pub site_id: JiraSiteId,
    pub locator: IssueLocator,
    pub body: String,
}

/// A bounded search for users that Jira permits as assignees on an issue.
///
/// An empty query is intentional: Jira can use it to return the initial set of
/// candidates before a user has typed a filter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignableUserSearchRequest {
    pub site_id: JiraSiteId,
    pub locator: IssueLocator,
    pub query: String,
    pub limit: usize,
}

/// A request for the transitions currently available for an issue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueTransitionsRequest {
    pub site_id: JiraSiteId,
    pub locator: IssueLocator,
}

/// A transition exposed by Jira's issue workflow.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IssueTransition {
    pub id: String,
    pub name: String,
    pub to: Status,
}

/// A persisted issue-scoped candidate list and the instant at which Jira
/// supplied it. The timestamp is the cache's freshness anchor, not a Jira
/// issue update timestamp.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CachedAssignableUsers {
    pub users: Vec<jira_domain::User>,
    pub fetched_at: Timestamp,
}

/// A persisted issue-scoped workflow transition list and its fetch timestamp.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CachedIssueTransitions {
    pub transitions: Vec<IssueTransition>,
    pub fetched_at: Timestamp,
}

/// A user-confirmed assignment request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignIssueRequest {
    pub site_id: JiraSiteId,
    pub locator: IssueLocator,
    pub assignee: Option<AccountId>,
}

/// A user-confirmed workflow transition request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionIssueRequest {
    pub site_id: JiraSiteId,
    pub locator: IssueLocator,
    pub transition_id: String,
}

/// Typed request for the core issue-detail payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IssueLocator {
    Id(IssueId),
    /// Jira key lookup accepts the key currently returned by Jira. If an issue
    /// was moved and its key changed, the old key is intentionally rejected.
    Key(IssueKey),
}

/// Typed request for the core issue-detail payload.
#[derive(Clone, Debug)]
pub struct IssueDetailRequest {
    pub site_id: JiraSiteId,
    pub locator: IssueLocator,
}

/// Typed request for one page of issue comments.
#[derive(Clone, Debug)]
pub struct IssueCommentsPageRequest {
    pub site_id: JiraSiteId,
    pub issue_id: IssueId,
    pub start_at: usize,
    pub page_cursor: Option<PageCursor>,
    pub page_size: usize,
}

/// A transport-neutral page of comments with explicit cursor/startAt progression.
#[derive(Clone, Debug)]
pub struct IssueCommentsPage {
    pub comments: Vec<IssueComment>,
    pub start_at: usize,
    pub next_start_at: Option<usize>,
    pub next_cursor: Option<PageCursor>,
    pub total: Option<usize>,
}

/// A request for one image attachment belonging to an issue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentImageRequest {
    pub site_id: JiraSiteId,
    pub issue_id: IssueId,
    pub attachment_id: String,
    /// Validated thumbnail width generated by `IssueMediaService`.
    pub width: usize,
    /// Validated thumbnail height generated by `IssueMediaService`.
    pub height: usize,
    /// Validated response limit generated by `IssueMediaService`.
    pub max_bytes: usize,
}

/// An image attachment read from Jira. The bytes are deliberately kept in memory only; callers
/// decide how and when to cache or display them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentImage {
    pub attachment_id: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

/// Arbitrary attachment content. Unlike `AttachmentImage`, the payload is not restricted to an
/// image MIME type and is suitable for explicit downloads such as PDFs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentContent {
    pub attachment_id: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

/// A request for the original bytes of one Jira attachment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentDownloadRequest {
    pub site_id: JiraSiteId,
    pub issue_id: IssueId,
    pub attachment_id: String,
    /// Validated response limit generated by `IssueMediaService`.
    pub max_bytes: usize,
}

pub type IssueCommentPage = IssueCommentsPage;

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
    /// Optional remote watcher restriction. When both restrictions are present, Jira returns
    /// their union (issues assigned to or watched by the supplied accounts).
    pub watchers: Option<Vec<AccountId>>,
    pub jql_scope: Option<String>,
    /// Optional local notification restriction. This does not change the remote fetch or cache
    /// membership; it only allows desktop delivery for incoming issues assigned to these users.
    /// `None` preserves the generic, unfiltered notification behavior.
    pub notification_assignees: Option<Vec<AccountId>>,
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
