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
