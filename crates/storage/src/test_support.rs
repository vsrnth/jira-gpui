//! Shared storage-port behavioral contracts.
//!
//! These scenarios deliberately use only public application ports. Keeping the
//! fixtures here makes the in-memory and SQLite adapters answer the same
//! questions without making either adapter's representation part of the test.

use futures_lite::future::block_on;
use jira_application::{
    IssueCachePort, IssueEditCachePort, IssueListQuery, IssueLocator, IssueTransition,
    MAX_ASSIGNABLE_USER_SEARCH_LIMIT, MAX_ISSUE_TRANSITIONS, SyncCommit, SyncState, UpdateFeedPort,
    UpdateFeedQuery, UserSetDraft, UserSetPort,
};
use jira_domain::{
    AccountId, ChangeValue, EventId, Issue, IssueId, IssueKey, IssueType, JiraSiteId,
    NotificationDelivery, Priority, Project, RichBlock, RichInline, RichTextDocument, Status,
    Timestamp, UpdateEvent, UpdateKind, User, UserSetId,
};
use time::macros::datetime;

fn site(value: &str) -> JiraSiteId {
    JiraSiteId::new(value).expect("valid site")
}

fn set<S: UserSetPort>(store: &S, site_id: JiraSiteId, name: &str) -> UserSetId {
    block_on(UserSetPort::save(
        store,
        UserSetDraft {
            site_id,
            name: name.to_owned(),
            members: vec![AccountId::new(format!("{name}-member")).expect("valid account")],
        },
    ))
    .expect("save user set")
    .id
}

fn issue(site_id: JiraSiteId, id: &str, key: &str, summary: &str, updated_at: Timestamp) -> Issue {
    Issue::new(
        site_id,
        IssueId::new(id).expect("valid issue id"),
        IssueKey::new(key).expect("valid issue key"),
        Project {
            id: "project-1".into(),
            key: "APP".into(),
            name: "Application".into(),
        },
        IssueType {
            id: "task".into(),
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
            id: Some("medium".into()),
            name: Some("Medium".into()),
            icon_url: None,
        },
        Some(AccountId::new("assignee-a").expect("valid account")),
        None,
        None,
        vec![],
        datetime!(2026-01-01 00:00 UTC),
        updated_at,
        None,
    )
}

fn event(
    id: &str,
    issue: &Issue,
    kind: UpdateKind,
    occurred_at: Timestamp,
    matching_user_set_ids: Vec<UserSetId>,
) -> UpdateEvent {
    UpdateEvent::new(
        EventId::new(id).expect("valid event id"),
        issue.site_id.clone(),
        issue.id.clone(),
        issue.key.clone(),
        kind,
        occurred_at,
        matching_user_set_ids,
    )
}

fn commit<S: IssueCachePort>(
    store: &S,
    site_id: JiraSiteId,
    user_set_id: UserSetId,
    issues: Vec<Issue>,
    update_events: Vec<UpdateEvent>,
    replace_membership: bool,
    state: SyncState,
) {
    block_on(store.commit_sync(SyncCommit {
        site_id,
        user_set_id,
        issues,
        update_events,
        replace_membership,
        state,
    }))
    .expect("commit succeeds");
}

fn issue_query(
    site_id: JiraSiteId,
    user_set_id: UserSetId,
    limit: usize,
    offset: usize,
) -> IssueListQuery {
    IssueListQuery {
        site_id,
        user_set_id,
        text: None,
        assignees: vec![],
        limit,
        offset,
    }
}

fn feed_query(
    site_id: JiraSiteId,
    unread_only: bool,
    kinds: Vec<UpdateKind>,
    limit: usize,
) -> UpdateFeedQuery {
    UpdateFeedQuery {
        site_id,
        unread_only,
        kinds,
        before: None,
        limit,
    }
}

pub(crate) fn issue_cache_ordering_and_pagination<S>(store: S)
where
    S: IssueCachePort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    let newest_high_key = issue(
        site_id.clone(),
        "301",
        "APP-301",
        "newest high",
        datetime!(2026-01-03 00:00 UTC),
    );
    let oldest = issue(
        site_id.clone(),
        "100",
        "APP-100",
        "oldest",
        datetime!(2026-01-01 00:00 UTC),
    );
    let newest_low_key = issue(
        site_id.clone(),
        "300",
        "APP-300",
        "newest low",
        datetime!(2026-01-03 00:00 UTC),
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![newest_high_key, oldest, newest_low_key],
        vec![],
        true,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );
    let keys = |issues: Vec<Issue>| {
        issues
            .into_iter()
            .map(|issue| issue.key.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(
            block_on(store.list_issues(&issue_query(site_id.clone(), user_set_id.clone(), 2, 0,)))
                .expect("first page")
        ),
        vec!["APP-300", "APP-301"]
    );
    assert_eq!(
        keys(
            block_on(store.list_issues(&issue_query(site_id, user_set_id, 2, 2)))
                .expect("second page")
        ),
        vec!["APP-100"]
    );
}

pub(crate) fn issue_cache_query_filters<S>(store: S)
where
    S: IssueCachePort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    let first = issue(
        site_id.clone(),
        "101",
        "APP-101",
        "Alpha text match",
        datetime!(2026-01-02 00:00 UTC),
    );
    let mut second = issue(
        site_id.clone(),
        "102",
        "APP-102",
        "Beta text match",
        datetime!(2026-01-03 00:00 UTC),
    );
    second.assignee = Some(AccountId::new("assignee-b").expect("valid account"));
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![first.clone(), second.clone()],
        vec![],
        true,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );

    let text_query = IssueListQuery {
        site_id: site_id.clone(),
        user_set_id: user_set_id.clone(),
        text: Some("alpha".into()),
        assignees: vec![],
        limit: 10,
        offset: 0,
    };
    assert_eq!(
        block_on(store.list_issues(&text_query)).expect("text query"),
        vec![first.clone()]
    );
    let assignee_query = IssueListQuery {
        text: Some("APP-102".into()),
        assignees: vec![AccountId::new("assignee-b").expect("valid account")],
        ..text_query
    };
    assert_eq!(
        block_on(store.list_issues(&assignee_query)).expect("assignee query"),
        vec![second]
    );
}

pub(crate) fn issue_cache_detail_snapshot<S>(store: S)
where
    S: IssueCachePort + UserSetPort,
{
    let site_id = site("site-detail");
    let user_set_id = set(&store, site_id.clone(), "detail");
    let baseline = issue(
        site_id.clone(),
        "700",
        "APP-700",
        "baseline",
        datetime!(2026-01-02 00:00 UTC),
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![baseline.clone()],
        vec![],
        true,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );

    let mut detailed = baseline.clone();
    detailed.description_text = Some("A detail description".into());
    detailed.rich_description = Some(RichTextDocument::new(
        vec![RichBlock::Paragraph(vec![RichInline::Text {
            text: "A bounded rich detail description.".into(),
            marks: vec![],
        }])],
        false,
    ));
    detailed.detail_loaded = true;
    assert!(block_on(store.cache_detail_issue(&detailed)).expect("cache detail"));
    assert!(!block_on(store.cache_detail_issue(&detailed)).expect("unchanged detail"));
    assert_eq!(
        block_on(store.get_issue(&site_id, &baseline.id))
            .expect("detail lookup")
            .expect("cached detail"),
        detailed
    );

    let mut changed_baseline = baseline.clone();
    changed_baseline.summary = "updated baseline".into();
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![changed_baseline.clone()],
        vec![],
        false,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );
    let preserved = block_on(store.get_issue(&site_id, &baseline.id))
        .expect("preserved lookup")
        .expect("preserved issue");
    assert_eq!(preserved.summary, "updated baseline");
    assert!(preserved.detail_loaded);
    assert_eq!(preserved.description_text, detailed.description_text);
    assert_eq!(preserved.rich_description, detailed.rich_description);

    let mut cleared = preserved;
    cleared.description_text = None;
    cleared.rich_description = None;
    cleared.detail_loaded = true;
    assert!(block_on(store.cache_detail_issue(&cleared)).expect("clear detail"));
    let cleared_snapshot = block_on(store.get_issue(&site_id, &baseline.id))
        .expect("cleared lookup")
        .expect("cleared issue");
    assert!(cleared_snapshot.detail_loaded);
    assert_eq!(cleared_snapshot.description_text, None);
    assert_eq!(cleared_snapshot.rich_description, None);
    assert_eq!(
        block_on(store.issues_for_user_set(&site_id, &user_set_id))
            .expect("membership lookup")
            .len(),
        1
    );
    assert!(
        !block_on(store.cache_detail_issue(&issue(
            site_id,
            "missing",
            "APP-999",
            "missing",
            datetime!(2026-01-03 00:00 UTC),
        )))
        .expect("missing detail")
    );
}

pub(crate) fn issue_cache_membership_replace_and_extend<S>(store: S)
where
    S: IssueCachePort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    let first = issue(
        site_id.clone(),
        "101",
        "APP-101",
        "first",
        datetime!(2026-01-02 00:00 UTC),
    );
    let second = issue(
        site_id.clone(),
        "102",
        "APP-102",
        "second",
        datetime!(2026-01-03 00:00 UTC),
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![first],
        vec![],
        true,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![second.clone()],
        vec![],
        false,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );
    assert_eq!(
        block_on(store.issues_for_user_set(&site_id, &user_set_id))
            .expect("extended membership")
            .len(),
        2
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![second.clone()],
        vec![],
        true,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );
    assert_eq!(
        block_on(store.issues_for_user_set(&site_id, &user_set_id)).expect("replaced membership"),
        vec![second]
    );
}

pub(crate) fn issue_cache_site_and_user_set_isolation<S>(store: S)
where
    S: IssueCachePort + UserSetPort,
{
    let first_site = site("site-a");
    let second_site = site("site-b");
    let first_set = set(&store, first_site.clone(), "first");
    let second_set = set(&store, second_site.clone(), "second");
    let first_issue = issue(
        first_site.clone(),
        "101",
        "APP-101",
        "first",
        datetime!(2026-01-02 00:00 UTC),
    );
    let second_issue = issue(
        second_site.clone(),
        "202",
        "APP-202",
        "second",
        datetime!(2026-01-03 00:00 UTC),
    );
    commit(
        &store,
        first_site.clone(),
        first_set.clone(),
        vec![first_issue.clone()],
        vec![],
        true,
        SyncState::new(first_site.clone(), first_set.clone()),
    );
    commit(
        &store,
        second_site.clone(),
        second_set.clone(),
        vec![second_issue],
        vec![],
        true,
        SyncState::new(second_site.clone(), second_set.clone()),
    );
    assert_eq!(
        block_on(store.issues_for_user_set(&first_site, &first_set)).expect("first set"),
        vec![first_issue]
    );
    assert!(
        block_on(store.issues_for_user_set(&first_site, &second_set))
            .expect("cross-site set")
            .is_empty()
    );
    assert!(
        block_on(store.list_issues(&issue_query(
            first_site.clone(),
            UserSetId::new("missing").expect("valid set"),
            10,
            0,
        )))
        .expect("missing set")
        .is_empty()
    );
    assert!(
        block_on(store.get_issue(&first_site, &IssueId::new("202").expect("valid issue")))
            .expect("cross-site issue")
            .is_none()
    );
}

pub(crate) fn issue_cache_sync_state_persistence<S>(store: S)
where
    S: IssueCachePort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    let mut expected = SyncState::new(site_id.clone(), user_set_id.clone());
    expected.last_incremental_started_at = Some(datetime!(2026-01-03 01:00 UTC));
    expected.last_incremental_succeeded_at = Some(datetime!(2026-01-03 01:01 UTC));
    expected.last_full_sync_at = Some(datetime!(2026-01-02 01:00 UTC));
    expected.consecutive_failures = 2;
    expected.last_error_kind = Some(jira_application::ErrorKind::Offline);
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![],
        vec![],
        true,
        expected.clone(),
    );
    let actual = block_on(store.sync_state(&site_id, &user_set_id))
        .expect("sync state lookup")
        .expect("sync state exists");
    assert_eq!(actual.site_id, expected.site_id);
    assert_eq!(actual.user_set_id, expected.user_set_id);
    assert_eq!(
        actual.last_incremental_started_at,
        expected.last_incremental_started_at
    );
    assert_eq!(
        actual.last_incremental_succeeded_at,
        expected.last_incremental_succeeded_at
    );
    assert_eq!(actual.last_full_sync_at, expected.last_full_sync_at);
    assert_eq!(actual.consecutive_failures, expected.consecutive_failures);
    assert_eq!(actual.last_error_kind, expected.last_error_kind);
}

pub(crate) fn issue_cache_invalid_commit_is_atomic<S>(store: S)
where
    S: IssueCachePort + UpdateFeedPort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    let existing = issue(
        site_id.clone(),
        "100",
        "APP-100",
        "existing",
        datetime!(2026-01-02 00:00 UTC),
    );
    let mut initial_state = SyncState::new(site_id.clone(), user_set_id.clone());
    initial_state.last_incremental_started_at = Some(datetime!(2026-01-01 01:00 UTC));
    initial_state.last_incremental_succeeded_at = Some(datetime!(2026-01-01 01:01 UTC));
    initial_state.last_full_sync_at = Some(datetime!(2025-12-31 01:00 UTC));
    initial_state.consecutive_failures = 2;
    initial_state.last_error_kind = Some(jira_application::ErrorKind::Offline);
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![existing.clone()],
        vec![],
        true,
        initial_state.clone(),
    );
    let incoming = issue(
        site_id.clone(),
        "200",
        "APP-200",
        "must not persist",
        datetime!(2026-01-03 00:00 UTC),
    );
    let invalid_event = event(
        "invalid-event",
        &issue(
            site("other-site"),
            "900",
            "OTHER-900",
            "wrong site",
            datetime!(2026-01-03 00:00 UTC),
        ),
        UpdateKind::IssueAddedToView,
        datetime!(2026-01-03 00:00 UTC),
        vec![user_set_id.clone()],
    );
    let mut attempted_state = SyncState::new(site_id.clone(), user_set_id.clone());
    attempted_state.last_incremental_started_at = Some(datetime!(2026-02-01 01:00 UTC));
    attempted_state.last_incremental_succeeded_at = Some(datetime!(2026-02-01 01:01 UTC));
    attempted_state.last_full_sync_at = Some(datetime!(2026-01-31 01:00 UTC));
    attempted_state.consecutive_failures = 9;
    attempted_state.last_error_kind = Some(jira_application::ErrorKind::Upstream);
    assert!(
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: user_set_id.clone(),
            issues: vec![incoming.clone()],
            update_events: vec![invalid_event.clone()],
            replace_membership: true,
            state: attempted_state,
        }))
        .is_err()
    );
    assert_eq!(
        block_on(store.get_issue(&site_id, &existing.id)).expect("existing issue"),
        Some(existing.clone())
    );
    assert!(
        block_on(store.get_issue(&site_id, &incoming.id))
            .expect("incoming issue")
            .is_none()
    );
    assert_eq!(
        block_on(store.issues_for_user_set(&site_id, &user_set_id)).expect("membership"),
        vec![existing]
    );
    let events = block_on(UpdateFeedPort::list(
        &store,
        &feed_query(site_id.clone(), false, vec![], 10),
    ))
    .expect("feed");
    assert!(events.is_empty(), "invalid event must not be inserted");
    let invalid_site_events = block_on(UpdateFeedPort::list(
        &store,
        &feed_query(invalid_event.site_id.clone(), false, vec![], 10),
    ))
    .expect("invalid event site feed");
    assert!(
        invalid_site_events.is_empty(),
        "invalid event must not be inserted at its own site"
    );
    let actual = block_on(store.sync_state(&site_id, &user_set_id))
        .expect("state lookup")
        .expect("state exists");
    assert_eq!(actual.site_id, initial_state.site_id);
    assert_eq!(actual.user_set_id, initial_state.user_set_id);
    assert_eq!(
        actual.last_incremental_started_at,
        initial_state.last_incremental_started_at
    );
    assert_eq!(
        actual.last_incremental_succeeded_at,
        initial_state.last_incremental_succeeded_at
    );
    assert_eq!(actual.last_full_sync_at, initial_state.last_full_sync_at);
    assert_eq!(
        actual.consecutive_failures,
        initial_state.consecutive_failures
    );
    assert_eq!(actual.last_error_kind, initial_state.last_error_kind);
}

pub(crate) fn issue_cache_records_sync_failures<S>(store: S)
where
    S: IssueCachePort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    block_on(store.record_sync_failure(
        &site_id,
        &user_set_id,
        jira_application::ErrorKind::Offline,
        datetime!(2026-01-03 01:00 UTC),
    ))
    .expect("record offline failure");
    block_on(store.record_sync_failure(
        &site_id,
        &user_set_id,
        jira_application::ErrorKind::Upstream,
        datetime!(2026-01-03 01:01 UTC),
    ))
    .expect("record upstream failure");
    let state = block_on(store.sync_state(&site_id, &user_set_id))
        .expect("state lookup")
        .expect("state exists");
    assert_eq!(state.consecutive_failures, 2);
    assert_eq!(
        state.last_error_kind,
        Some(jira_application::ErrorKind::Upstream)
    );
    block_on(store.record_sync_failure(
        &site_id,
        &user_set_id,
        jira_application::ErrorKind::UnknownOutcome,
        datetime!(2026-01-03 01:02 UTC),
    ))
    .expect("record unknown outcome");
    let state = block_on(store.sync_state(&site_id, &user_set_id))
        .expect("state lookup")
        .expect("state exists");
    assert_eq!(state.consecutive_failures, 3);
    assert_eq!(
        state.last_error_kind,
        Some(jira_application::ErrorKind::Upstream)
    );
}

pub(crate) fn issue_cache_records_notification_delivery<S>(store: S)
where
    S: IssueCachePort + UpdateFeedPort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    let cached_issue = issue(
        site_id.clone(),
        "100",
        "APP-100",
        "issue",
        datetime!(2026-01-02 00:00 UTC),
    );
    let event = event(
        "event-delivery",
        &cached_issue,
        UpdateKind::IssueAddedToView,
        datetime!(2026-01-03 00:00 UTC),
        vec![user_set_id.clone()],
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![cached_issue],
        vec![event.clone()],
        true,
        SyncState::new(site_id.clone(), user_set_id),
    );
    let feed = |store: &S| {
        block_on(UpdateFeedPort::list(
            store,
            &feed_query(site_id.clone(), false, vec![], 10),
        ))
        .expect("feed")
    };
    block_on(store.record_notification_delivery(
        &event.id,
        NotificationDelivery::Delivered,
        datetime!(2026-01-03 02:00 UTC),
    ))
    .expect("record delivery");
    let mut expected = event.clone();
    expected.notification_delivery = NotificationDelivery::Delivered;
    assert_eq!(feed(&store), vec![expected]);

    let before_missing = feed(&store);
    block_on(store.record_notification_delivery(
        &EventId::new("missing-event").expect("event"),
        NotificationDelivery::Unavailable,
        datetime!(2026-01-03 03:00 UTC),
    ))
    .expect("missing delivery is a no-op");
    assert_eq!(feed(&store), before_missing);
}

pub(crate) fn update_event_idempotency_and_association_union<S>(store: S)
where
    S: IssueCachePort + UpdateFeedPort + UserSetPort,
{
    let site_id = site("site-a");
    let first_set = set(&store, site_id.clone(), "first");
    let second_set = set(&store, site_id.clone(), "second");
    let cached_issue = issue(
        site_id.clone(),
        "100",
        "APP-100",
        "issue",
        datetime!(2026-01-02 00:00 UTC),
    );
    let first_event = event(
        "event-1",
        &cached_issue,
        UpdateKind::IssueAddedToView,
        datetime!(2026-01-03 00:00 UTC),
        vec![first_set.clone()],
    );
    let mut second_event = first_event.clone();
    second_event.matching_user_set_ids = vec![second_set.clone()];
    let mut expected_event = first_event.clone();
    expected_event.matching_user_set_ids = vec![first_set.clone(), second_set.clone()];

    let outcome = block_on(store.commit_sync(SyncCommit {
        site_id: site_id.clone(),
        user_set_id: first_set.clone(),
        issues: vec![cached_issue],
        update_events: vec![first_event, second_event.clone()],
        replace_membership: true,
        state: SyncState::new(site_id.clone(), first_set),
    }))
    .expect("same-commit union");
    assert_eq!(outcome.inserted_events, vec![expected_event.clone()]);

    let events = block_on(UpdateFeedPort::list(
        &store,
        &feed_query(site_id.clone(), false, vec![], 10),
    ))
    .expect("feed");
    assert_eq!(events, vec![expected_event.clone()]);

    let replay_outcome = block_on(store.commit_sync(SyncCommit {
        site_id: site_id.clone(),
        user_set_id: second_set.clone(),
        issues: vec![],
        update_events: vec![second_event],
        replace_membership: false,
        state: SyncState::new(site_id.clone(), second_set),
    }))
    .expect("replay commit");
    assert!(replay_outcome.inserted_events.is_empty());
    assert_eq!(
        block_on(UpdateFeedPort::list(
            &store,
            &feed_query(site_id, false, vec![], 10),
        ))
        .expect("replayed feed"),
        vec![expected_event]
    );
}

pub(crate) fn update_event_identity_formats_are_opaque_and_deduplicated<S>(store: S)
where
    S: IssueCachePort + UpdateFeedPort + UserSetPort,
{
    const SNAPSHOT_ID: &str = "v1-f7c343a8639b995e61431f1ac84575d9";
    const COMMENT_ID: &str = "v1-comment-66bae53b96dfa491611148a3ee34ba0e";

    let site_id = site("site-a");
    let first_set = set(&store, site_id.clone(), "first");
    let second_set = set(&store, site_id.clone(), "second");
    let cached_issue = issue(
        site_id.clone(),
        "10001",
        "APP-10001",
        "identity",
        datetime!(2026-08-16 11:00 UTC),
    );
    let snapshot_event = event(
        SNAPSHOT_ID,
        &cached_issue,
        UpdateKind::SummaryChanged {
            old: ChangeValue::Text("old summary".into()),
            new: ChangeValue::Text("new summary".into()),
        },
        datetime!(2026-08-16 11:00 UTC),
        vec![first_set.clone()],
    );
    let comment_event = event(
        COMMENT_ID,
        &cached_issue,
        UpdateKind::CommentAdded {
            comment_id: "comment-1".into(),
            author: None,
            excerpt: "mentioned".into(),
        },
        datetime!(2026-08-16 11:00 UTC),
        vec![first_set.clone()],
    );
    commit(
        &store,
        site_id.clone(),
        first_set.clone(),
        vec![cached_issue],
        vec![snapshot_event.clone(), comment_event.clone()],
        true,
        SyncState::new(site_id.clone(), first_set),
    );

    let mut replay_snapshot = snapshot_event;
    replay_snapshot.matching_user_set_ids = vec![second_set.clone()];
    let mut replay_comment = comment_event;
    replay_comment.matching_user_set_ids = vec![second_set.clone()];
    let replay = block_on(store.commit_sync(SyncCommit {
        site_id: site_id.clone(),
        user_set_id: second_set.clone(),
        issues: vec![],
        update_events: vec![replay_snapshot, replay_comment],
        replace_membership: false,
        state: SyncState::new(site_id.clone(), second_set),
    }))
    .expect("opaque IDs deduplicate");
    assert!(replay.inserted_events.is_empty());

    let mut ids = block_on(UpdateFeedPort::list(
        &store,
        &feed_query(site_id, false, vec![], 10),
    ))
    .expect("round-trip feed")
    .into_iter()
    .map(|event| event.id.as_str().to_owned())
    .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![COMMENT_ID, SNAPSHOT_ID]);
}

pub(crate) fn update_event_conflict_rejects_without_partial_mutation<S>(store: S)
where
    S: IssueCachePort + UpdateFeedPort + UserSetPort,
{
    let site_id = site("site-a");
    let first_set = set(&store, site_id.clone(), "first");
    let second_set = set(&store, site_id.clone(), "second");
    let cached_issue = issue(
        site_id.clone(),
        "100",
        "APP-100",
        "issue",
        datetime!(2026-01-02 00:00 UTC),
    );
    let first_event = event(
        "event-conflict",
        &cached_issue,
        UpdateKind::IssueAddedToView,
        datetime!(2026-01-03 00:00 UTC),
        vec![first_set.clone()],
    );
    commit(
        &store,
        site_id.clone(),
        first_set.clone(),
        vec![cached_issue.clone()],
        vec![first_event.clone()],
        true,
        SyncState::new(site_id.clone(), first_set.clone()),
    );
    let conflicting_event = event(
        "event-conflict",
        &cached_issue,
        UpdateKind::StatusChanged {
            old: ChangeValue::Text("Open".into()),
            new: ChangeValue::Text("Done".into()),
        },
        datetime!(2026-01-04 00:00 UTC),
        vec![second_set.clone()],
    );
    let incoming = issue(
        site_id.clone(),
        "200",
        "APP-200",
        "must not persist",
        datetime!(2026-01-04 00:00 UTC),
    );
    assert!(
        block_on(store.commit_sync(SyncCommit {
            site_id: site_id.clone(),
            user_set_id: second_set.clone(),
            issues: vec![incoming.clone()],
            update_events: vec![conflicting_event],
            replace_membership: true,
            state: SyncState::new(site_id.clone(), second_set.clone()),
        }))
        .is_err()
    );
    let events = block_on(UpdateFeedPort::list(
        &store,
        &feed_query(site_id.clone(), false, vec![], 10),
    ))
    .expect("feed");
    assert_eq!(events, vec![first_event]);
    assert!(
        block_on(store.get_issue(&site_id, &incoming.id))
            .expect("incoming issue")
            .is_none()
    );
    assert!(
        block_on(store.issues_for_user_set(&site_id, &second_set))
            .expect("second membership")
            .is_empty()
    );
    assert!(
        block_on(store.sync_state(&site_id, &second_set))
            .expect("state lookup")
            .is_none()
    );
}

pub(crate) fn update_feed_orders_filters_and_counts<S>(store: S)
where
    S: IssueCachePort + UpdateFeedPort + UserSetPort,
{
    let site_id = site("site-a");
    let other_site = site("site-b");
    let user_set_id = set(&store, site_id.clone(), "team");
    let other_set = set(&store, other_site.clone(), "other");
    let first_issue = issue(
        site_id.clone(),
        "101",
        "APP-101",
        "first",
        datetime!(2026-01-01 00:00 UTC),
    );
    let second_issue = issue(
        site_id.clone(),
        "102",
        "APP-102",
        "second",
        datetime!(2026-01-02 00:00 UTC),
    );
    let third_issue = issue(
        site_id.clone(),
        "103",
        "APP-103",
        "third",
        datetime!(2026-01-03 00:00 UTC),
    );
    let first_event = event(
        "event-1",
        &first_issue,
        UpdateKind::IssueAddedToView,
        datetime!(2026-01-01 01:00 UTC),
        vec![user_set_id.clone()],
    );
    let second_event = event(
        "event-2",
        &second_issue,
        UpdateKind::StatusChanged {
            old: ChangeValue::Text("Open".into()),
            new: ChangeValue::Text("Done".into()),
        },
        datetime!(2026-01-02 01:00 UTC),
        vec![user_set_id.clone()],
    );
    let third_event = event(
        "event-3",
        &third_issue,
        UpdateKind::CommentAdded {
            comment_id: "comment-1".into(),
            author: None,
            excerpt: "hello".into(),
        },
        datetime!(2026-01-03 01:00 UTC),
        vec![user_set_id.clone()],
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![first_issue, second_issue, third_issue],
        vec![first_event, second_event, third_event],
        true,
        SyncState::new(site_id.clone(), user_set_id),
    );
    let other_issue = issue(
        other_site.clone(),
        "201",
        "OTHER-201",
        "other",
        datetime!(2026-01-04 00:00 UTC),
    );
    commit(
        &store,
        other_site.clone(),
        other_set.clone(),
        vec![other_issue.clone()],
        vec![event(
            "event-other",
            &other_issue,
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-04 01:00 UTC),
            vec![other_set.clone()],
        )],
        true,
        SyncState::new(other_site.clone(), other_set),
    );
    assert_eq!(
        block_on(UpdateFeedPort::list(
            &store,
            &feed_query(site_id.clone(), false, vec![], 10)
        ))
        .expect("ordered feed")
        .into_iter()
        .map(|event| event.id.as_str().to_owned())
        .collect::<Vec<_>>(),
        vec!["event-3", "event-2", "event-1"]
    );
    assert_eq!(
        block_on(UpdateFeedPort::list(
            &store,
            &feed_query(
                site_id.clone(),
                false,
                vec![UpdateKind::StatusChanged {
                    old: ChangeValue::Text("filter".into()),
                    new: ChangeValue::Text("filter".into()),
                }],
                10,
            ),
        ))
        .expect("filtered feed")
        .into_iter()
        .map(|event| event.id.as_str().to_owned())
        .collect::<Vec<_>>(),
        vec!["event-2"]
    );
    assert_eq!(
        block_on(store.unread_count(&site_id)).expect("unread count"),
        3
    );
    assert_eq!(
        block_on(store.mark_read(&[EventId::new("event-1").expect("event")], true))
            .expect("mark read"),
        1
    );
    assert_eq!(
        block_on(store.mark_read(&[EventId::new("event-1").expect("event")], true))
            .expect("idempotent mark"),
        0
    );
    assert_eq!(
        block_on(store.unread_count(&site_id)).expect("unread count"),
        2
    );
    assert_eq!(
        block_on(UpdateFeedPort::list(
            &store,
            &feed_query(site_id.clone(), true, vec![], 10)
        ))
        .expect("unread feed")
        .len(),
        2
    );
    assert_eq!(
        block_on(store.mark_all_read(&site_id)).expect("mark all"),
        2
    );
    assert_eq!(
        block_on(store.unread_count(&site_id)).expect("unread count"),
        0
    );
    assert_eq!(
        block_on(store.unread_count(&other_site)).expect("other site count"),
        1
    );
}

pub(crate) fn user_sets_are_site_isolated_and_preserve_member_order<S>(store: S)
where
    S: UserSetPort,
{
    let first_site = site("site-a");
    let second_site = site("site-b");
    let first = block_on(UserSetPort::save(
        &store,
        UserSetDraft {
            site_id: first_site.clone(),
            name: "Team".into(),
            members: vec![
                AccountId::new("second").expect("account"),
                AccountId::new("first").expect("account"),
            ],
        },
    ))
    .expect("save first");
    let second = block_on(UserSetPort::save(
        &store,
        UserSetDraft {
            site_id: second_site.clone(),
            name: "Team".into(),
            members: vec![AccountId::new("other").expect("account")],
        },
    ))
    .expect("save second");
    assert_eq!(
        block_on(UserSetPort::list(&store, &first_site)).expect("first list"),
        vec![first.clone()]
    );
    assert_eq!(
        block_on(UserSetPort::list(&store, &second_site)).expect("second list"),
        vec![second]
    );
    assert_eq!(
        first
            .members
            .iter()
            .map(AccountId::as_str)
            .collect::<Vec<_>>(),
        vec!["second", "first"]
    );
}

pub(crate) fn deleting_user_set_cascades_observable_local_state<S>(store: S)
where
    S: IssueCachePort + UpdateFeedPort + UserSetPort,
{
    let site_id = site("site-a");
    let user_set_id = set(&store, site_id.clone(), "team");
    let cached_issue = issue(
        site_id.clone(),
        "100",
        "APP-100",
        "issue",
        datetime!(2026-01-02 00:00 UTC),
    );
    let update = event(
        "event-delete",
        &cached_issue,
        UpdateKind::IssueAddedToView,
        datetime!(2026-01-03 00:00 UTC),
        vec![user_set_id.clone()],
    );
    commit(
        &store,
        site_id.clone(),
        user_set_id.clone(),
        vec![cached_issue],
        vec![update],
        true,
        SyncState::new(site_id.clone(), user_set_id.clone()),
    );
    block_on(UserSetPort::delete(&store, &user_set_id)).expect("delete user set");
    assert!(
        block_on(UserSetPort::list(&store, &site_id))
            .expect("list")
            .is_empty()
    );
    assert!(
        block_on(store.issues_for_user_set(&site_id, &user_set_id))
            .expect("membership")
            .is_empty()
    );
    assert!(
        block_on(store.sync_state(&site_id, &user_set_id))
            .expect("sync state")
            .is_none()
    );
    let events = block_on(UpdateFeedPort::list(
        &store,
        &feed_query(site_id, false, vec![], 10),
    ))
    .expect("feed");
    assert_eq!(events.len(), 1);
    assert!(events[0].matching_user_set_ids.is_empty());
}

fn user(site_id: JiraSiteId, account_id: &str) -> User {
    User::new(
        site_id,
        AccountId::new(account_id).expect("valid account"),
        account_id,
        None,
        true,
    )
}

fn transition(id: &str) -> IssueTransition {
    IssueTransition {
        id: id.to_owned(),
        name: format!("Transition {id}"),
        to: Status {
            id: id.to_owned(),
            name: format!("Status {id}"),
            category: None,
        },
    }
}

pub(crate) fn issue_edit_cache_locator_kind_and_site_isolation<S>(store: S)
where
    S: IssueEditCachePort,
{
    let site_a = site("site-a");
    let site_b = site("site-b");
    let id_locator = IssueLocator::Id(IssueId::new("100").expect("issue id"));
    let key_locator = IssueLocator::Key(IssueKey::new("APP-100").expect("issue key"));
    let users = vec![user(site_a.clone(), "account-1")];
    block_on(store.replace_assignable_users(
        &site_a,
        &id_locator,
        users.clone(),
        datetime!(2026-01-03 00:00 UTC),
    ))
    .expect("replace users");
    assert_eq!(
        block_on(store.cached_assignable_users(&site_a, &id_locator))
            .expect("read users")
            .expect("cached")
            .users,
        users
    );
    assert!(
        block_on(store.cached_assignable_users(&site_a, &key_locator))
            .expect("key isolation")
            .is_none()
    );
    assert!(
        block_on(store.cached_assignable_users(&site_b, &id_locator))
            .expect("site isolation")
            .is_none()
    );
    let transitions = vec![transition("31")];
    block_on(store.replace_issue_transitions(
        &site_a,
        &key_locator,
        transitions.clone(),
        datetime!(2026-01-03 00:00 UTC),
    ))
    .expect("replace transitions");
    assert_eq!(
        block_on(store.cached_issue_transitions(&site_a, &key_locator))
            .expect("read transitions")
            .expect("cached")
            .transitions,
        transitions
    );
    assert!(
        block_on(store.cached_issue_transitions(&site_a, &id_locator))
            .expect("id isolation")
            .is_none()
    );
    assert!(
        block_on(store.cached_issue_transitions(&site_b, &key_locator))
            .expect("site isolation")
            .is_none()
    );
}

pub(crate) fn issue_edit_cache_replacement_and_timestamp_round_trip<S>(store: S)
where
    S: IssueEditCachePort,
{
    let site_id = site("site-a");
    let locator = IssueLocator::Id(IssueId::new("100").expect("issue id"));
    let first_time = datetime!(2026-01-03 00:00:00.123456789 UTC);
    let second_time = datetime!(2026-01-04 00:00:00.987654321 UTC);
    block_on(store.replace_assignable_users(
        &site_id,
        &locator,
        vec![user(site_id.clone(), "first")],
        first_time,
    ))
    .expect("first replacement");
    block_on(store.replace_assignable_users(
        &site_id,
        &locator,
        vec![user(site_id.clone(), "second")],
        second_time,
    ))
    .expect("second replacement");
    let cached = block_on(store.cached_assignable_users(&site_id, &locator))
        .expect("read replacement")
        .expect("cached users");
    assert_eq!(cached.fetched_at, second_time);
    assert_eq!(cached.users, vec![user(site_id.clone(), "second")]);
    let transition_locator = IssueLocator::Key(IssueKey::new("APP-100").expect("issue key"));
    block_on(store.replace_issue_transitions(
        &site_id,
        &transition_locator,
        vec![transition("1")],
        first_time,
    ))
    .expect("first transition replacement");
    block_on(store.replace_issue_transitions(
        &site_id,
        &transition_locator,
        vec![transition("2")],
        second_time,
    ))
    .expect("second transition replacement");
    let cached = block_on(store.cached_issue_transitions(&site_id, &transition_locator))
        .expect("read transition replacement")
        .expect("cached transitions");
    assert_eq!(cached.fetched_at, second_time);
    assert_eq!(cached.transitions, vec![transition("2")]);
}

pub(crate) fn issue_edit_cache_transition_invalidation<S>(store: S)
where
    S: IssueEditCachePort,
{
    let site_id = site("site-a");
    let locator = IssueLocator::Key(IssueKey::new("APP-100").expect("issue key"));
    block_on(store.replace_issue_transitions(
        &site_id,
        &locator,
        vec![transition("1")],
        datetime!(2026-01-03 00:00 UTC),
    ))
    .expect("replace transitions");
    block_on(store.invalidate_issue_transitions(&site_id, &locator)).expect("invalidate");
    assert!(
        block_on(store.cached_issue_transitions(&site_id, &locator))
            .expect("read")
            .is_none()
    );
}

pub(crate) fn issue_edit_cache_enforces_configured_bounds<S>(store: S)
where
    S: IssueEditCachePort,
{
    let site_id = site("site-a");
    let locator = IssueLocator::Id(IssueId::new("100").expect("issue id"));
    let valid_users = vec![user(site_id.clone(), "valid-user")];
    let valid_users_time = datetime!(2026-01-03 00:00:00.123456789 UTC);
    block_on(store.replace_assignable_users(
        &site_id,
        &locator,
        valid_users.clone(),
        valid_users_time,
    ))
    .expect("seed valid users");
    let too_many_users = (0..=MAX_ASSIGNABLE_USER_SEARCH_LIMIT)
        .map(|index| user(site_id.clone(), &format!("account-{index}")))
        .collect();
    assert!(
        block_on(store.replace_assignable_users(
            &site_id,
            &locator,
            too_many_users,
            datetime!(2026-01-03 00:00 UTC),
        ))
        .is_err()
    );
    let cached_users = block_on(store.cached_assignable_users(&site_id, &locator))
        .expect("read users")
        .expect("seeded users remain");
    assert_eq!(cached_users.users, valid_users);
    assert_eq!(cached_users.fetched_at, valid_users_time);

    let valid_transitions = vec![transition("valid-transition")];
    let valid_transitions_time = datetime!(2026-01-03 00:00:00.987654321 UTC);
    block_on(store.replace_issue_transitions(
        &site_id,
        &locator,
        valid_transitions.clone(),
        valid_transitions_time,
    ))
    .expect("seed valid transitions");
    let too_many_transitions = (0..=MAX_ISSUE_TRANSITIONS)
        .map(|index| transition(&index.to_string()))
        .collect();
    assert!(
        block_on(store.replace_issue_transitions(
            &site_id,
            &locator,
            too_many_transitions,
            datetime!(2026-01-03 00:00 UTC),
        ))
        .is_err()
    );
    let cached_transitions = block_on(store.cached_issue_transitions(&site_id, &locator))
        .expect("read transitions")
        .expect("seeded transitions remain");
    assert_eq!(cached_transitions.transitions, valid_transitions);
    assert_eq!(cached_transitions.fetched_at, valid_transitions_time);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InMemoryStore, SqliteStore};

    macro_rules! contract_tests {
        ($scenario:ident, $in_memory_name:ident, $sqlite_name:ident) => {
            #[test]
            fn $in_memory_name() {
                $scenario(InMemoryStore::new());
            }

            #[test]
            fn $sqlite_name() {
                $scenario(SqliteStore::in_memory().expect("open store"));
            }
        };
    }

    contract_tests!(
        issue_cache_ordering_and_pagination,
        in_memory_issue_cache_ordering_and_pagination,
        sqlite_issue_cache_ordering_and_pagination
    );
    contract_tests!(
        issue_cache_query_filters,
        in_memory_issue_cache_query_filters,
        sqlite_issue_cache_query_filters
    );
    contract_tests!(
        issue_cache_detail_snapshot,
        in_memory_issue_cache_detail_snapshot,
        sqlite_issue_cache_detail_snapshot
    );
    contract_tests!(
        issue_cache_membership_replace_and_extend,
        in_memory_issue_cache_membership_replace_and_extend,
        sqlite_issue_cache_membership_replace_and_extend
    );
    contract_tests!(
        issue_cache_site_and_user_set_isolation,
        in_memory_issue_cache_site_and_user_set_isolation,
        sqlite_issue_cache_site_and_user_set_isolation
    );
    contract_tests!(
        issue_cache_sync_state_persistence,
        in_memory_issue_cache_sync_state_persistence,
        sqlite_issue_cache_sync_state_persistence
    );
    contract_tests!(
        issue_cache_invalid_commit_is_atomic,
        in_memory_issue_cache_invalid_commit_is_atomic,
        sqlite_issue_cache_invalid_commit_is_atomic
    );
    contract_tests!(
        issue_cache_records_sync_failures,
        in_memory_issue_cache_records_sync_failures,
        sqlite_issue_cache_records_sync_failures
    );
    contract_tests!(
        issue_cache_records_notification_delivery,
        in_memory_issue_cache_records_notification_delivery,
        sqlite_issue_cache_records_notification_delivery
    );
    contract_tests!(
        update_event_idempotency_and_association_union,
        in_memory_update_event_idempotency_and_association_union,
        sqlite_update_event_idempotency_and_association_union
    );
    contract_tests!(
        update_event_identity_formats_are_opaque_and_deduplicated,
        in_memory_update_event_identity_formats_are_opaque_and_deduplicated,
        sqlite_update_event_identity_formats_are_opaque_and_deduplicated
    );
    contract_tests!(
        update_event_conflict_rejects_without_partial_mutation,
        in_memory_update_event_conflict_rejects_without_partial_mutation,
        sqlite_update_event_conflict_rejects_without_partial_mutation
    );
    contract_tests!(
        update_feed_orders_filters_and_counts,
        in_memory_update_feed_orders_filters_and_counts,
        sqlite_update_feed_orders_filters_and_counts
    );
    contract_tests!(
        user_sets_are_site_isolated_and_preserve_member_order,
        in_memory_user_sets_are_site_isolated_and_preserve_member_order,
        sqlite_user_sets_are_site_isolated_and_preserve_member_order
    );
    contract_tests!(
        deleting_user_set_cascades_observable_local_state,
        in_memory_deleting_user_set_cascades_observable_local_state,
        sqlite_deleting_user_set_cascades_observable_local_state
    );
    contract_tests!(
        issue_edit_cache_locator_kind_and_site_isolation,
        in_memory_issue_edit_cache_locator_kind_and_site_isolation,
        sqlite_issue_edit_cache_locator_kind_and_site_isolation
    );
    contract_tests!(
        issue_edit_cache_replacement_and_timestamp_round_trip,
        in_memory_issue_edit_cache_replacement_and_timestamp_round_trip,
        sqlite_issue_edit_cache_replacement_and_timestamp_round_trip
    );
    contract_tests!(
        issue_edit_cache_transition_invalidation,
        in_memory_issue_edit_cache_transition_invalidation,
        sqlite_issue_edit_cache_transition_invalidation
    );
    contract_tests!(
        issue_edit_cache_enforces_configured_bounds,
        in_memory_issue_edit_cache_enforces_configured_bounds,
        sqlite_issue_edit_cache_enforces_configured_bounds
    );
}
