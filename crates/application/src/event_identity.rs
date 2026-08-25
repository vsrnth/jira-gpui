use jira_domain::{ChangeValue, EventId, IssueId, IssueKey, JiraSiteId, Timestamp, UpdateKind};

/// Derives the identity used by snapshot diffs and changelog enrichment.
///
/// This is intentionally a separate entry point from comment identity. The
/// snapshot/changelog format is part of the persisted event contract: it uses
/// big-endian component lengths and includes the canonical update kind.
pub(crate) fn snapshot_event_id(
    site_id: &JiraSiteId,
    issue_id: &IssueId,
    transition: &str,
    update_boundary: Timestamp,
    field: &str,
    kind: &UpdateKind,
) -> EventId {
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
    let left = digest(&parts, 0xcbf29ce484222325, Endianness::Big);
    let right = digest(&parts, 0x84222325cbf29ce4, Endianness::Big);
    EventId::new(format!("v1-{left:016x}{right:016x}")).expect("event ID length")
}

/// Derives the identity used by comment activity events.
///
/// This preserves the pre-existing comment format independently of snapshot
/// identity: it uses little-endian component lengths and only site, issue,
/// comment, and activity inputs.
pub(crate) fn comment_event_id(
    site_id: &JiraSiteId,
    issue_id: &IssueId,
    comment_id: &str,
    activity_at: Timestamp,
) -> EventId {
    let activity = activity_at.unix_timestamp_nanos().to_string();
    let parts = [site_id.as_str(), issue_id.as_str(), comment_id, &activity];
    let left = digest(&parts, 0xcbf29ce484222325, Endianness::Little);
    let right = digest(&parts, 0x84222325cbf29ce4, Endianness::Little);
    EventId::new(format!("v1-comment-{left:016x}{right:016x}")).expect("event ID length")
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
        UpdateKind::FieldChanged { field, old, new } => {
            fields.push("field_changed".into());
            fields.push(field.clone());
            canonical_change_value(&mut fields, old);
            canonical_change_value(&mut fields, new);
        }
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

#[derive(Clone, Copy)]
enum Endianness {
    Big,
    Little,
}

fn digest(parts: &[&str], mut hash: u64, endianness: Endianness) -> u64 {
    for part in parts {
        let length = match endianness {
            Endianness::Big => (part.len() as u64).to_be_bytes(),
            Endianness::Little => (part.len() as u64).to_le_bytes(),
        };
        for byte in length.into_iter().chain(part.bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn site(value: &str) -> JiraSiteId {
        JiraSiteId::new(value).expect("valid site")
    }

    fn issue(value: &str) -> IssueId {
        IssueId::new(value).expect("valid issue")
    }

    #[test]
    fn golden_snapshot_field_and_changelog_identities() {
        let site_id = site("site-a");
        let issue_id = issue("10001");
        assert_eq!(
            snapshot_event_id(
                &site_id,
                &issue_id,
                "field",
                datetime!(2026-08-16 11:00 UTC),
                "summary",
                &UpdateKind::SummaryChanged {
                    old: ChangeValue::Text("old summary".into()),
                    new: ChangeValue::Text("new summary".into()),
                },
            )
            .as_str(),
            "v1-f7c343a8639b995e61431f1ac84575d9"
        );
        assert_eq!(
            snapshot_event_id(
                &site_id,
                &issue_id,
                "changelog",
                datetime!(2026-08-16 10:30 UTC),
                "history-1:0:labels",
                &UpdateKind::FieldChanged {
                    field: "Labels".into(),
                    old: ChangeValue::Empty,
                    new: ChangeValue::Text("new-label".into()),
                },
            )
            .as_str(),
            "v1-1074d99f53f973a3120aff8b74de6b2e"
        );
    }

    #[test]
    fn golden_comment_identity() {
        assert_eq!(
            comment_event_id(
                &site("site-a"),
                &issue("10001"),
                "comment-1",
                datetime!(2026-08-16 11:00 UTC),
            )
            .as_str(),
            "v1-comment-66bae53b96dfa491611148a3ee34ba0e"
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
            author: Some(jira_domain::AccountId::new("none").expect("valid test account")),
            excerpt: "excerpt".into(),
        });
        assert_ne!(without_author, with_account_named_none);
    }

    #[test]
    fn identity_inputs_are_separated() {
        let site_id = site("site-a");
        let issue_id = issue("10001");
        let kind = UpdateKind::IssueUpdated;
        let base = snapshot_event_id(
            &site_id,
            &issue_id,
            "field",
            datetime!(2026-08-16 11:00 UTC),
            "issue_updated",
            &kind,
        );
        assert_ne!(
            base,
            snapshot_event_id(
                &site_id,
                &issue_id,
                "field",
                datetime!(2026-08-16 11:00 UTC),
                "issue-updated",
                &kind,
            )
        );
        let comment = comment_event_id(
            &site_id,
            &issue_id,
            "comment-1",
            datetime!(2026-08-16 11:00 UTC),
        );
        assert_ne!(
            comment,
            comment_event_id(
                &site_id,
                &issue_id,
                "comment-2",
                datetime!(2026-08-16 11:00 UTC),
            )
        );
        assert_ne!(
            comment,
            comment_event_id(
                &site_id,
                &issue_id,
                "comment-1",
                datetime!(2026-08-16 11:01 UTC),
            )
        );
    }
}
