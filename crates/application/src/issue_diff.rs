//! Deterministic derivation of local update-feed events from issue snapshots.
//!
//! Jira does not provide a changelog for this read-only synchronization path,
//! so the application compares the last cached snapshot with the latest
//! snapshot. Event IDs intentionally do not contain the local observation
//! time: retrying the same Jira update at a different time must produce the
//! same ID and let persistence deduplicate it.

use std::collections::{BTreeMap, BTreeSet};

use jira_domain::{
    ChangeValue, Issue, IssueKey, IssueLifecycle, JiraSiteId, Timestamp, UpdateEvent, UpdateKind,
    UserSetId,
};

use crate::{ApplicationError, ChangeSet, ErrorKind, IssueDiffer};

/// Production snapshot differ used by the synchronization service.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultIssueDiffer;

impl IssueDiffer for DefaultIssueDiffer {
    fn diff(&self, change_set: ChangeSet) -> Result<Vec<UpdateEvent>, ApplicationError> {
        let existing = index_snapshot(&change_set.site_id, change_set.existing, "existing")?;
        let incoming = index_snapshot(&change_set.site_id, change_set.incoming, "incoming")?;
        let user_set_id = change_set.user_set_id;

        let mut issue_ids = BTreeSet::new();
        issue_ids.extend(existing.keys().cloned());
        issue_ids.extend(incoming.keys().cloned());

        let mut events = Vec::new();
        for issue_id in issue_ids {
            match (existing.get(&issue_id), incoming.get(&issue_id)) {
                (Some(old), Some(new)) if is_in_view(old) && is_in_view(new) => {
                    append_field_events(
                        &mut events,
                        old,
                        new,
                        &change_set.site_id,
                        &user_set_id,
                        change_set.detected_at,
                    );
                }
                // A cached tombstone becoming present again is a fresh
                // membership transition, not a field update.
                (Some(old), Some(new)) if !is_in_view(old) && is_in_view(new) => {
                    events.push(new_event(
                        &change_set.site_id,
                        new,
                        UpdateKind::IssueAddedToView,
                        change_set.detected_at,
                        &user_set_id,
                        "added",
                        new.updated_at,
                        "membership",
                    ));
                }
                (None, Some(new)) if is_in_view(new) => {
                    events.push(new_event(
                        &change_set.site_id,
                        new,
                        UpdateKind::IssueAddedToView,
                        change_set.detected_at,
                        &user_set_id,
                        "added",
                        new.updated_at,
                        "membership",
                    ));
                }
                (Some(old), None) if change_set.include_removed_from_view && is_in_view(old) => {
                    events.push(new_event(
                        &change_set.site_id,
                        old,
                        UpdateKind::IssueRemovedFromView,
                        change_set.detected_at,
                        &user_set_id,
                        "removed",
                        old.updated_at,
                        "membership",
                    ));
                }
                _ => {}
            }
        }

        Ok(events)
    }
}

fn index_snapshot(
    site_id: &JiraSiteId,
    issues: Vec<Issue>,
    snapshot_name: &str,
) -> Result<BTreeMap<jira_domain::IssueId, Issue>, ApplicationError> {
    let mut indexed = BTreeMap::new();
    for issue in issues {
        if issue.site_id != *site_id {
            return Err(snapshot_error("issue site does not match change set site"));
        }
        if indexed.insert(issue.id.clone(), issue).is_some() {
            return Err(snapshot_error(match snapshot_name {
                "existing" => "duplicate issue ID in existing snapshot",
                _ => "duplicate issue ID in incoming snapshot",
            }));
        }
    }
    Ok(indexed)
}

fn snapshot_error(message: &'static str) -> ApplicationError {
    // A duplicate or cross-site snapshot is an upstream contract violation,
    // rather than a user input problem. Keeping one category also gives the
    // sync layer a consistent retry/diagnostic policy.
    ApplicationError::new(ErrorKind::Upstream, message)
}

fn is_in_view(issue: &Issue) -> bool {
    issue.lifecycle == IssueLifecycle::Present
}

fn append_field_events(
    events: &mut Vec<UpdateEvent>,
    old: &Issue,
    new: &Issue,
    site_id: &JiraSiteId,
    user_set_id: &UserSetId,
    detected_at: Timestamp,
) {
    let initial_event_count = events.len();
    // The fixed order is part of the feed contract and keeps output stable
    // even if this function is later changed to use a map of field values.
    // The event exposes only the display name. Workflow category and status
    // ID metadata are intentionally not part of the event value.
    if old.status.name != new.status.name {
        events.push(new_event(
            site_id,
            new,
            UpdateKind::StatusChanged {
                old: ChangeValue::Text(old.status.name.clone()),
                new: ChangeValue::Text(new.status.name.clone()),
            },
            detected_at,
            user_set_id,
            "field",
            new.updated_at,
            "status",
        ));
    }
    if old.assignee != new.assignee {
        events.push(new_event(
            site_id,
            new,
            UpdateKind::AssigneeChanged {
                old: account_value(old.assignee.as_ref()),
                new: account_value(new.assignee.as_ref()),
            },
            detected_at,
            user_set_id,
            "field",
            new.updated_at,
            "assignee",
        ));
    }
    if priority_value(&old.priority) != priority_value(&new.priority) {
        events.push(new_event(
            site_id,
            new,
            UpdateKind::PriorityChanged {
                old: priority_value(&old.priority),
                new: priority_value(&new.priority),
            },
            detected_at,
            user_set_id,
            "field",
            new.updated_at,
            "priority",
        ));
    }
    if old.due_date != new.due_date {
        events.push(new_event(
            site_id,
            new,
            UpdateKind::DueDateChanged {
                old: ChangeValue::Date(old.due_date.map(|date| date.to_string())),
                new: ChangeValue::Date(new.due_date.map(|date| date.to_string())),
            },
            detected_at,
            user_set_id,
            "field",
            new.updated_at,
            "due_date",
        ));
    }
    if old.summary != new.summary {
        events.push(new_event(
            site_id,
            new,
            UpdateKind::SummaryChanged {
                old: ChangeValue::Text(old.summary.clone()),
                new: ChangeValue::Text(new.summary.clone()),
            },
            detected_at,
            user_set_id,
            "field",
            new.updated_at,
            "summary",
        ));
    }
    if parent_value(old.parent.as_ref()) != parent_value(new.parent.as_ref()) {
        events.push(new_event(
            site_id,
            new,
            UpdateKind::ParentChanged {
                old: parent_value(old.parent.as_ref()),
                new: parent_value(new.parent.as_ref()),
            },
            detected_at,
            user_set_id,
            "field",
            new.updated_at,
            "parent",
        ));
    }

    if old.updated_at != new.updated_at && events.len() == initial_event_count {
        events.push(new_event(
            site_id,
            new,
            UpdateKind::IssueUpdated,
            detected_at,
            user_set_id,
            "field",
            new.updated_at,
            "issue_updated",
        ));
    }
}

fn account_value(account: Option<&jira_domain::AccountId>) -> ChangeValue {
    account
        .cloned()
        .map(ChangeValue::Account)
        .unwrap_or(ChangeValue::Empty)
}

fn priority_value(priority: &jira_domain::Priority) -> ChangeValue {
    priority
        .name
        .clone()
        .or_else(|| priority.id.clone())
        .map(ChangeValue::Text)
        .unwrap_or(ChangeValue::Empty)
}

fn parent_value(parent: Option<&jira_domain::ParentIssue>) -> ChangeValue {
    ChangeValue::Parent(parent.map(|parent| parent.key.clone()))
}

#[allow(clippy::too_many_arguments)]
fn new_event(
    site_id: &JiraSiteId,
    issue: &Issue,
    kind: UpdateKind,
    occurred_at: Timestamp,
    user_set_id: &UserSetId,
    transition: &str,
    update_boundary: Timestamp,
    field: &str,
) -> UpdateEvent {
    let id = event_id(
        site_id,
        &issue.id,
        transition,
        update_boundary,
        field,
        &kind,
    );
    UpdateEvent::new(
        id,
        site_id.clone(),
        issue.id.clone(),
        issue.key.clone(),
        kind,
        occurred_at,
        vec![user_set_id.clone()],
    )
}

/// Event IDs use the versioned `v1-<32 hex digits>` format. Each canonical
/// component is length-prefixed before hashing, avoiding delimiter ambiguity.
/// Two independent FNV-1a 64-bit lanes provide a fixed-width 128-bit digest
/// without depending on a randomized or implementation-defined hasher.
///
/// `update_boundary` is the Jira issue's `updated_at` for field/add events and
/// the cached issue's `updated_at` for remove events. It is deliberately not
/// `detected_at`, which is a local observation time and changes across retries.
fn event_id(
    site_id: &JiraSiteId,
    issue_id: &jira_domain::IssueId,
    transition: &str,
    update_boundary: Timestamp,
    field: &str,
    kind: &UpdateKind,
) -> jira_domain::EventId {
    let boundary = update_boundary.unix_timestamp_nanos().to_string();
    let mut parts = vec![
        site_id.as_str(),
        issue_id.as_str(),
        transition,
        &boundary,
        field,
    ];
    let kind_parts = canonical_kind(kind);
    parts.extend(kind_parts.iter().map(String::as_str));
    let left = digest(&parts, 0xcbf29ce484222325);
    let right = digest(&parts, 0x84222325cbf29ce4);
    // Components are bounded by domain constructors, so this cannot exceed
    // EventId's 255-character limit.
    jira_domain::EventId::new(format!("v1-{left:016x}{right:016x}")).expect("event ID length")
}

/// Returns the complete, version-independent payload of an update kind as
/// canonical fields. This deliberately avoids `Debug`/serde formatting so an
/// event ID remains stable if presentation derives or serializer details
/// change. The comment arm is exhaustive for future callers even though the
/// snapshot differ currently emits no comment events.
fn canonical_kind(kind: &UpdateKind) -> Vec<String> {
    let mut fields = Vec::new();
    match kind {
        UpdateKind::IssueAddedToView => fields.push("issue_added_to_view".into()),
        UpdateKind::IssueRemovedFromView => fields.push("issue_removed_from_view".into()),
        UpdateKind::IssueUpdated => fields.push("issue_updated".into()),
        UpdateKind::StatusChanged { old, new } => {
            fields.push("status_changed".into());
            canonical_change_value(&mut fields, old);
            canonical_change_value(&mut fields, new);
        }
        UpdateKind::AssigneeChanged { old, new } => {
            fields.push("assignee_changed".into());
            canonical_change_value(&mut fields, old);
            canonical_change_value(&mut fields, new);
        }
        UpdateKind::PriorityChanged { old, new } => {
            fields.push("priority_changed".into());
            canonical_change_value(&mut fields, old);
            canonical_change_value(&mut fields, new);
        }
        UpdateKind::DueDateChanged { old, new } => {
            fields.push("due_date_changed".into());
            canonical_change_value(&mut fields, old);
            canonical_change_value(&mut fields, new);
        }
        UpdateKind::SummaryChanged { old, new } => {
            fields.push("summary_changed".into());
            canonical_change_value(&mut fields, old);
            canonical_change_value(&mut fields, new);
        }
        UpdateKind::ParentChanged { old, new } => {
            fields.push("parent_changed".into());
            canonical_change_value(&mut fields, old);
            canonical_change_value(&mut fields, new);
        }
        UpdateKind::CommentAdded {
            comment_id,
            author,
            excerpt,
        } => {
            fields.push("comment_added".into());
            fields.push(comment_id.clone());
            fields.push("author".into());
            match author {
                Some(account) => {
                    fields.push("some".into());
                    fields.push(account.as_str().into());
                }
                None => fields.push("none".into()),
            }
            fields.push(excerpt.clone());
        }
    }
    fields
}

fn canonical_change_value(fields: &mut Vec<String>, value: &ChangeValue) {
    match value {
        ChangeValue::Text(value) => {
            fields.push("text".into());
            fields.push(value.clone());
        }
        ChangeValue::Account(value) => {
            fields.push("account".into());
            fields.push(value.as_str().into());
        }
        ChangeValue::Date(value) => {
            fields.push("date".into());
            fields.push(value.as_deref().unwrap_or("none").into());
        }
        ChangeValue::Parent(value) => {
            fields.push("parent".into());
            fields.push(
                value
                    .as_ref()
                    .map(IssueKey::as_str)
                    .unwrap_or("none")
                    .into(),
            );
        }
        ChangeValue::Empty => fields.push("empty".into()),
    }
}

fn digest(parts: &[&str], mut hash: u64) -> u64 {
    for part in parts {
        for byte in (part.len() as u64)
            .to_be_bytes()
            .into_iter()
            .chain(part.bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
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
        new.due_date = Some(
            Date::from_calendar_date(2026, time::Month::August, 20).expect("valid test due date"),
        );
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
    fn canonical_comment_author_distinguishes_none_from_account_named_none() {
        let without_author = canonical_kind(&UpdateKind::CommentAdded {
            comment_id: "comment-1".into(),
            author: None,
            excerpt: "excerpt".into(),
        });
        let with_account_named_none = canonical_kind(&UpdateKind::CommentAdded {
            comment_id: "comment-1".into(),
            author: Some(AccountId::new("none").expect("valid test account")),
            excerpt: "excerpt".into(),
        });
        assert_ne!(without_author, with_account_named_none);
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
}
