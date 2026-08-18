//! Maps framework-independent domain objects into display-ready data.
//!
//! Keeping this mapping free of GPUI types means a future Tauri presentation
//! adapter can reuse the same decisions without depending on the native UI.

use std::collections::HashMap;

use jira_domain::{
    ChangeValue, Issue, IssueCommentAuthor, IssueDetail, IssueKey, RichTextDocument, UpdateEvent,
    UpdateKind, UpdateReadState, User,
};
use time::{Date, OffsetDateTime};

/// A compact, presentation-only selection of Jira status categories.
///
/// The empty selection means all statuses. Non-empty selections are ORed, which makes this type
/// directly usable as the value of a multi-select Combobox. Bit masks also make duplicate values
/// harmless and preserve a deterministic category order for trigger rendering.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IssueStatusSelection(u8);

/// Compatibility name retained while callers migrate from the former single-value contract.
pub type IssueStatusFilter = IssueStatusSelection;

#[allow(non_upper_case_globals)]
impl IssueStatusSelection {
    const TO_DO_MASK: u8 = 1 << 0;
    const IN_PROGRESS_MASK: u8 = 1 << 1;
    const DONE_MASK: u8 = 1 << 2;
    const UNCATEGORIZED_MASK: u8 = 1 << 3;
    const KNOWN_MASK: u8 =
        Self::TO_DO_MASK | Self::IN_PROGRESS_MASK | Self::DONE_MASK | Self::UNCATEGORIZED_MASK;

    /// Empty selection: no status restriction.
    pub const All: Self = Self(0);
    pub const ToDo: Self = Self(Self::TO_DO_MASK);
    pub const InProgress: Self = Self(Self::IN_PROGRESS_MASK);
    pub const Done: Self = Self(Self::DONE_MASK);
    pub const Uncategorized: Self = Self(Self::UNCATEGORIZED_MASK);

    /// Combines Combobox values into one normalized selection.
    pub fn from_values(values: impl IntoIterator<Item = Self>) -> Self {
        let mask = values.into_iter().fold(0, |mask, value| mask | value.0) & Self::KNOWN_MASK;
        Self(mask)
    }

    /// Returns selected singleton values in stable presentation order.
    pub fn values(self) -> Vec<Self> {
        [
            Self::ToDo,
            Self::InProgress,
            Self::Done,
            Self::Uncategorized,
        ]
        .into_iter()
        .filter(|value| self.0 & value.0 != 0)
        .collect()
    }

    pub fn is_all(self) -> bool {
        self.0 == 0
    }

    pub fn label(self) -> &'static str {
        match self.0 {
            0 => "All statuses",
            Self::TO_DO_MASK => "To do",
            Self::IN_PROGRESS_MASK => "In progress",
            Self::DONE_MASK => "Done",
            Self::UNCATEGORIZED_MASK => "Uncategorized",
            _ => "Multiple statuses",
        }
    }

    pub fn matches(self, category: &str) -> bool {
        if self.is_all() {
            return true;
        }
        let category = category.trim().to_ascii_lowercase();
        let value = match category.as_str() {
            "to do" => Self::ToDo,
            "in progress" => Self::InProgress,
            "done" => Self::Done,
            "" => Self::Uncategorized,
            _ => return false,
        };
        self.0 & value.0 != 0
    }
}

pub fn issue_views_for_filter(
    issues: &[Issue],
    users: &[User],
    filter: IssueStatusFilter,
    search: &str,
) -> Vec<IssueViewModel> {
    let filter = IssueStatusSelection::from_values(filter.values());
    let search = search.trim().to_ascii_lowercase();
    let mut identities = IdentityDirectory::from_users(users);
    for issue in issues {
        identities.include_issue(issue);
    }
    issues
        .iter()
        .filter(|issue| filter.matches(issue.status.category.as_deref().unwrap_or_default()))
        .filter(|issue| {
            search.is_empty()
                || issue.key.to_string().to_ascii_lowercase().contains(&search)
                || issue.summary.to_ascii_lowercase().contains(&search)
        })
        .map(|issue| IssueViewModel::from_domain_with_directory(issue, &identities))
        .collect()
}

/// Display-only directory for stable Jira identities.
///
/// Account IDs remain the domain identity and are never suitable UI labels. The directory starts
/// with the authenticated/user-search catalog, then fills missing entries from display metadata
/// carried by issue and comment payloads. Catalog entries win when the same account is present in
/// more than one source; embedded metadata still makes otherwise unknown users useful.
#[derive(Clone, Debug, Default)]
pub struct IdentityDirectory {
    names: HashMap<jira_domain::AccountId, String>,
}

impl IdentityDirectory {
    pub fn from_users(users: &[User]) -> Self {
        let mut directory = Self::default();
        for user in users {
            directory.insert(user.account_id.clone(), &user.display_name);
        }
        directory
    }

    pub fn include_issue(&mut self, issue: &Issue) {
        if let (Some(account_id), Some(display_name)) = (
            issue.assignee.as_ref(),
            issue.assignee_display_name.as_deref(),
        ) {
            self.insert_if_missing(account_id.clone(), display_name);
        }
        if let (Some(account_id), Some(display_name)) = (
            issue.reporter.as_ref(),
            issue.reporter_display_name.as_deref(),
        ) {
            self.insert_if_missing(account_id.clone(), display_name);
        }
    }

    pub fn include_comment_author(&mut self, author: Option<&IssueCommentAuthor>) {
        let Some(author) = author else {
            return;
        };
        if let Some(display_name) = author.display_name.as_deref() {
            self.insert_if_missing(author.account_id.clone(), display_name);
        }
    }

    fn insert(&mut self, account_id: jira_domain::AccountId, display_name: &str) {
        let display_name = display_name.trim();
        if !display_name.is_empty()
            && display_name != account_id.as_str().trim()
            && display_name.len() <= 255
        {
            self.names
                .entry(account_id)
                .or_insert_with(|| display_name.to_owned());
        }
    }

    fn insert_if_missing(&mut self, account_id: jira_domain::AccountId, display_name: &str) {
        self.insert(account_id, display_name);
    }

    pub fn display(&self, account_id: Option<&jira_domain::AccountId>, unassigned: &str) -> String {
        self.display_with_unknown(account_id, unassigned, "Unknown user")
    }

    fn display_with_unknown(
        &self,
        account_id: Option<&jira_domain::AccountId>,
        unassigned: &str,
        unknown: &str,
    ) -> String {
        let Some(account_id) = account_id else {
            return unassigned.to_owned();
        };
        self.names
            .get(account_id)
            .cloned()
            .unwrap_or_else(|| unknown.to_owned())
    }

    fn display_author(&self, author: Option<&IssueCommentAuthor>) -> String {
        author
            .map(|author| {
                self.display_with_unknown(
                    Some(&author.account_id),
                    "Unknown author",
                    "Unknown author",
                )
            })
            .unwrap_or_else(|| "Unknown author".to_owned())
    }
}

/// Normalize a user-entered exact-key lookup without allowing arbitrary text
/// to become a remote request. The transport lookup is intentionally owned by
/// the live workspace once its application locator contract is available.
pub fn normalized_issue_key(query: &str) -> Option<IssueKey> {
    let normalized = query.trim().to_ascii_uppercase();
    (!normalized.is_empty())
        .then(|| IssueKey::new(normalized).ok())
        .flatten()
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
    pub rich_description: Option<RichTextDocument>,
    pub created: String,
    pub updated: String,
    pub due_date: String,
}

impl IssueViewModel {
    pub fn from_domain(issue: &Issue, users: &[User]) -> Self {
        let mut identities = IdentityDirectory::from_users(users);
        identities.include_issue(issue);
        Self::from_domain_with_directory(issue, &identities)
    }

    fn from_domain_with_directory(issue: &Issue, identities: &IdentityDirectory) -> Self {
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
            assignee: identities.display(issue.assignee.as_ref(), "Unassigned"),
            reporter: identities.display(issue.reporter.as_ref(), "Unassigned"),
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
            rich_description: issue.rich_description.clone(),
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
    pub rich_description: Option<RichTextDocument>,
    pub comments: Vec<CommentViewModel>,
    pub attachments: Vec<AttachmentViewModel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentViewModel {
    pub author: String,
    pub body: String,
    pub rich_body: Option<RichTextDocument>,
    pub created: String,
    pub updated: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentViewModel {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub size: String,
}

impl IssueDetailViewModel {
    pub fn from_domain(detail: &IssueDetail, users: &[User]) -> Self {
        let mut identities = IdentityDirectory::from_users(users);
        identities.include_issue(&detail.core.issue);
        for comment in &detail.comments {
            identities.include_comment_author(comment.author.as_ref());
        }
        let comments = detail
            .comments
            .iter()
            .map(|comment| CommentViewModel {
                author: identities.display_author(comment.author.as_ref()),
                body: comment.body.clone(),
                rich_body: comment.rich_body.clone(),
                created: format_timestamp(comment.created_at),
                updated: comment.updated_at.map(format_timestamp),
            })
            .collect();
        let attachments = detail
            .core
            .attachments
            .iter()
            .map(|attachment| AttachmentViewModel {
                id: attachment.id.clone(),
                filename: attachment.filename.clone(),
                mime_type: attachment
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "Unknown type".to_owned()),
                size_bytes: attachment.size_bytes,
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
            rich_description: detail.core.issue.rich_description.clone(),
            comments,
            attachments,
        }
    }
}

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
    fn from_domain_with_directory(
        event: &UpdateEvent,
        issue: Option<&Issue>,
        identities: &IdentityDirectory,
    ) -> Self {
        Self {
            event_id: event.id.clone(),
            issue_id: event.issue_id.clone(),
            issue_key: event.issue_key.to_string(),
            issue_summary: issue
                .map(|issue| issue.summary.clone())
                .unwrap_or_else(|| "Issue no longer in this view".to_owned()),
            change: describe_change(&event.kind, identities),
            occurred_at: format_timestamp(event.occurred_at),
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
pub fn update_groups_for_events(
    events: &[UpdateEvent],
    issues: &[Issue],
    users: &[User],
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
        let update = UpdateViewModel::from_domain_with_directory(event, issue, &identities);
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
                group.latest_occurred_at = format_timestamp(event.occurred_at);
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
    use jira_domain::{
        AttachmentMetadata, IssueComment, IssueCommentAuthor, IssueDetail, IssueDetailCore,
    };
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
    fn never_renders_an_unknown_account_id_as_a_display_label() {
        let mut issues = sample_issues();
        issues[0].assignee = Some(
            jira_domain::AccountId::new("unknown-account").expect("test account ID must be valid"),
        );
        let view = IssueViewModel::from_domain(&issues[0], &[]);

        assert_eq!(view.assignee, "Unknown user");
        assert!(!view.assignee.contains("unknown-account"));
    }

    #[test]
    fn prefers_issue_embedded_display_names_for_assignee_and_reporter() {
        let mut issue = sample_issues().into_iter().next().expect("sample issue");
        issue.assignee_display_name = Some("Asha Patel".to_owned());
        issue.reporter_display_name = Some("Nina Smith".to_owned());

        let view = IssueViewModel::from_domain(&issue, &[]);

        assert_eq!(view.assignee, "Asha Patel");
        assert_eq!(view.reporter, "Nina Smith");
        assert!(!view.reporter.contains("marco"));
    }

    #[test]
    fn rejects_embedded_account_ids_as_display_names() {
        let mut issue = sample_issues().into_iter().next().expect("sample issue");
        let assignee = issue.assignee.clone().expect("sample assignee");
        let reporter = issue.reporter.clone().expect("sample reporter");
        issue.assignee_display_name = Some(format!("  {assignee}  "));
        issue.reporter_display_name = Some(reporter.to_string());

        let view = IssueViewModel::from_domain(&issue, &[]);

        assert_eq!(view.assignee, "Unknown user");
        assert_eq!(view.reporter, "Unknown user");
        assert!(!view.assignee.contains(assignee.as_str()));
        assert!(!view.reporter.contains(reporter.as_str()));
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
    fn status_selection_empty_means_all() {
        let selection = IssueStatusSelection::from_values([]);

        assert_eq!(selection, IssueStatusSelection::All);
        assert!(selection.matches("To Do"));
        assert!(selection.matches("In Progress"));
        assert!(selection.matches("Done"));
        assert!(selection.matches(""));
    }

    #[test]
    fn status_selection_matches_one_category() {
        let selection = IssueStatusSelection::from_values([IssueStatusSelection::Done]);

        assert!(selection.matches("done"));
        assert!(!selection.matches("to do"));
        assert_eq!(selection.values(), vec![IssueStatusSelection::Done]);
        assert_eq!(selection.label(), "Done");
    }

    #[test]
    fn status_selection_ors_multiple_categories_and_normalizes_duplicates() {
        let selection = IssueStatusSelection::from_values([
            IssueStatusSelection::Done,
            IssueStatusSelection::ToDo,
            IssueStatusSelection::Done,
        ]);

        assert!(selection.matches("Done"));
        assert!(selection.matches("To Do"));
        assert!(!selection.matches("In Progress"));
        assert_eq!(
            selection.values(),
            vec![IssueStatusSelection::ToDo, IssueStatusSelection::Done]
        );
        assert_eq!(selection.label(), "Multiple statuses");
    }

    #[test]
    fn status_selection_keeps_uncategorized_explicit() {
        let selection = IssueStatusSelection::from_values([IssueStatusSelection::Uncategorized]);

        assert!(selection.matches(""));
        assert!(selection.matches("  "));
        assert!(!selection.matches("Done"));
        assert_eq!(selection.label(), "Uncategorized");
    }

    #[test]
    fn filters_loaded_domain_issues_without_changing_their_display_mapping() {
        let issues = sample_issues();
        let users = sample_users();

        let views = issue_views_for_filter(&issues, &users, IssueStatusFilter::Done, "");

        assert_eq!(views.len(), 1);
        assert_eq!(views[0].key, "DESK-163");
        assert_eq!(views[0].assignee, "Devon Park");
    }

    #[test]
    fn searches_issue_key_and_summary_locally_and_composes_with_status() {
        let issues = sample_issues();
        let users = sample_users();

        assert_eq!(
            issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "DESK-184")
                .iter()
                .map(|issue| issue.key.as_str())
                .collect::<Vec<_>>(),
            vec!["DESK-184"]
        );
        assert_eq!(
            issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "desk-18").len(),
            1
        );
        assert_eq!(
            issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "notifications").len(),
            1
        );
        assert_eq!(
            issue_views_for_filter(&issues, &users, IssueStatusFilter::Done, "desk")
                .iter()
                .map(|issue| issue.key.as_str())
                .collect::<Vec<_>>(),
            vec!["DESK-163"]
        );
        assert_eq!(
            issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "   ").len(),
            issues.len()
        );
    }

    #[test]
    fn normalizes_only_strict_issue_keys_for_future_remote_lookup() {
        assert_eq!(
            normalized_issue_key("  ix-123 ")
                .as_ref()
                .map(IssueKey::as_str),
            Some("IX-123")
        );
        assert!(normalized_issue_key("summary text").is_none());
        assert!(normalized_issue_key("IX-").is_none());
        assert!(normalized_issue_key("   ").is_none());
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
                    Some(
                        IssueCommentAuthor::new(
                            jira_domain::AccountId::new("account-1").expect("account"),
                            None::<String>,
                        )
                        .expect("author"),
                    ),
                    "A comment body",
                    datetime!(2026-01-03 00:00 UTC),
                    None,
                    Vec::new(),
                )
                .expect("comment"),
            ],
        )
        .expect("detail");

        let view = IssueDetailViewModel::from_domain(&detail, &[]);

        assert_eq!(view.comments[0].author, "Unknown author");
        assert!(!view.comments[0].author.contains("account-1"));
        assert_eq!(view.comments[0].body, "A comment body");
        assert_eq!(view.attachments[0].filename, "report.txt");
        assert_eq!(view.attachments[0].id, "attachment-1");
        assert_eq!(view.attachments[0].size_bytes, 1024);
        assert_eq!(view.attachments[0].mime_type, "text/plain");
        assert_eq!(view.attachments[0].size, "1.0 KiB");
    }

    #[test]
    fn maps_comment_author_to_authenticated_catalog_display_name() {
        let account = jira_domain::AccountId::new("account-1").expect("account");
        let issue = sample_issues().into_iter().next().expect("sample issue");
        let detail = IssueDetail::new(
            IssueDetailCore::new(issue, Vec::new()).expect("detail core"),
            vec![
                IssueComment::new(
                    "comment-1",
                    Some(IssueCommentAuthor::new(account.clone(), Some("  ")).expect("author")),
                    "A comment body",
                    datetime!(2026-01-03 00:00 UTC),
                    None,
                    Vec::new(),
                )
                .expect("comment"),
            ],
        )
        .expect("detail");
        let user = User::new(
            detail.core.issue.site_id.clone(),
            account,
            "Asha",
            None,
            true,
        );

        let view = IssueDetailViewModel::from_domain(&detail, &[user]);

        assert_eq!(view.comments[0].author, "Asha");
        assert_ne!(view.comments[0].author, "account-1");
    }

    #[test]
    fn maps_comment_embedded_display_name_without_a_user_catalog() {
        let issue = sample_issues().into_iter().next().expect("sample issue");
        let detail = IssueDetail::new(
            IssueDetailCore::new(issue, Vec::new()).expect("detail core"),
            vec![
                IssueComment::new(
                    "comment-1",
                    Some(
                        IssueCommentAuthor::new(
                            jira_domain::AccountId::new("commenter-account").expect("account"),
                            Some("Asha Patel"),
                        )
                        .expect("author"),
                    ),
                    "A comment body",
                    datetime!(2026-01-03 00:00 UTC),
                    None,
                    Vec::new(),
                )
                .expect("comment"),
            ],
        )
        .expect("detail");

        let view = IssueDetailViewModel::from_domain(&detail, &[]);

        assert_eq!(view.comments[0].author, "Asha Patel");
    }

    #[test]
    fn maps_assignee_change_accounts_through_the_identity_directory() {
        let issue = sample_issues().into_iter().next().expect("sample issue");
        let old = jira_domain::AccountId::new("old-account").expect("account");
        let new = jira_domain::AccountId::new("new-account").expect("account");
        let users = vec![
            User::new(issue.site_id.clone(), old.clone(), "Old Name", None, true),
            User::new(issue.site_id.clone(), new.clone(), "New Name", None, true),
        ];
        let event = UpdateEvent::new(
            jira_domain::EventId::new("event-assignee").expect("event"),
            issue.site_id.clone(),
            issue.id.clone(),
            issue.key.clone(),
            UpdateKind::AssigneeChanged {
                old: ChangeValue::Account(old),
                new: ChangeValue::Account(new),
            },
            issue.updated_at,
            Vec::new(),
        );

        let view = &update_groups_for_events(
            std::slice::from_ref(&event),
            std::slice::from_ref(&issue),
            &users,
        )[0]
        .events[0];

        assert_eq!(view.change, "Assignee: Old Name → New Name");
        assert!(!view.change.contains("account"));
    }

    #[test]
    fn maps_comment_added_authors_without_exposing_account_ids() {
        let issue = sample_issues().into_iter().next().expect("sample issue");
        let author = jira_domain::AccountId::new("amina").expect("account");
        let event = UpdateEvent::new(
            jira_domain::EventId::new("event-comment").expect("event"),
            issue.site_id.clone(),
            issue.id.clone(),
            issue.key.clone(),
            UpdateKind::CommentAdded {
                comment_id: "comment-1".to_owned(),
                author: Some(author),
                excerpt: "A useful update".to_owned(),
            },
            issue.updated_at,
            Vec::new(),
        );

        let view = &update_groups_for_events(
            std::slice::from_ref(&event),
            std::slice::from_ref(&issue),
            &sample_users(),
        )[0]
        .events[0];

        assert_eq!(view.change, "Amina Yusuf commented: A useful update");
        assert!(!view.change.contains("amina"));
    }

    #[test]
    fn propagates_structured_issue_and_comment_content_with_plain_text_compatibility() {
        use jira_domain::{RichBlock, RichInline, RichTextDocument};

        let mut issue = sample_issues().into_iter().next().expect("sample issue");
        let rich_description = RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::Text {
                text: "Structured description".to_owned(),
                marks: Vec::new(),
            }])],
            false,
        );
        issue.rich_description = Some(rich_description.clone());
        issue.description_text = Some("Plain description fallback".to_owned());
        let mut comment = IssueComment::new(
            "comment-rich",
            None,
            "Plain comment fallback",
            datetime!(2026-01-03 00:00 UTC),
            None,
            Vec::new(),
        )
        .expect("comment");
        let rich_body = RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::Text {
                text: "Structured comment".to_owned(),
                marks: Vec::new(),
            }])],
            false,
        );
        comment.rich_body = Some(rich_body.clone());
        let detail = IssueDetail::new(
            IssueDetailCore::new(issue.clone(), Vec::new()).expect("detail core"),
            vec![comment],
        )
        .expect("detail");

        let issue_view = IssueViewModel::from_domain(&issue, &[]);
        let detail_view = IssueDetailViewModel::from_domain(&detail, &[]);

        assert_eq!(issue_view.description, "Plain description fallback");
        assert_eq!(issue_view.rich_description, Some(rich_description.clone()));
        assert_eq!(detail_view.description, "Plain description fallback");
        assert_eq!(detail_view.rich_description, Some(rich_description));
        assert_eq!(detail_view.comments[0].body, "Plain comment fallback");
        assert_eq!(detail_view.comments[0].rich_body, Some(rich_body));
    }

    #[test]
    fn renders_generic_issue_activity_update_without_raw_enum_name() {
        let issue = sample_issues().into_iter().next().expect("sample issue");
        let event = UpdateEvent::new(
            jira_domain::EventId::new("event-1").expect("event"),
            issue.site_id.clone(),
            issue.id.clone(),
            issue.key.clone(),
            UpdateKind::IssueUpdated,
            issue.updated_at,
            Vec::new(),
        );

        let view = &update_groups_for_events(
            std::slice::from_ref(&event),
            std::slice::from_ref(&issue),
            &[],
        )[0]
        .events[0];

        assert_eq!(view.issue_id, issue.id);
        assert_eq!(view.event_id.as_str(), "event-1");
        assert_eq!(view.change, "Issue activity changed");
    }

    fn test_update_event(
        event_id: &str,
        issue: &Issue,
        occurred_at: jira_domain::Timestamp,
    ) -> UpdateEvent {
        UpdateEvent::new(
            jira_domain::EventId::new(event_id).expect("event ID"),
            issue.site_id.clone(),
            issue.id.clone(),
            issue.key.clone(),
            UpdateKind::IssueUpdated,
            occurred_at,
            Vec::new(),
        )
    }

    #[test]
    fn groups_adjacent_events_for_one_issue_into_one_ticket_card() {
        let issues = sample_issues();
        let first = &issues[0];
        let second = &issues[1];
        let events = vec![
            test_update_event("event-a1", first, datetime!(2026-08-16 10:00 UTC)),
            test_update_event("event-a2", first, datetime!(2026-08-16 09:00 UTC)),
            test_update_event("event-b1", second, datetime!(2026-08-16 08:00 UTC)),
        ];

        let groups = update_groups_for_events(&events, &issues, &sample_users());

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].issue_id, first.id);
        assert_eq!(groups[0].issue_key, "DESK-184");
        assert_eq!(groups[0].issue_summary, first.summary);
        assert_eq!(
            groups[0]
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-a1", "event-a2"]
        );
        assert_eq!(groups[0].latest_occurred_at, "Aug 16, 2026 · 10:00 UTC");
    }

    #[test]
    fn groups_non_adjacent_events_without_reordering_groups_or_events() {
        let issues = sample_issues();
        let first = &issues[0];
        let second = &issues[1];
        let events = vec![
            test_update_event("event-a1", first, datetime!(2026-08-16 10:00 UTC)),
            test_update_event("event-b1", second, datetime!(2026-08-16 09:00 UTC)),
            test_update_event("event-a2", first, datetime!(2026-08-16 08:00 UTC)),
        ];

        let groups = update_groups_for_events(&events, &issues, &[]);

        assert_eq!(
            groups
                .iter()
                .map(|group| group.issue_key.as_str())
                .collect::<Vec<_>>(),
            vec!["DESK-184", "DESK-179"]
        );
        assert_eq!(
            groups[0]
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-a1", "event-a2"]
        );
        assert_eq!(
            groups[1]
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-b1"]
        );
    }

    #[test]
    fn groups_expose_all_event_ids_and_aggregate_mixed_read_states() {
        let issues = sample_issues();
        let issue = &issues[0];
        let mut read = test_update_event("event-read", issue, datetime!(2026-08-16 09:00 UTC));
        read.mark_read();
        let events = vec![
            test_update_event("event-unread-1", issue, datetime!(2026-08-16 10:00 UTC)),
            read,
            test_update_event("event-unread-2", issue, datetime!(2026-08-16 08:00 UTC)),
        ];

        let groups = update_groups_for_events(&events, &issues, &[]);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].unread_count, 2);
        assert!(groups[0].unread);
        assert_eq!(
            groups[0]
                .events
                .iter()
                .map(|event| (event.event_id.as_str(), event.unread))
                .collect::<Vec<_>>(),
            vec![
                ("event-unread-1", true),
                ("event-read", false),
                ("event-unread-2", true)
            ]
        );
    }

    #[test]
    fn grouping_missing_issue_uses_safe_event_metadata_fallbacks() {
        let issue_id = jira_domain::IssueId::new("missing-issue").expect("issue ID");
        let site_id = jira_domain::JiraSiteId::new("sample-site").expect("site ID");
        let issue_key = IssueKey::new("DESK-999").expect("issue key");
        let secret_account = jira_domain::AccountId::new("secret-account").expect("account ID");
        let event = UpdateEvent::new(
            jira_domain::EventId::new("event-missing").expect("event ID"),
            site_id,
            issue_id,
            issue_key,
            UpdateKind::AssigneeChanged {
                old: ChangeValue::Account(secret_account.clone()),
                new: ChangeValue::Account(secret_account),
            },
            datetime!(2026-08-16 10:00 UTC),
            Vec::new(),
        );

        let groups = update_groups_for_events(&[event], &[], &[]);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].issue_key, "DESK-999");
        assert_eq!(groups[0].issue_summary, "Issue no longer in this view");
        assert_eq!(
            groups[0].events[0].change,
            "Assignee: Unknown user → Unknown user"
        );
        assert!(!groups[0].events[0].change.contains("secret-account"));
    }
}
