use crate::IssueChangelogHistory;
use jira_domain::{
    AccountId, IssueId, IssueKey, IssueType, ParentIssue, Priority, Project, Status,
};
use time::{Date, macros::datetime};

use super::*;

fn site(value: &str) -> JiraSiteId {
    JiraSiteId::new(value).expect("valid test site")
}

fn user_set(value: &str) -> UserSetId {
    UserSetId::new(value).expect("valid test user set")
}

fn issue(id: &str, updated_at: Timestamp) -> Issue {
    Issue::new(
        site("site-a"),
        IssueId::new(id).expect("valid test issue ID"),
        IssueKey::new(format!("APP-{id}")).expect("valid test issue key"),
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
        "summary",
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
        Some(AccountId::new("account-old").expect("valid test account")),
        None,
        None,
        Vec::new(),
        datetime!(2026-08-16 10:00 UTC),
        updated_at,
        None,
    )
}

fn change_set(existing: Vec<Issue>, incoming: Vec<Issue>) -> ChangeSet {
    ChangeSet {
        existing,
        incoming,
        site_id: site("site-a"),
        user_set_id: user_set("team-a"),
        detected_at: datetime!(2026-08-16 12:00 UTC),
        include_removed_from_view: false,
    }
}

fn changed_issue() -> Issue {
    let mut new = issue("10001", datetime!(2026-08-16 11:00 UTC));
    new.summary = "new summary".into();
    new.status = Status {
        id: "done".into(),
        name: "Done".into(),
        category: Some("done".into()),
    };
    new.assignee = None;
    new.priority = Priority {
        id: None,
        name: None,
        icon_url: None,
    };
    new.due_date =
        Some(Date::from_calendar_date(2026, time::Month::August, 20).expect("valid test due date"));
    new.parent = Some(ParentIssue {
        id: IssueId::new("10000").expect("valid test parent ID"),
        key: IssueKey::new("APP-10000").expect("valid test parent key"),
        summary: Some("Epic".into()),
    });
    new
}

#[test]
fn timestamp_only_change_emits_one_generic_issue_update() {
    let old = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let new = issue("10001", datetime!(2026-08-16 11:00 UTC));
    let events = DefaultIssueDiffer
        .diff(change_set(vec![old.clone()], vec![new.clone()]))
        .expect("timestamp-only changes should diff");

    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].kind, UpdateKind::IssueUpdated));

    let mut retry = change_set(vec![old], vec![new]);
    retry.detected_at = datetime!(2026-08-17 12:00 UTC);
    let retry_events = DefaultIssueDiffer.diff(retry).expect("retry should diff");
    assert_eq!(events[0].id, retry_events[0].id);
    assert_ne!(events[0].occurred_at, retry_events[0].occurred_at);
}

#[test]
fn specific_field_change_with_timestamp_emits_only_specific_event() {
    let old = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let mut new = issue("10001", datetime!(2026-08-16 11:00 UTC));
    new.summary = "changed summary".into();

    let events = DefaultIssueDiffer
        .diff(change_set(vec![old], vec![new]))
        .expect("specific changes should diff");

    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].kind, UpdateKind::SummaryChanged { .. }));
}

#[test]
fn changelog_enrichment_filters_window_deduplicates_snapshot_fields_and_is_stable() {
    let old = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let mut new = issue("10001", datetime!(2026-08-16 11:00 UTC));
    new.summary = "changed summary".into();
    new.labels = vec!["new-label".into()];
    let base = DefaultIssueDiffer
        .diff(change_set(vec![old.clone()], vec![new.clone()]))
        .expect("snapshot diff");
    let changelogs = vec![IssueChangelog {
        issue_id: new.id.clone(),
        histories: vec![
            IssueChangelogHistory {
                id: "history-1".into(),
                created: datetime!(2026-08-16 10:30 UTC),
                items: vec![
                    IssueChangelogItem {
                        field: Some("summary".into()),
                        field_id: Some("summary".into()),
                        from_string: Some("summary".into()),
                        to_string: Some("changed summary".into()),
                    },
                    IssueChangelogItem {
                        field: Some("Labels".into()),
                        field_id: Some("labels".into()),
                        from_string: None,
                        to_string: Some("new-label".into()),
                    },
                ],
            },
            IssueChangelogHistory {
                id: "outside-window".into(),
                created: datetime!(2026-08-16 09:59 UTC),
                items: vec![IssueChangelogItem {
                    field: Some("description".into()),
                    field_id: None,
                    from_string: Some("old".into()),
                    to_string: Some("new".into()),
                }],
            },
        ],
    }];
    let enriched = enrich_with_changelog(
        base,
        &[old],
        &[new.clone()],
        &changelogs,
        &site("site-a"),
        &user_set("team-a"),
    );
    assert_eq!(enriched.len(), 2);
    assert!(
        enriched
            .iter()
            .all(|event| matches!(event.kind, UpdateKind::FieldChanged { .. }))
    );
    assert!(enriched.iter().any(|event| matches!(
        &event.kind,
        UpdateKind::FieldChanged { field, .. } if field == "summary"
    )));
    assert!(enriched.iter().any(|event| matches!(
        &event.kind,
        UpdateKind::FieldChanged { field, .. } if field == "Labels"
    )));
    let reversed = enrich_with_changelog(
        DefaultIssueDiffer
            .diff(change_set(
                vec![issue("10001", datetime!(2026-08-16 10:00 UTC))],
                vec![new.clone()],
            ))
            .expect("snapshot diff"),
        &[issue("10001", datetime!(2026-08-16 10:00 UTC))],
        &[new],
        &changelogs,
        &site("site-a"),
        &user_set("team-a"),
    );
    assert_eq!(
        enriched.iter().map(|event| &event.id).collect::<Vec<_>>(),
        reversed.iter().map(|event| &event.id).collect::<Vec<_>>()
    );
}

#[test]
fn changelog_deduplication_is_isolated_per_issue_and_missing_values_stay_generic() {
    let old_one = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let old_two = issue("10002", datetime!(2026-08-16 10:00 UTC));
    let mut new_one = issue("10001", datetime!(2026-08-16 11:00 UTC));
    new_one.labels = vec!["new".into()];
    let new_two = issue("10002", datetime!(2026-08-16 11:00 UTC));
    let events = DefaultIssueDiffer
        .diff(change_set(
            vec![old_one.clone(), old_two.clone()],
            vec![new_one.clone(), new_two.clone()],
        ))
        .expect("snapshot diff");
    let enriched = enrich_with_changelog(
        events,
        &[old_one, old_two],
        &[new_one.clone(), new_two],
        &[IssueChangelog {
            issue_id: new_one.id.clone(),
            histories: vec![IssueChangelogHistory {
                id: "history-1".into(),
                created: datetime!(2026-08-16 10:30 UTC),
                items: vec![
                    IssueChangelogItem {
                        field: Some("labels".into()),
                        field_id: None,
                        from_string: None,
                        to_string: Some("new".into()),
                    },
                    IssueChangelogItem {
                        field: Some("description".into()),
                        field_id: None,
                        from_string: None,
                        to_string: None,
                    },
                ],
            }],
        }],
        &site("site-a"),
        &user_set("team-a"),
    );
    assert!(enriched.iter().any(|event| {
        event.issue_id.as_str() == "10001" && matches!(event.kind, UpdateKind::FieldChanged { .. })
    }));
    assert!(enriched.iter().any(|event| {
        event.issue_id.as_str() == "10002" && matches!(event.kind, UpdateKind::IssueUpdated)
    }));
    assert_eq!(enriched.len(), 2);
}

#[test]
fn identical_snapshots_emit_no_events() {
    let snapshot = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let events = DefaultIssueDiffer
        .diff(change_set(vec![snapshot.clone()], vec![snapshot]))
        .expect("identical snapshots should diff");

    assert!(events.is_empty());
}

#[test]
fn emits_all_supported_field_events_with_none_semantics() {
    let events = DefaultIssueDiffer
        .diff(change_set(
            vec![issue("10001", datetime!(2026-08-16 10:00 UTC))],
            vec![changed_issue()],
        ))
        .expect("all supported fields should diff");
    assert_eq!(events.len(), 6);
    assert!(matches!(events[0].kind, UpdateKind::StatusChanged { .. }));
    assert!(matches!(events[1].kind, UpdateKind::AssigneeChanged { .. }));
    assert!(matches!(events[2].kind, UpdateKind::PriorityChanged { .. }));
    assert!(matches!(events[3].kind, UpdateKind::DueDateChanged { .. }));
    assert!(matches!(events[4].kind, UpdateKind::SummaryChanged { .. }));
    assert!(matches!(events[5].kind, UpdateKind::ParentChanged { .. }));
    assert!(matches!(
        events[1].kind,
        UpdateKind::AssigneeChanged {
            new: ChangeValue::Empty,
            ..
        }
    ));
    assert!(matches!(
        events[2].kind,
        UpdateKind::PriorityChanged {
            new: ChangeValue::Empty,
            ..
        }
    ));
    assert!(matches!(
        events[3].kind,
        UpdateKind::DueDateChanged {
            old: ChangeValue::Date(None),
            ..
        }
    ));
    assert!(matches!(
        events[5].kind,
        UpdateKind::ParentChanged {
            old: ChangeValue::Parent(None),
            ..
        }
    ));
}

#[test]
fn add_and_remove_follow_membership_policy() {
    let old = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let added = issue("10002", datetime!(2026-08-16 11:00 UTC));
    let no_remove = DefaultIssueDiffer
        .diff(change_set(vec![old.clone()], vec![added.clone()]))
        .expect("new issue should be added");
    assert_eq!(no_remove.len(), 1);
    assert!(matches!(no_remove[0].kind, UpdateKind::IssueAddedToView));

    let mut set = change_set(vec![old], vec![added]);
    set.include_removed_from_view = true;
    let with_remove = DefaultIssueDiffer
        .diff(set)
        .expect("membership removal should diff");
    assert_eq!(with_remove.len(), 2);
    assert!(matches!(
        with_remove[0].kind,
        UpdateKind::IssueRemovedFromView
    ));
    assert!(matches!(with_remove[1].kind, UpdateKind::IssueAddedToView));
}

#[test]
fn output_ids_and_order_are_stable_across_input_order_and_detection_time() {
    let old_a = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let old_b = issue("10002", datetime!(2026-08-16 10:00 UTC));
    let mut new_a = changed_issue();
    new_a.updated_at = datetime!(2026-08-16 11:00 UTC);
    let mut new_b = issue("10002", datetime!(2026-08-16 11:30 UTC));
    new_b.summary = "changed".into();

    let first = DefaultIssueDiffer
        .diff(change_set(
            vec![old_b.clone(), old_a.clone()],
            vec![new_b.clone(), new_a.clone()],
        ))
        .expect("reordered input should diff");
    let mut second_set = change_set(vec![old_a, old_b], vec![new_a, new_b]);
    second_set.detected_at = datetime!(2026-08-17 12:00 UTC);
    let second = DefaultIssueDiffer
        .diff(second_set)
        .expect("retry should diff");
    assert_eq!(
        first
            .iter()
            .map(|event| (&event.id, &event.kind))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|event| (&event.id, &event.kind))
            .collect::<Vec<_>>()
    );
    assert_ne!(first[0].occurred_at, second[0].occurred_at);
    assert!(
        first
            .iter()
            .all(|event| event.id.as_str().starts_with("v1-"))
    );
}

#[test]
fn different_values_at_the_same_update_boundary_have_different_ids() {
    let old = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let mut first = old.clone();
    first.updated_at = datetime!(2026-08-16 11:00 UTC);
    first.summary = "first transition".into();
    let mut second = old.clone();
    second.updated_at = first.updated_at;
    second.summary = "second transition".into();

    let first_event = DefaultIssueDiffer
        .diff(change_set(vec![old.clone()], vec![first]))
        .expect("first transition should diff")
        .remove(0);
    let second_event = DefaultIssueDiffer
        .diff(change_set(vec![old], vec![second]))
        .expect("second transition should diff")
        .remove(0);
    assert_ne!(first_event.id, second_event.id);
}

#[test]
fn metadata_only_changes_do_not_emit_identical_looking_events() {
    let old = issue("10001", datetime!(2026-08-16 10:00 UTC));

    let mut status_metadata = old.clone();
    status_metadata.status.category = Some("done".into());
    assert!(
        DefaultIssueDiffer
            .diff(change_set(vec![old.clone()], vec![status_metadata]))
            .expect("status metadata should diff")
            .is_empty()
    );

    let mut status_identity_metadata = old.clone();
    status_identity_metadata.status.id = "different-open-id".into();
    assert!(
        DefaultIssueDiffer
            .diff(change_set(
                vec![old.clone()],
                vec![status_identity_metadata]
            ))
            .expect("status ID metadata should diff")
            .is_empty()
    );

    let mut priority_metadata = old.clone();
    priority_metadata.priority.icon_url = Some("https://example.test/icon".into());
    assert!(
        DefaultIssueDiffer
            .diff(change_set(vec![old.clone()], vec![priority_metadata]))
            .expect("priority metadata should diff")
            .is_empty()
    );

    let mut parent_metadata = old.clone();
    parent_metadata.parent = Some(ParentIssue {
        id: IssueId::new("10000").expect("valid test parent ID"),
        key: IssueKey::new("APP-10000").expect("valid test parent key"),
        summary: Some("old summary".into()),
    });
    let mut parent_metadata_changed = parent_metadata.clone();
    parent_metadata_changed
        .parent
        .as_mut()
        .expect("parent exists")
        .summary = Some("new summary".into());
    assert!(
        DefaultIssueDiffer
            .diff(change_set(
                vec![parent_metadata],
                vec![parent_metadata_changed]
            ))
            .expect("parent metadata should diff")
            .is_empty()
    );
}

#[test]
fn rejects_cross_site_and_duplicate_snapshots() {
    let mut cross_site = issue("10001", datetime!(2026-08-16 10:00 UTC));
    cross_site.site_id = site("site-b");
    assert_eq!(
        DefaultIssueDiffer
            .diff(change_set(Vec::new(), vec![cross_site]))
            .expect_err("cross-site issue must be rejected")
            .kind(),
        ErrorKind::Upstream
    );

    let duplicate = issue("10001", datetime!(2026-08-16 10:00 UTC));
    let error = DefaultIssueDiffer
        .diff(change_set(vec![duplicate.clone(), duplicate], Vec::new()))
        .expect_err("duplicate issue must be rejected");
    assert_eq!(error.kind(), ErrorKind::Upstream);
}

#[test]
fn matches_exactly_one_requested_user_set() {
    let events = DefaultIssueDiffer
        .diff(change_set(
            Vec::new(),
            vec![issue("10001", datetime!(2026-08-16 10:00 UTC))],
        ))
        .expect("new issue should include user set");
    assert_eq!(events[0].matching_user_set_ids, vec![user_set("team-a")]);
}
