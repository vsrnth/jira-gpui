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
#[path = "lib_tests.rs"]
mod tests;
