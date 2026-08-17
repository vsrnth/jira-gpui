use serde::{Deserialize, Serialize};

use crate::{AccountId, DomainError, Issue, Timestamp};

const MAX_DETAIL_TEXT: usize = 1_000_000;

/// Read-only metadata for an issue attachment. Binary content is never part of the domain model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentMetadata {
    pub id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
}

impl AttachmentMetadata {
    pub fn new(
        id: impl Into<String>,
        filename: impl Into<String>,
        size_bytes: u64,
        mime_type: Option<impl Into<String>>,
    ) -> Result<Self, DomainError> {
        let id = validate_text(id.into(), "attachment id", 255)?;
        let filename = validate_text(filename.into(), "attachment filename", 255)?;
        let mime_type = mime_type
            .map(Into::into)
            .map(|value| validate_text(value, "attachment MIME type", 255))
            .transpose()?;
        Ok(Self {
            id,
            filename,
            size_bytes,
            mime_type,
        })
    }
}

/// A comment author identity with optional Jira display data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCommentAuthor {
    pub account_id: AccountId,
    pub display_name: Option<String>,
}

impl IssueCommentAuthor {
    pub fn new(
        account_id: AccountId,
        display_name: Option<impl Into<String>>,
    ) -> Result<Self, DomainError> {
        let display_name = display_name
            .map(Into::into)
            .map(|value: String| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(|value| validate_text(value, "comment author display name", 255))
            .transpose()?;
        Ok(Self {
            account_id,
            display_name,
        })
    }
}

/// A textual issue comment with attachment metadata only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: String,
    pub author: Option<IssueCommentAuthor>,
    pub body: String,
    pub created_at: Timestamp,
    pub updated_at: Option<Timestamp>,
    pub attachments: Vec<AttachmentMetadata>,
}

#[cfg(test)]
mod author_tests {
    use super::IssueCommentAuthor;
    use crate::AccountId;

    #[test]
    fn normalizes_optional_comment_author_display_name() {
        let author = IssueCommentAuthor::new(
            AccountId::new("account-1").expect("account"),
            Some("  Asha  "),
        )
        .expect("author");
        assert_eq!(author.display_name.as_deref(), Some("Asha"));

        let blank =
            IssueCommentAuthor::new(AccountId::new("account-1").expect("account"), Some("   "))
                .expect("blank display names are optional");
        assert_eq!(blank.display_name, None);
    }

    #[test]
    fn rejects_overlong_comment_author_display_name() {
        let error = IssueCommentAuthor::new(
            AccountId::new("account-1").expect("account"),
            Some("x".repeat(256)),
        )
        .expect_err("display names must stay bounded");

        assert_eq!(
            error,
            crate::DomainError::TooLong {
                field: "comment author display name",
                maximum: 255,
            }
        );
    }
}

impl IssueComment {
    pub fn new(
        id: impl Into<String>,
        author: Option<IssueCommentAuthor>,
        body: impl Into<String>,
        created_at: Timestamp,
        updated_at: Option<Timestamp>,
        attachments: Vec<AttachmentMetadata>,
    ) -> Result<Self, DomainError> {
        Ok(Self {
            id: validate_text(id.into(), "comment id", 255)?,
            author,
            body: validate_text(body.into(), "comment body", MAX_DETAIL_TEXT)?,
            created_at,
            updated_at,
            attachments,
        })
    }
}

/// The issue snapshot and top-level attachment metadata returned by an issue-detail request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetailCore {
    pub issue: Issue,
    pub attachments: Vec<AttachmentMetadata>,
}

impl IssueDetailCore {
    pub fn new(issue: Issue, attachments: Vec<AttachmentMetadata>) -> Result<Self, DomainError> {
        Ok(Self { issue, attachments })
    }
}

/// A complete read-only issue detail assembled from the core issue and all comment pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub core: IssueDetailCore,
    pub comments: Vec<IssueComment>,
}

impl IssueDetail {
    pub fn new(core: IssueDetailCore, comments: Vec<IssueComment>) -> Result<Self, DomainError> {
        Ok(Self { core, comments })
    }
}

fn validate_text(
    value: String,
    field: &'static str,
    maximum: usize,
) -> Result<String, DomainError> {
    if value.trim().is_empty() {
        return Err(DomainError::Empty { field });
    }
    if value.len() > maximum {
        return Err(DomainError::TooLong { field, maximum });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Issue, IssueId, JiraSiteId};
    use time::macros::datetime;

    #[test]
    fn builds_validated_issue_detail_with_comments_and_attachment_metadata() {
        let issue = Issue::new(
            JiraSiteId::new("site").unwrap(),
            IssueId::new("100").unwrap(),
            crate::IssueKey::new("APP-100").unwrap(),
            crate::Project {
                id: "1".into(),
                key: "APP".into(),
                name: "App".into(),
            },
            crate::IssueType {
                id: "1".into(),
                name: "Task".into(),
                icon_url: None,
            },
            "A task",
            crate::Status {
                id: "1".into(),
                name: "Open".into(),
                category: None,
            },
            crate::Priority {
                id: None,
                name: None,
                icon_url: None,
            },
            None,
            None,
            None,
            Vec::new(),
            datetime!(2026-01-01 00:00 UTC),
            datetime!(2026-01-01 00:00 UTC),
            None,
        );
        let attachment = AttachmentMetadata::new("att-1", "notes.txt", 42, Some("text/plain"))
            .expect("attachment");
        let comment = IssueComment::new(
            "comment-1",
            None,
            "Looks good",
            datetime!(2026-01-02 00:00 UTC),
            None,
            vec![attachment.clone()],
        )
        .expect("comment");

        let detail = IssueDetail::new(
            IssueDetailCore::new(issue, vec![attachment]).expect("core"),
            vec![comment],
        )
        .expect("detail");

        assert_eq!(detail.comments.len(), 1);
        assert_eq!(detail.core.attachments[0].filename, "notes.txt");
    }
}
