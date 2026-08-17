use crate::models::{
    EnhancedSearchPage, JiraAttachment, JiraComment, JiraCommentPage, JiraIssue, JiraNamedEntity,
    JiraProject, JiraUser,
};
use jira_application::{IssueCommentsPage, PageCursor};
use jira_domain::{
    AccountId, AttachmentMetadata, Issue, IssueComment, IssueCommentAuthor, IssueDetailCore,
    IssueId, IssueKey, IssueType, JiraSiteId, PanelKind, ParentIssue, Priority, Project, RichBlock,
    RichInline, RichListItem, RichMark, RichTextDocument, Status, Timestamp, User,
};
use serde_json::Value;
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

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
        let rich_description = issue.fields.description.as_ref().and_then(parse_adf);
        let description = rich_description
            .as_ref()
            .map(RichTextDocument::plain_text)
            .filter(|text| !text.is_empty());
        let attachments = issue
            .fields
            .attachment
            .iter()
            .map(map_attachment)
            .collect::<Result<Vec<_>, _>>()?;
        let mut domain_issue = self.map_domain_issue(site_id, issue)?;
        domain_issue.description_text = description;
        domain_issue.rich_description = rich_description;
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
const MAX_ADF_DEPTH: usize = 64;
const MAX_ADF_NODES: usize = 10_000;
const MAX_LINK_HREF_BYTES: usize = 2_048;
const MAX_LINK_TITLE_BYTES: usize = 512;
const UNSUPPORTED_CONTENT: &str = "[unsupported Jira content]";

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
    let rich_body = comment.body.as_ref().and_then(parse_adf);
    let body = rich_body
        .as_ref()
        .map(RichTextDocument::plain_text)
        .filter(|text| !text.is_empty())
        .or_else(|| comment.body.as_ref().and_then(adf_comment_text))
        .ok_or(MappingError::MissingRequiredField("comment body"))?;
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

/// Extracts the safe plain-text projection of a bounded ADF document.
pub fn adf_to_plain_text(value: &Value) -> Option<String> {
    parse_adf(value)
        .map(|document| document.plain_text())
        .filter(|text| !text.is_empty())
}

fn adf_comment_text(value: &Value) -> Option<String> {
    adf_to_plain_text(value).or_else(|| {
        let object = value.as_object()?;
        let content = object.get("content")?.as_array()?;
        (!content.is_empty()).then(|| UNSUPPORTED_CONTENT.to_owned())
    })
}

/// Parses a Jira ADF document into a bounded transport-neutral representation.
///
/// Invalid roots are rejected. Unsupported nodes become safe placeholders, and no raw attrs,
/// URLs, or account IDs are ever copied into visible fallback text.
pub fn parse_adf(value: &Value) -> Option<RichTextDocument> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("doc")
        || object.get("version").and_then(Value::as_u64) != Some(1)
    {
        return None;
    }
    let content = object.get("content")?.as_array()?;
    let mut state = AdfParserState::default();
    let blocks = parse_blocks(content, 1, &mut state);
    Some(RichTextDocument::new(blocks, state.truncated))
}

#[derive(Default)]
struct AdfParserState {
    nodes: usize,
    text_bytes: usize,
    truncated: bool,
}

impl AdfParserState {
    fn visit(&mut self) -> bool {
        if self.nodes >= MAX_ADF_NODES {
            self.truncated = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    fn text(&mut self, value: &str) -> String {
        let remaining = MAX_ADF_TEXT.saturating_sub(self.text_bytes);
        if remaining == 0 {
            self.truncated = true;
            return String::new();
        }
        let end = value
            .char_indices()
            .take_while(|(index, character)| index + character.len_utf8() <= remaining)
            .map(|(index, character)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        self.text_bytes += end;
        if end < value.len() {
            self.truncated = true;
        }
        value[..end].to_owned()
    }
}

fn parse_blocks(values: &[Value], depth: usize, state: &mut AdfParserState) -> Vec<RichBlock> {
    values
        .iter()
        .filter_map(|value| parse_block(value, depth, state))
        .collect()
}

fn parse_block(value: &Value, depth: usize, state: &mut AdfParserState) -> Option<RichBlock> {
    if depth > MAX_ADF_DEPTH || !state.visit() {
        state.truncated = true;
        return None;
    }
    let Some(object) = value.as_object() else {
        state.truncated = true;
        return Some(RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        });
    };
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = object.get("content").and_then(Value::as_array);
    let block = match kind {
        "paragraph" => match content {
            Some(content) => RichBlock::Paragraph(parse_inlines(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "heading" => match content {
            Some(content) => RichBlock::Heading {
                level: object
                    .get("attrs")
                    .and_then(Value::as_object)
                    .and_then(|attrs| attrs.get("level"))
                    .and_then(Value::as_u64)
                    .and_then(|level| u8::try_from(level).ok())
                    .filter(|level| (1..=6).contains(level))
                    .unwrap_or(1),
                content: parse_inlines(content, depth + 1, state),
            },
            None => malformed_block(state),
        },
        "bulletList" => match content {
            Some(content) => RichBlock::BulletList(parse_list_items(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "orderedList" => match content {
            Some(content) => RichBlock::OrderedList {
                order: object
                    .get("attrs")
                    .and_then(Value::as_object)
                    .and_then(|attrs| attrs.get("order"))
                    .and_then(Value::as_u64)
                    .and_then(|order| u32::try_from(order).ok())
                    .unwrap_or(1),
                items: parse_list_items(content, depth + 1, state),
            },
            None => malformed_block(state),
        },
        "listItem" => match content {
            Some(content) => RichBlock::BlockQuote(parse_blocks(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "codeBlock" => match content {
            Some(content) => RichBlock::CodeBlock {
                language: object
                    .get("attrs")
                    .and_then(Value::as_object)
                    .and_then(|attrs| attrs.get("language"))
                    .and_then(Value::as_str)
                    .map(|language| state.text(language))
                    .filter(|language| !language.is_empty()),
                text: parse_code_text(content, depth + 1, state),
            },
            None => malformed_block(state),
        },
        "blockquote" => match content {
            Some(content) => RichBlock::BlockQuote(parse_blocks(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "panel" => {
            let kind = object
                .get("attrs")
                .and_then(Value::as_object)
                .and_then(|attrs| attrs.get("panelType"))
                .and_then(Value::as_str)
                .and_then(panel_kind);
            match (kind, content) {
                (Some(kind), Some(content)) => RichBlock::Panel {
                    kind,
                    content: parse_blocks(content, depth + 1, state),
                },
                _ => malformed_block(state),
            }
        }
        "doc" => RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        },
        "media" | "mediaSingle" | "mediaGroup" | "mediaInline" => RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        },
        "rule" | "table" | "tableCell" | "tableHeader" | "tableRow" | "emoji" | "date"
        | "status" | "inlineCard" | "expand" | "nestedExpand" => RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        },
        _ => RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        },
    };
    Some(block)
}

fn malformed_block(state: &mut AdfParserState) -> RichBlock {
    state.truncated = true;
    RichBlock::Placeholder {
        label: UNSUPPORTED_CONTENT.to_owned(),
    }
}

fn parse_list_items(
    values: &[Value],
    depth: usize,
    state: &mut AdfParserState,
) -> Vec<RichListItem> {
    values
        .iter()
        .filter_map(|value| {
            let Some(object) = value.as_object() else {
                state.truncated = true;
                return Some(RichListItem {
                    blocks: vec![RichBlock::Placeholder {
                        label: UNSUPPORTED_CONTENT.to_owned(),
                    }],
                });
            };
            if object.get("type").and_then(Value::as_str) != Some("listItem") {
                state.truncated = true;
                return Some(RichListItem {
                    blocks: vec![RichBlock::Placeholder {
                        label: UNSUPPORTED_CONTENT.to_owned(),
                    }],
                });
            }
            if depth > MAX_ADF_DEPTH || !state.visit() {
                state.truncated = true;
                return None;
            }
            let Some(content) = object.get("content").and_then(Value::as_array) else {
                state.truncated = true;
                return Some(RichListItem {
                    blocks: vec![RichBlock::Placeholder {
                        label: UNSUPPORTED_CONTENT.to_owned(),
                    }],
                });
            };
            Some(RichListItem {
                blocks: parse_blocks(content, depth + 1, state),
            })
        })
        .collect()
}

fn parse_code_text(values: &[Value], depth: usize, state: &mut AdfParserState) -> String {
    let mut text = String::new();
    for value in values {
        if depth > MAX_ADF_DEPTH || !state.visit() {
            state.truncated = true;
            break;
        }
        if value.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(value) = value.get("text").and_then(Value::as_str) {
                text.push_str(&state.text(value));
            }
        } else {
            state.truncated = true;
        }
    }
    text
}

fn parse_inlines(values: &[Value], depth: usize, state: &mut AdfParserState) -> Vec<RichInline> {
    values
        .iter()
        .filter_map(|value| parse_inline(value, depth, state))
        .collect()
}

fn parse_inline(value: &Value, depth: usize, state: &mut AdfParserState) -> Option<RichInline> {
    if depth > MAX_ADF_DEPTH || !state.visit() {
        state.truncated = true;
        return None;
    }
    let Some(object) = value.as_object() else {
        state.truncated = true;
        return Some(RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        });
    };
    match object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => match object.get("text").and_then(Value::as_str) {
            Some(text) => Some(RichInline::Text {
                text: state.text(text),
                marks: parse_marks(object.get("marks").and_then(Value::as_array)),
            }),
            None => {
                state.truncated = true;
                Some(RichInline::Placeholder {
                    label: UNSUPPORTED_CONTENT.to_owned(),
                })
            }
        },
        "hardBreak" => Some(RichInline::HardBreak),
        "mention" => {
            let attrs = object.get("attrs").and_then(Value::as_object);
            let normalized_id = attrs
                .and_then(|attrs| attrs.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty());
            let account_id = normalized_id.and_then(|id| AccountId::new(id.to_owned()).ok());
            let label = attrs
                .and_then(|attrs| {
                    ["text", "displayText", "displayName"]
                        .into_iter()
                        .find_map(|key| attrs.get(key).and_then(Value::as_str))
                })
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .filter(|text| Some(*text) != normalized_id)
                .filter(|text| !looks_like_opaque_account_id(text))
                .map(|text| state.text(text))
                .unwrap_or_else(|| "Mentioned user".to_owned());
            Some(RichInline::Mention { account_id, label })
        }
        "emoji" | "date" | "status" | "inlineCard" | "mediaInline" => {
            Some(RichInline::Placeholder {
                label: UNSUPPORTED_CONTENT.to_owned(),
            })
        }
        _ => Some(RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        }),
    }
}

fn parse_marks(values: Option<&Vec<Value>>) -> Vec<RichMark> {
    values
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let object = value.as_object()?;
            match object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "code" => Some(RichMark::Code),
                "em" => Some(RichMark::Emphasis),
                "strong" => Some(RichMark::Strong),
                "strike" => Some(RichMark::Strike),
                "link" => {
                    let attrs = object.get("attrs").and_then(Value::as_object)?;
                    let href = attrs
                        .get("href")
                        .and_then(Value::as_str)
                        .and_then(safe_uri)?;
                    let title = attrs
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|title| title.len() <= MAX_LINK_TITLE_BYTES)
                        .map(str::to_owned);
                    Some(RichMark::Link { href, title })
                }
                _ => None,
            }
        })
        .collect()
}

fn panel_kind(value: &str) -> Option<PanelKind> {
    match value {
        "info" => Some(PanelKind::Info),
        "note" => Some(PanelKind::Note),
        "warning" => Some(PanelKind::Warning),
        "success" => Some(PanelKind::Success),
        "error" => Some(PanelKind::Error),
        _ => None,
    }
}

/// Jira Cloud account IDs commonly use a six-digit tenant prefix followed by an opaque token.
/// This deliberately recognizes only that narrow shape so ordinary human labels containing a
/// colon remain visible while an id-less mention cannot leak an account identifier.
fn looks_like_opaque_account_id(value: &str) -> bool {
    let value = value.trim();
    let Some((prefix, suffix)) = value.split_once(':') else {
        return false;
    };
    prefix.len() == 6
        && prefix.bytes().all(|byte| byte.is_ascii_digit())
        && suffix.len() >= 6
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn safe_uri(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() > MAX_LINK_HREF_BYTES {
        return None;
    }
    let parsed = Url::parse(value).ok()?;
    let authority_start = value.find("://")? + 3;
    let authority_end = value[authority_start..]
        .find(['/', '?', '#'])
        .map_or(value.len(), |offset| authority_start + offset);
    if value[authority_start..authority_end].contains('@') {
        return None;
    }
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(value.to_owned())
}

fn valid_display_name(user: &JiraUser) -> Option<String> {
    let display_name = user.display_name.trim();
    (!display_name.is_empty()
        && display_name.len() <= 255
        && display_name != user.account_id.trim())
    .then_some(display_name.to_owned())
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
    let display_name = valid_display_name(&user).unwrap_or_else(|| "Unknown user".to_owned());
    RemoteUser {
        account_id: user.account_id,
        display_name,
        active: user.active,
    }
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
    fn preserves_valid_issue_assignee_and_reporter_display_names() {
        let mut page: EnhancedSearchPage =
            serde_json::from_str(include_str!("../tests/fixtures/enhanced-search-page.json"))
                .unwrap();
        let mut issue = page.issues.pop().expect("issue fixture");
        issue.fields.reporter = Some(JiraUser {
            account_id: "712020:reporter".to_owned(),
            display_name: "Nina Smith".to_owned(),
            active: true,
            avatar_urls: Default::default(),
        });
        let mapped = IssueMapper
            .map_domain_issue(JiraSiteId::new("site-123").unwrap(), issue)
            .unwrap();

        assert_eq!(mapped.assignee_display_name.as_deref(), Some("Asha Patel"));
        assert_eq!(mapped.reporter_display_name.as_deref(), Some("Nina Smith"));
        assert_eq!(
            mapped.reporter.as_ref().unwrap().as_str(),
            "712020:reporter"
        );
    }

    #[test]
    fn map_user_never_exposes_account_id_as_display_name() {
        let user = JiraUser {
            account_id: "account-1".to_owned(),
            display_name: " account-1 ".to_owned(),
            active: true,
            avatar_urls: Default::default(),
        };
        let mapped = IssueMapper
            .map_user(JiraSiteId::new("site").expect("site"), user)
            .expect("user should retain stable identity");

        assert_eq!(mapped.display_name, "Unknown user");
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
        assert!(detail.issue.rich_description.is_some());
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
            mapped.comments[0]
                .author
                .as_ref()
                .unwrap()
                .account_id
                .as_str(),
            "557058:commenter"
        );
        assert_eq!(
            mapped.comments[0]
                .author
                .as_ref()
                .unwrap()
                .display_name
                .as_deref(),
            Some("Asha")
        );
        assert_eq!(mapped.comments[0].body, "Looks good");
        assert!(mapped.comments[0].rich_body.is_some());
        assert!(
            serde_json::to_string(&mapped.comments[0])
                .unwrap()
                .contains("Looks good")
        );
    }

    #[test]
    fn maps_one_created_comment_through_the_public_adapter_mapper() {
        let page: JiraCommentPage =
            serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
        let comment = page.comments.into_iter().next().expect("fixture comment");

        let mapped = IssueMapper.map_comment(comment).expect("mapped comment");

        assert_eq!(mapped.id.as_str(), "20001");
        assert_eq!(mapped.body, "Looks good");
        assert_eq!(
            mapped
                .author
                .as_ref()
                .and_then(|author| author.display_name.as_deref()),
            Some("Asha")
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
    fn rejects_non_document_adf_roots_and_versions() {
        assert!(parse_adf(&serde_json::json!({"type":"paragraph","content":[]})).is_none());
        assert!(
            parse_adf(&serde_json::json!({
                "type":"doc", "version": 2, "content": []
            }))
            .is_none()
        );
    }

    #[test]
    fn maps_supported_rich_text_and_safe_placeholders() {
        let document = serde_json::json!({
            "type": "doc", "version": 1, "content": [
                {"type":"heading", "attrs":{"level":2}, "content":[
                    {"type":"text", "text":"Title", "marks":[{"type":"strong"}]}
                ]},
                {"type":"paragraph", "content":[
                    {"type":"text", "text":"styled", "marks":[
                        {"type":"em"}, {"type":"strike"}, {"type":"code"},
                        {"type":"link", "attrs":{"href":"javascript:alert(1)","title":"bad"}}
                    ]},
                    {"type":"hardBreak"},
                    {"type":"mention", "attrs":{"id":"712020:secret","text":"@Asha"}}
                ]},
                {"type":"bulletList", "content":[{"type":"listItem", "content":[
                    {"type":"paragraph", "content":[{"type":"text","text":"one"}]}
                ]}]},
                {"type":"orderedList", "attrs":{"order":3}, "content":[{"type":"listItem", "content":[
                    {"type":"paragraph", "content":[{"type":"text","text":"two"}]}
                ]}]},
                {"type":"codeBlock", "attrs":{"language":"rust"}, "content":[
                    {"type":"text", "text":"let answer = 42;"}
                ]},
                {"type":"blockquote", "content":[{"type":"paragraph", "content":[
                    {"type":"text","text":"quoted"}
                ]}]},
                {"type":"panel", "attrs":{"panelType":"warning"}, "content":[
                    {"type":"paragraph", "content":[{"type":"text","text":"careful"}]}
                ]},
                {"type":"mediaSingle", "content":[{"type":"media", "attrs":{"id":"private"}}]},
                {"type":"table", "content":[]}
            ]
        });

        let parsed = parse_adf(&document).expect("valid ADF");
        assert!(!parsed.truncated);
        assert!(matches!(
            parsed.blocks[0],
            RichBlock::Heading { level: 2, .. }
        ));
        assert!(matches!(parsed.blocks[2], RichBlock::BulletList(_)));
        assert!(matches!(
            parsed.blocks[3],
            RichBlock::OrderedList { order: 3, .. }
        ));
        assert!(matches!(parsed.blocks[4], RichBlock::CodeBlock { .. }));
        assert!(matches!(parsed.blocks[5], RichBlock::BlockQuote(_)));
        assert!(matches!(
            parsed.blocks[6],
            RichBlock::Panel {
                kind: PanelKind::Warning,
                ..
            }
        ));
        assert!(matches!(parsed.blocks[7], RichBlock::Placeholder { .. }));
        let plain = parsed.plain_text();
        assert!(plain.contains("@Asha"));
        assert!(!plain.contains("712020:secret"));
        assert!(!plain.contains("javascript:"));
    }

    #[test]
    fn id_only_mentions_never_project_account_ids() {
        let document = serde_json::json!({
            "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
                {"type":"mention", "attrs":{"id":"712020:secret"}}
            ]}]
        });
        let parsed = parse_adf(&document).expect("valid ADF");
        assert_eq!(parsed.plain_text(), "Mentioned user");
        assert!(!parsed.plain_text().contains("712020:secret"));
    }

    #[test]
    fn mention_labels_equal_to_account_ids_use_a_safe_fallback() {
        let document = serde_json::json!({
            "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
                {"type":"mention", "attrs":{"id":" 712020:secret ", "text":"712020:secret"}},
                {"type":"mention", "attrs":{"id":"account-2", "displayName":"account-2"}}
            ]}]
        });
        let parsed = parse_adf(&document).expect("valid root");
        assert_eq!(parsed.plain_text(), "Mentioned userMentioned user");
        assert!(!parsed.plain_text().contains("712020:secret"));
        assert!(!parsed.plain_text().contains("account-2"));
    }

    #[test]
    fn idless_opaque_mention_labels_use_a_safe_fallback() {
        let document = serde_json::json!({
            "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
                {"type":"mention", "attrs":{"displayText":"712020:d98ae2"}},
                {"type":"mention", "attrs":{"displayText":"Team: Platform"}}
            ]}]
        });
        let parsed = parse_adf(&document).expect("valid root");
        assert_eq!(parsed.plain_text(), "Mentioned userTeam: Platform");
        assert!(!parsed.plain_text().contains("712020:d98ae2"));
    }

    #[test]
    fn malformed_and_oversized_links_drop_only_the_mark() {
        let oversized_href = format!("https://example.test/{}", "x".repeat(2_048));
        let oversized_title = "t".repeat(513);
        let document = serde_json::json!({
            "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
                {"type":"text", "text":"bad-scheme", "marks":[{"type":"link", "attrs":{"href":"javascript:alert(1)"}}]},
                {"type":"text", "text":"credentials", "marks":[{"type":"link", "attrs":{"href":"https://user:pass@example.test"}}]},
                {"type":"text", "text":"bad-authority", "marks":[{"type":"link", "attrs":{"href":"https://example.test:bad"}}]},
                {"type":"text", "text":"long-href", "marks":[{"type":"link", "attrs":{"href":oversized_href}}]},
                {"type":"text", "text":"long-title", "marks":[{"type":"link", "attrs":{"href":"https://example.test", "title":oversized_title}}]}
            ]}]
        });
        let parsed = parse_adf(&document).expect("valid root");
        assert_eq!(
            parsed.plain_text(),
            "bad-schemecredentialsbad-authoritylong-hreflong-title"
        );
        let RichBlock::Paragraph(inlines) = &parsed.blocks[0] else {
            panic!("expected paragraph")
        };
        assert!(inlines.iter().all(|inline| matches!(inline, RichInline::Text { marks, .. } if marks.is_empty() || marks.iter().all(|mark| matches!(mark, RichMark::Link { title: None, .. })) )));
    }

    #[test]
    fn malformed_recognized_nodes_emit_placeholders_and_truncation() {
        let document = serde_json::json!({
            "type":"doc", "version":1, "content":[
                {"type":"paragraph"},
                {"type":"heading", "content": [{"type":"text"}]},
                {"type":"bulletList", "content": [{"type":"listItem"}]}
            ]
        });
        let parsed = parse_adf(&document).expect("valid root");
        assert!(parsed.truncated);
        assert!(parsed.plain_text().contains("[unsupported Jira content]"));
    }

    #[test]
    fn deep_and_large_adf_is_deterministically_truncated() {
        let mut nested = serde_json::json!({
            "type":"paragraph", "content":[{"type":"text", "text":"deep"}]
        });
        for _ in 0..70 {
            nested = serde_json::json!({"type":"blockquote", "content":[nested]});
        }
        let deep = serde_json::json!({"type":"doc", "version":1, "content":[nested]});
        let parsed = parse_adf(&deep).expect("valid root");
        assert!(parsed.truncated);
        assert!(parsed.plain_text().contains("content truncated"));

        let many = (0..10_100)
            .map(|_| serde_json::json!({"type":"paragraph","content":[]}))
            .collect::<Vec<_>>();
        let parsed = parse_adf(&serde_json::json!({
            "type":"doc", "version":1, "content":many
        }))
        .expect("valid root");
        assert!(parsed.truncated);
        assert!(parsed.plain_text().contains("content truncated"));
    }

    #[test]
    fn text_limit_is_bounded_without_splitting_utf8() {
        let text = "é".repeat(600_000);
        let document = serde_json::json!({
            "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
                {"type":"text", "text":text}
            ]}]
        });
        let parsed = parse_adf(&document).expect("valid root");
        assert!(parsed.truncated);
        assert!(parsed.plain_text().contains("content truncated"));
        assert!(
            parsed
                .plain_text()
                .is_char_boundary(parsed.plain_text().len())
        );
        assert!(parsed.plain_text().len() <= 1_000_100);
    }

    #[test]
    fn blank_missing_and_oversized_author_names_fall_back_without_dropping_account_id() {
        let mut blank: JiraCommentPage =
            serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
        blank.comments[0]
            .author
            .as_mut()
            .expect("fixture author")
            .display_name = "   ".to_owned();
        let mapped = IssueMapper.map_comment_page(blank).unwrap();
        let author = mapped.comments[0].author.as_ref().expect("author");
        assert_eq!(author.account_id.as_str(), "557058:commenter");
        assert_eq!(author.display_name, None);

        let mut oversized: JiraCommentPage =
            serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
        oversized.comments[0]
            .author
            .as_mut()
            .expect("fixture author")
            .display_name = "x".repeat(256);
        let mapped = IssueMapper.map_comment_page(oversized).unwrap();
        assert_eq!(
            mapped.comments[0]
                .author
                .as_ref()
                .expect("author")
                .display_name,
            None
        );

        let missing: JiraCommentPage = serde_json::from_value(serde_json::json!({
            "startAt": 0,
            "comments": [{
                "id": "1",
                "author": {"accountId": "account-without-name"},
                "created": "2026-08-16T10:00:00.000+0000",
                "body": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"body"}]}]}
            }]
        })).unwrap();
        let mapped = IssueMapper.map_comment_page(missing).unwrap();
        let author = mapped.comments[0].author.as_ref().expect("author");
        assert_eq!(author.account_id.as_str(), "account-without-name");
        assert_eq!(author.display_name, None);
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
