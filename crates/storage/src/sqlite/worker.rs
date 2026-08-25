use std::{path::PathBuf, sync::mpsc, thread};

use futures_channel::oneshot;
use jira_application::{
    ApplicationError, CachedAssignableUsers, CachedIssueTransitions, CommitOutcome, ErrorKind,
    IssueListQuery, IssueLocator, IssueTransition, PortFuture, SyncCommit, SyncState,
    UpdateFeedQuery, UserSetDraft,
};
use jira_domain::{
    EventId, Issue, IssueId, JiraSiteId, NotificationDelivery, Timestamp, UpdateEvent, User,
    UserSet, UserSetId,
};
use rusqlite::{Connection, OpenFlags};

use super::{SqliteOpenError, ensure_database_file, migrations};

pub(super) enum Request {
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

type Reply<T> = oneshot::Sender<Result<T, ApplicationError>>;

pub(super) struct Worker {
    pub(super) sender: mpsc::Sender<Request>,
}

pub(super) fn start(path: Option<PathBuf>) -> Result<Worker, SqliteOpenError> {
    let (ready_sender, ready_receiver) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("jira-sqlite".to_owned())
        .spawn(move || {
            if let Some(path) = path.as_deref()
                && ensure_database_file(path).is_err()
            {
                let _ =
                    ready_sender.send(Err(SqliteOpenError::new("could not open local storage")));
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
            if migrations::initialize_connection(&mut connection, path.is_some()).is_err() {
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
    drop(worker);
    Ok(Worker { sender })
}

pub(super) fn dispatch<T: Send + 'static>(
    sender: mpsc::Sender<Request>,
    request: Request,
    receiver: oneshot::Receiver<Result<T, ApplicationError>>,
) -> PortFuture<'static, T> {
    if sender.send(request).is_err() {
        return Box::pin(async { Err(super::storage_error("storage worker is unavailable")) });
    }
    Box::pin(async move {
        receiver
            .await
            .unwrap_or_else(|_| Err(super::storage_error("storage worker stopped")))
    })
}

fn handle_request(connection: &mut Connection, request: Request) {
    match request {
        Request::ListIssues { query, reply } => {
            send(reply, super::issue_sync::list_issues(connection, &query))
        }
        Request::GetIssue {
            site_id,
            issue_id,
            reply,
        } => send(
            reply,
            super::issue_sync::get_issue(connection, &site_id, &issue_id),
        ),
        Request::IssuesForUserSet {
            site_id,
            user_set_id,
            reply,
        } => send(
            reply,
            super::issue_sync::issues_for_user_set(connection, &site_id, &user_set_id),
        ),
        Request::SyncState {
            site_id,
            user_set_id,
            reply,
        } => send(
            reply,
            super::issue_sync::sync_state(connection, &site_id, &user_set_id),
        ),
        Request::CommitSync { commit, reply } => {
            send(reply, super::issue_sync::commit_sync(connection, &commit))
        }
        Request::RecordSyncFailure {
            site_id,
            user_set_id,
            kind,
            at,
            reply,
        } => send(
            reply,
            super::issue_sync::record_sync_failure(connection, &site_id, &user_set_id, kind, at),
        ),
        Request::RecordNotificationDelivery {
            event_id,
            delivery,
            reply,
        } => send(
            reply,
            super::update_feed::record_notification_delivery(connection, &event_id, delivery),
        ),
        Request::ListEvents { query, reply } => {
            send(reply, super::update_feed::list_events(connection, &query))
        }
        Request::UnreadCount { site_id, reply } => send(
            reply,
            super::update_feed::unread_count(connection, &site_id),
        ),
        Request::MarkRead {
            event_ids,
            read,
            reply,
        } => send(
            reply,
            super::update_feed::mark_read(connection, &event_ids, read),
        ),
        Request::MarkAllRead { site_id, reply } => send(
            reply,
            super::update_feed::mark_all_read(connection, &site_id),
        ),
        Request::ListUserSets { site_id, reply } => send(
            reply,
            super::user_sets::list_user_sets(connection, &site_id),
        ),
        Request::SaveUserSet { draft, reply } => {
            send(reply, super::user_sets::save_user_set(connection, draft))
        }
        Request::DeleteUserSet { user_set_id, reply } => send(
            reply,
            super::user_sets::delete_user_set(connection, &user_set_id),
        ),
        Request::CachedAssignableUsers {
            site_id,
            locator,
            reply,
        } => send(
            reply,
            super::edit_cache::cached_assignable_users(connection, &site_id, &locator),
        ),
        Request::ReplaceAssignableUsers {
            site_id,
            locator,
            users,
            fetched_at,
            reply,
        } => send(
            reply,
            super::edit_cache::replace_assignable_users(
                connection, &site_id, &locator, &users, fetched_at,
            ),
        ),
        Request::CachedIssueTransitions {
            site_id,
            locator,
            reply,
        } => send(
            reply,
            super::edit_cache::cached_issue_transitions(connection, &site_id, &locator),
        ),
        Request::ReplaceIssueTransitions {
            site_id,
            locator,
            transitions,
            fetched_at,
            reply,
        } => send(
            reply,
            super::edit_cache::replace_issue_transitions(
                connection,
                &site_id,
                &locator,
                &transitions,
                fetched_at,
            ),
        ),
        Request::InvalidateIssueTransitions {
            site_id,
            locator,
            reply,
        } => send(
            reply,
            super::edit_cache::invalidate_issue_transitions(connection, &site_id, &locator),
        ),
    }
}

fn send<T>(reply: Reply<T>, result: Result<T, ApplicationError>) {
    let _ = reply.send(result);
}
