use rusqlite::{Connection, Transaction};

const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SUPPORTED_SCHEMA_VERSION: i32 = 4;

pub(super) fn initialize_connection(
    connection: &mut Connection,
    file_backed: bool,
) -> rusqlite::Result<()> {
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
        transaction.execute_batch(include_str!("../../../../migrations/0001_initial.sql"))?;
        transaction.execute_batch("PRAGMA user_version = 1;")?;
    }
    if version < 2 {
        migrate_update_event_kind_range(&transaction)?;
        transaction.execute_batch("PRAGMA user_version = 2;")?;
    }
    if version < 3 {
        transaction.execute_batch(include_str!(
            "../../../../migrations/0003_issue_edit_cache.sql"
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
        "../../../../migrations/0004_field_changed_kind.sql"
    ))
}
