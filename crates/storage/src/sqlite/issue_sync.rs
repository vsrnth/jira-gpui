use jira_application::{
    ApplicationError, CommitOutcome, ErrorKind, IssueListQuery, SyncCommit, SyncState,
};
use jira_domain::{Issue, IssueId, JiraSiteId, Timestamp, UpdateEvent, UserSetId};
use rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter, types::Value,
};

use super::codecs::*;
use super::{sqlite_error, storage_error};
use crate::event_semantics::{
    normalize_matching_user_set_ids, same_event_identity, union_matching_user_set_ids,
};

pub(super) fn list_issues(
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

pub(super) fn get_issue(
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

pub(super) fn cache_detail_issue(
    connection: &mut Connection,
    issue: &Issue,
) -> Result<bool, ApplicationError> {
    let transaction = connection.transaction().map_err(sqlite_error)?;
    let existing_snapshot = transaction
        .query_row(
            "SELECT snapshot FROM issues WHERE site_id = ?1 AND issue_id = ?2",
            params![issue.site_id.as_str(), issue.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    let Some(existing_snapshot) = existing_snapshot else {
        return Ok(false);
    };
    let existing: Issue = decode(&existing_snapshot, "issue")?;
    if existing == *issue {
        return Ok(false);
    }
    let snapshot = encode(issue, "issue")?;
    transaction
        .execute(
            "UPDATE issues SET issue_key = ?1, summary = ?2, assignee_id = ?3, updated_seconds = ?4, updated_nanos = ?5, snapshot = ?6 WHERE site_id = ?7 AND issue_id = ?8",
            params![
                issue.key.as_str(),
                issue.summary,
                issue.assignee.as_ref().map(|id| id.as_str()),
                stamp(issue.updated_at).0,
                stamp(issue.updated_at).1,
                snapshot,
                issue.site_id.as_str(),
                issue.id.as_str(),
            ],
        )
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)?;
    Ok(true)
}

pub(super) fn issues_for_user_set(
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

pub(super) fn sync_state(
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

pub(super) fn commit_sync(
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
    let issue = if issue.description_text.is_none() && issue.rich_description.is_none() {
        transaction
            .query_row(
                "SELECT snapshot FROM issues WHERE site_id = ?1 AND issue_id = ?2",
                params![issue.site_id.as_str(), issue.id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .map(|snapshot| decode::<Issue>(&snapshot, "issue"))
            .transpose()?
            .map(|existing| {
                let mut merged = issue.clone();
                merged.description_text = existing.description_text;
                merged.rich_description = existing.rich_description;
                merged
            })
            .unwrap_or_else(|| issue.clone())
    } else {
        issue.clone()
    };
    let snapshot = encode(&issue, "issue")?;
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

pub(super) fn record_sync_failure(
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
