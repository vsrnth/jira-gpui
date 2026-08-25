use crate::adf::{adf_comment_text, visible_adf, visible_adf_with_attachments};
use crate::models::{
    EnhancedSearchPage, JiraAttachment, JiraBulkChangelogResponse, JiraComment, JiraCommentPage,
    JiraIssue, JiraUser,
};
use jira_application::{
    IssueChangelog, IssueChangelogHistory, IssueChangelogItem, IssueChangelogPage,
    IssueCommentsPage, PageCursor,
};
use jira_domain::{
    AccountId, AttachmentMetadata, Issue, IssueComment, IssueCommentAuthor, IssueDetailCore,
    IssueId, IssueKey, IssueType, JiraSiteId, ParentIssue, Priority, Project, Status, Timestamp,
    User,
};
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};

/// Converts Jira transport values to portable records. These records have no HTTP, UI, or
/// persistence concerns and are a deliberately narrow adapter seam for a future Tauri UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct IssueMapper;

impl IssueMapper {
    pub fn map_domain_issue_detail(
        &self,
        site_id: JiraSiteId,
        issue: JiraIssue,
    ) -> Result<IssueDetailCore, MappingError> {
        let attachments = issue
            .fields
            .attachment
            .iter()
            .map(map_attachment)
            .collect::<Result<Vec<_>, _>>()?;
        let (rich_description, description) = issue
            .fields
            .description
            .as_ref()
            .and_then(|value| visible_adf_with_attachments(value, &attachments))
            .map_or((None, None), |(document, text)| {
                (Some(document), Some(text))
            });
        let mut domain_issue = self.map_domain_issue(site_id, issue)?;
        domain_issue.description_text = description;
        domain_issue.rich_description = rich_description;
        Ok(IssueDetailCore::new(domain_issue, attachments))
    }

    pub fn map_comment_page(
        &self,
        page: JiraCommentPage,
    ) -> Result<IssueCommentsPage, MappingError> {
        let start_at = page.start_at;
        let count = page.comments.len();
        let comments = page
            .comments
            .into_iter()
            .map(|comment| self.map_comment(comment))
            .collect::<Result<Vec<_>, _>>()?;
        let next_start_at = page
            .total
            .filter(|total| count > 0 && start_at.saturating_add(count) < *total)
            .map(|_| start_at.saturating_add(count));
        Ok(IssueCommentsPage {
            comments,
            start_at,
            next_start_at,
            next_cursor: None::<PageCursor>,
            total: page.total,
        })
    }

    /// Maps one comment returned by Jira after a successful comment creation.
    /// The public method keeps HTTP response mapping at the adapter boundary while allowing
    /// callers to preserve the transport-neutral domain comment.
    pub fn map_comment(&self, comment: JiraComment) -> Result<IssueComment, MappingError> {
        map_comment(comment)
    }

    /// Maps an enhanced-search page to the framework-independent domain model.
    ///
    /// The site ID is supplied by the authenticated connection; Jira issue search responses do
    /// not repeat it. This function performs no network traffic and cannot modify Jira.
    pub fn map_domain_page(
        &self,
        site_id: JiraSiteId,
        page: EnhancedSearchPage,
    ) -> Result<DomainIssuePage, MappingError> {
        let issues = page
            .issues
            .into_iter()
            .map(|issue| self.map_domain_issue(site_id.clone(), issue))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DomainIssuePage {
            issues,
            next_page_token: page.next_page_token,
            is_last: page.is_last,
        })
    }

    pub fn map_changelog_page(
        &self,
        response: JiraBulkChangelogResponse,
    ) -> Result<IssueChangelogPage, MappingError> {
        let changelogs = response
            .issue_change_logs
            .into_iter()
            .map(|log| {
                let issue_id = IssueId::new(bounded_changelog_text(
                    log.issue_id,
                    255,
                    "changelog issue ID",
                    false,
                )?)
                .map_err(MappingError::InvalidDomainValue)?;
                let histories = log
                    .change_histories
                    .into_iter()
                    .map(|history| {
                        bounded_changelog_text(history.id, 255, "history ID", false).and_then(
                            |id| {
                                Ok(IssueChangelogHistory {
                                    id,
                                    created: parse_timestamp(history.created)?,
                                    items: history
                                        .items
                                        .into_iter()
                                        .map(map_changelog_item)
                                        .collect::<Result<Vec<_>, _>>()?,
                                })
                            },
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(IssueChangelog {
                    issue_id,
                    histories,
                })
            })
            .collect::<Result<Vec<_>, MappingError>>()?;
        Ok(IssueChangelogPage {
            changelogs,
            next_page_token: response
                .next_page_token
                .map(|token| bounded_changelog_text(token, 255, "changelog page token", false))
                .transpose()?,
        })
    }

    pub fn map_domain_issue(
        &self,
        site_id: JiraSiteId,
        issue: JiraIssue,
    ) -> Result<Issue, MappingError> {
        let id = IssueId::new(issue.id).map_err(MappingError::InvalidDomainValue)?;
        let key = IssueKey::new(issue.key).map_err(MappingError::InvalidDomainValue)?;
        non_empty("issue summary", &issue.fields.summary)?;
        let summary = issue.fields.summary;
        let project = required("project", issue.fields.project)?;
        let issue_type = required("issue type", issue.fields.issuetype)?;
        let status = required("status", issue.fields.status)?;
        let created_at = parse_timestamp(required("created timestamp", issue.fields.created)?)?;
        let updated_at = parse_timestamp(required("updated timestamp", issue.fields.updated)?)?;
        let due_date = issue
            .fields
            .duedate
            .map(|value| parse_date(&value))
            .transpose()?;

        let parent = issue
            .fields
            .parent
            .map(|parent| {
                Ok(ParentIssue {
                    id: IssueId::new(parent.id).map_err(MappingError::InvalidDomainValue)?,
                    key: IssueKey::new(parent.key).map_err(MappingError::InvalidDomainValue)?,
                    summary: parent.fields.and_then(|fields| fields.summary),
                })
            })
            .transpose()?;

        let assignee_user = issue.fields.assignee;
        let reporter_user = issue.fields.reporter;
        let assignee = assignee_user.as_ref().map(domain_account_id).transpose()?;
        let reporter = reporter_user.as_ref().map(domain_account_id).transpose()?;
        let assignee_display_name = assignee_user.as_ref().and_then(valid_display_name);
        let reporter_display_name = reporter_user.as_ref().and_then(valid_display_name);
        let priority = issue
            .fields
            .priority
            .map(|priority| Priority {
                id: priority.id,
                name: Some(priority.name),
                icon_url: priority.icon_url,
            })
            .unwrap_or(Priority {
                id: None,
                name: None,
                icon_url: None,
            });

        let mut domain_issue = Issue::new(
            site_id,
            id,
            key,
            Project {
                id: project.id,
                key: project.key,
                name: project.name,
            },
            IssueType {
                id: issue_type.id.unwrap_or_default(),
                name: issue_type.name,
                icon_url: issue_type.icon_url,
            },
            summary,
            Status {
                id: status.id.unwrap_or_default(),
                name: status.name,
                category: status.status_category.and_then(|category| category.name),
            },
            priority,
            assignee,
            reporter,
            parent,
            issue.fields.labels,
            created_at,
            updated_at,
            due_date,
        );
        domain_issue.assignee_display_name = assignee_display_name;
        domain_issue.reporter_display_name = reporter_display_name;
        domain_issue.resolution_name = issue.fields.resolution.map(|resolution| resolution.name);
        Ok(domain_issue)
    }

    pub fn map_user(&self, site_id: JiraSiteId, user: JiraUser) -> Result<User, MappingError> {
        let account_id = domain_account_id(&user)?;
        Ok(User::new(
            site_id,
            account_id,
            valid_display_name(&user).unwrap_or_else(|| "Unknown user".to_owned()),
            avatar_url(&user.avatar_urls),
            user.active,
        ))
    }
}

fn map_attachment(attachment: &JiraAttachment) -> Result<AttachmentMetadata, MappingError> {
    AttachmentMetadata::new(
        attachment.id.clone(),
        attachment.filename.clone(),
        attachment.size,
        attachment.mime_type.clone(),
    )
    .map_err(MappingError::InvalidDomainValue)
}

fn map_comment(comment: JiraComment) -> Result<IssueComment, MappingError> {
    let (rich_body, body) = match comment.body.as_ref().and_then(visible_adf) {
        Some((document, text)) => (Some(document), text),
        None => (
            None,
            comment
                .body
                .as_ref()
                .and_then(adf_comment_text)
                .ok_or(MappingError::MissingRequiredField("comment body"))?,
        ),
    };
    let author = comment.author.map(map_comment_author).transpose()?;
    let created_at = parse_timestamp(comment.created)?;
    let updated_at = comment.updated.map(parse_timestamp).transpose()?;
    let mut mapped =
        IssueComment::new(comment.id, author, body, created_at, updated_at, Vec::new())
            .map_err(MappingError::InvalidDomainValue)?;
    mapped.rich_body = rich_body;
    Ok(mapped)
}

fn map_comment_author(user: JiraUser) -> Result<IssueCommentAuthor, MappingError> {
    let display_name = valid_display_name(&user);
    let account_id = domain_account_id(&user)?;
    IssueCommentAuthor::new(account_id, display_name).map_err(MappingError::InvalidDomainValue)
}

fn valid_display_name(user: &JiraUser) -> Option<String> {
    let display_name = user.display_name.trim();
    (!display_name.is_empty()
        && display_name.len() <= 255
        && display_name != user.account_id.trim())
    .then_some(display_name.to_owned())
}

fn domain_account_id(user: &JiraUser) -> Result<AccountId, MappingError> {
    AccountId::new(user.account_id.clone()).map_err(MappingError::InvalidDomainValue)
}

fn avatar_url(urls: &std::collections::BTreeMap<String, String>) -> Option<String> {
    ["48x48", "32x32", "24x24", "16x16"]
        .iter()
        .find_map(|size| urls.get(*size).cloned())
        .or_else(|| urls.values().next().cloned())
}

fn required<T>(field: &'static str, value: Option<T>) -> Result<T, MappingError> {
    value.ok_or(MappingError::MissingRequiredField(field))
}

fn parse_timestamp(value: String) -> Result<Timestamp, MappingError> {
    if let Ok(timestamp) = OffsetDateTime::parse(&value, &Rfc3339)
        .or_else(|_| OffsetDateTime::parse(&value, JIRA_OFFSET_TIMESTAMP_FORMAT))
    {
        return Ok(timestamp);
    }
    value
        .parse::<i64>()
        .ok()
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .ok_or(MappingError::InvalidTimestamp(value))
}

fn map_changelog_item(
    item: crate::models::JiraChangeItem,
) -> Result<IssueChangelogItem, MappingError> {
    Ok(IssueChangelogItem {
        field: item
            .field
            .map(|value| bounded_changelog_text(value, 255, "changelog field", false))
            .transpose()?,
        field_id: item
            .field_id
            .map(|value| bounded_changelog_text(value, 255, "changelog field ID", false))
            .transpose()?,
        from_string: item
            .from_string
            .map(|value| bounded_changelog_display(value, 4_096, "changelog old value"))
            .transpose()?,
        to_string: item
            .to_string
            .map(|value| bounded_changelog_display(value, 4_096, "changelog new value"))
            .transpose()?,
    })
}

fn bounded_changelog_display(
    value: String,
    maximum: usize,
    field: &'static str,
) -> Result<String, MappingError> {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_control() {
            if !output.ends_with(' ') {
                output.push(' ');
            }
        } else {
            output.push(character);
        }
        if output.chars().count() >= maximum {
            break;
        }
    }
    if output.trim().is_empty() && !value.trim().is_empty() {
        return Err(MappingError::InvalidChangelogValue(field));
    }
    Ok(output.trim().to_owned())
}

fn bounded_changelog_text(
    value: String,
    maximum: usize,
    field: &'static str,
    allow_empty: bool,
) -> Result<String, MappingError> {
    if (!allow_empty && value.trim().is_empty())
        || value.chars().any(char::is_control)
        || value.chars().count() > maximum
    {
        return Err(MappingError::InvalidChangelogValue(field));
    }
    Ok(value)
}

const JIRA_OFFSET_TIMESTAMP_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond][offset_hour sign:mandatory][offset_minute]"
);

const JIRA_DATE_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");

fn parse_date(value: &str) -> Result<Date, MappingError> {
    Date::parse(value, JIRA_DATE_FORMAT).map_err(|_| MappingError::InvalidDate(value.to_owned()))
}

fn non_empty(field: &'static str, value: &str) -> Result<(), MappingError> {
    if value.trim().is_empty() {
        Err(MappingError::MissingRequiredField(field))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainIssuePage {
    pub issues: Vec<Issue>,
    pub next_page_token: Option<String>,
    pub is_last: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MappingError {
    #[error("Jira response is missing required {0}")]
    MissingRequiredField(&'static str),
    #[error("Jira response contains an invalid domain value: {0}")]
    InvalidDomainValue(#[source] jira_domain::DomainError),
    #[error("Jira response has an invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("Jira response has an invalid due date: {0}")]
    InvalidDate(String),
    #[error("Jira response has an invalid changelog value: {0}")]
    InvalidChangelogValue(&'static str),
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
