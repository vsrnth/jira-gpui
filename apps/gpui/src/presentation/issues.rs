use jira_domain::{Issue, IssueDetail, IssueKey, RichTextDocument, User};

use super::{format, identity::IdentityDirectory};

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
            created: format::format_timestamp(issue.created_at),
            updated: format::format_timestamp(issue.updated_at),
            due_date: issue
                .due_date
                .map(format::format_date)
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
                created: format::format_timestamp(comment.created_at),
                updated: comment.updated_at.map(format::format_timestamp),
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
                size: format::format_bytes(attachment.size_bytes),
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
