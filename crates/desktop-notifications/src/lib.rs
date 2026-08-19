use std::collections::VecDeque;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};

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
#[cfg(target_os = "linux")]
const APP_DESKTOP_ENTRY: &str = "dev.jiradesk.JiraDesk";
const FAILURE_MESSAGE: &str = "desktop notification unavailable";
pub const TEST_NOTIFICATION_SUMMARY: &str = "Jira Desk notification test";
pub const TEST_NOTIFICATION_BODY: &str =
    "If this appears, Jira Desk desktop notifications are working.";

const MAX_RETAINED_HANDLES: usize = 32;

/// A local receipt from the Freedesktop notification service. This is not a
/// Jira or database identifier; it is the daemon-assigned notification ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopNotificationReceipt {
    notification_id: u32,
}

impl DesktopNotificationReceipt {
    pub const fn notification_id(self) -> u32 {
        self.notification_id
    }
}

/// A strictly bounded FIFO retention queue. Retaining notification handles is
/// required by some desktop environments to keep the D-Bus connection alive;
/// evicting the oldest handle prevents an unbounded connection leak.
#[derive(Debug)]
struct BoundedRetention<T> {
    capacity: usize,
    items: VecDeque<T>,
}

impl<T> BoundedRetention<T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: VecDeque::with_capacity(capacity),
        }
    }

    fn retain(&mut self, item: T) {
        if self.capacity == 0 {
            return;
        }
        if self.items.len() == self.capacity {
            let _ = self.items.pop_front();
        }
        self.items.push_back(item);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.items.len()
    }
}

#[derive(Debug, Clone)]
pub struct FreedesktopNotificationPort {
    #[cfg(target_os = "linux")]
    retained_handles: Arc<Mutex<BoundedRetention<notify_rust::NotificationHandle>>>,
}

impl Default for FreedesktopNotificationPort {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            retained_handles: Arc::new(Mutex::new(BoundedRetention::new(MAX_RETAINED_HANDLES))),
        }
    }
}

impl NotificationPort for FreedesktopNotificationPort {
    fn deliver<'a>(&'a self, request: NotificationRequest) -> PortFuture<'a, ()> {
        Box::pin(async move { self.deliver_notification(request.event).await })
    }
}

impl FreedesktopNotificationPort {
    pub fn new() -> Self {
        Self::default()
    }

    /// Send a fixed local diagnostic notification without contacting Jira or
    /// mutating the local cache. The daemon ID is returned only after the
    /// Freedesktop service accepts the request.
    pub fn test_notification<'a>(&'a self) -> PortFuture<'a, DesktopNotificationReceipt> {
        Box::pin(async move { self.send_test_notification().await })
    }

    #[cfg(target_os = "linux")]
    fn retain_handle(&self, handle: notify_rust::NotificationHandle) {
        let mut retained_handles = match self.retained_handles.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        retained_handles.retain(handle);
    }
}

#[cfg(any(target_os = "linux", test))]
fn notification_content(event: &UpdateEvent) -> (String, String) {
    let body = match &event.kind {
        UpdateKind::FieldChanged { field, .. } => format!("Field changed: {field}"),
        UpdateKind::AssigneeChanged { .. } => "Ticket assigned to you".to_owned(),
        UpdateKind::CommentAdded { .. } => "You were mentioned in a comment".to_owned(),
        _ => event_kind_label(&event.kind).to_owned(),
    };
    (event.issue_key.to_string(), body)
}

#[cfg(any(target_os = "linux", test))]
fn event_kind_label(kind: &UpdateKind) -> &'static str {
    match kind {
        UpdateKind::IssueAddedToView => "Issue added",
        UpdateKind::IssueRemovedFromView => "Issue removed",
        UpdateKind::IssueUpdated => "Issue updated",
        UpdateKind::StatusChanged { .. } => "Status changed",
        UpdateKind::AssigneeChanged { .. } => "Assignee changed",
        UpdateKind::PriorityChanged { .. } => "Priority changed",
        UpdateKind::DueDateChanged { .. } => "Due date changed",
        UpdateKind::SummaryChanged { .. } => "Summary changed",
        UpdateKind::ParentChanged { .. } => "Parent changed",
        UpdateKind::CommentAdded { .. } => "Comment added",
        UpdateKind::FieldChanged { .. } => "Field changed",
    }
}

fn notification_error() -> ApplicationError {
    ApplicationError::new(ErrorKind::Notification, FAILURE_MESSAGE)
}

#[cfg(target_os = "linux")]
fn event_notification(event: &UpdateEvent) -> notify_rust::Notification {
    let (summary, body) = notification_content(event);
    let mut notification = notify_rust::Notification::new();
    notification
        .appname(APP_NAME)
        .summary(&summary)
        .body(&body)
        .icon(APP_ICON)
        .hint(notify_rust::Hint::DesktopEntry(
            APP_DESKTOP_ENTRY.to_owned(),
        ))
        .timeout(notify_rust::Timeout::Never);
    notification
}

#[cfg(target_os = "linux")]
fn test_notification() -> notify_rust::Notification {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname(APP_NAME)
        .summary(TEST_NOTIFICATION_SUMMARY)
        .body(TEST_NOTIFICATION_BODY)
        .icon(APP_ICON)
        .hint(notify_rust::Hint::DesktopEntry(
            APP_DESKTOP_ENTRY.to_owned(),
        ))
        .timeout(notify_rust::Timeout::Never);
    notification
}

impl FreedesktopNotificationPort {
    async fn deliver_notification(&self, event: UpdateEvent) -> Result<(), ApplicationError> {
        #[cfg(target_os = "linux")]
        {
            let handle = event_notification(&event)
                .show_async()
                .await
                .map_err(|_| notification_error())?;
            self.retain_handle(handle);
            Ok(())
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (self, event);
            Err(notification_error())
        }
    }

    async fn send_test_notification(&self) -> Result<DesktopNotificationReceipt, ApplicationError> {
        #[cfg(target_os = "linux")]
        {
            let handle = test_notification()
                .show_async()
                .await
                .map_err(|_| notification_error())?;
            let receipt = DesktopNotificationReceipt {
                notification_id: handle.id(),
            };
            self.retain_handle(handle);
            Ok(receipt)
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = self;
            Err(notification_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use jira_domain::{AccountId, ChangeValue, EventId, IssueId, IssueKey, JiraSiteId, UserSetId};

    fn event(kind: UpdateKind) -> UpdateEvent {
        UpdateEvent::new(
            EventId::new("event-1").expect("valid event ID"),
            JiraSiteId::new("site").expect("valid site ID"),
            IssueId::new("issue-1").expect("valid issue ID"),
            IssueKey::new("PROJ-1").expect("valid issue key"),
            kind,
            time::OffsetDateTime::now_utc(),
            vec![UserSetId::new("set-1").expect("valid user-set ID")],
        )
    }

    #[test]
    fn maps_every_kind_to_constant_safe_content() {
        let change = ChangeValue::Text("<script>secret-summary</script>".into());
        let cases = vec![
            (UpdateKind::IssueAddedToView, "Issue added"),
            (UpdateKind::IssueRemovedFromView, "Issue removed"),
            (UpdateKind::IssueUpdated, "Issue updated"),
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
                "Ticket assigned to you",
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
                    author: Some(AccountId::new("author-secret").expect("valid account ID")),
                    excerpt: "comment-secret <img>".into(),
                },
                "You were mentioned in a comment",
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

    #[test]
    fn test_notification_copy_is_fixed_and_privacy_safe() {
        assert_eq!(TEST_NOTIFICATION_SUMMARY, "Jira Desk notification test");
        assert_eq!(
            TEST_NOTIFICATION_BODY,
            "If this appears, Jira Desk desktop notifications are working."
        );
        assert!(!TEST_NOTIFICATION_BODY.contains("jira.atlassian"));
    }

    #[derive(Clone)]
    struct DropCounter(Arc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn retained_handles_are_fifo_bounded_and_live_until_eviction() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut retained = BoundedRetention::new(MAX_RETAINED_HANDLES);

        for _ in 0..MAX_RETAINED_HANDLES {
            retained.retain(DropCounter(drops.clone()));
        }
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(retained.len(), MAX_RETAINED_HANDLES);

        retained.retain(DropCounter(drops.clone()));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(retained.len(), MAX_RETAINED_HANDLES);

        drop(retained);
        assert_eq!(drops.load(Ordering::SeqCst), MAX_RETAINED_HANDLES + 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notification_builders_are_persistent_and_not_transient() {
        let event_notification = event_notification(&event(UpdateKind::IssueUpdated));
        let test_notification = test_notification();

        assert_eq!(event_notification.timeout, notify_rust::Timeout::Never);
        assert_eq!(test_notification.timeout, notify_rust::Timeout::Never);
        assert!(
            !event_notification
                .hints
                .contains(&notify_rust::Hint::Transient(true))
        );
        assert!(
            !test_notification
                .hints
                .contains(&notify_rust::Hint::Transient(true))
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_delivery_is_redacted_unavailable() {
        use jira_application::NotificationPort;
        let request = NotificationRequest {
            event: event(UpdateKind::IssueAddedToView),
        };
        let port = FreedesktopNotificationPort::new();
        let result = futures_lite::future::block_on(port.deliver(request));
        let error = result.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Notification);
        assert_eq!(error.message(), FAILURE_MESSAGE);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_test_notification_is_redacted_unavailable() {
        let port = FreedesktopNotificationPort::new();
        let result = futures_lite::future::block_on(port.test_notification());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Notification);
        assert_eq!(error.message(), FAILURE_MESSAGE);
    }
}
