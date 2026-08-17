//! Bounded, privacy-safe image diagnostics.
//!
//! This module deliberately has no string-bearing event fields. The sink is
//! best effort: a read-only or malformed state directory simply disables
//! diagnostics and never changes application startup or image rendering.

use std::{
    collections::{HashSet, VecDeque},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

const DIAGNOSTICS_FILENAME: &str = "diagnostics.jsonl";
const DIAGNOSTICS_BACKUP_FILENAME: &str = "diagnostics.jsonl.1";
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_LINE_BYTES: usize = 2 * 1024;
const MAX_CANDIDATE_ORDINAL: usize = 32;
const MAX_REPORTED_BYTE_COUNT: usize = 64 * 1024 * 1024;
const MAX_ONCE_EVENTS: usize = 256;

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

impl DecodeFallbackReason {
    const fn as_json(self) -> &'static str {
        match self {
            Self::DecodeFailed => "decode_failed",
        }
    }
}

/// The only values that can be emitted. In particular, this enum has no
/// `String`, `&str`, URL, identifier, filename, or arbitrary message field.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DiagnosticEvent {
    SessionStarted,
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
}

fn bounded_candidate(value: usize) -> u8 {
    value.min(MAX_CANDIDATE_ORDINAL) as u8
}

fn bounded_byte_count(value: usize) -> u32 {
    value.min(MAX_REPORTED_BYTE_COUNT) as u32
}

#[derive(Clone)]
pub(crate) struct DiagnosticsSink {
    state: Arc<Mutex<SinkState>>,
    next_load_token: Arc<AtomicU64>,
}

struct SinkState {
    active_path: Option<PathBuf>,
    backup_path: Option<PathBuf>,
    next_sequence: u64,
    once_events: HashSet<DiagnosticEvent>,
    once_order: VecDeque<DiagnosticEvent>,
}

impl DiagnosticsSink {
    /// Construct the process sink. Any environment or filesystem failure
    /// yields a disabled sink and is intentionally not returned to startup.
    pub(crate) fn from_environment() -> Self {
        let directory = crate::local_data::prepare_diagnostics_directory_from_environment().ok();
        Self::from_prepared_directory(directory)
    }

    /// Injectable constructor used by the GPUI adapter and tests. The path is
    /// treated as the final app directory and receives the same safety checks
    /// and permissions as the environment-selected directory.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn for_directory(directory: &Path) -> Self {
        let directory = prepare_directory(directory).ok();
        Self::from_prepared_directory(directory)
    }

    pub(crate) fn disabled() -> Self {
        Self::from_prepared_directory(None)
    }

    fn from_prepared_directory(directory: Option<PathBuf>) -> Self {
        let (active_path, backup_path) = directory
            .map(|path| {
                (
                    Some(path.join(DIAGNOSTICS_FILENAME)),
                    Some(path.join(DIAGNOSTICS_BACKUP_FILENAME)),
                )
            })
            .unwrap_or((None, None));
        Self {
            state: Arc::new(Mutex::new(SinkState {
                active_path,
                backup_path,
                next_sequence: 0,
                once_events: HashSet::new(),
                once_order: VecDeque::new(),
            })),
            next_load_token: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Allocate a process-local correlation token. It is intentionally
    /// independent of Jira identifiers and remains available when logging is
    /// disabled, so diagnostics never affect application control flow.
    pub(crate) fn begin_image_load(&self) -> u64 {
        self.next_load_token
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |token| {
                Some(token.saturating_add(1))
            })
            .unwrap_or(0)
            .saturating_add(1)
    }

    pub(crate) fn record(&self, event: DiagnosticEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let (Some(active_path), Some(backup_path)) =
            (state.active_path.clone(), state.backup_path.clone())
        else {
            return;
        };
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        let timestamp = unix_timestamp_millis();
        let Some(line) = serialize_event(event, sequence, timestamp) else {
            return;
        };
        if append_line(&active_path, &backup_path, &line).is_err() {
            state.active_path = None;
            state.backup_path = None;
        }
    }

    /// Record a render callback at most once per process sink. GPUI may invoke
    /// these callbacks repeatedly while painting the same image.
    pub(crate) fn record_once(&self, event: DiagnosticEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let (Some(active_path), Some(backup_path)) =
            (state.active_path.clone(), state.backup_path.clone())
        else {
            return;
        };
        if state.once_events.contains(&event) {
            return;
        }
        if state.once_events.len() >= MAX_ONCE_EVENTS
            && let Some(evicted) = state.once_order.pop_front()
        {
            state.once_events.remove(&evicted);
        }
        let sequence = state.next_sequence;
        let timestamp = unix_timestamp_millis();
        let Some(line) = serialize_event(event, sequence, timestamp) else {
            return;
        };
        if append_line(&active_path, &backup_path, &line).is_err() {
            state.active_path = None;
            state.backup_path = None;
            return;
        }
        state.next_sequence = state.next_sequence.saturating_add(1);
        state.once_events.insert(event);
        state.once_order.push_back(event);
    }

    pub(crate) fn session_started(&self) {
        self.record(DiagnosticEvent::SessionStarted);
    }

    pub(crate) fn image_fetch_started(
        &self,
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
    ) {
        self.record(DiagnosticEvent::image_fetch_started(
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
        ));
    }

    pub(crate) fn image_fetch_result(
        &self,
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        result: ImageFetchResult,
    ) {
        self.record(DiagnosticEvent::image_fetch_result(
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            result,
        ));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn image_response(
        &self,
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        mime: ResponseMime,
        signature: ImageSignature,
        byte_count: usize,
        preflight: ImagePreflight,
    ) {
        self.record(DiagnosticEvent::image_response(
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            mime,
            signature,
            byte_count,
            preflight,
        ));
    }

    pub(crate) fn image_state(
        &self,
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        reason: ImageStateReason,
    ) {
        self.record(DiagnosticEvent::image_state(
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            reason,
        ));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn gpui_decode_fallback(
        &self,
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        reason: DecodeFallbackReason,
    ) {
        self.record(DiagnosticEvent::gpui_decode_fallback(
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            reason,
        ));
    }
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg_attr(not(test), allow(dead_code))]
fn prepare_directory(directory: &Path) -> Result<PathBuf, ()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => return Err(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(directory).map_err(|_| ())?;
        }
        Err(_) => return Err(()),
    }
    let metadata = fs::symlink_metadata(directory).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(());
    }
    restrict_directory(directory)?;
    Ok(directory.to_path_buf())
}

#[cfg_attr(not(test), allow(dead_code))]
fn restrict_directory(path: &Path) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| ())?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn ensure_regular_or_missing(path: &Path) -> Result<bool, ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(()),
    }
}

fn append_line(active_path: &Path, backup_path: &Path, line: &[u8]) -> Result<(), ()> {
    if line.len() > MAX_LINE_BYTES {
        return Err(());
    }
    let active_exists = ensure_regular_or_missing(active_path)?;
    let backup_exists = ensure_regular_or_missing(backup_path)?;
    if active_exists {
        restrict_file(active_path)?;
    }
    if backup_exists {
        restrict_file(backup_path)?;
        let size = fs::metadata(backup_path).map_err(|_| ())?.len();
        if size > MAX_FILE_BYTES {
            truncate_regular(backup_path)?;
        }
    }

    let active_size = if active_exists {
        let size = fs::metadata(active_path).map_err(|_| ())?.len();
        if size > MAX_FILE_BYTES {
            truncate_regular(active_path)?;
            0
        } else {
            size
        }
    } else {
        0
    };
    let line_size = u64::try_from(line.len()).map_err(|_| ())?.saturating_add(1);
    if active_size.saturating_add(line_size) > MAX_FILE_BYTES && active_exists {
        // The backup was validated as a regular file above. Remove it before
        // rename so rotation remains portable and never leaves two oversized
        // files behind.
        if ensure_regular_or_missing(backup_path)? {
            fs::remove_file(backup_path).map_err(|_| ())?;
        }
        fs::rename(active_path, backup_path).map_err(|_| ())?;
        restrict_file(backup_path)?;
    }

    let _ = ensure_regular_or_missing(active_path)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(active_path).map_err(|_| ())?;
    file.write_all(line).map_err(|_| ())?;
    file.write_all(b"\n").map_err(|_| ())?;
    file.flush().map_err(|_| ())?;
    restrict_file(active_path)
}

fn truncate_regular(path: &Path) -> Result<(), ()> {
    if !ensure_regular_or_missing(path)? {
        return Err(());
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|_| ())?;
    file.set_len(0).map_err(|_| ())
}

fn restrict_file(path: &Path) -> Result<(), ()> {
    if !ensure_regular_or_missing(path)? {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|_| ())?;
    }
    Ok(())
}

fn serialize_event(event: DiagnosticEvent, sequence: u64, timestamp: u64) -> Option<Vec<u8>> {
    let prefix = format!(
        r#"{{"v":1,"seq":{},"ts_unix_ms":{},"event":""#,
        sequence, timestamp
    );
    let json = match event {
        DiagnosticEvent::SessionStarted => format!(r#"{prefix}session_started"}}"#),
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
    };
    let bytes = json.into_bytes();
    (bytes.len().saturating_add(1) <= MAX_LINE_BYTES).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "jira-desk-diagnostics-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn read_lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .expect("diagnostics")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn creates_private_directory_and_files() {
        let root = temporary_root("permissions");
        let sink = DiagnosticsSink::for_directory(&root);
        sink.session_started();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&root).expect("root").permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(root.join(DIAGNOSTICS_FILENAME))
                    .expect("active")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn load_tokens_are_local_monotonic_and_available_when_disabled() {
        let sink = DiagnosticsSink::disabled();
        let first = sink.begin_image_load();
        let second = sink.begin_image_load();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn once_events_use_bounded_fifo_and_keep_later_loads_observable() {
        let root = temporary_root("once-fifo");
        let sink = DiagnosticsSink::for_directory(&root);
        for index in 0..MAX_ONCE_EVENTS {
            sink.record_once(DiagnosticEvent::image_state(
                DiagnosticFlow::SelectedDetail,
                1,
                index % (MAX_CANDIDATE_ORDINAL + 1),
                index / (MAX_CANDIDATE_ORDINAL + 1),
                ImageSource::ResolvedAdf,
                ImageStateReason::Missing,
            ));
        }
        let before = read_lines(&root.join(DIAGNOSTICS_FILENAME)).len();
        sink.record_once(DiagnosticEvent::image_state(
            DiagnosticFlow::SelectedDetail,
            2,
            0,
            0,
            ImageSource::ResolvedAdf,
            ImageStateReason::Missing,
        ));
        sink.record_once(DiagnosticEvent::image_state(
            DiagnosticFlow::SelectedDetail,
            2,
            0,
            0,
            ImageSource::ResolvedAdf,
            ImageStateReason::Missing,
        ));
        let after = read_lines(&root.join(DIAGNOSTICS_FILENAME));
        assert_eq!(before, MAX_ONCE_EVENTS);
        assert_eq!(after.len(), before + 1);
        assert!(
            after
                .last()
                .expect("later load")
                .contains("\"load_token\":2")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_directory_and_files() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("symlink");
        let target = root.join("target");
        fs::create_dir_all(&target).expect("target");
        let linked = root.join("linked");
        symlink(&target, &linked).expect("directory symlink");
        let disabled = DiagnosticsSink::for_directory(&linked);
        disabled.session_started();
        assert!(!target.join(DIAGNOSTICS_FILENAME).exists());
        fs::remove_file(&linked).expect("remove directory symlink");
        fs::create_dir_all(&root).expect("root");

        let active_target = root.join("active-target");
        fs::write(&active_target, b"outside\n").expect("target file");
        symlink(&active_target, root.join(DIAGNOSTICS_FILENAME)).expect("active symlink");
        let active_sink = DiagnosticsSink::for_directory(&root);
        active_sink.session_started();
        assert_eq!(fs::read(&active_target).expect("outside"), b"outside\n");
        fs::remove_file(root.join(DIAGNOSTICS_FILENAME)).expect("remove active symlink");

        let backup_target = root.join("backup-target");
        fs::write(&backup_target, b"outside\n").expect("backup target");
        symlink(&backup_target, root.join(DIAGNOSTICS_BACKUP_FILENAME)).expect("backup symlink");
        let backup_sink = DiagnosticsSink::for_directory(&root);
        backup_sink.session_started();
        assert_eq!(fs::read(&backup_target).expect("outside"), b"outside\n");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rotates_before_append_and_retains_one_bounded_backup() {
        let root = temporary_root("rotation");
        let sink = DiagnosticsSink::for_directory(&root);
        for _ in 0..5000 {
            sink.image_response(
                DiagnosticFlow::SelectedDetail,
                1,
                99,
                0,
                ImageSource::ResolvedAdf,
                ResponseMime::Png,
                ImageSignature::Png,
                12_345,
                ImagePreflight::Accepted,
            );
        }
        let active = root.join(DIAGNOSTICS_FILENAME);
        let backup = root.join(DIAGNOSTICS_BACKUP_FILENAME);
        assert!(fs::metadata(&active).expect("active").len() <= MAX_FILE_BYTES);
        assert!(fs::metadata(&backup).expect("backup").len() <= MAX_FILE_BYTES);
        assert!(fs::metadata(&backup).expect("backup").len() > 0);
        assert!(
            fs::metadata(&active).expect("active").len()
                + fs::metadata(&backup).expect("backup").len()
                <= MAX_FILE_BYTES * 2
        );
        assert!(read_lines(&active).iter().all(|line| is_json_line(line)));
        assert!(read_lines(&backup).iter().all(|line| is_json_line(line)));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn recovers_oversized_active_and_backup_without_preserving_them() {
        let root = temporary_root("oversized-recovery");
        fs::create_dir_all(&root).expect("root");
        fs::write(
            root.join(DIAGNOSTICS_FILENAME),
            vec![b'a'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("active");
        fs::write(
            root.join(DIAGNOSTICS_BACKUP_FILENAME),
            vec![b'b'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("backup");

        let sink = DiagnosticsSink::for_directory(&root);
        sink.session_started();

        assert!(
            fs::metadata(root.join(DIAGNOSTICS_FILENAME))
                .expect("active")
                .len()
                <= MAX_FILE_BYTES
        );
        assert!(
            fs::metadata(root.join(DIAGNOSTICS_BACKUP_FILENAME))
                .expect("backup")
                .len()
                <= MAX_FILE_BYTES
        );
        assert!(
            read_lines(&root.join(DIAGNOSTICS_FILENAME))
                .iter()
                .all(|line| is_json_line(line))
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn every_emitted_line_is_valid_bounded_json_without_free_form_values() {
        let root = temporary_root("events");
        let sink = DiagnosticsSink::for_directory(&root);
        sink.session_started();
        sink.image_fetch_started(
            DiagnosticFlow::RemoteLookup,
            1,
            usize::MAX,
            0,
            ImageSource::ResolvedAdf,
        );
        sink.image_fetch_result(
            DiagnosticFlow::RemoteLookup,
            1,
            1,
            0,
            ImageSource::ResolvedAdf,
            ImageFetchResult::Failed(DiagnosticErrorKind::Upstream),
        );
        sink.image_response(
            DiagnosticFlow::RemoteLookup,
            1,
            1,
            0,
            ImageSource::ResolvedAdf,
            ResponseMime::Unsupported,
            ImageSignature::Unknown,
            usize::MAX,
            ImagePreflight::AggregateRejected,
        );
        sink.image_state(
            DiagnosticFlow::RemoteLookup,
            1,
            1,
            0,
            ImageSource::ResolvedAdf,
            ImageStateReason::Failed,
        );
        sink.gpui_decode_fallback(
            DiagnosticFlow::RemoteLookup,
            1,
            1,
            0,
            ImageSource::ResolvedAdf,
            DecodeFallbackReason::DecodeFailed,
        );
        for (expected_sequence, line) in read_lines(&root.join(DIAGNOSTICS_FILENAME))
            .into_iter()
            .enumerate()
        {
            assert!(line.len() <= MAX_LINE_BYTES);
            assert!(is_json_line(&line));
            assert_eq!(numeric_field(&line, "seq"), expected_sequence as u64);
            let _timestamp = numeric_field(&line, "ts_unix_ms");
            if line.contains("\"event\":\"image_")
                || line.contains("\"event\":\"gpui_decode_fallback")
            {
                assert_eq!(numeric_field(&line, "load_token"), 1);
            }
            assert!(!line.contains("http"));
            assert!(!line.contains("attachment"));
            assert!(!line.contains("filename"));
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    fn is_json_line(line: &str) -> bool {
        let bytes = line.as_bytes();
        if bytes.len() < 2 || bytes[0] != b'{' || bytes[bytes.len() - 1] != b'}' {
            return false;
        }
        let mut quoted = false;
        let mut escaped = false;
        for byte in bytes {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' && quoted {
                escaped = true;
            } else if *byte == b'"' {
                quoted = !quoted;
            }
        }
        !quoted && !escaped
    }

    fn numeric_field(line: &str, key: &str) -> u64 {
        let marker = format!(r#""{key}":"#);
        let start = line
            .find(&marker)
            .map(|index| index + marker.len())
            .expect("numeric field");
        let end = line[start..]
            .find(|character: char| !character.is_ascii_digit())
            .map(|offset| start + offset)
            .unwrap_or(line.len());
        line[start..end].parse().expect("number")
    }
}
