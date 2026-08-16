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
            | UpdateKind::CommentAdded { .. } => true,
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
            EventId::new("event-1").unwrap(),
            JiraSiteId::new("site").unwrap(),
            IssueId::new("issue-1").unwrap(),
            IssueKey::new("PROJ-1").unwrap(),
            kind,
            time::OffsetDateTime::now_utc(),
            vec![UserSetId::new("set-1").unwrap()],
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
                author: Some(AccountId::new("account-1").unwrap()),
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
}
