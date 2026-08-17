use std::time::Duration;

use crate::{AttachmentReadAttempt, AttachmentReadDiagnostic};

/// Stable error categories that presentation adapters can map to UI states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Authentication,
    Authorization,
    RateLimited,
    Offline,
    Cancelled,
    InvalidInput,
    NotFound,
    /// A write may have been accepted even though its response was not observed.
    UnknownOutcome,
    Storage,
    Upstream,
    Notification,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ApplicationError {
    kind: ErrorKind,
    message: String,
    retry_after: Option<Duration>,
    attachment_diagnostic: Option<AttachmentReadDiagnostic>,
}

impl ApplicationError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
            attachment_diagnostic: None,
        }
    }

    pub fn rate_limited(message: impl Into<String>, retry_after: Option<Duration>) -> Self {
        Self {
            kind: ErrorKind::RateLimited,
            message: message.into(),
            retry_after,
            attachment_diagnostic: None,
        }
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "operation cancelled")
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    pub fn attachment_diagnostic(&self) -> Option<AttachmentReadDiagnostic> {
        self.attachment_diagnostic
    }

    pub fn with_attachment_diagnostic(mut self, diagnostic: AttachmentReadDiagnostic) -> Self {
        self.attachment_diagnostic = Some(diagnostic);
        self
    }

    pub fn with_attachment_attempt(mut self, attempt: AttachmentReadAttempt) -> Self {
        self.attachment_diagnostic = self
            .attachment_diagnostic
            .map(|diagnostic| diagnostic.with_attempt(attempt));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttachmentReadStage, AttachmentTransportClass};

    #[test]
    fn attachment_context_preserves_error_semantics_and_overrides_attempt() {
        let diagnostic = AttachmentReadDiagnostic::transport(
            AttachmentReadAttempt::Thumbnail,
            AttachmentTransportClass::TimedOut,
        );
        let error = ApplicationError::rate_limited("slow down", Some(Duration::from_secs(2)))
            .with_attachment_diagnostic(diagnostic)
            .with_attachment_attempt(AttachmentReadAttempt::OriginalFallback);

        assert_eq!(error.kind(), ErrorKind::RateLimited);
        assert_eq!(error.message(), "slow down");
        assert_eq!(error.retry_after(), Some(Duration::from_secs(2)));
        let diagnostic = error.attachment_diagnostic().expect("diagnostic");
        assert_eq!(
            diagnostic.attempt(),
            AttachmentReadAttempt::OriginalFallback
        );
        assert_eq!(diagnostic.stage(), AttachmentReadStage::Transport);
        assert_eq!(
            diagnostic.transport_class(),
            Some(AttachmentTransportClass::TimedOut)
        );
    }

    #[test]
    fn attachment_attempt_does_not_fabricate_context_for_unrelated_errors() {
        let error = ApplicationError::new(ErrorKind::Upstream, "unrelated")
            .with_attachment_attempt(AttachmentReadAttempt::OriginalFallback);

        assert_eq!(error.message(), "unrelated");
        assert_eq!(error.attachment_diagnostic(), None);
    }
}
