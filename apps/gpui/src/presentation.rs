//! Maps framework-independent domain objects into display-ready data.
//!
//! Keeping this mapping free of GPUI types means a future Tauri presentation
//! adapter can reuse the same decisions without depending on the native UI.

use jira_domain::{ChangeValue, Issue, UpdateEvent, UpdateKind, UpdateReadState, User};
use time::{Date, OffsetDateTime};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueViewModel {
    pub key: String,
    pub summary: String,
    pub project: String,
    pub issue_type: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub assignee: String,
    pub reporter: String,
    pub parent: Option<String>,
    pub labels: Vec<String>,
    pub description: String,
    pub created: String,
    pub updated: String,
    pub due_date: String,
}

impl IssueViewModel {
    pub fn from_domain(issue: &Issue, users: &[User]) -> Self {
        let display_user = |account_id: Option<&jira_domain::AccountId>| {
            account_id
                .and_then(|account_id| {
                    users
                        .iter()
                        .find(|user| &user.account_id == account_id)
                        .map(|user| user.display_name.clone())
                })
                .or_else(|| account_id.map(ToString::to_string))
                .unwrap_or_else(|| "Unassigned".to_owned())
        };

        Self {
            key: issue.key.to_string(),
            summary: issue.summary.clone(),
            project: issue.project.name.clone(),
            issue_type: issue.issue_type.name.clone(),
            status: issue.status.name.clone(),
            status_category: issue
                .status
                .category
                .clone()
                .unwrap_or_else(|| "No category".to_owned()),
            priority: issue
                .priority
                .name
                .clone()
                .unwrap_or_else(|| "None".to_owned()),
            assignee: display_user(issue.assignee.as_ref()),
            reporter: display_user(issue.reporter.as_ref()),
            parent: issue.parent.as_ref().map(|parent| {
                parent.summary.as_ref().map_or_else(
                    || parent.key.to_string(),
                    |summary| format!("{} · {summary}", parent.key),
                )
            }),
            labels: issue.labels.clone(),
            description: issue
                .description_text
                .clone()
                .unwrap_or_else(|| "No description supplied.".to_owned()),
            created: format_timestamp(issue.created_at),
            updated: format_timestamp(issue.updated_at),
            due_date: issue
                .due_date
                .map(format_date)
                .unwrap_or_else(|| "Not set".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateViewModel {
    pub issue_key: String,
    pub issue_summary: String,
    pub change: String,
    pub occurred_at: String,
    pub unread: bool,
}

impl UpdateViewModel {
    pub fn from_domain(event: &UpdateEvent, issue: Option<&Issue>) -> Self {
        Self {
            issue_key: event.issue_key.to_string(),
            issue_summary: issue
                .map(|issue| issue.summary.clone())
                .unwrap_or_else(|| "Issue no longer in this view".to_owned()),
            change: describe_change(&event.kind),
            occurred_at: format_timestamp(event.occurred_at),
            unread: event.read_state == UpdateReadState::Unread,
        }
    }
}

fn describe_change(kind: &UpdateKind) -> String {
    match kind {
        UpdateKind::IssueAddedToView => "Added to this user set".to_owned(),
        UpdateKind::IssueRemovedFromView => "Removed from this user set".to_owned(),
        UpdateKind::StatusChanged { old, new } => change_sentence("Status", old, new),
        UpdateKind::AssigneeChanged { old, new } => change_sentence("Assignee", old, new),
        UpdateKind::PriorityChanged { old, new } => change_sentence("Priority", old, new),
        UpdateKind::DueDateChanged { old, new } => change_sentence("Due date", old, new),
        UpdateKind::SummaryChanged { old, new } => change_sentence("Summary", old, new),
        UpdateKind::ParentChanged { old, new } => change_sentence("Parent", old, new),
        UpdateKind::CommentAdded {
            author, excerpt, ..
        } => {
            let author = author
                .as_ref()
                .map_or("Someone", jira_domain::AccountId::as_str);
            format!("{author} commented: {excerpt}")
        }
    }
}

fn change_sentence(field: &str, old: &ChangeValue, new: &ChangeValue) -> String {
    format!(
        "{field}: {} → {}",
        display_change_value(old),
        display_change_value(new)
    )
}

fn display_change_value(value: &ChangeValue) -> String {
    match value {
        ChangeValue::Text(value) => value.clone(),
        ChangeValue::Account(value) => value.to_string(),
        ChangeValue::Date(value) => value.clone().unwrap_or_else(|| "not set".to_owned()),
        ChangeValue::Parent(value) => value
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "none".to_owned()),
        ChangeValue::Empty => "empty".to_owned(),
    }
}

fn format_timestamp(value: OffsetDateTime) -> String {
    format!(
        "{} {:02}, {} · {:02}:{:02} UTC",
        month_name(value.month() as u8),
        value.day(),
        value.year(),
        value.hour(),
        value.minute()
    )
}

fn format_date(value: Date) -> String {
    format!(
        "{} {:02}, {}",
        month_name(value.month() as u8),
        value.day(),
        value.year()
    )
}

fn month_name(month: u8) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_data::{sample_issues, sample_users};

    #[test]
    fn maps_domain_identity_to_display_name() {
        let issues = sample_issues();
        let users = sample_users();
        let view = IssueViewModel::from_domain(&issues[0], &users);

        assert_eq!(view.key, "DESK-184");
        assert_eq!(view.assignee, "Amina Yusuf");
        assert_eq!(view.project, "Developer Experience");
    }

    #[test]
    fn preserves_unknown_account_ids_instead_of_calling_them_unassigned() {
        let mut issues = sample_issues();
        issues[0].assignee = Some(
            jira_domain::AccountId::new("unknown-account").expect("test account ID must be valid"),
        );
        let view = IssueViewModel::from_domain(&issues[0], &[]);

        assert_eq!(view.assignee, "unknown-account");
    }
}
