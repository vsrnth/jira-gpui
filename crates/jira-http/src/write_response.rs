use std::sync::Arc;

use jira_application::{ApplicationError, CancellationToken, ErrorKind, PortFuture};
use jira_domain::IssueComment;
use reqwest::{Response, StatusCode, header};

use super::{read_response, runtime_bridge::RuntimeBridge};

pub(super) fn submit_write<T, F>(
    runtime: Arc<RuntimeBridge>,
    cancellation: &CancellationToken,
    operation: F,
) -> PortFuture<'static, T>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, ApplicationError>> + Send + 'static,
{
    if let Err(error) = cancellation.check() {
        return Box::pin(std::future::ready(Err(error)));
    }
    Box::pin(async move {
        runtime
            .dispatch(operation)
            .await
            .map_err(write_dispatch_error)?
    })
}

pub(super) async fn read_write_response(response: Response) -> Result<(), ApplicationError> {
    let status = response.status();
    if status == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err(write_status_error(status, response.headers()))
    }
}

pub(super) fn comment_transport_error(error: reqwest::Error) -> ApplicationError {
    write_transport_error(error)
}

pub(super) fn write_dispatch_error(_error: ApplicationError) -> ApplicationError {
    write_unknown_outcome()
}

pub(super) fn comment_status_error(
    status: StatusCode,
    headers: &header::HeaderMap,
) -> ApplicationError {
    write_status_error(status, headers)
}

pub(super) fn write_transport_error(error: reqwest::Error) -> ApplicationError {
    if error.is_connect() && !error.is_timeout() {
        ApplicationError::new(ErrorKind::Offline, "could not connect to Jira")
    } else {
        write_unknown_outcome()
    }
}

pub(super) fn write_unknown_outcome() -> ApplicationError {
    ApplicationError::new(ErrorKind::UnknownOutcome, "Jira write outcome is unknown")
}

pub(super) fn write_status_error(
    status: StatusCode,
    headers: &header::HeaderMap,
) -> ApplicationError {
    match status {
        StatusCode::BAD_REQUEST
        | StatusCode::PAYLOAD_TOO_LARGE
        | StatusCode::UNPROCESSABLE_ENTITY => {
            ApplicationError::invalid_input("Jira rejected the write request")
        }
        StatusCode::UNAUTHORIZED => {
            ApplicationError::new(ErrorKind::Authentication, "Jira authentication failed")
        }
        StatusCode::FORBIDDEN => {
            ApplicationError::new(ErrorKind::Authorization, "Jira authorization was denied")
        }
        StatusCode::NOT_FOUND => {
            ApplicationError::new(ErrorKind::NotFound, "Jira issue was not found")
        }
        StatusCode::CONFLICT => ApplicationError::new(
            ErrorKind::Upstream,
            "Jira rejected the write due to a conflict",
        ),
        StatusCode::TOO_MANY_REQUESTS => ApplicationError::rate_limited(
            "Jira rate limit exceeded",
            read_response::retry_after(headers),
        ),
        _ => write_unknown_outcome(),
    }
}

pub(super) async fn read_created_comment(
    response: Response,
    max_bytes: usize,
) -> Result<IssueComment, ApplicationError> {
    let status = response.status();
    if status != StatusCode::CREATED {
        return Err(comment_status_error(status, response.headers()));
    }
    let mut response = response;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(write_unknown_outcome());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| write_unknown_outcome())?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(write_unknown_outcome());
        }
        body.extend_from_slice(&chunk);
    }
    map_created_comment_body(&body)
}

pub(super) fn map_created_comment_body(body: &[u8]) -> Result<IssueComment, ApplicationError> {
    jira_adapter::decode_created_comment_response(body).map_err(|_| write_unknown_outcome())
}
