use std::sync::Arc;

use jira_domain::IssueComment;

use crate::{AddCommentRequest, ApplicationError, CancellationToken, JiraCommentWritePort};

pub const MAX_COMMENT_CHARS: usize = 10_000;
pub const MAX_COMMENT_BYTES: usize = 64 * 1024;

/// Application orchestration for the sole permitted Jira write.
#[derive(Clone)]
pub struct CommentService {
    writer: Arc<dyn JiraCommentWritePort>,
}

impl CommentService {
    pub fn new(writer: Arc<dyn JiraCommentWritePort>) -> Self {
        Self { writer }
    }

    /// Validate and dispatch one already-confirmed comment creation. This
    /// method deliberately performs no retry: a transport failure can follow
    /// a committed Jira write.
    pub async fn create(
        &self,
        request: AddCommentRequest,
        cancellation: &CancellationToken,
    ) -> Result<IssueComment, ApplicationError> {
        validate_body(&request.body)?;
        cancellation.check()?;
        self.writer.create_comment(&request, cancellation).await
    }
}

fn validate_body(body: &str) -> Result<(), ApplicationError> {
    if body.trim().is_empty() {
        return Err(ApplicationError::invalid_input(
            "comment body must not be empty",
        ));
    }
    if body.len() > MAX_COMMENT_BYTES {
        return Err(ApplicationError::invalid_input(
            "comment body exceeds the byte limit",
        ));
    }
    if body.chars().count() > MAX_COMMENT_CHARS {
        return Err(ApplicationError::invalid_input(
            "comment body exceeds the character limit",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        sync::{Arc, Mutex},
        task::{Context, Poll, Wake, Waker},
    };

    use jira_domain::{
        AccountId, IssueComment, IssueCommentAuthor, IssueKey, JiraSiteId, Timestamp,
    };

    use super::*;
    use crate::IssueLocator;

    #[derive(Clone)]
    struct FakeWriter {
        calls: Arc<Mutex<usize>>,
        result: Arc<Mutex<Result<IssueComment, ApplicationError>>>,
        observed: Arc<Mutex<Vec<AddCommentRequest>>>,
    }

    impl FakeWriter {
        fn success() -> Self {
            let account = AccountId::new("account").expect("account");
            let author = IssueCommentAuthor::new(account, Some("Asha")).expect("author");
            let comment = IssueComment::new(
                "comment",
                Some(author),
                "created",
                Timestamp::now_utc(),
                None,
                Vec::new(),
            )
            .expect("comment");
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: Arc::new(Mutex::new(Ok(comment))),
                observed: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_error(error: ApplicationError) -> Self {
            let writer = Self::success();
            *writer.result.lock().expect("result lock") = Err(error);
            writer
        }

        fn calls(&self) -> usize {
            *self.calls.lock().expect("calls lock")
        }
    }

    impl JiraCommentWritePort for FakeWriter {
        fn create_comment<'a>(
            &'a self,
            request: &'a AddCommentRequest,
            _cancellation: &'a CancellationToken,
        ) -> crate::PortFuture<'a, IssueComment> {
            *self.calls.lock().expect("calls lock") += 1;
            self.observed
                .lock()
                .expect("observed lock")
                .push(request.clone());
            let result = self.result.lock().expect("result lock").clone();
            Box::pin(async move { result })
        }
    }

    fn request(body: &str) -> AddCommentRequest {
        AddCommentRequest {
            site_id: JiraSiteId::new("site").expect("site"),
            locator: IssueLocator::Key(IssueKey::new("IX-123").expect("key")),
            body: body.to_owned(),
        }
    }

    #[test]
    fn rejects_blank_body_before_calling_port() {
        let writer = FakeWriter::success();
        let service = CommentService::new(Arc::new(writer.clone()));

        let error = block_on(service.create(request("  \n\t"), &CancellationToken::new()))
            .expect_err("blank body");

        assert_eq!(error.kind(), crate::ErrorKind::InvalidInput);
        assert_eq!(writer.calls(), 0);
    }

    #[test]
    fn enforces_character_and_byte_limits_before_calling_port() {
        let writer = FakeWriter::success();
        let service = CommentService::new(Arc::new(writer.clone()));

        let character_error = block_on(service.create(
            request(&"x".repeat(MAX_COMMENT_CHARS + 1)),
            &CancellationToken::new(),
        ))
        .expect_err("character limit");
        assert_eq!(character_error.kind(), crate::ErrorKind::InvalidInput);

        let byte_error = block_on(service.create(
            request(&"😀".repeat(MAX_COMMENT_BYTES / 4 + 1)),
            &CancellationToken::new(),
        ))
        .expect_err("byte limit");
        assert_eq!(byte_error.kind(), crate::ErrorKind::InvalidInput);
        assert_eq!(writer.calls(), 0);
    }

    #[test]
    fn cancelled_request_never_calls_port() {
        let writer = FakeWriter::success();
        let service = CommentService::new(Arc::new(writer.clone()));
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = block_on(service.create(request("hello"), &cancellation))
            .expect_err("cancelled request");

        assert_eq!(error.kind(), crate::ErrorKind::Cancelled);
        assert_eq!(writer.calls(), 0);
    }

    #[test]
    fn successful_request_calls_port_once_and_returns_comment() {
        let writer = FakeWriter::success();
        let service = CommentService::new(Arc::new(writer.clone()));

        let comment = block_on(service.create(request("hello"), &CancellationToken::new()))
            .expect("created comment");

        assert_eq!(comment.body, "created");
        assert_eq!(writer.calls(), 1);
    }

    #[test]
    fn port_errors_are_returned_unchanged_without_retry() {
        let errors = [
            ApplicationError::new(crate::ErrorKind::Authentication, "auth"),
            ApplicationError::new(crate::ErrorKind::Authorization, "permission"),
            ApplicationError::new(crate::ErrorKind::NotFound, "missing"),
            ApplicationError::rate_limited("slow down", None),
            ApplicationError::new(crate::ErrorKind::Offline, "offline"),
            ApplicationError::new(crate::ErrorKind::UnknownOutcome, "check Jira"),
        ];

        for expected in errors {
            let writer = FakeWriter::with_error(expected.clone());
            let service = CommentService::new(Arc::new(writer.clone()));
            let error = block_on(service.create(request("hello"), &CancellationToken::new()))
                .expect_err("port error");

            assert_eq!(error, expected);
            assert_eq!(writer.calls(), 1);
        }
    }

    #[test]
    fn request_keeps_locator_and_plain_text_body_unchanged() {
        let writer = FakeWriter::success();
        let service = CommentService::new(Arc::new(writer.clone()));
        let request = request("  hello  ");

        block_on(service.create(request.clone(), &CancellationToken::new())).expect("created");

        assert_eq!(writer.observed.lock().expect("observed lock")[0], request);
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }

        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }
}
