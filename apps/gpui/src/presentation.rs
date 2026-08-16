//! Maps framework-independent domain objects into display-ready data.
//!
//! Keeping this mapping free of GPUI types means a future Tauri presentation
//! adapter can reuse the same decisions without depending on the native UI.

use jira_domain::{
    ChangeValue, Issue, IssueDetail, UpdateEvent, UpdateKind, UpdateReadState, User,
};
use time::{Date, OffsetDateTime};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IssueStatusFilter {
    #[default]
    All,
    ToDo,
    InProgress,
    Done,
    Uncategorized,
}

impl IssueStatusFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All statuses",
            Self::ToDo => "To do",
            Self::InProgress => "In progress",
            Self::Done => "Done",
            Self::Uncategorized => "Uncategorized",
        }
    }

    pub fn matches(self, category: &str) -> bool {
        let category = category.trim().to_ascii_lowercase();
        match self {
            Self::All => true,
            Self::ToDo => category == "to do",
            Self::InProgress => category == "in progress",
            Self::Done => category == "done",
            Self::Uncategorized => category.is_empty(),
        }
    }
}

pub fn issue_views_for_filter(
    issues: &[Issue],
    users: &[User],
    filter: IssueStatusFilter,
) -> Vec<IssueViewModel> {
    issues
        .iter()
        .filter(|issue| filter.matches(issue.status.category.as_deref().unwrap_or_default()))
        .map(|issue| IssueViewModel::from_domain(issue, users))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueViewModel {
    pub id: jira_domain::IssueId,
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
            id: issue.id.clone(),
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
pub struct IssueDetailViewModel {
    pub description: String,
    pub comments: Vec<CommentViewModel>,
    pub attachments: Vec<AttachmentViewModel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentViewModel {
    pub author: String,
    pub body: String,
    pub created: String,
    pub updated: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentViewModel {
    pub filename: String,
    pub mime_type: String,
    pub size: String,
}

impl IssueDetailViewModel {
    pub fn from_domain(detail: &IssueDetail) -> Self {
        let comments = detail
            .comments
            .iter()
            .map(|comment| CommentViewModel {
                author: comment
                    .author
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "Unknown author".to_owned()),
                body: comment.body.clone(),
                created: format_timestamp(comment.created_at),
                updated: comment.updated_at.map(format_timestamp),
            })
            .collect();
        let attachments = detail
            .core
            .attachments
            .iter()
            .map(|attachment| AttachmentViewModel {
                filename: attachment.filename.clone(),
                mime_type: attachment
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "Unknown type".to_owned()),
                size: format_bytes(attachment.size_bytes),
            })
            .collect();

        Self {
            description: detail
                .core
                .issue
                .description_text
                .clone()
                .unwrap_or_else(|| "No description supplied.".to_owned()),
            comments,
            attachments,
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

fn format_bytes(value: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value_f64 = value as f64;
    if value_f64 >= GIB {
        format!("{:.1} GiB", value_f64 / GIB)
    } else if value_f64 >= MIB {
        format!("{:.1} MiB", value_f64 / MIB)
    } else if value_f64 >= KIB {
        format!("{:.1} KiB", value_f64 / KIB)
    } else {
        format!("{value} B")
    }
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
    use jira_domain::{AttachmentMetadata, IssueComment, IssueDetail, IssueDetailCore};
    use time::macros::datetime;

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

    #[test]
    fn status_filters_match_categories_case_insensitively_and_keep_uncategorized_separate() {
        assert!(IssueStatusFilter::All.matches("anything"));
        assert!(IssueStatusFilter::ToDo.matches("TO DO"));
        assert!(IssueStatusFilter::InProgress.matches("in progress"));
        assert!(IssueStatusFilter::Done.matches("Done"));
        assert!(IssueStatusFilter::Uncategorized.matches(""));
        assert!(!IssueStatusFilter::Uncategorized.matches("In Review"));
        assert!(IssueStatusFilter::Uncategorized.matches("  "));
    }

    #[test]
    fn filters_loaded_domain_issues_without_changing_their_display_mapping() {
        let issues = sample_issues();
        let users = sample_users();

        let views = issue_views_for_filter(&issues, &users, IssueStatusFilter::Done);

        assert_eq!(views.len(), 1);
        assert_eq!(views[0].key, "DESK-163");
        assert_eq!(views[0].assignee, "Devon Park");
    }

    #[test]
    fn maps_issue_detail_comments_and_attachment_metadata_for_display() {
        let issue = sample_issues().into_iter().next().expect("sample issue");
        let detail = IssueDetail::new(
            IssueDetailCore::new(
                issue,
                vec![
                    AttachmentMetadata::new("attachment-1", "report.txt", 1024, Some("text/plain"))
                        .expect("attachment"),
                ],
            )
            .expect("detail core"),
            vec![
                IssueComment::new(
                    "comment-1",
                    Some(jira_domain::AccountId::new("account-1").expect("account")),
                    "A comment body",
                    datetime!(2026-01-03 00:00 UTC),
                    None,
                    Vec::new(),
                )
                .expect("comment"),
            ],
        )
        .expect("detail");

        let view = IssueDetailViewModel::from_domain(&detail);

        assert_eq!(view.comments[0].author, "account-1");
        assert_eq!(view.comments[0].body, "A comment body");
        assert_eq!(view.attachments[0].filename, "report.txt");
        assert_eq!(view.attachments[0].mime_type, "text/plain");
        assert_eq!(view.attachments[0].size, "1.0 KiB");
    }
}
