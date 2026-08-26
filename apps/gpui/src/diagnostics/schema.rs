//! Private diagnostics event schema and serialization.
//!
//! This module deliberately has no filesystem dependencies. It contains only
//! bounded, privacy-safe event values and their fixed JSON representation.

pub(super) const MAX_LINE_BYTES: usize = 2 * 1024;
pub(super) const MAX_CANDIDATE_ORDINAL: usize = 32;
pub(super) const MAX_REPORTED_BYTE_COUNT: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DiagnosticFlow {
    SelectedDetail,
    RemoteLookup,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ImageSource {
    ResolvedAdf,
    FallbackCandidate,
}

impl ImageSource {
    const fn as_json(self) -> &'static str {
        match self {
            Self::ResolvedAdf => "resolved_adf",
            Self::FallbackCandidate => "fallback_candidate",
        }
    }
}

impl DiagnosticFlow {
    const fn as_json(self) -> &'static str {
        match self {
            Self::SelectedDetail => "selected_detail",
            Self::RemoteLookup => "remote_lookup",
        }
    }
}

/// Error classes intentionally mirror the safe application error categories,
/// but this local type prevents a raw application error from reaching the log.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DiagnosticErrorKind {
    Authentication,
    Authorization,
    RateLimited,
    Offline,
    Cancelled,
    InvalidInput,
    NotFound,
    Upstream,
    Storage,
    Notification,
    Internal,
    UnknownOutcome,
}

impl DiagnosticErrorKind {
    const fn as_json(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::RateLimited => "rate_limited",
            Self::Offline => "offline",
            Self::Cancelled => "cancelled",
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::Upstream => "upstream",
            Self::Storage => "storage",
            Self::Notification => "notification",
            Self::Internal => "internal",
            Self::UnknownOutcome => "unknown_outcome",
        }
    }
}

impl From<jira_application::ErrorKind> for DiagnosticErrorKind {
    fn from(kind: jira_application::ErrorKind) -> Self {
        match kind {
            jira_application::ErrorKind::Authentication => Self::Authentication,
            jira_application::ErrorKind::Authorization => Self::Authorization,
            jira_application::ErrorKind::RateLimited => Self::RateLimited,
            jira_application::ErrorKind::Offline => Self::Offline,
            jira_application::ErrorKind::Cancelled => Self::Cancelled,
            jira_application::ErrorKind::InvalidInput => Self::InvalidInput,
            jira_application::ErrorKind::NotFound => Self::NotFound,
            jira_application::ErrorKind::Upstream => Self::Upstream,
            jira_application::ErrorKind::Storage => Self::Storage,
            jira_application::ErrorKind::Notification => Self::Notification,
            jira_application::ErrorKind::Internal => Self::Internal,
            jira_application::ErrorKind::UnknownOutcome => Self::UnknownOutcome,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ImageFetchResult {
    Succeeded,
    Failed(DiagnosticErrorKind),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DesktopNotificationTestResult {
    Accepted { notification_id: u32 },
    Failed(DiagnosticErrorKind),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ResponseMime {
    Png,
    Jpeg,
    Gif,
    Webp,
    OctetStream,
    Unsupported,
}

impl ResponseMime {
    /// Classifies a response MIME without retaining the supplied value.
    pub(crate) fn classify(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "image/png" => Self::Png,
            "image/jpeg" | "image/jpg" => Self::Jpeg,
            "image/gif" => Self::Gif,
            "image/webp" => Self::Webp,
            "application/octet-stream" => Self::OctetStream,
            _ => Self::Unsupported,
        }
    }

    const fn as_json(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::OctetStream => "octet_stream",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ImageSignature {
    Png,
    Jpeg,
    Gif,
    Webp,
    Unknown,
}

impl ImageSignature {
    /// Classifies only the leading bytes and never retains or serializes them.
    pub(crate) fn classify(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Self::Png
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Self::Jpeg
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Self::Gif
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Self::Webp
        } else {
            Self::Unknown
        }
    }

    const fn as_json(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ImagePreflight {
    Accepted,
    Empty,
    UnsupportedCachedMime,
    ResponseMimeRejected,
    SignatureRejected,
    AggregateRejected,
}

impl ImagePreflight {
    const fn as_json(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Empty => "empty",
            Self::UnsupportedCachedMime => "unsupported_cached_mime",
            Self::ResponseMimeRejected => "response_mime_rejected",
            Self::SignatureRejected => "signature_rejected",
            Self::AggregateRejected => "aggregate_rejected",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ImageStateReason {
    Loading,
    Ready,
    Failed,
    Cancelled,
    Stale,
    Missing,
    Unsupported,
}

impl ImageStateReason {
    const fn as_json(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Stale => "stale",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DecodeFallbackReason {
    DecodeFailed,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AttachmentDiagnosticAttempt {
    Thumbnail,
    OriginalFallback,
    ExplicitDownload,
}

impl AttachmentDiagnosticAttempt {
    const fn as_json(self) -> &'static str {
        match self {
            Self::Thumbnail => "thumbnail",
            Self::OriginalFallback => "original_fallback",
            Self::ExplicitDownload => "explicit_download",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AttachmentDiagnosticStage {
    Transport,
    Status,
    ContentType,
    Body,
    Validation,
}

impl AttachmentDiagnosticStage {
    const fn as_json(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::Status => "status",
            Self::ContentType => "content_type",
            Self::Body => "body",
            Self::Validation => "validation",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AttachmentDiagnosticMime {
    Missing,
    Malformed,
    Png,
    Jpeg,
    Gif,
    Webp,
    OctetStream,
    Other,
}

impl AttachmentDiagnosticMime {
    const fn as_json(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Malformed => "malformed",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::OctetStream => "octet_stream",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AttachmentDiagnosticBody {
    Empty,
    TooLarge,
    ReadFailed,
}

impl AttachmentDiagnosticBody {
    const fn as_json(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLarge => "too_large",
            Self::ReadFailed => "read_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AttachmentDiagnosticTransport {
    ConnectFailed,
    TimedOut,
    RequestFailed,
}

impl AttachmentDiagnosticTransport {
    const fn as_json(self) -> &'static str {
        match self {
            Self::ConnectFailed => "connect_failed",
            Self::TimedOut => "timed_out",
            Self::RequestFailed => "request_failed",
        }
    }
}

impl DecodeFallbackReason {
    const fn as_json(self) -> &'static str {
        match self {
            Self::DecodeFailed => "decode_failed",
        }
    }
}

/// The only values that can be emitted. In particular, this enum has no
/// `String`, `&str`, URL, filename, or arbitrary message field. The one
/// numeric identifier is the bounded daemon receipt ID from a local test.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DiagnosticEvent {
    SessionStarted,
    DesktopNotificationTestStarted,
    DesktopNotificationTestResult(DesktopNotificationTestResult),
    ImageFetchStarted {
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: u8,
        surface_ordinal: u8,
        source: ImageSource,
    },
    ImageFetchResult {
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: u8,
        surface_ordinal: u8,
        source: ImageSource,
        result: ImageFetchResult,
    },
    ImageResponse {
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: u8,
        surface_ordinal: u8,
        source: ImageSource,
        mime: ResponseMime,
        signature: ImageSignature,
        byte_count: u32,
        preflight: ImagePreflight,
    },
    ImageState {
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: u8,
        surface_ordinal: u8,
        source: ImageSource,
        reason: ImageStateReason,
    },
    GpuiDecodeFallback {
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: u8,
        surface_ordinal: u8,
        source: ImageSource,
        reason: DecodeFallbackReason,
    },
    AttachmentReadDiagnostic {
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: u8,
        surface_ordinal: u8,
        source: ImageSource,
        attempt: AttachmentDiagnosticAttempt,
        stage: AttachmentDiagnosticStage,
        status_code: Option<u16>,
        mime_class: Option<AttachmentDiagnosticMime>,
        body_class: Option<AttachmentDiagnosticBody>,
        transport_class: Option<AttachmentDiagnosticTransport>,
    },
}

impl DiagnosticEvent {
    pub(crate) fn image_fetch_started(
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
    ) -> Self {
        Self::ImageFetchStarted {
            flow,
            load_token,
            candidate_ordinal: bounded_candidate(candidate_ordinal),
            surface_ordinal: bounded_candidate(surface_ordinal),
            source,
        }
    }

    pub(crate) fn image_fetch_result(
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        result: ImageFetchResult,
    ) -> Self {
        Self::ImageFetchResult {
            flow,
            load_token,
            candidate_ordinal: bounded_candidate(candidate_ordinal),
            surface_ordinal: bounded_candidate(surface_ordinal),
            source,
            result,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn image_response(
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        mime: ResponseMime,
        signature: ImageSignature,
        byte_count: usize,
        preflight: ImagePreflight,
    ) -> Self {
        Self::ImageResponse {
            flow,
            load_token,
            candidate_ordinal: bounded_candidate(candidate_ordinal),
            surface_ordinal: bounded_candidate(surface_ordinal),
            source,
            mime,
            signature,
            byte_count: bounded_byte_count(byte_count),
            preflight,
        }
    }

    pub(crate) fn image_state(
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        reason: ImageStateReason,
    ) -> Self {
        Self::ImageState {
            flow,
            load_token,
            candidate_ordinal: bounded_candidate(candidate_ordinal),
            surface_ordinal: bounded_candidate(surface_ordinal),
            source,
            reason,
        }
    }

    pub(crate) fn gpui_decode_fallback(
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        reason: DecodeFallbackReason,
    ) -> Self {
        Self::GpuiDecodeFallback {
            flow,
            load_token,
            candidate_ordinal: bounded_candidate(candidate_ordinal),
            surface_ordinal: bounded_candidate(surface_ordinal),
            source,
            reason,
        }
    }

    pub(crate) fn attachment_read_diagnostic(
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        diagnostic: jira_application::AttachmentReadDiagnostic,
    ) -> Self {
        Self::AttachmentReadDiagnostic {
            flow,
            load_token,
            candidate_ordinal: bounded_candidate(candidate_ordinal),
            surface_ordinal: bounded_candidate(surface_ordinal),
            source,
            attempt: attachment_diagnostic_attempt(diagnostic.attempt()),
            stage: attachment_diagnostic_stage(diagnostic.stage()),
            status_code: diagnostic.status_code(),
            mime_class: diagnostic.mime_class().map(attachment_diagnostic_mime),
            body_class: diagnostic.body_class().map(attachment_diagnostic_body),
            transport_class: diagnostic
                .transport_class()
                .map(attachment_diagnostic_transport),
        }
    }
}

fn attachment_diagnostic_attempt(
    attempt: jira_application::AttachmentReadAttempt,
) -> AttachmentDiagnosticAttempt {
    match attempt {
        jira_application::AttachmentReadAttempt::Thumbnail => {
            AttachmentDiagnosticAttempt::Thumbnail
        }
        jira_application::AttachmentReadAttempt::OriginalFallback => {
            AttachmentDiagnosticAttempt::OriginalFallback
        }
        jira_application::AttachmentReadAttempt::ExplicitDownload => {
            AttachmentDiagnosticAttempt::ExplicitDownload
        }
    }
}

fn attachment_diagnostic_stage(
    stage: jira_application::AttachmentReadStage,
) -> AttachmentDiagnosticStage {
    match stage {
        jira_application::AttachmentReadStage::Transport => AttachmentDiagnosticStage::Transport,
        jira_application::AttachmentReadStage::Status => AttachmentDiagnosticStage::Status,
        jira_application::AttachmentReadStage::ContentType => {
            AttachmentDiagnosticStage::ContentType
        }
        jira_application::AttachmentReadStage::Body => AttachmentDiagnosticStage::Body,
        jira_application::AttachmentReadStage::Validation => AttachmentDiagnosticStage::Validation,
    }
}

fn attachment_diagnostic_mime(
    mime: jira_application::AttachmentMimeClass,
) -> AttachmentDiagnosticMime {
    match mime {
        jira_application::AttachmentMimeClass::Missing => AttachmentDiagnosticMime::Missing,
        jira_application::AttachmentMimeClass::Malformed => AttachmentDiagnosticMime::Malformed,
        jira_application::AttachmentMimeClass::Png => AttachmentDiagnosticMime::Png,
        jira_application::AttachmentMimeClass::Jpeg => AttachmentDiagnosticMime::Jpeg,
        jira_application::AttachmentMimeClass::Gif => AttachmentDiagnosticMime::Gif,
        jira_application::AttachmentMimeClass::Webp => AttachmentDiagnosticMime::Webp,
        jira_application::AttachmentMimeClass::OctetStream => AttachmentDiagnosticMime::OctetStream,
        jira_application::AttachmentMimeClass::Other => AttachmentDiagnosticMime::Other,
    }
}

fn attachment_diagnostic_body(
    body: jira_application::AttachmentBodyClass,
) -> AttachmentDiagnosticBody {
    match body {
        jira_application::AttachmentBodyClass::Empty => AttachmentDiagnosticBody::Empty,
        jira_application::AttachmentBodyClass::TooLarge => AttachmentDiagnosticBody::TooLarge,
        jira_application::AttachmentBodyClass::ReadFailed => AttachmentDiagnosticBody::ReadFailed,
    }
}

fn attachment_diagnostic_transport(
    transport: jira_application::AttachmentTransportClass,
) -> AttachmentDiagnosticTransport {
    match transport {
        jira_application::AttachmentTransportClass::ConnectFailed => {
            AttachmentDiagnosticTransport::ConnectFailed
        }
        jira_application::AttachmentTransportClass::TimedOut => {
            AttachmentDiagnosticTransport::TimedOut
        }
        jira_application::AttachmentTransportClass::RequestFailed => {
            AttachmentDiagnosticTransport::RequestFailed
        }
    }
}

fn bounded_candidate(value: usize) -> u8 {
    value.min(MAX_CANDIDATE_ORDINAL) as u8
}

fn bounded_byte_count(value: usize) -> u32 {
    value.min(MAX_REPORTED_BYTE_COUNT) as u32
}

pub(super) fn serialize_event(
    event: DiagnosticEvent,
    sequence: u64,
    timestamp: u64,
) -> Option<Vec<u8>> {
    let prefix = format!(
        r#"{{"v":1,"seq":{},"ts_unix_ms":{},"event":""#,
        sequence, timestamp
    );
    let json = match event {
        DiagnosticEvent::SessionStarted => format!(r#"{prefix}session_started"}}"#),
        DiagnosticEvent::DesktopNotificationTestStarted => {
            format!(r#"{prefix}desktop_notification_test_started"}}"#)
        }
        DiagnosticEvent::DesktopNotificationTestResult(result) => match result {
            DesktopNotificationTestResult::Accepted { notification_id } => format!(
                r#"{prefix}desktop_notification_test_result","outcome":"accepted","notification_id":{notification_id}}}"#
            ),
            DesktopNotificationTestResult::Failed(error) => format!(
                r#"{prefix}desktop_notification_test_result","outcome":"failed","error":"{}"}}"#,
                error.as_json()
            ),
        },
        DiagnosticEvent::ImageFetchStarted {
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
        } => format!(
            r#"{prefix}image_fetch_started","flow":"{}","load_token":{},"candidate":{},"surface":{},"source":"{}"}}"#,
            flow.as_json(),
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source.as_json()
        ),
        DiagnosticEvent::ImageFetchResult {
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            result,
        } => {
            let (outcome, error) = match result {
                ImageFetchResult::Succeeded => ("succeeded", None),
                ImageFetchResult::Failed(kind) => ("failed", Some(kind.as_json())),
            };
            match error {
                Some(error) => format!(
                    r#"{prefix}image_fetch_result","flow":"{}","load_token":{},"candidate":{},"surface":{},"source":"{}","outcome":"{}","error":"{}"}}"#,
                    flow.as_json(),
                    load_token,
                    candidate_ordinal,
                    surface_ordinal,
                    source.as_json(),
                    outcome,
                    error
                ),
                None => format!(
                    r#"{prefix}image_fetch_result","flow":"{}","load_token":{},"candidate":{},"surface":{},"source":"{}","outcome":"{}"}}"#,
                    flow.as_json(),
                    load_token,
                    candidate_ordinal,
                    surface_ordinal,
                    source.as_json(),
                    outcome
                ),
            }
        }
        DiagnosticEvent::ImageResponse {
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            mime,
            signature,
            byte_count,
            preflight,
        } => format!(
            r#"{prefix}image_response","flow":"{}","load_token":{},"candidate":{},"surface":{},"source":"{}","mime":"{}","signature":"{}","bytes":{},"preflight":"{}"}}"#,
            flow.as_json(),
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source.as_json(),
            mime.as_json(),
            signature.as_json(),
            byte_count,
            preflight.as_json()
        ),
        DiagnosticEvent::ImageState {
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            reason,
        } => format!(
            r#"{prefix}image_state","flow":"{}","load_token":{},"candidate":{},"surface":{},"source":"{}","reason":"{}"}}"#,
            flow.as_json(),
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source.as_json(),
            reason.as_json()
        ),
        DiagnosticEvent::GpuiDecodeFallback {
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            reason,
        } => format!(
            r#"{prefix}gpui_decode_fallback","flow":"{}","load_token":{},"candidate":{},"surface":{},"source":"{}","reason":"{}"}}"#,
            flow.as_json(),
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source.as_json(),
            reason.as_json()
        ),
        DiagnosticEvent::AttachmentReadDiagnostic {
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            attempt,
            stage,
            status_code,
            mime_class,
            body_class,
            transport_class,
        } => format!(
            r#"{prefix}attachment_read_diagnostic","flow":"{}","load_token":{},"candidate":{},"surface":{},"source":"{}","attempt":"{}","stage":"{}","status_code":{},"mime_class":{},"body_class":{},"transport_class":{}}}"#,
            flow.as_json(),
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source.as_json(),
            attempt.as_json(),
            stage.as_json(),
            status_code
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".to_owned()),
            mime_class
                .map(|value| format!(r#""{}""#, value.as_json()))
                .unwrap_or_else(|| "null".to_owned()),
            body_class
                .map(|value| format!(r#""{}""#, value.as_json()))
                .unwrap_or_else(|| "null".to_owned()),
            transport_class
                .map(|value| format!(r#""{}""#, value.as_json()))
                .unwrap_or_else(|| "null".to_owned()),
        ),
    };
    let bytes = json.into_bytes();
    (bytes.len().saturating_add(1) <= MAX_LINE_BYTES).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_is_fixed_bounded_json_without_free_form_values() {
        let line = serialize_event(
            DiagnosticEvent::image_response(
                DiagnosticFlow::RemoteLookup,
                u64::MAX,
                usize::MAX,
                usize::MAX,
                ImageSource::FallbackCandidate,
                ResponseMime::Unsupported,
                ImageSignature::Unknown,
                usize::MAX,
                ImagePreflight::AggregateRejected,
            ),
            u64::MAX,
            u64::MAX,
        )
        .expect("bounded event");
        let line = String::from_utf8(line).expect("utf8");
        assert!(line.len() < MAX_LINE_BYTES);
        assert!(line.starts_with(r#"{"v":1,"seq":18446744073709551615"#));
        assert!(line.contains(r#""event":"image_response""#));
        assert!(line.contains(r#""candidate":32"#));
        assert!(line.contains(r#""bytes":67108864"#));
        assert!(!line.contains("http"));
        assert!(!line.contains("filename"));
    }

    #[test]
    fn classifiers_emit_only_known_categories() {
        assert_eq!(ResponseMime::classify(" IMAGE/JPG "), ResponseMime::Jpeg);
        assert_eq!(
            ResponseMime::classify("text/plain"),
            ResponseMime::Unsupported
        );
        assert_eq!(ImageSignature::classify(b"GIF89a"), ImageSignature::Gif);
        assert_eq!(ImageSignature::classify(b"secret"), ImageSignature::Unknown);
    }
}
