use jira_application::{
    ApplicationError, AttachmentImage, MAX_CACHED_ATTACHMENT_IMAGE_BYTES,
    MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES, MAX_CACHED_ATTACHMENT_IMAGE_TOTAL_BYTES,
    validate_cached_image,
};
use jira_domain::{IssueId, JiraSiteId};
use rusqlite::{Connection, OptionalExtension, params};

use super::sqlite_error;

pub(super) fn cached_attachment_image(
    connection: &Connection,
    site_id: &JiraSiteId,
    issue_id: &IssueId,
    attachment_id: &str,
) -> Result<Option<AttachmentImage>, ApplicationError> {
    let row = connection
        .query_row(
            "SELECT mime_type, bytes FROM issue_media_cache WHERE site_id = ?1 AND issue_id = ?2 AND attachment_id = ?3",
            params![site_id.as_str(), issue_id.as_str(), attachment_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?;
    let Some((mime_type, bytes)) = row else {
        return Ok(None);
    };
    let image = AttachmentImage {
        attachment_id: attachment_id.to_owned(),
        mime_type,
        bytes,
    };
    if validate_cached_image(&image, attachment_id, MAX_CACHED_ATTACHMENT_IMAGE_BYTES).is_err() {
        connection
            .execute(
                "DELETE FROM issue_media_cache WHERE site_id = ?1 AND issue_id = ?2 AND attachment_id = ?3",
                params![site_id.as_str(), issue_id.as_str(), attachment_id],
            )
            .map_err(sqlite_error)?;
        return Ok(None);
    }
    Ok(Some(image))
}

pub(super) fn cache_attachment_image(
    connection: &mut Connection,
    site_id: &JiraSiteId,
    issue_id: &IssueId,
    image: &AttachmentImage,
) -> Result<(), ApplicationError> {
    validate_cached_image(
        image,
        &image.attachment_id,
        MAX_CACHED_ATTACHMENT_IMAGE_BYTES,
    )?;
    let transaction = connection.transaction().map_err(sqlite_error)?;
    transaction
        .execute(
            "DELETE FROM issue_media_cache WHERE site_id = ?1 AND issue_id = ?2 AND attachment_id = ?3",
            params![site_id.as_str(), issue_id.as_str(), image.attachment_id],
        )
        .map_err(sqlite_error)?;
    let next_order: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(stored_order), 0) + 1 FROM issue_media_cache",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    let mut current_entries: i64 = transaction
        .query_row("SELECT COUNT(*) FROM issue_media_cache", [], |row| {
            row.get(0)
        })
        .map_err(sqlite_error)?;
    let mut current_bytes: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(length(bytes)), 0) FROM issue_media_cache",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    let incoming = i64::try_from(image.bytes.len()).map_err(|_| {
        ApplicationError::invalid_input("cached image size is outside the configured bound")
    })?;
    let max_total = i64::try_from(MAX_CACHED_ATTACHMENT_IMAGE_TOTAL_BYTES)
        .expect("cache total fits sqlite integer");
    while current_entries + 1 > MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES as i64
        || current_bytes + incoming > max_total
    {
        let removed = transaction
            .execute(
                "DELETE FROM issue_media_cache WHERE rowid IN (SELECT rowid FROM issue_media_cache ORDER BY stored_order ASC, site_id ASC, issue_id ASC, attachment_id ASC LIMIT 1)",
                [],
            )
            .map_err(sqlite_error)?;
        if removed == 0 {
            break;
        }
        // Re-read bounded counters after each deterministic eviction. The
        // cache is tiny enough that this keeps the SQL straightforward.
        current_entries = transaction
            .query_row("SELECT COUNT(*) FROM issue_media_cache", [], |row| {
                row.get(0)
            })
            .map_err(sqlite_error)?;
        current_bytes = transaction
            .query_row(
                "SELECT COALESCE(SUM(length(bytes)), 0) FROM issue_media_cache",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_error)?;
        if current_entries < MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES as i64
            && current_bytes + incoming <= max_total
        {
            break;
        }
        // Shadow the values for the next loop iteration through a local
        // condition; the next query is intentionally deterministic.
        if current_entries == 0 && current_bytes == 0 {
            break;
        }
    }
    // A single image larger than the aggregate bound is rejected above by the
    // per-image bound only when limits diverge; retain an explicit guard.
    let remaining_bytes: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(length(bytes)), 0) FROM issue_media_cache",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    if remaining_bytes + incoming > max_total {
        return Err(ApplicationError::invalid_input(
            "cached image exceeds the aggregate cache bound",
        ));
    }
    transaction
        .execute(
            "INSERT INTO issue_media_cache (site_id, issue_id, attachment_id, mime_type, bytes, stored_order) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                site_id.as_str(),
                issue_id.as_str(),
                image.attachment_id,
                image.mime_type.trim(),
                image.bytes,
                next_order,
            ],
        )
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

pub(super) fn remove_cached_attachment_image(
    connection: &Connection,
    site_id: &JiraSiteId,
    issue_id: &IssueId,
    attachment_id: &str,
) -> Result<(), ApplicationError> {
    connection
        .execute(
            "DELETE FROM issue_media_cache WHERE site_id = ?1 AND issue_id = ?2 AND attachment_id = ?3",
            params![site_id.as_str(), issue_id.as_str(), attachment_id],
        )
        .map_err(sqlite_error)?;
    Ok(())
}
