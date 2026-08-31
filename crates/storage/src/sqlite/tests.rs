use futures_lite::future::block_on;
use jira_application::{
    AttachmentImage, ErrorKind, IssueCachePort, IssueEditCachePort, IssueLocator,
    IssueMediaCachePort, IssueTransition, SyncCommit, SyncState, UpdateFeedPort, UpdateFeedQuery,
    UserSetDraft, UserSetPort,
};
use jira_domain::{
    AccountId, EventId, Issue, IssueId, IssueKey, IssueType, JiraSiteId, Priority, Project,
    RichBlock, RichInline, RichTextDocument, Status, Timestamp, UpdateEvent, UpdateKind, User,
    UserSetId,
};
use tempfile::tempdir;
use time::macros::datetime;

use super::migrations::initialize_connection;
use super::{SqliteStore, normalize_database_path};

fn site(value: &str) -> JiraSiteId {
    JiraSiteId::new(value).expect("valid site")
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
fn migrations_enable_pragmas_and_reject_newer_schema() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("cache.sqlite");
    let store = SqliteStore::open(&path).expect("open store");
    drop(store);
    let connection = rusqlite::Connection::open(&path).expect("open sqlite");
    let version: i32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 5);
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
        .execute_batch("PRAGMA user_version = 6")
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
    assert_eq!(restored.detail_loaded, cached_issue.detail_loaded);
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
fn attachment_image_cache_is_site_isolated_durable_and_rejects_corruption() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("media-cache.sqlite");
    let store = SqliteStore::open(&path).expect("open store");
    let site_id = site("site-a");
    let user_set_id = saved_set(&store, site_id.clone());
    let cached_issue = issue(
        site_id.clone(),
        "100",
        "Image issue",
        datetime!(2026-01-03 00:00 UTC),
    );
    block_on(store.commit_sync(SyncCommit {
        site_id: site_id.clone(),
        user_set_id: user_set_id.clone(),
        issues: vec![cached_issue],
        update_events: Vec::new(),
        replace_membership: true,
        state: SyncState::new(site_id.clone(), user_set_id),
    }))
    .expect("commit issue");
    let image = AttachmentImage {
        attachment_id: "attachment-1".into(),
        mime_type: "image/png".into(),
        bytes: b"\x89PNG\r\n\x1a\nvalid".to_vec(),
    };
    let issue_id = IssueId::new("100").expect("issue");
    block_on(store.cache_attachment_image(&site_id, &issue_id, &image)).expect("cache image");
    assert_eq!(
        block_on(store.cached_attachment_image(&site_id, &issue_id, "attachment-1"))
            .expect("read image"),
        Some(image.clone())
    );
    assert!(
        block_on(store.cached_attachment_image(&site("site-b"), &issue_id, "attachment-1"))
            .expect("isolated read")
            .is_none()
    );
    drop(store);
    let reopened = SqliteStore::open(&path).expect("reopen store");
    assert!(
        block_on(reopened.cached_attachment_image(&site_id, &issue_id, "attachment-1"))
            .expect("durable read")
            .is_some()
    );
    let connection = rusqlite::Connection::open(&path).expect("raw connection");
    connection
        .execute(
            "UPDATE issue_media_cache SET bytes = ?1 WHERE site_id = ?2 AND issue_id = ?3 AND attachment_id = ?4",
            rusqlite::params![b"corrupt".as_slice(), site_id.as_str(), issue_id.as_str(), "attachment-1"],
        )
        .expect("corrupt fixture");
    assert!(
        block_on(reopened.cached_attachment_image(&site_id, &issue_id, "attachment-1"))
            .expect("corruption read")
            .is_none()
    );
}

#[test]
fn attachment_image_cache_evicts_oldest_entry_at_deterministic_entry_bound() {
    let store = SqliteStore::in_memory().expect("open store");
    let site_id = site("site-a");
    let user_set_id = saved_set(&store, site_id.clone());
    let cached_issue = issue(
        site_id.clone(),
        "100",
        "Image issue",
        datetime!(2026-01-03 00:00 UTC),
    );
    block_on(store.commit_sync(SyncCommit {
        site_id: site_id.clone(),
        user_set_id: user_set_id.clone(),
        issues: vec![cached_issue],
        update_events: Vec::new(),
        replace_membership: true,
        state: SyncState::new(site_id.clone(), user_set_id),
    }))
    .expect("commit issue");
    let issue_id = IssueId::new("100").expect("issue");
    for index in 0..=jira_application::MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES {
        let image = AttachmentImage {
            attachment_id: format!("image-{index}"),
            mime_type: "image/png".into(),
            bytes: b"\x89PNG\r\n\x1a\nvalid".to_vec(),
        };
        block_on(store.cache_attachment_image(&site_id, &issue_id, &image)).expect("cache image");
    }
    assert!(
        block_on(store.cached_attachment_image(&site_id, &issue_id, "image-0"))
            .expect("oldest read")
            .is_none()
    );
    assert!(
        block_on(store.cached_attachment_image(
            &site_id,
            &issue_id,
            &format!(
                "image-{}",
                jira_application::MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES
            )
        ))
        .expect("newest read")
        .is_some()
    );
}

#[test]
fn attachment_image_cache_evicts_oldest_entry_at_deterministic_aggregate_byte_bound() {
    let store = SqliteStore::in_memory().expect("open store");
    let site_id = site("site-a");
    let user_set_id = saved_set(&store, site_id.clone());
    let cached_issue = issue(
        site_id.clone(),
        "100",
        "Image issue",
        datetime!(2026-01-03 00:00 UTC),
    );
    block_on(store.commit_sync(SyncCommit {
        site_id: site_id.clone(),
        user_set_id: user_set_id.clone(),
        issues: vec![cached_issue],
        update_events: Vec::new(),
        replace_membership: true,
        state: SyncState::new(site_id.clone(), user_set_id),
    }))
    .expect("commit issue");
    let issue_id = IssueId::new("100").expect("issue");
    let mut bytes = vec![0; jira_application::MAX_CACHED_ATTACHMENT_IMAGE_BYTES];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    for index in 0..5 {
        let image = AttachmentImage {
            attachment_id: format!("large-image-{index}"),
            mime_type: "image/png".into(),
            bytes: bytes.clone(),
        };
        block_on(store.cache_attachment_image(&site_id, &issue_id, &image)).expect("cache image");
    }
    assert!(
        block_on(store.cached_attachment_image(&site_id, &issue_id, "large-image-0"))
            .expect("oldest read")
            .is_none()
    );
    assert!(
        block_on(store.cached_attachment_image(&site_id, &issue_id, "large-image-4"))
            .expect("newest read")
            .is_some()
    );
}

#[test]
fn detail_snapshot_survives_reopen_and_can_be_cleared() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("detail.sqlite");
    let site_id = site("site-detail");
    let user_set_id;
    let cached_issue;
    {
        let store = SqliteStore::open(&path).expect("open store");
        user_set_id = saved_set(&store, site_id.clone());
        cached_issue = issue(
            site_id.clone(),
            "701",
            "Detail",
            datetime!(2026-01-03 00:00 UTC),
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![cached_issue.clone()],
            update_events: vec![],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), user_set_id.clone()),
        }))
        .expect("baseline commit");
        let mut detailed = cached_issue.clone();
        detailed.description_text = Some("Persisted detail".into());
        detailed.rich_description = Some(RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::Text {
                text: "Persisted rich detail".into(),
                marks: vec![],
            }])],
            false,
        ));
        detailed.detail_loaded = true;
        assert!(block_on(store.cache_detail_issue(&detailed)).expect("cache detail"));
    }
    let store = SqliteStore::open(&path).expect("reopen store");
    let mut cleared = block_on(store.get_issue(&site_id, &cached_issue.id))
        .expect("detail lookup")
        .expect("detail exists");
    assert_eq!(
        cleared.description_text.as_deref(),
        Some("Persisted detail")
    );
    assert_eq!(
        cleared
            .rich_description
            .as_ref()
            .map(RichTextDocument::plain_text)
            .as_deref(),
        Some("Persisted rich detail")
    );
    assert!(cleared.detail_loaded);
    cleared.description_text = None;
    cleared.rich_description = None;
    assert!(block_on(store.cache_detail_issue(&cleared)).expect("clear detail"));
    let restored = block_on(store.get_issue(&site_id, &cached_issue.id))
        .expect("cleared lookup")
        .expect("cleared exists");
    assert_eq!(restored.description_text, None);
    assert_eq!(restored.rich_description, None);
    assert!(restored.detail_loaded);
    assert_eq!(
        block_on(store.issues_for_user_set(&site_id, &user_set_id))
            .expect("membership lookup")
            .len(),
        1
    );
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
        block_on(store.replace_assignable_users(&site_a, &id_locator, users.clone(), fetched_at))
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
