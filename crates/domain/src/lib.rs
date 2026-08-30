//! Framework-independent vocabulary and invariants for the Jira desktop app.
//!
//! This crate intentionally contains no HTTP, database, or UI code. Adapters
//! map their transport and persistence representations at the application edge.

mod issue;
mod issue_detail;
mod rich_text;
mod update_event;
mod user;
mod user_set;
mod value;

pub use issue::{
    Issue, IssueField, IssueLifecycle, IssueType, ParentIssue, Priority, Project, Status,
};
pub use issue_detail::{
    AttachmentMetadata, IssueComment, IssueCommentAuthor, IssueDetail, IssueDetailCore,
};
pub use rich_text::{
    HORIZONTAL_RULE_LABEL, PanelKind, RichAttachmentCard, RichBlock, RichDecisionItem,
    RichDecisionState, RichImage, RichInline, RichListItem, RichMark, RichStatusColor, RichTable,
    RichTableCell, RichTableRow, RichTaskItem, RichTaskState, RichTextDocument,
};
pub use update_event::{
    ChangeValue, NotificationDelivery, UpdateEvent, UpdateKind, UpdateReadState,
};
pub use user::User;
pub use user_set::{UserSet, UserSetError};
pub use value::{
    AccountId, DomainError, EventId, IssueId, IssueKey, JiraSiteId, Timestamp, UserSetId,
};
