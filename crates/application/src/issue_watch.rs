use std::sync::Arc;

use crate::issue_edit::validate_locator_request;
use crate::{
    ApplicationError, CancellationToken, IssueWatchRequest, JiraIssueWatchPort,
    SetIssueWatchingRequest,
};

/// Application service for reading and changing the authenticated account's
/// watcher state on an issue.
#[derive(Clone)]
pub struct IssueWatchService {
    watcher: Arc<dyn JiraIssueWatchPort>,
}

impl IssueWatchService {
    pub fn new(watcher: Arc<dyn JiraIssueWatchPort>) -> Self {
        Self { watcher }
    }

    /// Read whether the authenticated Jira account watches the issue.
    pub async fn watch_state(
        &self,
        request: IssueWatchRequest,
        cancellation: &CancellationToken,
    ) -> Result<bool, ApplicationError> {
        validate_locator_request(&request.site_id, &request.locator)?;
        cancellation.check()?;
        let watching = self
            .watcher
            .fetch_issue_watch_state(&request, cancellation)
            .await?;
        cancellation.check()?;
        Ok(watching)
    }

    /// Dispatch one already-confirmed watch/unwatch request. This deliberately
    /// does not retry because Jira may have accepted the write before a response
    /// failure made its outcome unknown.
    pub async fn set_watching(
        &self,
        request: SetIssueWatchingRequest,
        cancellation: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        validate_locator_request(&request.site_id, &request.locator)?;
        cancellation.check()?;
        self.watcher
            .set_issue_watching(&request, cancellation)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use jira_domain::{IssueKey, JiraSiteId};

    use super::*;
    use crate::{ErrorKind, IssueLocator, test_support::block_on};

    #[derive(Clone)]
    struct FakeWatcher {
        read_calls: Arc<Mutex<usize>>,
        write_calls: Arc<Mutex<Vec<SetIssueWatchingRequest>>>,
        state: bool,
        write_result: Result<(), ApplicationError>,
    }

    impl JiraIssueWatchPort for FakeWatcher {
        fn fetch_issue_watch_state<'a>(
            &'a self,
            _request: &'a IssueWatchRequest,
            _cancellation: &'a CancellationToken,
        ) -> crate::PortFuture<'a, bool> {
            *self.read_calls.lock().expect("read lock") += 1;
            let state = self.state;
            Box::pin(async move { Ok(state) })
        }

        fn set_issue_watching<'a>(
            &'a self,
            request: &'a SetIssueWatchingRequest,
            _cancellation: &'a CancellationToken,
        ) -> crate::PortFuture<'a, ()> {
            self.write_calls
                .lock()
                .expect("write lock")
                .push(request.clone());
            let result = self.write_result.clone();
            Box::pin(async move { result })
        }
    }

    fn watch_request() -> IssueWatchRequest {
        IssueWatchRequest {
            site_id: JiraSiteId::new("site").expect("site"),
            locator: IssueLocator::Key(IssueKey::new("APP-1").expect("key")),
        }
    }

    fn watcher(write_result: Result<(), ApplicationError>) -> FakeWatcher {
        FakeWatcher {
            read_calls: Arc::new(Mutex::new(0)),
            write_calls: Arc::new(Mutex::new(Vec::new())),
            state: true,
            write_result,
        }
    }

    #[test]
    fn returns_authenticated_watch_state_from_read_port() {
        let watcher = watcher(Ok(()));
        let service = IssueWatchService::new(Arc::new(watcher.clone()));

        assert!(
            block_on(service.watch_state(watch_request(), &CancellationToken::new()))
                .expect("watch state")
        );
        assert_eq!(*watcher.read_calls.lock().expect("read lock"), 1);
        assert!(watcher.write_calls.lock().expect("write lock").is_empty());
    }

    #[test]
    fn confirmed_watch_and_unwatch_each_dispatch_once() {
        let watcher = watcher(Ok(()));
        let service = IssueWatchService::new(Arc::new(watcher.clone()));
        let base = watch_request();

        for watching in [true, false] {
            block_on(service.set_watching(
                SetIssueWatchingRequest {
                    site_id: base.site_id.clone(),
                    locator: base.locator.clone(),
                    watching,
                },
                &CancellationToken::new(),
            ))
            .expect("watch change");
        }

        let calls = watcher.write_calls.lock().expect("write lock");
        assert_eq!(calls.len(), 2);
        assert!(calls[0].watching);
        assert!(!calls[1].watching);
    }

    #[test]
    fn unknown_write_outcome_is_returned_after_one_dispatch() {
        let expected = ApplicationError::new(ErrorKind::UnknownOutcome, "check Jira");
        let watcher = watcher(Err(expected.clone()));
        let service = IssueWatchService::new(Arc::new(watcher.clone()));
        let request = watch_request();

        let error = block_on(service.set_watching(
            SetIssueWatchingRequest {
                site_id: request.site_id,
                locator: request.locator,
                watching: false,
            },
            &CancellationToken::new(),
        ))
        .expect_err("unknown outcome");

        assert_eq!(error, expected);
        assert_eq!(watcher.write_calls.lock().expect("write lock").len(), 1);
    }

    #[test]
    fn cancellation_prevents_read_and_write_dispatch() {
        let watcher = watcher(Ok(()));
        let service = IssueWatchService::new(Arc::new(watcher.clone()));
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert_eq!(
            block_on(service.watch_state(watch_request(), &cancellation))
                .expect_err("cancelled read")
                .kind(),
            ErrorKind::Cancelled
        );
        let request = watch_request();
        assert_eq!(
            block_on(service.set_watching(
                SetIssueWatchingRequest {
                    site_id: request.site_id,
                    locator: request.locator,
                    watching: true,
                },
                &cancellation,
            ))
            .expect_err("cancelled write")
            .kind(),
            ErrorKind::Cancelled
        );
        assert_eq!(*watcher.read_calls.lock().expect("read lock"), 0);
        assert!(watcher.write_calls.lock().expect("write lock").is_empty());
    }

    #[test]
    fn invalid_issue_identity_prevents_read_and_write_dispatch() {
        let watcher = watcher(Ok(()));
        let service = IssueWatchService::new(Arc::new(watcher.clone()));
        let request = IssueWatchRequest {
            site_id: JiraSiteId::new("site").expect("site"),
            locator: IssueLocator::Id(
                jira_domain::IssueId::new("bad\nidentity").expect("typed id"),
            ),
        };

        assert_eq!(
            block_on(service.watch_state(request.clone(), &CancellationToken::new()))
                .expect_err("invalid issue identity")
                .kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(
            block_on(service.set_watching(
                SetIssueWatchingRequest {
                    site_id: request.site_id,
                    locator: request.locator,
                    watching: true,
                },
                &CancellationToken::new(),
            ))
            .expect_err("invalid issue identity")
            .kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(*watcher.read_calls.lock().expect("read lock"), 0);
        assert!(watcher.write_calls.lock().expect("write lock").is_empty());
    }
}
