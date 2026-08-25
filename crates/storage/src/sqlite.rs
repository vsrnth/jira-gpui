use std::{
    fs::OpenOptions as FsOpenOptions,
    io::ErrorKind as IoErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};

use crate::event_semantics::{
    normalize_matching_user_set_ids, same_event_identity, union_matching_user_set_ids,
};
use futures_channel::oneshot;
use jira_application::{
    ApplicationError, CachedAssignableUsers, CachedIssueTransitions, CommitOutcome, ErrorKind,
    IssueCachePort, IssueEditCachePort, IssueListQuery, IssueLocator, IssueTransition,
    MAX_ASSIGNABLE_USER_SEARCH_LIMIT, MAX_ISSUE_TRANSITIONS, PortFuture, SyncCommit, SyncState,
    UpdateFeedPort, UpdateFeedQuery, UserSetDraft, UserSetPort,
};
use jira_domain::{
    EventId, Issue, IssueId, JiraSiteId, NotificationDelivery, Timestamp, UpdateEvent, UpdateKind,
    UpdateReadState, User, UserSet, UserSetId,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, params, params_from_iter, types::Value,
};
use time::{OffsetDateTime, UtcOffset};

const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SUPPORTED_SCHEMA_VERSION: i32 = 4;

type Reply<T> = oneshot::Sender<Result<T, ApplicationError>>;

enum Request {
    ListIssues {
        query: IssueListQuery,
        reply: Reply<Vec<Issue>>,
    },
    GetIssue {
        site_id: JiraSiteId,
        issue_id: IssueId,
        reply: Reply<Option<Issue>>,
    },
    IssuesForUserSet {
        site_id: JiraSiteId,
        user_set_id: UserSetId,
        reply: Reply<Vec<Issue>>,
    },
    SyncState {
        site_id: JiraSiteId,
        user_set_id: UserSetId,
        reply: Reply<Option<SyncState>>,
    },
    CommitSync {
        commit: SyncCommit,
        reply: Reply<CommitOutcome>,
    },
    RecordSyncFailure {
        site_id: JiraSiteId,
        user_set_id: UserSetId,
        kind: ErrorKind,
        at: Timestamp,
        reply: Reply<()>,
    },
    RecordNotificationDelivery {
        event_id: EventId,
        delivery: NotificationDelivery,
        reply: Reply<()>,
    },
    ListEvents {
        query: UpdateFeedQuery,
        reply: Reply<Vec<UpdateEvent>>,
    },
    UnreadCount {
        site_id: JiraSiteId,
        reply: Reply<usize>,
    },
    MarkRead {
        event_ids: Vec<EventId>,
        read: bool,
        reply: Reply<usize>,
    },
    MarkAllRead {
        site_id: JiraSiteId,
        reply: Reply<usize>,
    },
    ListUserSets {
        site_id: JiraSiteId,
        reply: Reply<Vec<UserSet>>,
    },
    SaveUserSet {
        draft: UserSetDraft,
        reply: Reply<UserSet>,
    },
    DeleteUserSet {
        user_set_id: UserSetId,
        reply: Reply<()>,
    },
    CachedAssignableUsers {
        site_id: JiraSiteId,
        locator: IssueLocator,
        reply: Reply<Option<CachedAssignableUsers>>,
    },
    ReplaceAssignableUsers {
        site_id: JiraSiteId,
        locator: IssueLocator,
        users: Vec<User>,
        fetched_at: Timestamp,
        reply: Reply<()>,
    },
    CachedIssueTransitions {
        site_id: JiraSiteId,
        locator: IssueLocator,
        reply: Reply<Option<CachedIssueTransitions>>,
    },
    ReplaceIssueTransitions {
        site_id: JiraSiteId,
        locator: IssueLocator,
        transitions: Vec<IssueTransition>,
        fetched_at: Timestamp,
        reply: Reply<()>,
    },
    InvalidateIssueTransitions {
        site_id: JiraSiteId,
        locator: IssueLocator,
        reply: Reply<()>,
    },
}

struct Worker {
    sender: mpsc::Sender<Request>,
}

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
    worker: Arc<Worker>,
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
        let (ready_sender, ready_receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("jira-sqlite".to_owned())
            .spawn(move || {
                if let Some(path) = path.as_deref()
                    && ensure_database_file(path).is_err()
                {
                    let _ = ready_sender
                        .send(Err(SqliteOpenError::new("could not open local storage")));
                    return;
                }
                let connection = path
                    .as_deref()
                    .map_or_else(Connection::open_in_memory, |path| {
                        Connection::open_with_flags(
                            path,
                            OpenFlags::SQLITE_OPEN_READ_WRITE
                                | OpenFlags::SQLITE_OPEN_CREATE
                                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                        )
                    });
                let mut connection = match connection {
                    Ok(connection) => connection,
                    Err(_) => {
                        let _ = ready_sender
                            .send(Err(SqliteOpenError::new("could not open local storage")));
                        return;
                    }
                };
                if initialize_connection(&mut connection, path.is_some()).is_err() {
                    let _ = ready_sender.send(Err(SqliteOpenError::new(
                        "could not initialize local storage",
                    )));
                    return;
                }
                let (sender, receiver) = mpsc::channel();
                let _ = ready_sender.send(Ok(sender.clone()));
                drop(sender);
                while let Ok(request) = receiver.recv() {
                    handle_request(&mut connection, request);
                }
            })
            .map_err(|_| SqliteOpenError::new("could not start local storage worker"))?;

        let sender = ready_receiver
            .recv()
            .map_err(|_| SqliteOpenError::new("local storage worker stopped during startup"))??;
        // The worker is intentionally detached. Once the last store is dropped,
        // its sender closes and the worker exits after processing queued work.
        drop(worker);
        Ok(Self {
            worker: Arc::new(Worker { sender }),
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

fn dispatch<T: Send + 'static>(
    sender: mpsc::Sender<Request>,
    request: Request,
    receiver: oneshot::Receiver<Result<T, ApplicationError>>,
) -> PortFuture<'static, T> {
    if sender.send(request).is_err() {
        return Box::pin(async { Err(storage_error("storage worker is unavailable")) });
    }
    Box::pin(async move {
        receiver
            .await
            .unwrap_or_else(|_| Err(storage_error("storage worker stopped")))
    })
}

impl IssueCachePort for SqliteStore {
    fn list_issues<'a>(&'a self, query: &'a IssueListQuery) -> PortFuture<'a, Vec<Issue>> {
        let (reply, receiver) = oneshot::channel();
        dispatch(
            self.worker.sender.clone(),
            Request::ListIssues {
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
        dispatch(
            self.worker.sender.clone(),
            Request::GetIssue {
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
        dispatch(
            self.worker.sender.clone(),
            Request::IssuesForUserSet {
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
        dispatch(
            self.worker.sender.clone(),
            Request::SyncState {
                site_id: site_id.clone(),
                user_set_id: user_set_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn commit_sync<'a>(&'a self, commit: SyncCommit) -> PortFuture<'a, CommitOutcome> {
        let (reply, receiver) = oneshot::channel();
        dispatch(
            self.worker.sender.clone(),
            Request::CommitSync { commit, reply },
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
        dispatch(
            self.worker.sender.clone(),
            Request::RecordSyncFailure {
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
        dispatch(
            self.worker.sender.clone(),
            Request::RecordNotificationDelivery {
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
        dispatch(
            self.worker.sender.clone(),
            Request::ListEvents {
                query: query.clone(),
                reply,
            },
            receiver,
        )
    }

    fn unread_count<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize> {
        let (reply, receiver) = oneshot::channel();
        dispatch(
            self.worker.sender.clone(),
            Request::UnreadCount {
                site_id: site_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn mark_read<'a>(&'a self, event_ids: &'a [EventId], read: bool) -> PortFuture<'a, usize> {
        let (reply, receiver) = oneshot::channel();
        dispatch(
            self.worker.sender.clone(),
            Request::MarkRead {
                event_ids: event_ids.to_vec(),
                read,
                reply,
            },
            receiver,
        )
    }

    fn mark_all_read<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize> {
        let (reply, receiver) = oneshot::channel();
        dispatch(
            self.worker.sender.clone(),
            Request::MarkAllRead {
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
        dispatch(
            self.worker.sender.clone(),
            Request::ListUserSets {
                site_id: site_id.clone(),
                reply,
            },
            receiver,
        )
    }

    fn save<'a>(&'a self, draft: UserSetDraft) -> PortFuture<'a, UserSet> {
        let (reply, receiver) = oneshot::channel();
        dispatch(
            self.worker.sender.clone(),
            Request::SaveUserSet { draft, reply },
            receiver,
        )
    }

    fn delete<'a>(&'a self, user_set_id: &'a UserSetId) -> PortFuture<'a, ()> {
        let (reply, receiver) = oneshot::channel();
        dispatch(
            self.worker.sender.clone(),
            Request::DeleteUserSet {
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
        dispatch(
            self.worker.sender.clone(),
            Request::CachedAssignableUsers {
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
        dispatch(
            self.worker.sender.clone(),
            Request::ReplaceAssignableUsers {
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
        dispatch(
            self.worker.sender.clone(),
            Request::CachedIssueTransitions {
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
        dispatch(
            self.worker.sender.clone(),
            Request::ReplaceIssueTransitions {
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
        dispatch(
            self.worker.sender.clone(),
            Request::InvalidateIssueTransitions {
                site_id: site_id.clone(),
                locator: locator.clone(),
                reply,
            },
            receiver,
        )
    }
}

fn initialize_connection(connection: &mut Connection, file_backed: bool) -> rusqlite::Result<()> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let journal_mode: String = if file_backed {
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?
    } else {
        connection.query_row("PRAGMA journal_mode = MEMORY", [], |row| row.get(0))?
    };
    let expected_mode = if file_backed { "wal" } else { "memory" };
    if !journal_mode.eq_ignore_ascii_case(expected_mode) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    migrate(connection)?;
    let mut foreign_key_check = connection.prepare("PRAGMA foreign_key_check")?;
    if foreign_key_check.query([])?.next()?.is_some() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let version: i32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > SUPPORTED_SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if version == SUPPORTED_SCHEMA_VERSION {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    if version < 1 {
        transaction.execute_batch(include_str!("../../../migrations/0001_initial.sql"))?;
        transaction.execute_batch("PRAGMA user_version = 1;")?;
    }
    if version < 2 {
        migrate_update_event_kind_range(&transaction)?;
        transaction.execute_batch("PRAGMA user_version = 2;")?;
    }
    if version < 3 {
        transaction.execute_batch(include_str!(
            "../../../migrations/0003_issue_edit_cache.sql"
        ))?;
        transaction.execute_batch("PRAGMA user_version = 3;")?;
    }
    if version < 4 {
        migrate_field_changed_kind(&transaction)?;
        transaction.execute_batch("PRAGMA user_version = 4;")?;
    }
    transaction.commit()
}

/// Extends the update-event kind range without changing the meaning of any
/// existing tag. SQLite cannot alter a CHECK constraint in place, so rebuild
/// the two directly related tables while preserving all rows and associations.
fn migrate_update_event_kind_range(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "
        CREATE TABLE update_events_v2 (
            event_id TEXT PRIMARY KEY,
            site_id TEXT NOT NULL,
            issue_id TEXT NOT NULL,
            issue_key TEXT NOT NULL,
            kind INTEGER NOT NULL,
            occurred_seconds INTEGER NOT NULL,
            occurred_nanos INTEGER NOT NULL,
            read_state INTEGER NOT NULL DEFAULT 0,
            notification_delivery INTEGER NOT NULL DEFAULT 0,
            snapshot TEXT NOT NULL,
            UNIQUE (event_id, site_id),
            FOREIGN KEY (site_id, issue_id) REFERENCES issues(site_id, issue_id) ON DELETE CASCADE,
            CHECK (kind BETWEEN 0 AND 9),
            CHECK (occurred_nanos BETWEEN 0 AND 999999999),
            CHECK (read_state IN (0, 1)),
            CHECK (notification_delivery BETWEEN 0 AND 3)
        );
        INSERT INTO update_events_v2
            SELECT event_id, site_id, issue_id, issue_key, kind, occurred_seconds,
                   occurred_nanos, read_state, notification_delivery, snapshot
            FROM update_events;
        CREATE TABLE event_user_sets_v2 (
            event_id TEXT NOT NULL,
            site_id TEXT NOT NULL,
            user_set_id TEXT NOT NULL,
            PRIMARY KEY (event_id, site_id, user_set_id),
            FOREIGN KEY (event_id, site_id) REFERENCES update_events_v2(event_id, site_id) ON DELETE CASCADE,
            FOREIGN KEY (site_id, user_set_id) REFERENCES user_sets(site_id, id) ON DELETE CASCADE
        );
        INSERT INTO event_user_sets_v2
            SELECT event_id, site_id, user_set_id
            FROM event_user_sets;
        DROP TABLE event_user_sets;
        DROP TABLE update_events;
        ALTER TABLE update_events_v2 RENAME TO update_events;
        ALTER TABLE event_user_sets_v2 RENAME TO event_user_sets;
        CREATE INDEX update_events_feed_idx
            ON update_events(site_id, occurred_seconds DESC, occurred_nanos DESC, event_id ASC);
        CREATE INDEX update_events_unread_idx
            ON update_events(site_id, read_state, occurred_seconds DESC, occurred_nanos DESC);
        CREATE INDEX event_user_sets_lookup_idx
            ON event_user_sets(site_id, user_set_id, event_id);
        ",
    )
}

fn migrate_field_changed_kind(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(include_str!(
        "../../../migrations/0004_field_changed_kind.sql"
    ))
}

fn handle_request(connection: &mut Connection, request: Request) {
    match request {
        Request::ListIssues { query, reply } => send(reply, list_issues(connection, &query)),
        Request::GetIssue {
            site_id,
            issue_id,
            reply,
        } => send(reply, get_issue(connection, &site_id, &issue_id)),
        Request::IssuesForUserSet {
            site_id,
            user_set_id,
            reply,
        } => send(
            reply,
            issues_for_user_set(connection, &site_id, &user_set_id),
        ),
        Request::SyncState {
            site_id,
            user_set_id,
            reply,
        } => send(reply, sync_state(connection, &site_id, &user_set_id)),
        Request::CommitSync { commit, reply } => send(reply, commit_sync(connection, &commit)),
        Request::RecordSyncFailure {
            site_id,
            user_set_id,
            kind,
            at,
            reply,
        } => send(
            reply,
            record_sync_failure(connection, &site_id, &user_set_id, kind, at),
        ),
        Request::RecordNotificationDelivery {
            event_id,
            delivery,
            reply,
        } => send(
            reply,
            record_notification_delivery(connection, &event_id, delivery),
        ),
        Request::ListEvents { query, reply } => send(reply, list_events(connection, &query)),
        Request::UnreadCount { site_id, reply } => send(reply, unread_count(connection, &site_id)),
        Request::MarkRead {
            event_ids,
            read,
            reply,
        } => send(reply, mark_read(connection, &event_ids, read)),
        Request::MarkAllRead { site_id, reply } => send(reply, mark_all_read(connection, &site_id)),
        Request::ListUserSets { site_id, reply } => {
            send(reply, list_user_sets(connection, &site_id))
        }
        Request::SaveUserSet { draft, reply } => send(reply, save_user_set(connection, draft)),
        Request::DeleteUserSet { user_set_id, reply } => {
            send(reply, delete_user_set(connection, &user_set_id))
        }
        Request::CachedAssignableUsers {
            site_id,
            locator,
            reply,
        } => send(
            reply,
            cached_assignable_users(connection, &site_id, &locator),
        ),
        Request::ReplaceAssignableUsers {
            site_id,
            locator,
            users,
            fetched_at,
            reply,
        } => send(
            reply,
            replace_assignable_users(connection, &site_id, &locator, &users, fetched_at),
        ),
        Request::CachedIssueTransitions {
            site_id,
            locator,
            reply,
        } => send(
            reply,
            cached_issue_transitions(connection, &site_id, &locator),
        ),
        Request::ReplaceIssueTransitions {
            site_id,
            locator,
            transitions,
            fetched_at,
            reply,
        } => send(
            reply,
            replace_issue_transitions(connection, &site_id, &locator, &transitions, fetched_at),
        ),
        Request::InvalidateIssueTransitions {
            site_id,
            locator,
            reply,
        } => send(
            reply,
            invalidate_issue_transitions(connection, &site_id, &locator),
        ),
    }
}

fn send<T>(reply: Reply<T>, result: Result<T, ApplicationError>) {
    let _ = reply.send(result);
}

fn locator_parts(locator: &IssueLocator) -> (&'static str, &str) {
    match locator {
        IssueLocator::Id(value) => ("id", value.as_str()),
        IssueLocator::Key(value) => ("key", value.as_str()),
    }
}

fn cached_assignable_users(
    connection: &Connection,
    site_id: &JiraSiteId,
    locator: &IssueLocator,
) -> Result<Option<CachedAssignableUsers>, ApplicationError> {
    let (kind, value) = locator_parts(locator);
    let row = connection
        .query_row(
            "SELECT fetched_seconds, fetched_nanos, snapshot FROM issue_edit_users WHERE site_id = ?1 AND locator_kind = ?2 AND locator_value = ?3",
            params![site_id.as_str(), kind, value],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(|(seconds, nanos, snapshot)| {
        Ok(CachedAssignableUsers {
            users: decode(&snapshot, "assignable users")?,
            fetched_at: from_stamp(seconds, nanos)?,
        })
    })
    .transpose()
}

fn replace_assignable_users(
    connection: &mut Connection,
    site_id: &JiraSiteId,
    locator: &IssueLocator,
    users: &[User],
    fetched_at: Timestamp,
) -> Result<(), ApplicationError> {
    if users.len() > MAX_ASSIGNABLE_USER_SEARCH_LIMIT
        || users.iter().any(|user| user.site_id != *site_id)
    {
        return Err(ApplicationError::invalid_input(
            "cached assignable user belongs to another site",
        ));
    }
    let snapshot = encode(&users, "assignable users")?;
    let (kind, value) = locator_parts(locator);
    let (seconds, nanos) = stamp(fetched_at);
    connection
        .execute(
            "INSERT INTO issue_edit_users (site_id, locator_kind, locator_value, fetched_seconds, fetched_nanos, snapshot) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(site_id, locator_kind, locator_value) DO UPDATE SET fetched_seconds = excluded.fetched_seconds, fetched_nanos = excluded.fetched_nanos, snapshot = excluded.snapshot",
            params![site_id.as_str(), kind, value, seconds, nanos, snapshot],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn cached_issue_transitions(
    connection: &Connection,
    site_id: &JiraSiteId,
    locator: &IssueLocator,
) -> Result<Option<CachedIssueTransitions>, ApplicationError> {
    let (kind, value) = locator_parts(locator);
    let row = connection
        .query_row(
            "SELECT fetched_seconds, fetched_nanos, snapshot FROM issue_edit_transitions WHERE site_id = ?1 AND locator_kind = ?2 AND locator_value = ?3",
            params![site_id.as_str(), kind, value],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(|(seconds, nanos, snapshot)| {
        Ok(CachedIssueTransitions {
            transitions: decode(&snapshot, "issue transitions")?,
            fetched_at: from_stamp(seconds, nanos)?,
        })
    })
    .transpose()
}

fn replace_issue_transitions(
    connection: &mut Connection,
    site_id: &JiraSiteId,
    locator: &IssueLocator,
    transitions: &[IssueTransition],
    fetched_at: Timestamp,
) -> Result<(), ApplicationError> {
    if transitions.len() > MAX_ISSUE_TRANSITIONS {
        return Err(ApplicationError::invalid_input(
            "cached transition list exceeds configured limit",
        ));
    }
    let snapshot = encode(&transitions, "issue transitions")?;
    let (kind, value) = locator_parts(locator);
    let (seconds, nanos) = stamp(fetched_at);
    connection
        .execute(
            "INSERT INTO issue_edit_transitions (site_id, locator_kind, locator_value, fetched_seconds, fetched_nanos, snapshot) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(site_id, locator_kind, locator_value) DO UPDATE SET fetched_seconds = excluded.fetched_seconds, fetched_nanos = excluded.fetched_nanos, snapshot = excluded.snapshot",
            params![site_id.as_str(), kind, value, seconds, nanos, snapshot],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn invalidate_issue_transitions(
    connection: &mut Connection,
    site_id: &JiraSiteId,
    locator: &IssueLocator,
) -> Result<(), ApplicationError> {
    let (kind, value) = locator_parts(locator);
    connection
        .execute(
            "DELETE FROM issue_edit_transitions WHERE site_id = ?1 AND locator_kind = ?2 AND locator_value = ?3",
            params![site_id.as_str(), kind, value],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn list_issues(
    connection: &Connection,
    query: &IssueListQuery,
) -> Result<Vec<Issue>, ApplicationError> {
    let limit = i64::try_from(query.limit)
        .map_err(|_| ApplicationError::invalid_input("issue limit is too large"))?;
    let offset = i64::try_from(query.offset)
        .map_err(|_| ApplicationError::invalid_input("issue offset is too large"))?;
    let mut sql = String::from(
        "SELECT i.snapshot FROM issues i JOIN issue_membership m ON m.site_id = i.site_id AND m.issue_id = i.issue_id WHERE i.site_id = ? AND m.site_id = ? AND m.user_set_id = ?",
    );
    let mut values = vec![
        text(query.site_id.as_str()),
        text(query.site_id.as_str()),
        text(query.user_set_id.as_str()),
    ];
    if let Some(search) = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        let pattern = format!("%{}%", escape_like(search.to_lowercase()));
        sql.push_str(
            " AND (lower(i.issue_key) LIKE ? ESCAPE '\\' OR lower(i.summary) LIKE ? ESCAPE '\\')",
        );
        values.push(text(&pattern));
        values.push(text(&pattern));
    }
    if !query.assignees.is_empty() {
        sql.push_str(" AND i.assignee_id IN (");
        for (index, assignee) in query.assignees.iter().enumerate() {
            if index > 0 {
                sql.push(',');
            }
            sql.push('?');
            values.push(text(assignee.as_str()));
        }
        sql.push(')');
    }
    sql.push_str(
        " ORDER BY i.updated_seconds DESC, i.updated_nanos DESC, i.issue_key ASC LIMIT ? OFFSET ?",
    );
    values.push(Value::Integer(limit));
    values.push(Value::Integer(offset));
    let mut statement = connection.prepare(&sql).map_err(sqlite_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(sqlite_error)?;
    rows.map(|row| {
        row.map_err(sqlite_error)
            .and_then(|snapshot| decode(&snapshot, "issue"))
    })
    .collect()
}

fn get_issue(
    connection: &Connection,
    site_id: &JiraSiteId,
    issue_id: &IssueId,
) -> Result<Option<Issue>, ApplicationError> {
    let snapshot = connection
        .query_row(
            "SELECT snapshot FROM issues WHERE site_id = ?1 AND issue_id = ?2",
            params![site_id.as_str(), issue_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    snapshot.map(|value| decode(&value, "issue")).transpose()
}

fn issues_for_user_set(
    connection: &Connection,
    site_id: &JiraSiteId,
    user_set_id: &UserSetId,
) -> Result<Vec<Issue>, ApplicationError> {
    let mut statement = connection
        .prepare("SELECT i.snapshot FROM issues i JOIN issue_membership m ON m.site_id = i.site_id AND m.issue_id = i.issue_id WHERE m.site_id = ?1 AND m.user_set_id = ?2 ORDER BY i.updated_seconds DESC, i.updated_nanos DESC, i.issue_key ASC")
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![site_id.as_str(), user_set_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sqlite_error)?;
    rows.map(|row| {
        row.map_err(sqlite_error)
            .and_then(|snapshot| decode(&snapshot, "issue"))
    })
    .collect()
}

fn sync_state(
    connection: &Connection,
    site_id: &JiraSiteId,
    user_set_id: &UserSetId,
) -> Result<Option<SyncState>, ApplicationError> {
    let values = connection
        .query_row(
            "SELECT last_incremental_started_seconds, last_incremental_started_nanos, last_incremental_succeeded_seconds, last_incremental_succeeded_nanos, last_full_sync_seconds, last_full_sync_nanos, consecutive_failures, last_error_kind FROM sync_states WHERE site_id = ?1 AND user_set_id = ?2",
            params![site_id.as_str(), user_set_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i32>>(1)?,
                    row.get::<_, Option<i64>>(2)?, row.get::<_, Option<i32>>(3)?,
                    row.get::<_, Option<i64>>(4)?, row.get::<_, Option<i32>>(5)?,
                    row.get::<_, i64>(6)?, row.get::<_, Option<i32>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    values
        .map(
            |(
                started_s,
                started_n,
                succeeded_s,
                succeeded_n,
                full_s,
                full_n,
                failures,
                error_kind,
            )| {
                Ok(SyncState {
                    site_id: site_id.clone(),
                    user_set_id: user_set_id.clone(),
                    last_incremental_started_at: optional_timestamp(started_s, started_n)?,
                    last_incremental_succeeded_at: optional_timestamp(succeeded_s, succeeded_n)?,
                    last_full_sync_at: optional_timestamp(full_s, full_n)?,
                    consecutive_failures: u32::try_from(failures)
                        .map_err(|_| storage_error("stored sync state is invalid"))?,
                    last_error_kind: error_kind.map(error_kind_from_i32).transpose()?,
                })
            },
        )
        .transpose()
}

fn commit_sync(
    connection: &mut Connection,
    commit: &SyncCommit,
) -> Result<CommitOutcome, ApplicationError> {
    if commit.state.site_id != commit.site_id || commit.state.user_set_id != commit.user_set_id {
        return Err(ApplicationError::invalid_input(
            "sync state does not match commit",
        ));
    }
    for issue in &commit.issues {
        if issue.site_id != commit.site_id {
            return Err(ApplicationError::invalid_input(
                "issue site does not match commit",
            ));
        }
    }
    let transaction = connection.transaction().map_err(sqlite_error)?;
    if commit.replace_membership {
        transaction
            .execute(
                "DELETE FROM issue_membership WHERE site_id = ?1 AND user_set_id = ?2",
                params![commit.site_id.as_str(), commit.user_set_id.as_str()],
            )
            .map_err(sqlite_error)?;
    }
    for issue in &commit.issues {
        insert_issue(&transaction, issue)?;
        transaction
            .execute("INSERT OR IGNORE INTO issue_membership (site_id, user_set_id, issue_id) VALUES (?1, ?2, ?3)", params![commit.site_id.as_str(), commit.user_set_id.as_str(), issue.id.as_str()])
            .map_err(sqlite_error)?;
    }
    let mut inserted_events = Vec::new();
    for event in &commit.update_events {
        if event.site_id != commit.site_id {
            return Err(ApplicationError::invalid_input(
                "event site does not match commit",
            ));
        }
        let snapshot = encode(event, "update event")?;
        let changed = transaction
            .execute("INSERT OR IGNORE INTO update_events (event_id, site_id, issue_id, issue_key, kind, occurred_seconds, occurred_nanos, read_state, notification_delivery, snapshot) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)", params![event.id.as_str(), event.site_id.as_str(), event.issue_id.as_str(), event.issue_key.as_str(), kind_tag(&event.kind), stamp(event.occurred_at).0, stamp(event.occurred_at).1, read_state_tag(event.read_state), delivery_tag(event.notification_delivery.clone()), snapshot])
            .map_err(sqlite_error)?;
        if changed == 0 {
            let existing_snapshot = transaction
                .query_row(
                    "SELECT snapshot FROM update_events WHERE event_id = ?1",
                    params![event.id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            let compatible = existing_snapshot
                .map(|snapshot| decode::<UpdateEvent>(&snapshot, "update event"))
                .transpose()?
                .is_some_and(|existing| same_event_identity(&existing, event));
            if !compatible {
                return Err(ApplicationError::invalid_input(
                    "event ID conflicts with a different update event",
                ));
            }
        }
        for user_set_id in &event.matching_user_set_ids {
            transaction
                .execute("INSERT OR IGNORE INTO event_user_sets (event_id, site_id, user_set_id) VALUES (?1, ?2, ?3)", params![event.id.as_str(), event.site_id.as_str(), user_set_id.as_str()])
                .map_err(sqlite_error)?;
        }
        if changed == 1 {
            let mut event = event.clone();
            normalize_matching_user_set_ids(&mut event);
            inserted_events.push(event);
        } else if let Some(inserted) = inserted_events
            .iter_mut()
            .find(|inserted| inserted.id == event.id)
        {
            union_matching_user_set_ids(inserted, event);
        }
    }
    upsert_sync_state(&transaction, &commit.state)?;
    transaction.commit().map_err(sqlite_error)?;
    Ok(CommitOutcome { inserted_events })
}

fn insert_issue(transaction: &Transaction<'_>, issue: &Issue) -> Result<(), ApplicationError> {
    let snapshot = encode(issue, "issue")?;
    transaction
        .execute("INSERT INTO issues (site_id, issue_id, issue_key, summary, assignee_id, updated_seconds, updated_nanos, snapshot) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(site_id, issue_id) DO UPDATE SET issue_key = excluded.issue_key, summary = excluded.summary, assignee_id = excluded.assignee_id, updated_seconds = excluded.updated_seconds, updated_nanos = excluded.updated_nanos, snapshot = excluded.snapshot", params![issue.site_id.as_str(), issue.id.as_str(), issue.key.as_str(), issue.summary, issue.assignee.as_ref().map(|id| id.as_str()), stamp(issue.updated_at).0, stamp(issue.updated_at).1, snapshot])
        .map_err(sqlite_error)?;
    Ok(())
}

fn upsert_sync_state(
    transaction: &Transaction<'_>,
    state: &SyncState,
) -> Result<(), ApplicationError> {
    let started = state.last_incremental_started_at.map(stamp);
    let succeeded = state.last_incremental_succeeded_at.map(stamp);
    let full = state.last_full_sync_at.map(stamp);
    transaction
        .execute("INSERT INTO sync_states (site_id, user_set_id, last_incremental_started_seconds, last_incremental_started_nanos, last_incremental_succeeded_seconds, last_incremental_succeeded_nanos, last_full_sync_seconds, last_full_sync_nanos, consecutive_failures, last_error_kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(site_id, user_set_id) DO UPDATE SET last_incremental_started_seconds = excluded.last_incremental_started_seconds, last_incremental_started_nanos = excluded.last_incremental_started_nanos, last_incremental_succeeded_seconds = excluded.last_incremental_succeeded_seconds, last_incremental_succeeded_nanos = excluded.last_incremental_succeeded_nanos, last_full_sync_seconds = excluded.last_full_sync_seconds, last_full_sync_nanos = excluded.last_full_sync_nanos, consecutive_failures = excluded.consecutive_failures, last_error_kind = excluded.last_error_kind", params![state.site_id.as_str(), state.user_set_id.as_str(), started.map(|v| v.0), started.map(|v| v.1), succeeded.map(|v| v.0), succeeded.map(|v| v.1), full.map(|v| v.0), full.map(|v| v.1), i64::from(state.consecutive_failures), state.last_error_kind.map(error_kind_tag)])
        .map_err(sqlite_error)?;
    Ok(())
}

fn record_sync_failure(
    connection: &mut Connection,
    site_id: &JiraSiteId,
    user_set_id: &UserSetId,
    kind: ErrorKind,
    _at: Timestamp,
) -> Result<(), ApplicationError> {
    connection
        .execute("INSERT INTO sync_states (site_id, user_set_id, consecutive_failures, last_error_kind) VALUES (?1, ?2, 1, ?3) ON CONFLICT(site_id, user_set_id) DO UPDATE SET consecutive_failures = sync_states.consecutive_failures + 1, last_error_kind = excluded.last_error_kind", params![site_id.as_str(), user_set_id.as_str(), error_kind_tag(kind)])
        .map_err(sqlite_error)?;
    Ok(())
}

fn record_notification_delivery(
    connection: &Connection,
    event_id: &EventId,
    delivery: NotificationDelivery,
) -> Result<(), ApplicationError> {
    connection
        .execute(
            "UPDATE update_events SET notification_delivery = ?1 WHERE event_id = ?2",
            params![delivery_tag(delivery), event_id.as_str()],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn list_events(
    connection: &Connection,
    query: &UpdateFeedQuery,
) -> Result<Vec<UpdateEvent>, ApplicationError> {
    let limit = i64::try_from(query.limit)
        .map_err(|_| ApplicationError::invalid_input("event limit is too large"))?;
    let mut sql = String::from(
        "SELECT event_id, snapshot, read_state, notification_delivery FROM update_events WHERE site_id = ?",
    );
    let mut values = vec![text(query.site_id.as_str())];
    if query.unread_only {
        sql.push_str(" AND read_state = 0");
    }
    if let Some(before) = query.before {
        let (seconds, nanos) = stamp(before);
        sql.push_str(
            " AND (occurred_seconds < ? OR (occurred_seconds = ? AND occurred_nanos < ?))",
        );
        values.extend([
            Value::Integer(seconds),
            Value::Integer(seconds),
            Value::Integer(i64::from(nanos)),
        ]);
    }
    if !query.kinds.is_empty() {
        sql.push_str(" AND kind IN (");
        for (index, kind) in query.kinds.iter().enumerate() {
            if index > 0 {
                sql.push(',');
            }
            sql.push('?');
            values.push(Value::Integer(kind_tag(kind)));
        }
        sql.push(')');
    }
    sql.push_str(" ORDER BY occurred_seconds DESC, occurred_nanos DESC, event_id ASC LIMIT ?");
    values.push(Value::Integer(limit));
    let mut statement = connection.prepare(&sql).map_err(sqlite_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut events = Vec::new();
    for row in rows {
        let (event_id, snapshot, read_state, delivery) = row.map_err(sqlite_error)?;
        let mut event: UpdateEvent = decode(&snapshot, "update event")?;
        event.read_state = read_state_from_i64(read_state)?;
        event.notification_delivery = delivery_from_i64(delivery)?;
        let mut member_statement = connection.prepare("SELECT user_set_id FROM event_user_sets WHERE event_id = ?1 AND site_id = ?2 ORDER BY user_set_id").map_err(sqlite_error)?;
        event.matching_user_set_ids = member_statement
            .query_map(params![event_id, query.site_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sqlite_error)?
            .map(|value| {
                value.map_err(sqlite_error).and_then(|id| {
                    UserSetId::new(id).map_err(|_| storage_error("stored update event is invalid"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        events.push(event);
    }
    Ok(events)
}

fn unread_count(connection: &Connection, site_id: &JiraSiteId) -> Result<usize, ApplicationError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM update_events WHERE site_id = ?1 AND read_state = 0",
            params![site_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?;
    usize::try_from(count).map_err(|_| storage_error("stored unread count is invalid"))
}

fn mark_read(
    connection: &mut Connection,
    event_ids: &[EventId],
    read: bool,
) -> Result<usize, ApplicationError> {
    let transaction = connection.transaction().map_err(sqlite_error)?;
    let desired = i64::from(read);
    let mut changed = 0usize;
    for event_id in event_ids {
        changed = changed.saturating_add(transaction.execute("UPDATE update_events SET read_state = ?1 WHERE event_id = ?2 AND read_state != ?1", params![desired, event_id.as_str()]).map_err(sqlite_error)?);
    }
    transaction.commit().map_err(sqlite_error)?;
    Ok(changed)
}

fn mark_all_read(connection: &Connection, site_id: &JiraSiteId) -> Result<usize, ApplicationError> {
    let changed = connection
        .execute(
            "UPDATE update_events SET read_state = 1 WHERE site_id = ?1 AND read_state = 0",
            params![site_id.as_str()],
        )
        .map_err(sqlite_error)?;
    Ok(changed)
}

fn list_user_sets(
    connection: &Connection,
    site_id: &JiraSiteId,
) -> Result<Vec<UserSet>, ApplicationError> {
    let mut statement = connection.prepare("SELECT id, name, created_seconds, created_nanos, updated_seconds, updated_nanos FROM user_sets WHERE site_id = ?1 ORDER BY name ASC, id ASC").map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![site_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i32>(5)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut sets = Vec::new();
    for row in rows {
        let (id, name, created_s, created_n, updated_s, updated_n) = row.map_err(sqlite_error)?;
        let mut member_statement = connection.prepare("SELECT account_id FROM user_set_members WHERE site_id = ?1 AND user_set_id = ?2 ORDER BY member_order ASC").map_err(sqlite_error)?;
        let members = member_statement
            .query_map(params![site_id.as_str(), id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sqlite_error)?
            .map(|value| {
                value.map_err(sqlite_error).and_then(|id| {
                    jira_domain::AccountId::new(id)
                        .map_err(|_| storage_error("stored user set is invalid"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        sets.push(UserSet {
            id: UserSetId::new(id).map_err(|_| storage_error("stored user set is invalid"))?,
            site_id: site_id.clone(),
            name,
            members,
            created_at: from_stamp(created_s, created_n)?,
            updated_at: from_stamp(updated_s, updated_n)?,
        });
    }
    Ok(sets)
}

fn save_user_set(
    connection: &mut Connection,
    draft: UserSetDraft,
) -> Result<UserSet, ApplicationError> {
    let now = OffsetDateTime::now_utc();
    let id = next_user_set_id(connection)?;
    let set = UserSet::new(id, draft.site_id, draft.name, draft.members, now)
        .map_err(|error| ApplicationError::invalid_input(error.to_string()))?;
    let transaction = connection.transaction().map_err(sqlite_error)?;
    let (created_s, created_n) = stamp(set.created_at);
    transaction.execute("INSERT INTO user_sets (site_id, id, name, created_seconds, created_nanos, updated_seconds, updated_nanos) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![set.site_id.as_str(), set.id.as_str(), set.name, created_s, created_n, stamp(set.updated_at).0, stamp(set.updated_at).1]).map_err(sqlite_error)?;
    for (order, member) in set.members.iter().enumerate() {
        let order =
            i64::try_from(order).map_err(|_| storage_error("user set has too many members"))?;
        transaction.execute("INSERT INTO user_set_members (site_id, user_set_id, member_order, account_id) VALUES (?1, ?2, ?3, ?4)", params![set.site_id.as_str(), set.id.as_str(), order, member.as_str()]).map_err(sqlite_error)?;
    }
    transaction.commit().map_err(sqlite_error)?;
    Ok(set)
}

fn next_user_set_id(connection: &Connection) -> Result<UserSetId, ApplicationError> {
    let mut number: i64 = connection
        .query_row("SELECT COUNT(*) FROM user_sets", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(sqlite_error)?
        .saturating_add(1);
    loop {
        let candidate = format!("user-set-{number}");
        let present: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM user_sets WHERE id = ?1)",
                params![candidate.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(sqlite_error)?;
        if !present {
            return UserSetId::new(candidate)
                .map_err(|_| storage_error("could not allocate user set id"));
        }
        number = number.saturating_add(1);
    }
}

fn delete_user_set(
    connection: &mut Connection,
    user_set_id: &UserSetId,
) -> Result<(), ApplicationError> {
    let transaction = connection.transaction().map_err(sqlite_error)?;
    transaction
        .execute(
            "DELETE FROM issue_membership WHERE user_set_id = ?1",
            params![user_set_id.as_str()],
        )
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "DELETE FROM sync_states WHERE user_set_id = ?1",
            params![user_set_id.as_str()],
        )
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "DELETE FROM event_user_sets WHERE user_set_id = ?1",
            params![user_set_id.as_str()],
        )
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "DELETE FROM user_set_members WHERE user_set_id = ?1",
            params![user_set_id.as_str()],
        )
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "DELETE FROM user_sets WHERE id = ?1",
            params![user_set_id.as_str()],
        )
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)?;
    Ok(())
}

fn encode<T: serde::Serialize>(value: &T, what: &str) -> Result<String, ApplicationError> {
    serde_json::to_string(value).map_err(|_| storage_error(format!("could not encode {what}")))
}

fn decode<T: serde::de::DeserializeOwned>(value: &str, what: &str) -> Result<T, ApplicationError> {
    serde_json::from_str(value)
        .map_err(|_| storage_error(format!("could not decode stored {what}")))
}

fn stamp(timestamp: Timestamp) -> (i64, i32) {
    (timestamp.unix_timestamp(), timestamp.nanosecond() as i32)
}

fn from_stamp(seconds: i64, nanos: i32) -> Result<Timestamp, ApplicationError> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| storage_error("stored timestamp is invalid"))?
        .replace_nanosecond(
            u32::try_from(nanos).map_err(|_| storage_error("stored timestamp is invalid"))?,
        )
        .map(|timestamp| timestamp.to_offset(UtcOffset::UTC))
        .map_err(|_| storage_error("stored timestamp is invalid"))
}

fn optional_timestamp(
    seconds: Option<i64>,
    nanos: Option<i32>,
) -> Result<Option<Timestamp>, ApplicationError> {
    match (seconds, nanos) {
        (None, None) => Ok(None),
        (Some(seconds), Some(nanos)) => from_stamp(seconds, nanos).map(Some),
        _ => Err(storage_error("stored sync state is invalid")),
    }
}

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

fn escape_like(value: String) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn kind_tag(kind: &UpdateKind) -> i64 {
    match kind {
        UpdateKind::IssueAddedToView => 0,
        UpdateKind::IssueRemovedFromView => 1,
        UpdateKind::IssueUpdated => 9,
        UpdateKind::FieldChanged { .. } => 10,
        UpdateKind::StatusChanged { .. } => 2,
        UpdateKind::AssigneeChanged { .. } => 3,
        UpdateKind::PriorityChanged { .. } => 4,
        UpdateKind::DueDateChanged { .. } => 5,
        UpdateKind::SummaryChanged { .. } => 6,
        UpdateKind::ParentChanged { .. } => 7,
        UpdateKind::CommentAdded { .. } => 8,
    }
}

fn read_state_tag(state: UpdateReadState) -> i64 {
    i64::from(matches!(state, UpdateReadState::Read))
}

fn read_state_from_i64(value: i64) -> Result<UpdateReadState, ApplicationError> {
    match value {
        0 => Ok(UpdateReadState::Unread),
        1 => Ok(UpdateReadState::Read),
        _ => Err(storage_error("stored update event is invalid")),
    }
}

fn delivery_tag(delivery: NotificationDelivery) -> i64 {
    match delivery {
        NotificationDelivery::NotAttempted => 0,
        NotificationDelivery::Delivered => 1,
        NotificationDelivery::Unavailable => 2,
        NotificationDelivery::SuppressedByPolicy => 3,
    }
}

fn delivery_from_i64(value: i64) -> Result<NotificationDelivery, ApplicationError> {
    match value {
        0 => Ok(NotificationDelivery::NotAttempted),
        1 => Ok(NotificationDelivery::Delivered),
        2 => Ok(NotificationDelivery::Unavailable),
        3 => Ok(NotificationDelivery::SuppressedByPolicy),
        _ => Err(storage_error("stored update event is invalid")),
    }
}
fn error_kind_tag(kind: ErrorKind) -> i64 {
    match kind {
        ErrorKind::Authentication => 0,
        ErrorKind::Authorization => 1,
        ErrorKind::RateLimited => 2,
        ErrorKind::Offline => 3,
        ErrorKind::Cancelled => 4,
        ErrorKind::InvalidInput => 5,
        ErrorKind::NotFound => 6,
        // Comment-write outcomes are never sync-state categories. Keep this
        // defensive fallback on the existing Upstream tag for old databases.
        ErrorKind::UnknownOutcome => 8,
        ErrorKind::Storage => 7,
        ErrorKind::Upstream => 8,
        ErrorKind::Notification => 9,
        ErrorKind::Internal => 10,
    }
}

fn error_kind_from_i32(value: i32) -> Result<ErrorKind, ApplicationError> {
    match value {
        0 => Ok(ErrorKind::Authentication),
        1 => Ok(ErrorKind::Authorization),
        2 => Ok(ErrorKind::RateLimited),
        3 => Ok(ErrorKind::Offline),
        4 => Ok(ErrorKind::Cancelled),
        5 => Ok(ErrorKind::InvalidInput),
        6 => Ok(ErrorKind::NotFound),
        7 => Ok(ErrorKind::Storage),
        8 => Ok(ErrorKind::Upstream),
        9 => Ok(ErrorKind::Notification),
        10 => Ok(ErrorKind::Internal),
        _ => Err(storage_error("stored sync state is invalid")),
    }
}

fn sqlite_error(_: rusqlite::Error) -> ApplicationError {
    storage_error("local storage operation failed")
}
fn storage_error(message: impl Into<String>) -> ApplicationError {
    ApplicationError::new(ErrorKind::Storage, message)
}

#[cfg(test)]
mod tests {
    use futures_lite::future::block_on;
    use jira_application::{
        ErrorKind, IssueCachePort, IssueEditCachePort, IssueListQuery, IssueLocator,
        IssueTransition, SyncCommit, SyncState, UpdateFeedPort, UpdateFeedQuery, UserSetDraft,
        UserSetPort,
    };
    use jira_domain::{
        AccountId, EventId, Issue, IssueId, IssueKey, IssueType, JiraSiteId, NotificationDelivery,
        Priority, Project, RichBlock, RichInline, RichTextDocument, Status, Timestamp, UpdateEvent,
        UpdateKind, UpdateReadState, User, UserSetId,
    };
    use tempfile::tempdir;
    use time::macros::datetime;

    use super::{SqliteStore, initialize_connection, normalize_database_path};

    fn site(value: &str) -> JiraSiteId {
        JiraSiteId::new(value).expect("valid site")
    }
    fn set_id(value: &str) -> UserSetId {
        UserSetId::new(value).expect("valid set")
    }
    fn issue(site_id: JiraSiteId, id: &str, summary: &str, updated_at: Timestamp) -> Issue {
        Issue::new(
            site_id,
            IssueId::new(id).expect("valid issue id"),
            IssueKey::new(format!("APP-{id}")).expect("valid issue key"),
            Project {
                id: "10".into(),
                key: "APP".into(),
                name: "Application".into(),
            },
            IssueType {
                id: "1".into(),
                name: "Task".into(),
                icon_url: None,
            },
            summary,
            Status {
                id: "open".into(),
                name: "Open".into(),
                category: None,
            },
            Priority {
                id: Some("2".into()),
                name: Some("Medium".into()),
                icon_url: None,
            },
            Some(AccountId::new("account-a").expect("valid account")),
            None,
            None,
            vec!["local".into()],
            datetime!(2026-01-01 00:00 UTC),
            updated_at,
            None,
        )
    }

    fn saved_set(store: &SqliteStore, site_id: JiraSiteId) -> UserSetId {
        block_on(store.save(UserSetDraft {
            site_id,
            name: "Team".into(),
            members: vec![AccountId::new("account-a").expect("valid account")],
        }))
        .expect("save set")
        .id
    }

    #[test]
    fn list_issues_orders_by_latest_update_and_keeps_pagination_global() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());
        let newest_high_key = issue(
            site_id.clone(),
            "301",
            "Newest high key",
            datetime!(2026-01-03 00:00 UTC),
        );
        let oldest = issue(
            site_id.clone(),
            "100",
            "Oldest",
            datetime!(2026-01-01 00:00 UTC),
        );
        let newest_low_key = issue(
            site_id.clone(),
            "300",
            "Newest low key",
            datetime!(2026-01-03 00:00 UTC),
        );

        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![newest_high_key, oldest, newest_low_key],
            update_events: vec![],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id.clone()),
        }))
        .expect("commit succeeds");

        let page = |limit, offset| IssueListQuery {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            text: None,
            assignees: vec![],
            limit,
            offset,
        };
        let keys = |issues: Vec<Issue>| {
            issues
                .into_iter()
                .map(|issue| issue.key.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(block_on(store.list_issues(&page(2, 0))).expect("first page")),
            vec!["APP-300", "APP-301"]
        );
        assert_eq!(
            keys(block_on(store.list_issues(&page(2, 2))).expect("second page")),
            vec!["APP-100"]
        );
    }

    #[test]
    fn migrations_enable_pragmas_and_reject_newer_schema() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("cache.sqlite");
        let store = SqliteStore::open(&path).expect("open store");
        drop(store);
        let connection = rusqlite::Connection::open(&path).expect("open sqlite");
        let version: i32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, 4);
        let foreign_keys: i32 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign keys");
        assert_eq!(foreign_keys, 1);
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal_mode.to_lowercase(), "wal");
        let schema = connection
            .prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL")
            .expect("schema query")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("schema rows")
            .map(|row| row.expect("schema row"))
            .collect::<Vec<_>>();
        let schema_text = schema
            .iter()
            .map(|(name, sql)| format!("{name} {sql}"))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert!(!schema_text.contains("credential"));
        assert!(!schema_text.contains("token"));
        assert!(!schema_text.contains("password"));
        let normalized = normalize_database_path(&path).expect("normalized path");
        let mut verified = rusqlite::Connection::open_with_flags(
            normalized,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .expect("verified connection");
        initialize_connection(&mut verified, true).expect("verified pragmas");
        assert_eq!(
            verified
                .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i32>(0))
                .expect("foreign keys"),
            1
        );
        assert_eq!(
            verified
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .expect("journal mode")
                .to_lowercase(),
            "wal"
        );
        connection
            .execute_batch("PRAGMA user_version = 5")
            .expect("set newer version");
        drop(connection);
        assert!(SqliteStore::open(&path).is_err());
    }

    #[test]
    fn unknown_outcome_uses_legacy_upstream_sync_tag() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());

        block_on(store.record_sync_failure(
            &site_id,
            &user_set_id,
            ErrorKind::UnknownOutcome,
            datetime!(2026-01-03 02:00 UTC),
        ))
        .expect("record failure");

        let persisted_state = block_on(store.sync_state(&site_id, &user_set_id))
            .expect("sync state")
            .expect("stored state");
        assert_eq!(persisted_state.consecutive_failures, 1);
        assert_eq!(persisted_state.last_error_kind, Some(ErrorKind::Upstream));
    }

    #[cfg(unix)]
    #[test]
    fn file_store_rejects_symlink_database_paths() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("tempdir");
        let target = directory.path().join("target.sqlite");
        let link = directory.path().join("link.sqlite");
        let store = SqliteStore::open(&target).expect("open target");
        drop(store);
        symlink(&target, &link).expect("create symlink");
        assert!(SqliteStore::open(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn newly_created_database_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("permissions.sqlite");
        let store = SqliteStore::open(&path).expect("open store");
        drop(store);
        assert_eq!(
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn commits_round_trip_and_are_idempotent() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());
        let mut cached_issue = issue(
            site_id.clone(),
            "100",
            "Round trip",
            datetime!(2026-01-03 00:00:00.123 UTC),
        );
        cached_issue.description_text = Some("description".into());
        cached_issue.assignee_display_name = Some("Amina Yusuf".into());
        cached_issue.reporter = Some(AccountId::new("account-reporter").expect("valid account"));
        cached_issue.reporter_display_name = Some("Nina Smith".into());
        cached_issue.rich_description = Some(RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::Text {
                text: "A bounded rich description.".into(),
                marks: vec![],
            }])],
            false,
        ));
        cached_issue.resolution_name = Some("Done".into());
        cached_issue.lifecycle = jira_domain::IssueLifecycle::RemovedFromView;
        let event = UpdateEvent::new(
            EventId::new("event-1").expect("event"),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![user_set_id.clone()],
        );
        let mut state = SyncState::new(site_id.clone(), user_set_id.clone());
        state.last_incremental_started_at = Some(datetime!(2026-01-03 01:00 UTC));
        state.last_incremental_succeeded_at = Some(datetime!(2026-01-03 01:01 UTC));
        state.last_full_sync_at = Some(datetime!(2026-01-02 01:00 UTC));
        state.consecutive_failures = 2;
        state.last_error_kind = Some(ErrorKind::Offline);
        let expected_state = state.clone();
        let commit = SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![cached_issue.clone()],
            update_events: vec![event.clone()],
            replace_membership: true,
            state,
        };
        let outcome = block_on(store.commit_sync(commit.clone())).expect("commit");
        assert_eq!(outcome.inserted_events, vec![event]);
        assert!(
            block_on(store.commit_sync(commit))
                .expect("idempotent commit")
                .inserted_events
                .is_empty()
        );
        let restored = block_on(store.get_issue(&site_id, &cached_issue.id))
            .expect("get issue")
            .expect("stored issue");
        assert_eq!(
            restored.assignee_display_name.as_deref(),
            Some("Amina Yusuf")
        );
        assert_eq!(
            restored.reporter_display_name.as_deref(),
            Some("Nina Smith")
        );
        assert_eq!(restored.rich_description, cached_issue.rich_description);
        assert_eq!(restored, cached_issue);
        assert_eq!(
            block_on(store.issues_for_user_set(&site_id, &user_set_id)).expect("members"),
            vec![cached_issue]
        );
        assert_eq!(
            block_on(store.sync_state(&site_id, &user_set_id))
                .expect("sync state")
                .expect("stored state")
                .last_incremental_started_at,
            expected_state.last_incremental_started_at
        );
        let persisted_state = block_on(store.sync_state(&site_id, &user_set_id))
            .expect("sync state")
            .expect("stored state");
        assert_eq!(
            persisted_state.last_incremental_succeeded_at,
            expected_state.last_incremental_succeeded_at
        );
        assert_eq!(
            persisted_state.last_full_sync_at,
            expected_state.last_full_sync_at
        );
        assert_eq!(persisted_state.consecutive_failures, 2);
        assert_eq!(persisted_state.last_error_kind, Some(ErrorKind::Offline));
        block_on(store.record_sync_failure(
            &site_id,
            &user_set_id,
            ErrorKind::Upstream,
            datetime!(2026-01-03 02:00 UTC),
        ))
        .expect("record failure");
        let persisted_state = block_on(store.sync_state(&site_id, &user_set_id))
            .expect("sync state")
            .expect("stored state");
        assert_eq!(persisted_state.consecutive_failures, 3);
        assert_eq!(persisted_state.last_error_kind, Some(ErrorKind::Upstream));
        let events = block_on(UpdateFeedPort::list(
            &store,
            &UpdateFeedQuery {
                site_id: site_id.clone(),
                unread_only: false,
                kinds: vec![],
                before: None,
                limit: 10,
            },
        ))
        .expect("events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].matching_user_set_ids, vec![user_set_id]);
    }

    #[test]
    fn issue_updated_kind_round_trips_through_update_feed_storage() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());
        let cached_issue = issue(
            site_id.clone(),
            "100",
            "Timestamp-only update",
            datetime!(2026-01-03 00:00 UTC),
        );
        let event = UpdateEvent::new(
            EventId::new("event-updated").expect("event"),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueUpdated,
            datetime!(2026-01-03 00:00 UTC),
            vec![user_set_id.clone()],
        );

        let outcome = block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![cached_issue],
            update_events: vec![event.clone()],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id.clone()),
        }))
        .expect("commit");
        assert_eq!(outcome.inserted_events, vec![event]);

        let events = block_on(UpdateFeedPort::list(
            &store,
            &UpdateFeedQuery {
                site_id,
                unread_only: false,
                kinds: vec![UpdateKind::IssueUpdated],
                before: None,
                limit: 10,
            },
        ))
        .expect("events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, UpdateKind::IssueUpdated);
    }

    #[test]
    fn kind_range_migration_preserves_existing_events_and_associations() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("kind-migration.sqlite");
        let store = SqliteStore::open(&path).expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());
        let cached_issue = issue(
            site_id.clone(),
            "100",
            "Existing event",
            datetime!(2026-01-03 00:00 UTC),
        );
        let event = UpdateEvent::new(
            EventId::new("event-existing").expect("event"),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![user_set_id.clone()],
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![cached_issue],
            update_events: vec![event.clone()],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id),
        }))
        .expect("commit");
        drop(store);

        let connection = rusqlite::Connection::open(&path).expect("open raw database");
        connection
            .execute_batch("PRAGMA user_version = 1")
            .expect("mark database as legacy version");
        drop(connection);

        let reopened = SqliteStore::open(&path).expect("migrate store");
        let events = block_on(UpdateFeedPort::list(
            &reopened,
            &UpdateFeedQuery {
                site_id,
                unread_only: false,
                kinds: vec![UpdateKind::IssueAddedToView],
                before: None,
                limit: 10,
            },
        ))
        .expect("events");
        assert_eq!(events, vec![event]);
    }

    #[test]
    fn file_store_reopens_with_durable_snapshots_and_rolls_back_failures() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("cache.sqlite");
        let store = SqliteStore::open(&path).expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());
        let cached_issue = issue(
            site_id.clone(),
            "100",
            "Durable",
            datetime!(2026-01-03 00:00 UTC),
        );
        let bad_event = UpdateEvent::new(
            EventId::new("bad-event").expect("event"),
            site("different-site"),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![user_set_id.clone()],
        );
        let failed = block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![cached_issue.clone()],
            update_events: vec![bad_event],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id.clone()),
        }));
        assert!(failed.is_err());
        assert!(
            block_on(store.get_issue(&site_id, &cached_issue.id))
                .expect("lookup")
                .is_none()
        );
        let good_event = UpdateEvent::new(
            EventId::new("good-event").expect("event"),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![user_set_id.clone()],
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![cached_issue.clone()],
            update_events: vec![good_event],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id),
        }))
        .expect("good commit");
        drop(store);
        let reopened = SqliteStore::open(&path).expect("reopen store");
        assert_eq!(
            block_on(reopened.get_issue(&site_id, &cached_issue.id)).expect("lookup"),
            Some(cached_issue)
        );
    }

    #[test]
    fn repeated_event_ids_union_matching_user_sets() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let first_set = saved_set(&store, site_id.clone());
        let second_set = block_on(store.save(UserSetDraft {
            site_id: site_id.clone(),
            name: "Other".into(),
            members: vec![AccountId::new("account-b").expect("account")],
        }))
        .expect("save set")
        .id;
        let cached_issue = issue(
            site_id.clone(),
            "100",
            "Issue",
            datetime!(2026-01-03 00:00 UTC),
        );
        let event = UpdateEvent::new(
            EventId::new("event-1").expect("event"),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![first_set.clone()],
        );
        let mut second_event = event.clone();
        second_event.matching_user_set_ids = vec![second_set.clone()];
        let first_outcome = block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: first_set.clone(),
            issues: vec![cached_issue.clone()],
            update_events: vec![event, second_event.clone()],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), first_set),
        }))
        .expect("first commit");
        assert_eq!(first_outcome.inserted_events.len(), 1);
        assert_eq!(
            first_outcome.inserted_events[0].matching_user_set_ids.len(),
            2
        );
        let outcome = block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: second_set.clone(),
            issues: vec![cached_issue],
            update_events: vec![second_event],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), second_set),
        }))
        .expect("second commit");
        assert!(outcome.inserted_events.is_empty());
        let events = block_on(UpdateFeedPort::list(
            &store,
            &UpdateFeedQuery {
                site_id,
                unread_only: false,
                kinds: vec![],
                before: None,
                limit: 10,
            },
        ))
        .expect("events");
        assert_eq!(events[0].matching_user_set_ids.len(), 2);
    }

    #[test]
    fn conflicting_duplicate_event_rolls_back_and_preserves_associations() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let first_set = saved_set(&store, site_id.clone());
        let second_set = block_on(store.save(UserSetDraft {
            site_id: site_id.clone(),
            name: "Other".into(),
            members: vec![AccountId::new("account-b").expect("account")],
        }))
        .expect("save set")
        .id;
        let cached_issue = issue(
            site_id.clone(),
            "100",
            "Issue",
            datetime!(2026-01-03 00:00 UTC),
        );
        let first_event = UpdateEvent::new(
            EventId::new("event-conflict").expect("event"),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![first_set.clone()],
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: first_set.clone(),
            issues: vec![cached_issue.clone()],
            update_events: vec![first_event.clone()],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), first_set),
        }))
        .expect("first commit");
        let conflicting_event = UpdateEvent::new(
            first_event.id.clone(),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::StatusChanged {
                old: jira_domain::ChangeValue::Text("Open".into()),
                new: jira_domain::ChangeValue::Text("Done".into()),
            },
            datetime!(2026-01-04 00:00 UTC),
            vec![second_set.clone()],
        );
        assert!(
            block_on(store.commit_sync(SyncCommit {
                site_id: site_id.clone(),
                user_set_id: second_set.clone(),
                issues: vec![cached_issue],
                update_events: vec![conflicting_event],
                replace_membership: true,
                state: SyncState::new(site_id.clone(), second_set.clone()),
            }))
            .is_err()
        );
        let events = block_on(UpdateFeedPort::list(
            &store,
            &UpdateFeedQuery {
                site_id: site_id.clone(),
                unread_only: false,
                kinds: vec![],
                before: None,
                limit: 10,
            },
        ))
        .expect("events");
        assert_eq!(
            events[0].matching_user_set_ids,
            vec![first_event.matching_user_set_ids[0].clone()]
        );
        assert!(
            block_on(store.issues_for_user_set(&site_id, &second_set))
                .expect("membership")
                .is_empty()
        );
    }

    #[test]
    fn user_sets_round_trip_member_order_and_site_isolation() {
        let store = SqliteStore::in_memory().expect("open store");
        let first_site = site("site-a");
        let second_site = site("site-b");
        let first = block_on(store.save(UserSetDraft {
            site_id: first_site.clone(),
            name: "Platform".into(),
            members: vec![
                AccountId::new("second").expect("account"),
                AccountId::new("first").expect("account"),
            ],
        }))
        .expect("save first");
        let second = block_on(store.save(UserSetDraft {
            site_id: second_site.clone(),
            name: "Platform".into(),
            members: vec![AccountId::new("other").expect("account")],
        }))
        .expect("save second");
        assert_eq!(
            block_on(UserSetPort::list(&store, &first_site)).expect("list first"),
            vec![first.clone()]
        );
        assert_eq!(
            block_on(UserSetPort::list(&store, &second_site)).expect("list second"),
            vec![second]
        );
        assert_eq!(first.members[0].as_str(), "second");
        assert_eq!(first.members[1].as_str(), "first");
    }

    #[test]
    fn issue_queries_isolate_membership_and_support_filters() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let set_one = saved_set(&store, site_id.clone());
        let set_two = block_on(store.save(UserSetDraft {
            site_id: site_id.clone(),
            name: "Other".into(),
            members: vec![AccountId::new("account-b").expect("account")],
        }))
        .expect("save set")
        .id;
        let first = issue(
            site_id.clone(),
            "101",
            "Alpha issue",
            datetime!(2026-01-02 00:00 UTC),
        );
        let second = issue(
            site_id.clone(),
            "102",
            "Beta issue",
            datetime!(2026-01-03 00:00 UTC),
        );
        let state = SyncState::new(site_id.clone(), set_one.clone());
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: set_one.clone(),
            issues: vec![first.clone()],
            update_events: vec![],
            replace_membership: true,
            state,
        }))
        .expect("commit one");
        let state = SyncState::new(site_id.clone(), set_two.clone());
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: set_two.clone(),
            issues: vec![second.clone()],
            update_events: vec![],
            replace_membership: true,
            state,
        }))
        .expect("commit two");
        let missing = set_id("never-synced");
        assert!(
            block_on(store.issues_for_user_set(&site_id, &missing))
                .expect("missing set")
                .is_empty()
        );
        let query = IssueListQuery {
            site_id,
            user_set_id: set_one,
            text: Some("alpha".into()),
            assignees: vec![AccountId::new("account-a").expect("account")],
            limit: 10,
            offset: 0,
        };
        assert_eq!(
            block_on(store.list_issues(&query)).expect("query"),
            vec![first]
        );
    }

    #[test]
    fn membership_replace_and_extend_have_distinct_semantics() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());
        let first = issue(
            site_id.clone(),
            "101",
            "First",
            datetime!(2026-01-02 00:00 UTC),
        );
        let second = issue(
            site_id.clone(),
            "102",
            "Second",
            datetime!(2026-01-03 00:00 UTC),
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![first],
            update_events: vec![],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id.clone()),
        }))
        .expect("replace commit");
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![second.clone()],
            update_events: vec![],
            replace_membership: false,
            state: SyncState::new(site_id.clone(), user_set_id.clone()),
        }))
        .expect("extend commit");
        assert_eq!(
            block_on(store.issues_for_user_set(&site_id, &user_set_id))
                .expect("members")
                .len(),
            2
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![second.clone()],
            update_events: vec![],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id.clone()),
        }))
        .expect("replace commit");
        assert_eq!(
            block_on(store.issues_for_user_set(&site_id, &user_set_id)).expect("members"),
            vec![second]
        );
    }

    #[test]
    fn update_feed_state_changes_return_exact_counts() {
        let store = SqliteStore::in_memory().expect("open store");
        let site_id = site("site-a");
        let user_set_id = saved_set(&store, site_id.clone());
        let cached_issue = issue(
            site_id.clone(),
            "100",
            "Issue",
            datetime!(2026-01-03 00:00 UTC),
        );
        let event = UpdateEvent::new(
            EventId::new("event-1").expect("event"),
            site_id.clone(),
            cached_issue.id.clone(),
            cached_issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![user_set_id.clone()],
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id,
            issues: vec![cached_issue],
            update_events: vec![event.clone()],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), set_id("user-set-1")),
        }))
        .expect("commit");
        assert_eq!(block_on(store.unread_count(&site_id)).expect("count"), 1);
        assert_eq!(
            block_on(store.mark_read(std::slice::from_ref(&event.id), true)).expect("mark"),
            1
        );
        assert_eq!(
            block_on(store.mark_read(std::slice::from_ref(&event.id), true))
                .expect("idempotent mark"),
            0
        );
        block_on(store.record_notification_delivery(
            &event.id,
            NotificationDelivery::Delivered,
            datetime!(2026-01-03 00:00 UTC),
        ))
        .expect("delivery");
        let events = block_on(UpdateFeedPort::list(
            &store,
            &UpdateFeedQuery {
                site_id,
                unread_only: false,
                kinds: vec![],
                before: None,
                limit: 10,
            },
        ))
        .expect("events");
        assert_eq!(
            events[0].notification_delivery,
            NotificationDelivery::Delivered
        );
        assert_eq!(events[0].read_state, UpdateReadState::Read);
    }

    #[test]
    fn issue_edit_cache_persists_and_isolates_locator_kind_and_site() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("edit-cache.sqlite");
        let site_a = site("site-a");
        let site_b = site("site-b");
        let id_locator = IssueLocator::Id(IssueId::new("100").expect("id"));
        let key_locator = IssueLocator::Key(IssueKey::new("APP-100").expect("key"));
        let fetched_at = datetime!(2026-01-03 00:00 UTC);
        let users = vec![User::new(
            site_a.clone(),
            AccountId::new("acct-1").expect("account"),
            "Alice Example",
            None,
            true,
        )];
        let transitions = vec![IssueTransition {
            id: "31".into(),
            name: "In progress".into(),
            to: Status {
                id: "3".into(),
                name: "In Progress".into(),
                category: None,
            },
        }];
        {
            let store = SqliteStore::open(&path).expect("open");
            block_on(store.replace_assignable_users(
                &site_a,
                &id_locator,
                users.clone(),
                fetched_at,
            ))
            .expect("replace users");
            block_on(store.replace_issue_transitions(
                &site_a,
                &key_locator,
                transitions.clone(),
                fetched_at,
            ))
            .expect("replace transitions");
            assert!(
                block_on(store.cached_assignable_users(&site_a, &key_locator))
                    .expect("different locator")
                    .is_none()
            );
            assert!(
                block_on(store.cached_assignable_users(&site_b, &id_locator))
                    .expect("different site")
                    .is_none()
            );
        }
        let reopened = SqliteStore::open(&path).expect("reopen");
        assert_eq!(
            block_on(reopened.cached_assignable_users(&site_a, &id_locator))
                .expect("users")
                .expect("cached")
                .users,
            users
        );
        assert_eq!(
            block_on(reopened.cached_issue_transitions(&site_a, &key_locator))
                .expect("transitions")
                .expect("cached")
                .transitions,
            transitions
        );
        block_on(reopened.invalidate_issue_transitions(&site_a, &key_locator)).expect("invalidate");
        assert!(
            block_on(reopened.cached_issue_transitions(&site_a, &key_locator))
                .expect("read")
                .is_none()
        );
    }
}
