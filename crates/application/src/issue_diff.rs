//! Deterministic derivation of local update-feed events from issue snapshots.
//!
//! Jira does not provide a changelog for this read-only synchronization path,
//! so the application compares the last cached snapshot with the latest
//! snapshot. Event IDs intentionally do not contain the local observation
//! time: retrying the same Jira update at a different time must produce the
//! same ID and let persistence deduplicate it.

use std::collections::{BTreeMap, BTreeSet};

use jira_domain::{
    ChangeValue, Issue, IssueLifecycle, JiraSiteId, Timestamp, UpdateEvent, UpdateKind, UserSetId,
};

use crate::{
    ApplicationError, ChangeSet, ErrorKind, IssueChangelog, IssueChangelogItem, IssueDiffer,
};

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

/// Replaces snapshot-only events with bounded changelog events when Jira
/// returned usable history inside the snapshot update window. A failed or
/// unsupported changelog read is represented by the caller passing no logs;
/// the original generic/snapshot events then remain honest fallbacks.
pub fn enrich_with_changelog(
    mut events: Vec<UpdateEvent>,
    existing: &[Issue],
    incoming: &[Issue],
    changelogs: &[IssueChangelog],
    site_id: &JiraSiteId,
    user_set_id: &UserSetId,
) -> Vec<UpdateEvent> {
    let old_by_id = existing
        .iter()
        .map(|issue| (&issue.id, issue))
        .collect::<BTreeMap<_, _>>();
    let new_by_id = incoming
        .iter()
        .map(|issue| (&issue.id, issue))
        .collect::<BTreeMap<_, _>>();
    let mut mapped = Vec::new();
    let mut seen_items = BTreeSet::new();
    let mut changed_fields = BTreeSet::new();
    let mut enriched_issue_ids = BTreeSet::new();
    for changelog in changelogs {
        let Some(old) = old_by_id.get(&changelog.issue_id) else {
            continue;
        };
        let Some(new) = new_by_id.get(&changelog.issue_id) else {
            continue;
        };
        if !is_in_view(old) || !is_in_view(new) || old.updated_at == new.updated_at {
            continue;
        }
        for history in &changelog.histories {
            if history.created <= old.updated_at || history.created > new.updated_at {
                continue;
            }
            if history.id.trim().is_empty() {
                continue;
            }
            for (item_index, item) in history.items.iter().enumerate() {
                if !seen_items.insert((changelog.issue_id.clone(), history.id.clone(), item_index))
                {
                    continue;
                }
                let Some((field_key, field_name, old_value, new_value)) = map_changelog_item(item)
                else {
                    continue;
                };
                if old_value == new_value {
                    continue;
                }
                changed_fields.insert((changelog.issue_id.clone(), field_key.clone()));
                enriched_issue_ids.insert(changelog.issue_id.clone());
                let token = format!("{}:{}:{}", history.id, item_index, field_key);
                mapped.push(new_event(
                    site_id,
                    new,
                    UpdateKind::FieldChanged {
                        field: field_name,
                        old: old_value,
                        new: new_value,
                    },
                    history.created,
                    user_set_id,
                    "changelog",
                    history.created,
                    &token,
                ));
            }
        }
    }
    if mapped.is_empty() {
        return events;
    }
    events.retain(|event| {
        if matches!(event.kind, UpdateKind::IssueUpdated) {
            return !enriched_issue_ids.contains(&event.issue_id);
        }
        event_field_key(&event.kind).is_none_or(|field| {
            !changed_fields.contains(&(event.issue_id.clone(), field.to_owned()))
        })
    });
    events.extend(mapped);
    events.sort_by(|left, right| {
        left.occurred_at
            .cmp(&right.occurred_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    events
}

fn event_field_key(kind: &UpdateKind) -> Option<&'static str> {
    match kind {
        UpdateKind::StatusChanged { .. } => Some("status"),
        UpdateKind::AssigneeChanged { .. } => Some("assignee"),
        UpdateKind::PriorityChanged { .. } => Some("priority"),
        UpdateKind::DueDateChanged { .. } => Some("duedate"),
        UpdateKind::SummaryChanged { .. } => Some("summary"),
        UpdateKind::ParentChanged { .. } => Some("parent"),
        _ => None,
    }
}

fn map_changelog_item(
    item: &IssueChangelogItem,
) -> Option<(String, String, ChangeValue, ChangeValue)> {
    let raw_field = item
        .field
        .as_deref()
        .or(item.field_id.as_deref())
        .map(str::trim)
        .filter(|field| !field.is_empty())?;
    let field_key = raw_field
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '_')
        .collect::<String>();
    if field_key.is_empty() || field_key.len() > 255 || field_key.chars().any(char::is_control) {
        return None;
    }
    let field_name = match field_key.as_str() {
        "duedate" => "Due date".to_owned(),
        "fixversions" => "Fix version".to_owned(),
        "worklog" => "Worklog".to_owned(),
        _ => bounded_display(raw_field)?,
    };
    let old = changelog_value(item.from_string.as_deref())?;
    let new = changelog_value(item.to_string.as_deref())?;
    Some((field_key, field_name, old, new))
}

fn changelog_value(value: Option<&str>) -> Option<ChangeValue> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Some(ChangeValue::Text(bounded_display(value)?)),
        None => Some(ChangeValue::Empty),
    }
}

fn bounded_display(value: &str) -> Option<String> {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_control() {
            if !output.ends_with(' ') {
                output.push(' ');
            }
        } else {
            output.push(character);
        }
        if output.chars().count() >= 255 {
            break;
        }
    }
    let output = output.trim().to_owned();
    (!output.is_empty()).then_some(output)
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
    let id = crate::event_identity::snapshot_event_id(
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

#[cfg(test)]
#[path = "issue_diff_tests.rs"]
mod tests;
