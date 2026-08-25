use std::{
    fs::OpenOptions as FsOpenOptions,
    io::ErrorKind as IoErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures_channel::oneshot;
use jira_application::{
    ApplicationError, CachedAssignableUsers, CachedIssueTransitions, CommitOutcome, ErrorKind,
    IssueCachePort, IssueEditCachePort, IssueListQuery, IssueLocator, IssueTransition, PortFuture,
    SyncCommit, SyncState, UpdateFeedPort, UpdateFeedQuery, UserSetDraft, UserSetPort,
};
use jira_domain::{
    EventId, Issue, IssueId, JiraSiteId, NotificationDelivery, Timestamp, UpdateEvent, User,
    UserSet, UserSetId,
};

mod codecs;
mod edit_cache;
mod issue_sync;
mod migrations;
mod update_feed;
mod user_sets;
mod worker;

#[cfg(test)]
mod tests;

/// A redacted error returned when a SQLite store cannot be opened or migrated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteOpenError {
    message: &'static str,
}

impl SqliteOpenError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for SqliteOpenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for SqliteOpenError {}

/// Durable local storage. A single worker owns the SQLite connection, so no
/// SQL runs on the GPUI executor or behind a connection mutex.
#[derive(Clone)]
pub struct SqliteStore {
    worker: Arc<worker::Worker>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SqliteOpenError> {
        let path = normalize_database_path(path.as_ref())?;
        Self::start(Some(path))
    }

    pub fn in_memory() -> Result<Self, SqliteOpenError> {
        Self::start(None)
    }

    pub fn open_in_memory() -> Result<Self, SqliteOpenError> {
        Self::in_memory()
    }

    fn start(path: Option<PathBuf>) -> Result<Self, SqliteOpenError> {
        let worker = worker::start(path)?;
        Ok(Self {
            worker: Arc::new(worker),
        })
    }
}

fn normalize_database_path(path: &Path) -> Result<PathBuf, SqliteOpenError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| SqliteOpenError::new("could not open local storage"))?;
    let parent = parent
        .canonicalize()
        .map_err(|_| SqliteOpenError::new("could not open local storage"))?;
    Ok(parent.join(file_name))
}

fn ensure_database_file(path: &Path) -> std::io::Result<()> {
    let mut options = FsOpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => {
            drop(file);
            Ok(())
        }
        Err(error) if error.kind() == IoErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

impl IssueCachePort for SqliteStore {
    fn list_issues<'a>(&'a self, query: &'a IssueListQuery) -> PortFuture<'a, Vec<Issue>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::ListIssues {
                query: query.clone(),
                reply,
            },
            receiver,
        )
    }

    fn get_issue<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_id: &'a IssueId,
    ) -> PortFuture<'a, Option<Issue>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::GetIssue {
                site_id: site_id.clone(),
                issue_id: issue_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn issues_for_user_set<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
    ) -> PortFuture<'a, Vec<Issue>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::IssuesForUserSet {
                site_id: site_id.clone(),
                user_set_id: user_set_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn sync_state<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
    ) -> PortFuture<'a, Option<SyncState>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::SyncState {
                site_id: site_id.clone(),
                user_set_id: user_set_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn commit_sync<'a>(&'a self, commit: SyncCommit) -> PortFuture<'a, CommitOutcome> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::CommitSync { commit, reply },
            receiver,
        )
    }

    fn record_sync_failure<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
        kind: ErrorKind,
        at: Timestamp,
    ) -> PortFuture<'a, ()> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::RecordSyncFailure {
                site_id: site_id.clone(),
                user_set_id: user_set_id.clone(),
                kind,
                at,
                reply,
            },
            receiver,
        )
    }

    fn record_notification_delivery<'a>(
        &'a self,
        event_id: &'a EventId,
        delivery: NotificationDelivery,
        _at: Timestamp,
    ) -> PortFuture<'a, ()> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::RecordNotificationDelivery {
                event_id: event_id.clone(),
                delivery,
                reply,
            },
            receiver,
        )
    }
}

impl UpdateFeedPort for SqliteStore {
    fn list<'a>(&'a self, query: &'a UpdateFeedQuery) -> PortFuture<'a, Vec<UpdateEvent>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::ListEvents {
                query: query.clone(),
                reply,
            },
            receiver,
        )
    }

    fn unread_count<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::UnreadCount {
                site_id: site_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn mark_read<'a>(&'a self, event_ids: &'a [EventId], read: bool) -> PortFuture<'a, usize> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::MarkRead {
                event_ids: event_ids.to_vec(),
                read,
                reply,
            },
            receiver,
        )
    }

    fn mark_all_read<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::MarkAllRead {
                site_id: site_id.clone(),
                reply,
            },
            receiver,
        )
    }
}

impl UserSetPort for SqliteStore {
    fn list<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, Vec<UserSet>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::ListUserSets {
                site_id: site_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn save<'a>(&'a self, draft: UserSetDraft) -> PortFuture<'a, UserSet> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::SaveUserSet { draft, reply },
            receiver,
        )
    }

    fn delete<'a>(&'a self, user_set_id: &'a UserSetId) -> PortFuture<'a, ()> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::DeleteUserSet {
                user_set_id: user_set_id.clone(),
                reply,
            },
            receiver,
        )
    }
}

impl IssueEditCachePort for SqliteStore {
    fn cached_assignable_users<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
    ) -> PortFuture<'a, Option<CachedAssignableUsers>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::CachedAssignableUsers {
                site_id: site_id.clone(),
                locator: locator.clone(),
                reply,
            },
            receiver,
        )
    }

    fn replace_assignable_users<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
        users: Vec<User>,
        fetched_at: Timestamp,
    ) -> PortFuture<'a, ()> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::ReplaceAssignableUsers {
                site_id: site_id.clone(),
                locator: locator.clone(),
                users,
                fetched_at,
                reply,
            },
            receiver,
        )
    }

    fn cached_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
    ) -> PortFuture<'a, Option<CachedIssueTransitions>> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::CachedIssueTransitions {
                site_id: site_id.clone(),
                locator: locator.clone(),
                reply,
            },
            receiver,
        )
    }

    fn replace_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
        transitions: Vec<IssueTransition>,
        fetched_at: Timestamp,
    ) -> PortFuture<'a, ()> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::ReplaceIssueTransitions {
                site_id: site_id.clone(),
                locator: locator.clone(),
                transitions,
                fetched_at,
                reply,
            },
            receiver,
        )
    }

    fn invalidate_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
    ) -> PortFuture<'a, ()> {
        let (reply, receiver) = oneshot::channel();
        worker::dispatch(
            self.worker.sender.clone(),
            worker::Request::InvalidateIssueTransitions {
                site_id: site_id.clone(),
                locator: locator.clone(),
                reply,
            },
            receiver,
        )
    }
}

fn sqlite_error(_: rusqlite::Error) -> ApplicationError {
    storage_error("local storage operation failed")
}

fn storage_error(message: impl Into<String>) -> ApplicationError {
    ApplicationError::new(ErrorKind::Storage, message)
}
