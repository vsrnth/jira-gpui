#[cfg(any(target_os = "linux", test))]
use std::collections::VecDeque;
#[cfg(target_os = "linux")]
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

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

#[cfg(any(target_os = "linux", test))]
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

/// A private handle abstraction lets deterministic tests exercise retention
/// without constructing a live D-Bus handle.
#[cfg(target_os = "linux")]
trait RetainedNotificationHandle: Send + std::fmt::Debug {}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct NotifyRustHandle {
    // The handle is intentionally held for its Drop/lifetime behavior; callers
    // only need the daemon ID, not access to this private backend value.
    #[allow(dead_code)]
    handle: notify_rust::NotificationHandle,
}

#[cfg(target_os = "linux")]
impl RetainedNotificationHandle for NotifyRustHandle {}

/// A strictly bounded FIFO retention queue. Retaining notification handles is
/// required by some desktop environments to keep the D-Bus connection alive;
/// evicting the oldest handle prevents an unbounded connection leak.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
struct BoundedRetention<T> {
    capacity: usize,
    items: VecDeque<T>,
}

#[cfg(any(target_os = "linux", test))]
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

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct BackendNotification {
    notification_id: u32,
    handle: Box<dyn RetainedNotificationHandle>,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct BackendError;

#[cfg(target_os = "linux")]
type BackendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<BackendNotification, BackendError>> + Send + 'a>>;

#[cfg(target_os = "linux")]
trait NotificationBackend: Send + Sync + std::fmt::Debug {
    fn show<'a>(&'a self, notification: notify_rust::Notification) -> BackendFuture<'a>;
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct NotifyRustBackend;

#[cfg(target_os = "linux")]
impl NotificationBackend for NotifyRustBackend {
    fn show<'a>(&'a self, notification: notify_rust::Notification) -> BackendFuture<'a> {
        Box::pin(async move {
            let handle = notification.show_async().await.map_err(|_| BackendError)?;
            Ok(BackendNotification {
                notification_id: handle.id(),
                handle: Box::new(NotifyRustHandle { handle }),
            })
        })
    }
}

#[cfg_attr(not(target_os = "linux"), derive(Default))]
#[derive(Debug, Clone)]
pub struct FreedesktopNotificationPort {
    #[cfg(target_os = "linux")]
    retained_handles: Arc<Mutex<BoundedRetention<Box<dyn RetainedNotificationHandle>>>>,
    #[cfg(target_os = "linux")]
    backend: Arc<dyn NotificationBackend>,
}

#[cfg(target_os = "linux")]
impl Default for FreedesktopNotificationPort {
    fn default() -> Self {
        Self {
            retained_handles: Arc::new(Mutex::new(BoundedRetention::new(MAX_RETAINED_HANDLES))),
            backend: Arc::new(NotifyRustBackend),
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
    fn retain_handle(&self, handle: Box<dyn RetainedNotificationHandle>) {
        let mut retained_handles = match self.retained_handles.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        retained_handles.retain(handle);
    }

    #[cfg(all(test, target_os = "linux"))]
    fn with_backend(backend: Arc<dyn NotificationBackend>) -> Self {
        Self {
            retained_handles: Arc::new(Mutex::new(BoundedRetention::new(MAX_RETAINED_HANDLES))),
            backend,
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    fn retained_handle_count(&self) -> usize {
        let retained_handles = match self.retained_handles.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        retained_handles.len()
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
            let result = self
                .backend
                .show(event_notification(&event))
                .await
                .map_err(|_| notification_error())?;
            self.retain_handle(result.handle);
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
            let result = self
                .backend
                .show(test_notification())
                .await
                .map_err(|_| notification_error())?;
            let receipt = DesktopNotificationReceipt {
                notification_id: result.notification_id,
            };
            self.retain_handle(result.handle);
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

    #[cfg(target_os = "linux")]
    #[derive(Debug)]
    struct TestHandle {
        id: u32,
        dropped_ids: Arc<Mutex<Vec<u32>>>,
    }

    #[cfg(target_os = "linux")]
    impl TestHandle {
        fn new(id: u32, dropped_ids: Arc<Mutex<Vec<u32>>>) -> Self {
            Self { id, dropped_ids }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for TestHandle {
        fn drop(&mut self) {
            let mut dropped_ids = self
                .dropped_ids
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            dropped_ids.push(self.id);
        }
    }

    #[cfg(target_os = "linux")]
    impl RetainedNotificationHandle for TestHandle {}

    #[cfg(target_os = "linux")]
    #[derive(Debug)]
    struct TestBackend {
        calls: Arc<AtomicUsize>,
        result: Mutex<Option<Result<BackendNotification, BackendError>>>,
    }

    #[cfg(target_os = "linux")]
    impl NotificationBackend for TestBackend {
        fn show<'a>(&'a self, _notification: notify_rust::Notification) -> BackendFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                let mut result = match self.result.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                result
                    .take()
                    .unwrap_or_else(|| panic!("test backend called more than once"))
            })
        }
    }

    #[cfg(target_os = "linux")]
    fn successful_backend_notification(
        notification_id: u32,
        dropped_ids: Arc<Mutex<Vec<u32>>>,
    ) -> BackendNotification {
        BackendNotification {
            notification_id,
            handle: Box::new(TestHandle::new(notification_id, dropped_ids)),
        }
    }

    #[cfg(target_os = "linux")]
    fn snapshot_drop_ids(dropped_ids: &Arc<Mutex<Vec<u32>>>) -> Vec<u32> {
        dropped_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn backend_success_through_notification_port_is_one_call_and_retains_handle() {
        let calls = Arc::new(AtomicUsize::new(0));
        let dropped_ids = Arc::new(Mutex::new(Vec::new()));
        let backend = Arc::new(TestBackend {
            calls: calls.clone(),
            result: Mutex::new(Some(Ok(successful_backend_notification(
                41,
                dropped_ids.clone(),
            )))),
        });
        let port = FreedesktopNotificationPort::with_backend(backend);
        let request = NotificationRequest {
            event: event(UpdateKind::IssueUpdated),
        };

        let result = futures_lite::future::block_on(
            <FreedesktopNotificationPort as jira_application::NotificationPort>::deliver(
                &port, request,
            ),
        );

        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(port.retained_handle_count(), 1);
        assert!(snapshot_drop_ids(&dropped_ids).is_empty());
        drop(port);
        assert_eq!(snapshot_drop_ids(&dropped_ids), vec![41]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn backend_failure_through_notification_port_is_redacted_and_not_retried() {
        let calls = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(TestBackend {
            calls: calls.clone(),
            result: Mutex::new(Some(Err(BackendError))),
        });
        let port = FreedesktopNotificationPort::with_backend(backend);
        let request = NotificationRequest {
            event: event(UpdateKind::IssueAddedToView),
        };

        let error = futures_lite::future::block_on(
            <FreedesktopNotificationPort as jira_application::NotificationPort>::deliver(
                &port, request,
            ),
        )
        .expect_err("test backend should fail");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(error.kind(), ErrorKind::Notification);
        assert_eq!(error.message(), FAILURE_MESSAGE);
        assert_eq!(port.retained_handle_count(), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn backend_success_returns_daemon_id_for_test_notification() {
        let calls = Arc::new(AtomicUsize::new(0));
        let dropped_ids = Arc::new(Mutex::new(Vec::new()));
        let backend = Arc::new(TestBackend {
            calls: calls.clone(),
            result: Mutex::new(Some(Ok(successful_backend_notification(
                41,
                dropped_ids.clone(),
            )))),
        });
        let port = FreedesktopNotificationPort::with_backend(backend);

        let receipt = futures_lite::future::block_on(port.test_notification())
            .expect("test backend should succeed");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(receipt.notification_id(), 41);
        assert_eq!(port.retained_handle_count(), 1);
        assert!(snapshot_drop_ids(&dropped_ids).is_empty());
        drop(port);
        assert_eq!(snapshot_drop_ids(&dropped_ids), vec![41]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn port_retention_evicts_oldest_handle_and_keeps_newer_handles() {
        let dropped_ids = Arc::new(Mutex::new(Vec::new()));
        let port = FreedesktopNotificationPort::new();

        for id in 0..MAX_RETAINED_HANDLES as u32 {
            port.retain_handle(Box::new(TestHandle::new(id, dropped_ids.clone())));
        }
        assert_eq!(port.retained_handle_count(), MAX_RETAINED_HANDLES);
        assert!(snapshot_drop_ids(&dropped_ids).is_empty());

        port.retain_handle(Box::new(TestHandle::new(
            MAX_RETAINED_HANDLES as u32,
            dropped_ids.clone(),
        )));
        assert_eq!(port.retained_handle_count(), MAX_RETAINED_HANDLES);
        assert_eq!(snapshot_drop_ids(&dropped_ids), vec![0]);

        drop(port);
        let mut all_dropped = snapshot_drop_ids(&dropped_ids);
        all_dropped.sort_unstable();
        assert_eq!(
            all_dropped,
            (0..=MAX_RETAINED_HANDLES as u32).collect::<Vec<_>>()
        );
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
        let error = result.expect_err("non-Linux delivery should be unavailable");
        assert_eq!(error.kind(), ErrorKind::Notification);
        assert_eq!(error.message(), FAILURE_MESSAGE);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_test_notification_is_redacted_unavailable() {
        let port = FreedesktopNotificationPort::new();
        let result = futures_lite::future::block_on(port.test_notification());
        let error = result.expect_err("non-Linux test notification should be unavailable");
        assert_eq!(error.kind(), ErrorKind::Notification);
        assert_eq!(error.message(), FAILURE_MESSAGE);
    }
}
