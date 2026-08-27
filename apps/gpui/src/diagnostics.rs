//! Bounded, privacy-safe image diagnostics.
//!
//! This private facade preserves the application-facing sink API while keeping
//! the event schema/serialization and hardened filesystem persistence in
//! separate private modules. The sink is best effort: a read-only or malformed
//! state directory simply disables diagnostics and never changes application
//! startup or image rendering.

mod persistence;
mod schema;

use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use persistence::prepare_directory;
use persistence::{DIAGNOSTICS_BACKUP_FILENAME, DIAGNOSTICS_FILENAME, append_line};
use schema::serialize_event;
#[cfg(test)]
use schema::{MAX_CANDIDATE_ORDINAL, MAX_LINE_BYTES};
#[cfg(test)]
use std::path::Path;

const MAX_ONCE_EVENTS: usize = 256;

// Keep the schema values private to this facade while retaining all existing
// `pub(crate)` application APIs and paths for consumers.
#[allow(unused_imports)]
pub(crate) use schema::{
    AttachmentDiagnosticAttempt, AttachmentDiagnosticBody, AttachmentDiagnosticMime,
    AttachmentDiagnosticStage, AttachmentDiagnosticTransport, DecodeFallbackReason,
    DesktopNotificationTestResult, DiagnosticErrorKind, DiagnosticEvent, DiagnosticFlow,
    ImageFetchResult, ImagePreflight, ImageSignature, ImageSource, ImageStateReason, ResponseMime,
};

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
    #[cfg(test)]
    pub(crate) fn for_directory(directory: &Path) -> Self {
        let directory = prepare_directory(directory).ok();
        Self::from_prepared_directory(directory)
    }

    #[cfg(any(test, feature = "ui-lab"))]
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

    pub(crate) fn desktop_notification_test_started(&self) {
        self.record(DiagnosticEvent::DesktopNotificationTestStarted);
    }

    pub(crate) fn desktop_notification_test_result(&self, result: DesktopNotificationTestResult) {
        self.record(DiagnosticEvent::DesktopNotificationTestResult(result));
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

    #[cfg(test)]
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

    pub(crate) fn attachment_read_diagnostic(
        &self,
        flow: DiagnosticFlow,
        load_token: u64,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: ImageSource,
        diagnostic: jira_application::AttachmentReadDiagnostic,
    ) {
        self.record(DiagnosticEvent::attachment_read_diagnostic(
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
            diagnostic,
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

    #[test]
    fn desktop_notification_events_use_fixed_schema_and_safe_outcomes() {
        let root = temporary_root("notification-events");
        let sink = DiagnosticsSink::for_directory(&root);
        sink.desktop_notification_test_started();
        sink.desktop_notification_test_result(DesktopNotificationTestResult::Accepted {
            notification_id: u32::MAX,
        });
        sink.desktop_notification_test_result(DesktopNotificationTestResult::Failed(
            DiagnosticErrorKind::Notification,
        ));
        let lines = read_lines(&root.join(DIAGNOSTICS_FILENAME));
        assert!(lines[0].contains("\"event\":\"desktop_notification_test_started\""));
        assert!(lines[1].contains("\"outcome\":\"accepted\""));
        assert!(lines[1].contains("\"notification_id\":4294967295"));
        assert!(lines[2].contains("\"outcome\":\"failed\""));
        assert!(lines[2].contains("\"error\":\"notification\""));
        assert!(lines.iter().all(|line| is_json_line(line)));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn attachment_diagnostic_is_fixed_schema_enum_only_and_bounded() {
        let root = temporary_root("attachment-diagnostic");
        let sink = DiagnosticsSink::for_directory(&root);
        sink.attachment_read_diagnostic(
            DiagnosticFlow::RemoteLookup,
            u64::MAX,
            usize::MAX,
            usize::MAX,
            ImageSource::FallbackCandidate,
            jira_application::AttachmentReadDiagnostic::status(
                jira_application::AttachmentReadAttempt::OriginalFallback,
                599,
            ),
        );
        sink.attachment_read_diagnostic(
            DiagnosticFlow::RemoteLookup,
            2,
            0,
            1,
            ImageSource::FallbackCandidate,
            jira_application::AttachmentReadDiagnostic::content_type(
                jira_application::AttachmentReadAttempt::Thumbnail,
                jira_application::AttachmentMimeClass::OctetStream,
            ),
        );
        sink.attachment_read_diagnostic(
            DiagnosticFlow::SelectedDetail,
            3,
            1,
            0,
            ImageSource::ResolvedAdf,
            jira_application::AttachmentReadDiagnostic::body(
                jira_application::AttachmentReadAttempt::OriginalFallback,
                jira_application::AttachmentBodyClass::TooLarge,
            ),
        );
        sink.attachment_read_diagnostic(
            DiagnosticFlow::SelectedDetail,
            4,
            2,
            1,
            ImageSource::ResolvedAdf,
            jira_application::AttachmentReadDiagnostic::transport(
                jira_application::AttachmentReadAttempt::ExplicitDownload,
                jira_application::AttachmentTransportClass::TimedOut,
            ),
        );

        let lines = read_lines(&root.join(DIAGNOSTICS_FILENAME));
        assert_eq!(lines.len(), 4);
        let status_line = &lines[0];
        assert!(is_json_line(status_line));
        assert!(status_line.len() <= MAX_LINE_BYTES);
        assert!(status_line.contains(r#""event":"attachment_read_diagnostic""#));
        assert!(status_line.contains(r#""attempt":"original_fallback""#));
        assert!(status_line.contains(r#""stage":"status""#));
        assert!(status_line.contains(r#""status_code":599"#));
        assert!(status_line.contains(r#""mime_class":null"#));
        assert!(status_line.contains(r#""body_class":null"#));
        assert!(status_line.contains(r#""transport_class":null"#));
        assert!(!status_line.contains("https://jira.example"));
        assert!(!status_line.contains("attachment-123"));
        assert!(!status_line.contains("secret.png"));
        assert!(!status_line.contains("Content-Type"));
        assert!(!status_line.contains("image/octet-stream"));
        assert!(!status_line.contains("raw response body"));

        let mime_line = &lines[1];
        assert!(is_json_line(mime_line));
        assert!(mime_line.contains(r#""stage":"content_type""#));
        assert!(mime_line.contains(r#""status_code":null"#));
        assert!(mime_line.contains(r#""mime_class":"octet_stream""#));
        assert!(mime_line.contains(r#""body_class":null"#));
        assert!(mime_line.contains(r#""transport_class":null"#));

        let body_line = &lines[2];
        assert!(is_json_line(body_line));
        assert!(body_line.contains(r#""stage":"body""#));
        assert!(body_line.contains(r#""body_class":"too_large""#));

        let transport_line = &lines[3];
        assert!(is_json_line(transport_line));
        assert!(transport_line.contains(r#""stage":"transport""#));
        assert!(transport_line.contains(r#""transport_class":"timed_out""#));
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
