use std::collections::HashMap;

use jira_domain::{ChangeValue, Issue, UpdateEvent, UpdateKind, UpdateReadState, User};

use super::{format, identity::IdentityDirectory};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateViewModel {
    pub event_id: jira_domain::EventId,
    pub issue_id: jira_domain::IssueId,
    pub issue_key: String,
    pub issue_summary: String,
    pub change: String,
    pub occurred_at: String,
    pub unread: bool,
}

impl UpdateViewModel {
    fn from_domain_with_directory_and_offset(
        event: &UpdateEvent,
        issue: Option<&Issue>,
        identities: &IdentityDirectory,
        offset: Option<time::UtcOffset>,
    ) -> Self {
        Self {
            event_id: event.id.clone(),
            issue_id: event.issue_id.clone(),
            issue_key: event.issue_key.to_string(),
            issue_summary: issue
                .map(|issue| issue.summary.clone())
                .unwrap_or_else(|| "Issue no longer in this view".to_owned()),
            change: describe_change(&event.kind, identities),
            occurred_at: format::format_timestamp_for(event.occurred_at, offset),
            unread: event.read_state == UpdateReadState::Unread,
        }
    }
}

/// Presentation model for one issue's activity in the local update feed.
///
/// The domain feed is newest-first. Grouping retains the first appearance order of issues and
/// the input order of events inside each issue, so the UI can render a stable ticket card without
/// changing the feed's chronology.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateGroupViewModel {
    pub issue_id: jira_domain::IssueId,
    pub issue_key: String,
    pub issue_summary: String,
    pub events: Vec<UpdateViewModel>,
    pub latest_occurred_at: String,
    pub unread_count: usize,
    pub unread: bool,
}

/// Groups update events by stable Jira issue ID for presentation.
///
/// Issue metadata is looked up by ID, while the event's key is retained as the safe fallback when
/// the issue is no longer present in the current issue view. Account IDs are resolved through one
/// shared identity directory, matching the existing individual update mapping behavior.
#[allow(dead_code)]
pub fn update_groups_for_events(
    events: &[UpdateEvent],
    issues: &[Issue],
    users: &[User],
) -> Vec<UpdateGroupViewModel> {
    update_groups_for_events_with_offset(events, issues, users, None)
}

/// Groups updates while optionally using a fixed presentation offset for fixture rendering.
pub(crate) fn update_groups_for_events_with_offset(
    events: &[UpdateEvent],
    issues: &[Issue],
    users: &[User],
    offset: Option<time::UtcOffset>,
) -> Vec<UpdateGroupViewModel> {
    let mut identities = IdentityDirectory::from_users(users);
    let issues_by_id: HashMap<jira_domain::IssueId, &Issue> = issues
        .iter()
        .map(|issue| {
            identities.include_issue(issue);
            (issue.id.clone(), issue)
        })
        .collect();
    let mut groups: Vec<UpdateGroupViewModel> = Vec::new();
    let mut group_indexes: HashMap<jira_domain::IssueId, usize> = HashMap::new();
    let mut latest_times: HashMap<jira_domain::IssueId, jira_domain::Timestamp> = HashMap::new();

    for event in events {
        let issue = issues_by_id.get(&event.issue_id).copied();
        let update = UpdateViewModel::from_domain_with_directory_and_offset(
            event,
            issue,
            &identities,
            offset,
        );
        let event_id = event.issue_id.clone();
        if let Some(&group_index) = group_indexes.get(&event_id) {
            let group = &mut groups[group_index];
            group.unread_count += usize::from(update.unread);
            group.unread |= update.unread;
            group.events.push(update);
            if latest_times
                .get(&event_id)
                .is_none_or(|latest| event.occurred_at > *latest)
            {
                latest_times.insert(event_id, event.occurred_at);
                group.latest_occurred_at = format::format_timestamp_for(event.occurred_at, offset);
            }
        } else {
            let group_index = groups.len();
            group_indexes.insert(event_id.clone(), group_index);
            latest_times.insert(event_id, event.occurred_at);
            groups.push(UpdateGroupViewModel {
                issue_id: update.issue_id.clone(),
                issue_key: update.issue_key.clone(),
                issue_summary: update.issue_summary.clone(),
                latest_occurred_at: update.occurred_at.clone(),
                unread_count: usize::from(update.unread),
                unread: update.unread,
                events: vec![update],
            });
        }
    }

    groups
}

fn describe_change(kind: &UpdateKind, identities: &IdentityDirectory) -> String {
    match kind {
        UpdateKind::IssueAddedToView => "Added to this user set".to_owned(),
        UpdateKind::IssueRemovedFromView => "Removed from this user set".to_owned(),
        UpdateKind::IssueUpdated => "Issue activity changed".to_owned(),
        UpdateKind::FieldChanged { field, old, new } => {
            change_sentence(field, old, new, identities)
        }
        UpdateKind::StatusChanged { old, new } => change_sentence("Status", old, new, identities),
        UpdateKind::AssigneeChanged { old, new } => {
            change_sentence("Assignee", old, new, identities)
        }
        UpdateKind::PriorityChanged { old, new } => {
            change_sentence("Priority", old, new, identities)
        }
        UpdateKind::DueDateChanged { old, new } => {
            change_sentence("Due date", old, new, identities)
        }
        UpdateKind::SummaryChanged { old, new } => change_sentence("Summary", old, new, identities),
        UpdateKind::ParentChanged { old, new } => change_sentence("Parent", old, new, identities),
        UpdateKind::CommentAdded {
            author, excerpt, ..
        } => {
            let author = identities.display(author.as_ref(), "Unknown author");
            format!("{author} commented: {excerpt}")
        }
    }
}

pub(crate) fn update_group_event_ids(group: &UpdateGroupViewModel) -> Vec<jira_domain::EventId> {
    group
        .events
        .iter()
        .map(|event| event.event_id.clone())
        .collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum UpdateFilter {
    #[default]
    All,
    Unread,
}

pub(crate) fn filtered_update_group_indices(
    groups: &[UpdateGroupViewModel],
    filter: UpdateFilter,
) -> Vec<usize> {
    groups
        .iter()
        .enumerate()
        .filter(|(_, group)| filter == UpdateFilter::All || group.unread)
        .map(|(index, _)| index)
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompactedUpdateRow {
    Event(UpdateViewModel),
    GenericSummary { count: usize, occurred_at: String },
}

pub(crate) fn compact_update_rows(events: &[UpdateViewModel]) -> Vec<CompactedUpdateRow> {
    let generic_count = events
        .iter()
        .filter(|event| event.change == "Issue activity changed")
        .count();
    let mut summary_inserted = false;
    events
        .iter()
        .filter_map(|event| {
            if event.change == "Issue activity changed" {
                if summary_inserted {
                    None
                } else {
                    summary_inserted = true;
                    Some(CompactedUpdateRow::GenericSummary {
                        count: generic_count,
                        occurred_at: event.occurred_at.clone(),
                    })
                }
            } else {
                Some(CompactedUpdateRow::Event(event.clone()))
            }
        })
        .collect()
}

pub(crate) fn generic_summary_label(count: usize) -> String {
    if count == 1 {
        "Other Jira activity · exact field not available from sync".to_owned()
    } else {
        format!("Other Jira activity · {count} events · exact field not available from sync")
    }
}

pub(crate) const UPDATE_PREVIEW_LIMIT: usize = 3;

pub(crate) fn visible_update_row_count(row_count: usize, expanded: bool) -> usize {
    if expanded {
        row_count
    } else {
        row_count.min(UPDATE_PREVIEW_LIMIT)
    }
}

pub(crate) fn hidden_update_row_count(row_count: usize, expanded: bool) -> usize {
    row_count.saturating_sub(visible_update_row_count(row_count, expanded))
}

pub(crate) fn describe_update_with_directory(
    event: &UpdateEvent,
    identities: &IdentityDirectory,
) -> String {
    describe_change(&event.kind, identities)
}

fn change_sentence(
    field: &str,
    old: &ChangeValue,
    new: &ChangeValue,
    identities: &IdentityDirectory,
) -> String {
    format!(
        "{field}: {} → {}",
        display_change_value(old, identities),
        display_change_value(new, identities)
    )
}

fn display_change_value(value: &ChangeValue, identities: &IdentityDirectory) -> String {
    match value {
        ChangeValue::Text(value) => value.clone(),
        ChangeValue::Account(value) => identities.display(Some(value), "Unknown user"),
        ChangeValue::Date(value) => value.clone().unwrap_or_else(|| "not set".to_owned()),
        ChangeValue::Parent(value) => value
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "none".to_owned()),
        ChangeValue::Empty => "empty".to_owned(),
    }
}
