use crate::models::{
    EnhancedSearchPage, JiraAttachment, JiraComment, JiraCommentPage, JiraIssue, JiraNamedEntity,
    JiraProject, JiraUser,
};
use jira_application::{IssueCommentsPage, PageCursor};
use jira_domain::{
    AccountId, AttachmentMetadata, Issue, IssueComment, IssueDetailCore, IssueId, IssueKey,
    IssueType, JiraSiteId, ParentIssue, Priority, Project, Status, Timestamp, User,
};
use serde_json::Value;
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
        let description = issue
            .fields
            .description
            .as_ref()
            .and_then(adf_to_plain_text);
        let attachments = issue
            .fields
            .attachment
            .iter()
            .map(map_attachment)
            .collect::<Result<Vec<_>, _>>()?;
        let mut domain_issue = self.map_domain_issue(site_id, issue)?;
        domain_issue.description_text = description;
        IssueDetailCore::new(domain_issue, attachments).map_err(MappingError::InvalidDomainValue)
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
            .map(map_comment)
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

        let assignee = issue.fields.assignee.map(domain_account_id).transpose()?;
        let reporter = issue.fields.reporter.map(domain_account_id).transpose()?;
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
        domain_issue.resolution_name = issue.fields.resolution.map(|resolution| resolution.name);
        Ok(domain_issue)
    }

    pub fn map_user(&self, site_id: JiraSiteId, user: JiraUser) -> Result<User, MappingError> {
        let account_id = domain_account_id(user.clone())?;
        Ok(User::new(
            site_id,
            account_id,
            user.display_name,
            avatar_url(&user.avatar_urls),
            user.active,
        ))
    }

    pub fn map_page(&self, page: EnhancedSearchPage) -> Result<RemoteIssuePage, MappingError> {
        let issues = page
            .issues
            .into_iter()
            .map(|issue| self.map_issue(issue))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RemoteIssuePage {
            issues,
            next_page_token: page.next_page_token,
            is_last: page.is_last,
        })
    }

    pub fn map_issue(&self, issue: JiraIssue) -> Result<RemoteIssue, MappingError> {
        non_empty("issue id", &issue.id)?;
        non_empty("issue key", &issue.key)?;
        non_empty("issue summary", &issue.fields.summary)?;

        let parent = issue.fields.parent.map(|parent| RemoteIssueReference {
            id: parent.id,
            key: parent.key,
            summary: parent.fields.and_then(|fields| fields.summary),
        });

        Ok(RemoteIssue {
            id: issue.id,
            key: issue.key,
            summary: issue.fields.summary,
            issue_type: issue.fields.issuetype.map(map_named_entity),
            project: issue.fields.project.map(map_project),
            status: issue.fields.status.map(map_named_entity),
            priority: issue.fields.priority.map(map_named_entity),
            assignee: issue.fields.assignee.map(map_user),
            parent,
            labels: issue.fields.labels,
            created: issue.fields.created,
            updated: issue.fields.updated,
            due_date: issue.fields.duedate,
            resolution: issue.fields.resolution.map(map_named_entity),
        })
    }
}

const MAX_ADF_TEXT: usize = 1_000_000;

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
    let body = comment
        .body
        .as_ref()
        .and_then(adf_comment_text)
        .ok_or(MappingError::MissingRequiredField("comment body"))?;
    let author = comment.author.map(domain_account_id).transpose()?;
    let created_at = parse_timestamp(comment.created)?;
    let updated_at = comment.updated.map(parse_timestamp).transpose()?;
    IssueComment::new(comment.id, author, body, created_at, updated_at, Vec::new())
        .map_err(MappingError::InvalidDomainValue)
}

/// Extracts only user-visible ADF text, preserving block boundaries while ignoring links,
/// mentions, embedded media, and all other raw JSON. The output is bounded before it reaches
/// the domain/cache boundary.
pub fn adf_to_plain_text(value: &Value) -> Option<String> {
    if !value.is_object() {
        return None;
    }
    let mut output = String::new();
    append_adf_text(value, &mut output);
    let mut normalized = String::with_capacity(output.len());
    for character in output.chars() {
        if character == '\n' && normalized.ends_with('\n') {
            continue;
        }
        normalized.push(character);
    }
    let trimmed = normalized.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn adf_comment_text(value: &Value) -> Option<String> {
    adf_to_plain_text(value).or_else(|| {
        let object = value.as_object()?;
        let content = object.get("content")?.as_array()?;
        (!content.is_empty()).then(|| "[unsupported Jira content]".to_owned())
    })
}

fn append_adf_text(value: &Value, output: &mut String) {
    if output.len() >= MAX_ADF_TEXT {
        return;
    }
    let Some(object) = value.as_object() else {
        return;
    };
    match object.get("type").and_then(Value::as_str) {
        Some("text") => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                append_limited(output, text);
            }
        }
        Some("hardBreak") => append_limited(output, "\n"),
        Some("mention") => {
            if let Some(attrs) = object.get("attrs").and_then(Value::as_object) {
                for key in ["displayText", "displayName", "text"] {
                    if let Some(text) = attrs.get(key).and_then(Value::as_str) {
                        append_limited(output, text);
                        break;
                    }
                }
            }
        }
        _ => {
            if let Some(content) = object.get("content").and_then(Value::as_array) {
                for child in content {
                    append_adf_text(child, output);
                }
                if matches!(
                    object.get("type").and_then(Value::as_str),
                    Some("paragraph" | "heading" | "blockquote" | "listItem" | "codeBlock")
                ) {
                    append_limited(output, "\n");
                }
            }
        }
    }
}

fn append_limited(output: &mut String, value: &str) {
    let remaining = MAX_ADF_TEXT.saturating_sub(output.len());
    if remaining == 0 {
        return;
    }
    let end = value
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= remaining)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    output.push_str(&value[..end]);
}

fn map_named_entity(entity: JiraNamedEntity) -> RemoteNamedEntity {
    RemoteNamedEntity {
        id: entity.id,
        name: entity.name,
    }
}

fn map_project(project: JiraProject) -> RemoteProject {
    RemoteProject {
        id: project.id,
        key: project.key,
        name: project.name,
    }
}

fn map_user(user: JiraUser) -> RemoteUser {
    RemoteUser {
        account_id: user.account_id,
        display_name: user.display_name,
        active: user.active,
    }
}

fn domain_account_id(user: JiraUser) -> Result<AccountId, MappingError> {
    AccountId::new(user.account_id).map_err(MappingError::InvalidDomainValue)
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
    OffsetDateTime::parse(&value, &Rfc3339)
        .or_else(|_| OffsetDateTime::parse(&value, JIRA_OFFSET_TIMESTAMP_FORMAT))
        .map_err(|_| MappingError::InvalidTimestamp(value))
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
pub struct RemoteIssuePage {
    pub issues: Vec<RemoteIssue>,
    pub next_page_token: Option<String>,
    pub is_last: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainIssuePage {
    pub issues: Vec<Issue>,
    pub next_page_token: Option<String>,
    pub is_last: bool,
}

/// Jira's read-only issue representation, normalized just enough to isolate Jira JSON field
/// spelling from the rest of the program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteIssue {
    pub id: String,
    pub key: String,
    pub summary: String,
    pub issue_type: Option<RemoteNamedEntity>,
    pub project: Option<RemoteProject>,
    pub status: Option<RemoteNamedEntity>,
    pub priority: Option<RemoteNamedEntity>,
    pub assignee: Option<RemoteUser>,
    pub parent: Option<RemoteIssueReference>,
    pub labels: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub due_date: Option<String>,
    pub resolution: Option<RemoteNamedEntity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteIssueReference {
    pub id: String,
    pub key: String,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteProject {
    pub id: String,
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteNamedEntity {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteUser {
    pub account_id: String,
    pub display_name: String,
    pub active: bool,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_an_enhanced_search_page_without_leaking_json_field_names() {
        let page: EnhancedSearchPage =
            serde_json::from_str(include_str!("../tests/fixtures/enhanced-search-page.json"))
                .unwrap();
        let mapped = IssueMapper.map_page(page).unwrap();

        assert!(mapped.is_last);
        assert_eq!(mapped.next_page_token, None);
        assert_eq!(mapped.issues.len(), 1);

        let issue = &mapped.issues[0];
        assert_eq!(issue.key, "ENG-42");
        assert_eq!(issue.summary, "Ship the Wayland dashboard");
        assert_eq!(
            issue.assignee.as_ref().unwrap().account_id,
            "557058:abc-123"
        );
        assert_eq!(issue.parent.as_ref().unwrap().key, "ENG-1");
        assert_eq!(issue.project.as_ref().unwrap().name, "Engineering");
    }

    #[test]
    fn maps_the_same_fixture_into_the_domain_model() {
        let page: EnhancedSearchPage =
            serde_json::from_str(include_str!("../tests/fixtures/enhanced-search-page.json"))
                .unwrap();
        let site_id = JiraSiteId::new("site-123").unwrap();
        let mapped = IssueMapper.map_domain_page(site_id, page).unwrap();
        let issue = &mapped.issues[0];

        assert_eq!(issue.key.as_str(), "ENG-42");
        assert_eq!(issue.status.name, "In Progress");
        assert_eq!(issue.due_date.unwrap().to_string(), "2026-08-30");
        assert_eq!(issue.parent.as_ref().unwrap().key.as_str(), "ENG-1");
    }

    #[test]
    fn rejects_a_blank_issue_key() {
        let issue: JiraIssue = serde_json::from_str(
            r#"{"id":"10001","key":" ","fields":{"summary":"A real summary"}}"#,
        )
        .unwrap();

        assert_eq!(
            IssueMapper.map_issue(issue),
            Err(MappingError::MissingRequiredField("issue key"))
        );
    }

    #[test]
    fn maps_issue_detail_adf_and_attachment_metadata_without_content_urls() {
        let issue: JiraIssue =
            serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
        let detail = IssueMapper
            .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
            .unwrap();

        assert_eq!(
            detail.issue.description_text.as_deref(),
            Some("First line\nSecond line\nA link label")
        );
        assert_eq!(detail.attachments.len(), 1);
        assert_eq!(detail.attachments[0].id, "10001");
        assert_eq!(detail.attachments[0].size_bytes, 2048);
    }

    #[test]
    fn maps_comment_page_and_calculates_next_start_without_exposing_visibility_json() {
        let page: JiraCommentPage =
            serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
        let mapped = IssueMapper.map_comment_page(page).unwrap();

        assert_eq!(mapped.start_at, 0);
        assert_eq!(mapped.next_start_at, Some(2));
        assert_eq!(mapped.total, Some(3));
        assert_eq!(
            mapped.comments[0].author.as_ref().unwrap().as_str(),
            "557058:commenter"
        );
        assert_eq!(mapped.comments[0].body, "Looks good");
        assert!(
            serde_json::to_string(&mapped.comments[0])
                .unwrap()
                .contains("Looks good")
        );
    }

    #[test]
    fn malformed_adf_is_ignored_safely_and_missing_comment_body_is_rejected() {
        assert_eq!(
            adf_to_plain_text(&serde_json::json!({"type":"mention"})),
            None
        );
        let page: JiraCommentPage = serde_json::from_value(serde_json::json!({
            "startAt": 0,
            "comments": [{
                "id": "1",
                "created": "2026-08-16T10:00:00.000+0000",
                "body": {"type":"doc"}
            }]
        }))
        .unwrap();
        assert!(matches!(
            IssueMapper.map_comment_page(page),
            Err(MappingError::MissingRequiredField("comment body"))
        ));
    }

    #[test]
    fn malformed_attachment_metadata_is_rejected_without_following_remote_urls() {
        let mut issue: JiraIssue =
            serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
        issue.fields.attachment[0].id.clear();
        assert!(matches!(
            IssueMapper.map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue),
            Err(MappingError::InvalidDomainValue(_))
        ));
    }

    #[test]
    fn comment_adf_preserves_mention_display_text_and_uses_a_safe_placeholder_for_media_only() {
        let mention = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [{
                "type": "paragraph",
                "content": [{
                    "type": "mention",
                    "attrs": {"displayText": "@Asha"}
                }, {"type": "hardBreak"}, {
                    "type": "text",
                    "text": "reviewed"
                }]
            }]
        });
        assert_eq!(
            adf_comment_text(&mention).as_deref(),
            Some("@Asha\nreviewed")
        );

        let media_only = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [{"type": "media", "attrs": {"id": "private"}}]
        });
        assert_eq!(
            adf_comment_text(&media_only).as_deref(),
            Some("[unsupported Jira content]")
        );
    }

    #[test]
    fn adf_keeps_inline_children_contiguous_and_separates_block_children() {
        let document = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "Hello "},
                    {"type": "text", "text": "linked", "marks": [{
                        "type": "link", "attrs": {"href": "https://example.invalid"}
                    }]},
                    {"type": "mention", "attrs": {"displayText": "@Asha"}},
                    {"type": "text", "text": "!"}
                ]},
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "Next"}
                ]}
            ]
        });

        assert_eq!(
            adf_to_plain_text(&document).as_deref(),
            Some("Hello linked@Asha!\nNext")
        );
    }
}
