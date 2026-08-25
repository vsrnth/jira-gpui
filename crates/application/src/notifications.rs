use jira_domain::{UpdateEvent, UpdateKind};

/// Stateless default policy for desktop notifications.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultDesktopNotificationPolicy;

impl crate::NotificationPolicy for DefaultDesktopNotificationPolicy {
    fn should_notify(&self, event: &UpdateEvent) -> bool {
        match &event.kind {
            UpdateKind::IssueAddedToView
            | UpdateKind::StatusChanged { .. }
            | UpdateKind::AssigneeChanged { .. }
            | UpdateKind::PriorityChanged { .. }
            | UpdateKind::DueDateChanged { .. }
            | UpdateKind::CommentAdded { .. }
            | UpdateKind::FieldChanged { .. }
            | UpdateKind::IssueUpdated => true,
            UpdateKind::IssueRemovedFromView
            | UpdateKind::SummaryChanged { .. }
            | UpdateKind::ParentChanged { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NotificationPolicy;
    use jira_domain::ChangeValue;
    use jira_domain::{AccountId, EventId, IssueId, IssueKey, JiraSiteId, UserSetId};

    fn event(kind: UpdateKind) -> UpdateEvent {
        UpdateEvent::new(
            EventId::new("event-1").expect("test event ID should be valid"),
            JiraSiteId::new("site").expect("test Jira site ID should be valid"),
            IssueId::new("issue-1").expect("test issue ID should be valid"),
            IssueKey::new("PROJ-1").expect("test issue key should be valid"),
            kind,
            time::OffsetDateTime::now_utc(),
            vec![UserSetId::new("set-1").expect("test user set ID should be valid")],
        )
    }

    #[test]
    fn allows_all_actionable_update_kinds() {
        let policy = DefaultDesktopNotificationPolicy;
        let change = ChangeValue::Text("new".into());
        for kind in [
            UpdateKind::IssueAddedToView,
            UpdateKind::StatusChanged {
                old: change.clone(),
                new: change.clone(),
            },
            UpdateKind::AssigneeChanged {
                old: change.clone(),
                new: change.clone(),
            },
            UpdateKind::PriorityChanged {
                old: change.clone(),
                new: change.clone(),
            },
            UpdateKind::DueDateChanged {
                old: change.clone(),
                new: change.clone(),
            },
            UpdateKind::CommentAdded {
                comment_id: "comment-1".into(),
                author: Some(AccountId::new("account-1").expect("test account ID should be valid")),
                excerpt: "hello".into(),
            },
        ] {
            assert!(policy.should_notify(&event(kind)));
        }
    }

    #[test]
    fn suppresses_neutral_and_low_signal_update_kinds() {
        let policy = DefaultDesktopNotificationPolicy;
        let change = ChangeValue::Text("new".into());
        for kind in [
            UpdateKind::IssueRemovedFromView,
            UpdateKind::SummaryChanged {
                old: change.clone(),
                new: change.clone(),
            },
            UpdateKind::ParentChanged {
                old: change.clone(),
                new: change,
            },
        ] {
            assert!(!policy.should_notify(&event(kind)));
        }
    }

    #[test]
    fn allows_generic_issue_updates() {
        let policy = DefaultDesktopNotificationPolicy;

        assert!(policy.should_notify(&event(UpdateKind::IssueUpdated)));
    }
}
