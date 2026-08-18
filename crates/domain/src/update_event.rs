use serde::{Deserialize, Serialize};

use crate::{AccountId, EventId, IssueId, IssueKey, JiraSiteId, Timestamp, UserSetId};

/// The limited values that may be included in an update event. Issue descriptions
/// and complete comment bodies deliberately do not belong in notification data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum ChangeValue {
    Text(String),
    Account(AccountId),
    Date(Option<String>),
    Parent(Option<IssueKey>),
    Empty,
}

/// Locally derived changes that can appear in the update inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UpdateKind {
    IssueAddedToView,
    IssueRemovedFromView,
    /// A Jira update was observed, but no more specific tracked field changed.
    IssueUpdated,
    /// A bounded Jira changelog item with display-safe before/after values.
    FieldChanged {
        field: String,
        old: ChangeValue,
        new: ChangeValue,
    },
    StatusChanged {
        old: ChangeValue,
        new: ChangeValue,
    },
    AssigneeChanged {
        old: ChangeValue,
        new: ChangeValue,
    },
    PriorityChanged {
        old: ChangeValue,
        new: ChangeValue,
    },
    DueDateChanged {
        old: ChangeValue,
        new: ChangeValue,
    },
    SummaryChanged {
        old: ChangeValue,
        new: ChangeValue,
    },
    ParentChanged {
        old: ChangeValue,
        new: ChangeValue,
    },
    CommentAdded {
        comment_id: String,
        author: Option<AccountId>,
        excerpt: String,
    },
}

/// Local inbox state; never synchronized to Jira.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateReadState {
    #[default]
    Unread,
    Read,
}

/// The result of attempting a local desktop notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDelivery {
    #[default]
    NotAttempted,
    Delivered,
    Unavailable,
    SuppressedByPolicy,
}

/// An immutable, locally derived timeline entry for a Jira issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateEvent {
    pub id: EventId,
    pub site_id: JiraSiteId,
    pub issue_id: IssueId,
    pub issue_key: IssueKey,
    pub kind: UpdateKind,
    pub occurred_at: Timestamp,
    pub matching_user_set_ids: Vec<UserSetId>,
    pub read_state: UpdateReadState,
    pub notification_delivery: NotificationDelivery,
}

impl UpdateEvent {
    pub fn new(
        id: EventId,
        site_id: JiraSiteId,
        issue_id: IssueId,
        issue_key: IssueKey,
        kind: UpdateKind,
        occurred_at: Timestamp,
        matching_user_set_ids: Vec<UserSetId>,
    ) -> Self {
        Self {
            id,
            site_id,
            issue_id,
            issue_key,
            kind,
            occurred_at,
            matching_user_set_ids,
            read_state: UpdateReadState::Unread,
            notification_delivery: NotificationDelivery::NotAttempted,
        }
    }

    pub fn mark_read(&mut self) {
        self.read_state = UpdateReadState::Read;
    }

    pub fn mark_unread(&mut self) {
        self.read_state = UpdateReadState::Unread;
    }

    pub fn record_notification_delivery(&mut self, delivery: NotificationDelivery) {
        self.notification_delivery = delivery;
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn update_events_start_unread_and_can_be_marked_read_locally() {
        let mut event = UpdateEvent::new(
            EventId::new("event").unwrap(),
            JiraSiteId::new("site").unwrap(),
            IssueId::new("10001").unwrap(),
            IssueKey::new("APP-1").unwrap(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-01 00:00 UTC),
            vec![],
        );
        assert_eq!(event.read_state, UpdateReadState::Unread);
        event.mark_read();
        assert_eq!(event.read_state, UpdateReadState::Read);
    }
}
