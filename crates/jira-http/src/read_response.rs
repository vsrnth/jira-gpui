use std::time::Duration;

use jira_application::{ApplicationError, ErrorKind};
use reqwest::{Response, StatusCode, header};
use serde::de::DeserializeOwned;

#[derive(Debug, Eq, PartialEq)]
enum BodyReadFailure {
    Read,
    TooLarge,
}

pub(super) async fn read_json<T: DeserializeOwned>(
    response: Response,
    max_bytes: usize,
) -> Result<T, ApplicationError> {
    let body = read_body(response, max_bytes).await?;
    serde_json::from_slice(&body)
        .map_err(|_| ApplicationError::new(ErrorKind::Upstream, "Jira returned malformed JSON"))
}

pub(super) async fn read_body(
    response: Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ApplicationError> {
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(status, response.headers()));
    }
    collect_bounded_body(response, max_bytes)
        .await
        .map_err(|failure| match failure {
            BodyReadFailure::Read => {
                ApplicationError::new(ErrorKind::Offline, "could not read Jira response")
            }
            BodyReadFailure::TooLarge => {
                ApplicationError::new(ErrorKind::Upstream, "Jira response exceeded the size limit")
            }
        })
}

async fn collect_bounded_body(
    response: Response,
    max_bytes: usize,
) -> Result<Vec<u8>, BodyReadFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(BodyReadFailure::TooLarge);
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| BodyReadFailure::Read)? {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(BodyReadFailure::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) fn transport_error(error: reqwest::Error) -> ApplicationError {
    if error.is_timeout() || error.is_connect() {
        ApplicationError::new(ErrorKind::Offline, "could not connect to Jira")
    } else {
        ApplicationError::new(ErrorKind::Upstream, "Jira request failed")
    }
}

pub(super) fn status_error(status: StatusCode, headers: &header::HeaderMap) -> ApplicationError {
    match status {
        StatusCode::UNAUTHORIZED => {
            ApplicationError::new(ErrorKind::Authentication, "Jira authentication failed")
        }
        StatusCode::FORBIDDEN => {
            ApplicationError::new(ErrorKind::Authorization, "Jira authorization was denied")
        }
        StatusCode::NOT_FOUND => {
            ApplicationError::new(ErrorKind::NotFound, "Jira resource was not found")
        }
        StatusCode::TOO_MANY_REQUESTS => {
            ApplicationError::rate_limited("Jira rate limit exceeded", retry_after(headers))
        }
        _ => ApplicationError::new(
            ErrorKind::Upstream,
            "Jira returned an unsuccessful response",
        ),
    }
}

pub(super) fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}
