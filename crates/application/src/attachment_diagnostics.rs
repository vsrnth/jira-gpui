//! Privacy-safe, application-owned diagnostics for attachment reads.
//!
//! These values intentionally contain only fixed enums and a bounded status code. They must not
//! grow to include URLs, attachment identifiers, headers, or adapter error text.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentReadAttempt {
    Thumbnail,
    OriginalFallback,
    ExplicitDownload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentReadStage {
    Transport,
    Status,
    ContentType,
    Body,
    Validation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentMimeClass {
    Missing,
    Malformed,
    Png,
    Jpeg,
    Gif,
    Webp,
    OctetStream,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentBodyClass {
    Empty,
    TooLarge,
    ReadFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentTransportClass {
    ConnectFailed,
    TimedOut,
    RequestFailed,
}

/// A bounded diagnostic that can be safely persisted or emitted by presentation adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachmentReadDiagnostic {
    attempt: AttachmentReadAttempt,
    stage: AttachmentReadStage,
    status_code: Option<u16>,
    mime_class: Option<AttachmentMimeClass>,
    body_class: Option<AttachmentBodyClass>,
    transport_class: Option<AttachmentTransportClass>,
}

impl AttachmentReadDiagnostic {
    const fn new(attempt: AttachmentReadAttempt, stage: AttachmentReadStage) -> Self {
        Self {
            attempt,
            stage,
            status_code: None,
            mime_class: None,
            body_class: None,
            transport_class: None,
        }
    }

    pub const fn status(attempt: AttachmentReadAttempt, status_code: u16) -> Self {
        Self::new(attempt, AttachmentReadStage::Status).with_status_code(status_code)
    }

    pub const fn content_type(
        attempt: AttachmentReadAttempt,
        mime_class: AttachmentMimeClass,
    ) -> Self {
        Self::new(attempt, AttachmentReadStage::ContentType).with_mime_class(mime_class)
    }

    pub const fn body(attempt: AttachmentReadAttempt, body_class: AttachmentBodyClass) -> Self {
        Self::new(attempt, AttachmentReadStage::Body).with_body_class(body_class)
    }

    pub const fn transport(
        attempt: AttachmentReadAttempt,
        transport_class: AttachmentTransportClass,
    ) -> Self {
        Self::new(attempt, AttachmentReadStage::Transport).with_transport_class(transport_class)
    }

    pub const fn validation(attempt: AttachmentReadAttempt) -> Self {
        Self::new(attempt, AttachmentReadStage::Validation)
    }

    pub const fn attempt(self) -> AttachmentReadAttempt {
        self.attempt
    }

    pub const fn stage(self) -> AttachmentReadStage {
        self.stage
    }

    pub const fn status_code(self) -> Option<u16> {
        self.status_code
    }

    pub const fn mime_class(self) -> Option<AttachmentMimeClass> {
        self.mime_class
    }

    pub const fn body_class(self) -> Option<AttachmentBodyClass> {
        self.body_class
    }

    pub const fn transport_class(self) -> Option<AttachmentTransportClass> {
        self.transport_class
    }

    pub const fn with_attempt(self, attempt: AttachmentReadAttempt) -> Self {
        Self { attempt, ..self }
    }

    const fn with_status_code(self, status_code: u16) -> Self {
        Self {
            status_code: Some(status_code),
            ..self
        }
    }

    const fn with_mime_class(self, mime_class: AttachmentMimeClass) -> Self {
        Self {
            mime_class: Some(mime_class),
            ..self
        }
    }

    const fn with_body_class(self, body_class: AttachmentBodyClass) -> Self {
        Self {
            body_class: Some(body_class),
            ..self
        }
    }

    const fn with_transport_class(self, transport_class: AttachmentTransportClass) -> Self {
        Self {
            transport_class: Some(transport_class),
            ..self
        }
    }
}
