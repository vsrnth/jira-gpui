use jira_application::{
    ApplicationError, CachedAssignableUsers, CachedIssueTransitions, IssueLocator, IssueTransition,
    MAX_ASSIGNABLE_USER_SEARCH_LIMIT, MAX_ISSUE_TRANSITIONS,
};
use jira_domain::{JiraSiteId, Timestamp, User};
use rusqlite::{Connection, OptionalExtension, params};

use super::codecs::*;
use super::sqlite_error;

fn locator_parts(locator: &IssueLocator) -> (&'static str, &str) {
    match locator {
        IssueLocator::Id(value) => ("id", value.as_str()),
        IssueLocator::Key(value) => ("key", value.as_str()),
    }
}

pub(super) fn cached_assignable_users(
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

pub(super) fn replace_assignable_users(
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

pub(super) fn cached_issue_transitions(
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

pub(super) fn replace_issue_transitions(
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

pub(super) fn invalidate_issue_transitions(
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
