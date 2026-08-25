use jira_application::{ApplicationError, UserSetDraft};
use jira_domain::{JiraSiteId, UserSet, UserSetId};
use rusqlite::{Connection, params};
use time::OffsetDateTime;

use super::codecs::*;
use super::{sqlite_error, storage_error};

pub(super) fn list_user_sets(
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

pub(super) fn save_user_set(
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

pub(super) fn delete_user_set(
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
