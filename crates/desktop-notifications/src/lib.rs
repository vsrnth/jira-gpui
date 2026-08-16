use jira_application::{
    ApplicationError, ErrorKind, NotificationPort, NotificationRequest, PortFuture,
};
use jira_domain::UpdateEvent;
#[cfg(any(target_os = "linux", test))]
use jira_domain::UpdateKind;

#[cfg(target_os = "linux")]
const APP_NAME: &str = "Jira Desk";
#[cfg(target_os = "linux")]
const APP_ICON: &str = "dev.jiradesk.JiraDesk";
const FAILURE_MESSAGE: &str = "desktop notification unavailable";

#[derive(Debug, Default, Clone, Copy)]
pub struct FreedesktopNotificationPort;

impl NotificationPort for FreedesktopNotificationPort {
    fn deliver<'a>(&'a self, request: NotificationRequest) -> PortFuture<'a, ()> {
        Box::pin(async move { deliver_notification(request.event).await })
    }
}

#[cfg(any(target_os = "linux", test))]
fn notification_content(event: &UpdateEvent) -> (String, String) {
    (
        event.issue_key.to_string(),
        event_kind_label(&event.kind).to_owned(),
    )
}

#[cfg(any(target_os = "linux", test))]
fn event_kind_label(kind: &UpdateKind) -> &'static str {
    match kind {
        UpdateKind::IssueAddedToView => "Issue added",
        UpdateKind::IssueRemovedFromView => "Issue removed",
        UpdateKind::StatusChanged { .. } => "Status changed",
        UpdateKind::AssigneeChanged { .. } => "Assignee changed",
        UpdateKind::PriorityChanged { .. } => "Priority changed",
        UpdateKind::DueDateChanged { .. } => "Due date changed",
        UpdateKind::SummaryChanged { .. } => "Summary changed",
        UpdateKind::ParentChanged { .. } => "Parent changed",
        UpdateKind::CommentAdded { .. } => "Comment added",
    }
}

fn notification_error() -> ApplicationError {
    ApplicationError::new(ErrorKind::Notification, FAILURE_MESSAGE)
}

#[cfg(target_os = "linux")]
async fn deliver_notification(event: UpdateEvent) -> Result<(), ApplicationError> {
    let (summary, body) = notification_content(&event);
    notify_rust::Notification::new()
        .appname(APP_NAME)
        .summary(&summary)
        .body(&body)
        .icon(APP_ICON)
        .show_async()
        .await
        .map(|_| ())
        .map_err(|_| notification_error())
}

#[cfg(not(target_os = "linux"))]
async fn deliver_notification(_event: UpdateEvent) -> Result<(), ApplicationError> {
    Err(notification_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jira_domain::{AccountId, ChangeValue, EventId, IssueId, IssueKey, JiraSiteId, UserSetId};

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
    fn maps_every_kind_to_constant_safe_content() {
        let change = ChangeValue::Text("<script>secret-summary</script>".into());
        let cases = vec![
            (UpdateKind::IssueAddedToView, "Issue added"),
            (UpdateKind::IssueRemovedFromView, "Issue removed"),
            (
                UpdateKind::StatusChanged {
                    old: change.clone(),
                    new: change.clone(),
                },
                "Status changed",
            ),
            (
                UpdateKind::AssigneeChanged {
                    old: change.clone(),
                    new: change.clone(),
                },
                "Assignee changed",
            ),
            (
                UpdateKind::PriorityChanged {
                    old: change.clone(),
                    new: change.clone(),
                },
                "Priority changed",
            ),
            (
                UpdateKind::DueDateChanged {
                    old: change.clone(),
                    new: change.clone(),
                },
                "Due date changed",
            ),
            (
                UpdateKind::SummaryChanged {
                    old: change.clone(),
                    new: change.clone(),
                },
                "Summary changed",
            ),
            (
                UpdateKind::ParentChanged {
                    old: change.clone(),
                    new: change.clone(),
                },
                "Parent changed",
            ),
            (
                UpdateKind::CommentAdded {
                    comment_id: "comment-secret".into(),
                    author: Some(AccountId::new("author-secret").unwrap()),
                    excerpt: "comment-secret <img>".into(),
                },
                "Comment added",
            ),
        ];
        for (kind, expected) in cases {
            let (summary, body) = notification_content(&event(kind));
            assert_eq!(summary, "PROJ-1");
            assert_eq!(body, expected);
            assert!(!body.contains("secret"));
            assert!(!body.contains("<"));
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_delivery_is_redacted_unavailable() {
        use jira_application::NotificationPort;
        let request = NotificationRequest {
            event: event(UpdateKind::IssueAddedToView),
        };
        let result = futures_lite::future::block_on(FreedesktopNotificationPort.deliver(request));
        let error = result.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Notification);
        assert_eq!(error.message(), FAILURE_MESSAGE);
    }
}
