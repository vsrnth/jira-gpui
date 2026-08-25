use jira_application::{ApplicationError, UpdateFeedQuery};
use jira_domain::{EventId, JiraSiteId, NotificationDelivery, UpdateEvent, UserSetId};
use rusqlite::{Connection, params, params_from_iter, types::Value};

use super::codecs::*;
use super::{sqlite_error, storage_error};

pub(super) fn record_notification_delivery(
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

pub(super) fn list_events(
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

pub(super) fn unread_count(
    connection: &Connection,
    site_id: &JiraSiteId,
) -> Result<usize, ApplicationError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM update_events WHERE site_id = ?1 AND read_state = 0",
            params![site_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?;
    usize::try_from(count).map_err(|_| storage_error("stored unread count is invalid"))
}

pub(super) fn mark_read(
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

pub(super) fn mark_all_read(
    connection: &Connection,
    site_id: &JiraSiteId,
) -> Result<usize, ApplicationError> {
    let changed = connection
        .execute(
            "UPDATE update_events SET read_state = 1 WHERE site_id = ?1 AND read_state = 0",
            params![site_id.as_str()],
        )
        .map_err(sqlite_error)?;
    Ok(changed)
}
