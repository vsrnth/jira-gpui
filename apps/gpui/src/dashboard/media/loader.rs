use std::sync::Arc;

use gpui::{Image, ImageFormat};
use jira_application::{AttachmentImage, AttachmentImageRequest, CancellationToken, PortFuture};
use jira_domain::{IssueId, JiraSiteId, RichImage};

use super::policy;
use crate::{
    diagnostics::{
        DiagnosticErrorKind, DiagnosticFlow, DiagnosticsSink, ImageFetchResult, ImageSource,
        ImageStateReason,
    },
    live_workspace::LiveWorkspace,
    rich_text_view::{RichImageRenderState, RichImageRenderStates},
};

fn diagnostic_mime(mime: policy::MediaMime) -> crate::diagnostics::ResponseMime {
    match mime {
        policy::MediaMime::Png => crate::diagnostics::ResponseMime::Png,
        policy::MediaMime::Jpeg => crate::diagnostics::ResponseMime::Jpeg,
        policy::MediaMime::Gif => crate::diagnostics::ResponseMime::Gif,
        policy::MediaMime::Webp => crate::diagnostics::ResponseMime::Webp,
        policy::MediaMime::OctetStream => crate::diagnostics::ResponseMime::OctetStream,
        policy::MediaMime::Unsupported => crate::diagnostics::ResponseMime::Unsupported,
    }
}

fn diagnostic_signature(signature: policy::MediaSignature) -> crate::diagnostics::ImageSignature {
    match signature {
        policy::MediaSignature::Png => crate::diagnostics::ImageSignature::Png,
        policy::MediaSignature::Jpeg => crate::diagnostics::ImageSignature::Jpeg,
        policy::MediaSignature::Gif => crate::diagnostics::ImageSignature::Gif,
        policy::MediaSignature::Webp => crate::diagnostics::ImageSignature::Webp,
        policy::MediaSignature::Unknown => crate::diagnostics::ImageSignature::Unknown,
    }
}

fn diagnostic_preflight(preflight: policy::MediaPreflight) -> crate::diagnostics::ImagePreflight {
    match preflight {
        policy::MediaPreflight::Accepted => crate::diagnostics::ImagePreflight::Accepted,
        policy::MediaPreflight::Empty => crate::diagnostics::ImagePreflight::Empty,
        policy::MediaPreflight::UnsupportedCachedMime => {
            crate::diagnostics::ImagePreflight::UnsupportedCachedMime
        }
        policy::MediaPreflight::ResponseMimeRejected => {
            crate::diagnostics::ImagePreflight::ResponseMimeRejected
        }
        policy::MediaPreflight::SignatureRejected => {
            crate::diagnostics::ImagePreflight::SignatureRejected
        }
        policy::MediaPreflight::AggregateRejected => {
            crate::diagnostics::ImagePreflight::AggregateRejected
        }
    }
}

fn gpui_image_format(format: policy::MediaFormat) -> ImageFormat {
    match format {
        policy::MediaFormat::Png => ImageFormat::Png,
        policy::MediaFormat::Jpeg => ImageFormat::Jpeg,
        policy::MediaFormat::Gif => ImageFormat::Gif,
        policy::MediaFormat::Webp => ImageFormat::Webp,
    }
}

trait AuthenticatedImageService {
    fn fetch_attachment_image<'a>(
        &'a self,
        request: AttachmentImageRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentImage>;

    fn cached_attachment_image<'a>(
        &'a self,
        _request: AttachmentImageRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Option<AttachmentImage>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}

impl AuthenticatedImageService for LiveWorkspace {
    fn fetch_attachment_image<'a>(
        &'a self,
        request: AttachmentImageRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentImage> {
        Box::pin(self.fetch_attachment_image(request, cancellation))
    }

    fn cached_attachment_image<'a>(
        &'a self,
        request: AttachmentImageRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Option<AttachmentImage>> {
        Box::pin(self.cached_attachment_image(request, cancellation))
    }
}

/// Hydrate the bounded image catalog from durable cache only. Missing entries
/// remain Loading so the ordinary authenticated fetch can fill them later.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn fetch_cached_rich_image_states(
    workspace: Arc<LiveWorkspace>,
    site_id: JiraSiteId,
    issue_id: IssueId,
    images: Vec<(RichImage, usize, ImageSource)>,
    cancellation: CancellationToken,
    diagnostics: DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> Result<RichImageRenderStates, ()> {
    fetch_cached_rich_image_states_with_loader(
        workspace.as_ref(),
        site_id,
        issue_id,
        images,
        cancellation,
        diagnostics,
        flow,
        load_token,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn fetch_cached_rich_image_states_with_loader<L: AuthenticatedImageService + ?Sized>(
    loader: &L,
    site_id: JiraSiteId,
    issue_id: IssueId,
    images: Vec<(RichImage, usize, ImageSource)>,
    cancellation: CancellationToken,
    diagnostics: DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> Result<RichImageRenderStates, ()> {
    let mut states = super::catalog::loading_image_states(&images, &diagnostics, flow, load_token);
    for (image, _, _) in &images {
        if cancellation.is_cancelled() {
            return Err(());
        }
        if policy::image_format_for_mime(&image.mime_type).is_none() {
            continue;
        }
        let result = loader
            .cached_attachment_image(
                AttachmentImageRequest {
                    site_id: site_id.clone(),
                    issue_id: issue_id.clone(),
                    attachment_id: image.attachment_id.clone(),
                    width: policy::MAX_IMAGE_REQUEST_WIDTH,
                    height: policy::MAX_IMAGE_REQUEST_HEIGHT,
                    max_bytes: policy::MAX_IMAGE_REQUEST_BYTES,
                },
                &cancellation,
            )
            .await;
        let Ok(Some(image_bytes)) = result else {
            continue;
        };
        let preflight = policy::image_response_preflight(
            &image.mime_type,
            &image_bytes.mime_type,
            &image_bytes.bytes,
            0,
        );
        let Some(format) = policy::fetched_image_format(
            &image.mime_type,
            &image_bytes.mime_type,
            &image_bytes.bytes,
        )
        .filter(|_| preflight == policy::MediaPreflight::Accepted) else {
            continue;
        };
        states.insert(
            image.attachment_id.clone(),
            RichImageRenderState::Ready(Arc::new(Image::from_bytes(
                gpui_image_format(format),
                image_bytes.bytes,
            ))),
        );
    }
    Ok(states)
}

/// Sequentially load the catalog through the authenticated workspace service.
/// A failed candidate is nonfatal so later fallback candidates retain their
/// original ordering and can still be attempted.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn fetch_rich_image_states(
    workspace: Arc<LiveWorkspace>,
    site_id: JiraSiteId,
    issue_id: IssueId,
    images: Vec<(RichImage, usize, ImageSource)>,
    cancellation: CancellationToken,
    diagnostics: DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> Result<RichImageRenderStates, ()> {
    fetch_rich_image_states_with_loader(
        workspace.as_ref(),
        site_id,
        issue_id,
        images,
        cancellation,
        diagnostics,
        flow,
        load_token,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn fetch_rich_image_states_with_loader<L: AuthenticatedImageService + ?Sized>(
    loader: &L,
    site_id: JiraSiteId,
    issue_id: IssueId,
    images: Vec<(RichImage, usize, ImageSource)>,
    cancellation: CancellationToken,
    diagnostics: DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> Result<RichImageRenderStates, ()> {
    let mut states = RichImageRenderStates::with_context(diagnostics.clone(), flow, load_token);
    let mut resident_bytes = 0usize;
    for (candidate_ordinal, collected) in images.into_iter().enumerate() {
        let (image, surface_ordinal, source) = collected;
        if cancellation.is_cancelled() {
            diagnostics.image_fetch_result(
                flow,
                load_token,
                candidate_ordinal,
                surface_ordinal,
                source,
                ImageFetchResult::Failed(DiagnosticErrorKind::Cancelled),
            );
            diagnostics.image_state(
                flow,
                load_token,
                candidate_ordinal,
                surface_ordinal,
                source,
                ImageStateReason::Cancelled,
            );
            return Err(());
        }
        diagnostics.image_fetch_started(
            flow,
            load_token,
            candidate_ordinal,
            surface_ordinal,
            source,
        );
        if policy::image_format_for_mime(&image.mime_type).is_none() {
            diagnostics.image_response(
                flow,
                load_token,
                candidate_ordinal,
                surface_ordinal,
                source,
                diagnostic_mime(policy::MediaMime::classify(&image.mime_type)),
                diagnostic_signature(policy::MediaSignature::Unknown),
                0,
                diagnostic_preflight(policy::MediaPreflight::UnsupportedCachedMime),
            );
            diagnostics.image_fetch_result(
                flow,
                load_token,
                candidate_ordinal,
                surface_ordinal,
                source,
                ImageFetchResult::Failed(DiagnosticErrorKind::InvalidInput),
            );
            diagnostics.image_state(
                flow,
                load_token,
                candidate_ordinal,
                surface_ordinal,
                source,
                ImageStateReason::Unsupported,
            );
            states.insert_with_context(
                image.attachment_id,
                RichImageRenderState::Failed,
                candidate_ordinal,
                surface_ordinal,
                source,
            );
            continue;
        }
        let result = loader
            .fetch_attachment_image(
                AttachmentImageRequest {
                    site_id: site_id.clone(),
                    issue_id: issue_id.clone(),
                    attachment_id: image.attachment_id.clone(),
                    width: policy::MAX_IMAGE_REQUEST_WIDTH,
                    height: policy::MAX_IMAGE_REQUEST_HEIGHT,
                    max_bytes: policy::MAX_IMAGE_REQUEST_BYTES,
                },
                &cancellation,
            )
            .await;
        match result {
            Ok(image_bytes) => {
                let response_mime =
                    diagnostic_mime(policy::MediaMime::classify(&image_bytes.mime_type));
                let signature = diagnostic_signature(policy::image_signature(&image_bytes.bytes));
                let preflight = policy::image_response_preflight(
                    &image.mime_type,
                    &image_bytes.mime_type,
                    &image_bytes.bytes,
                    resident_bytes,
                );
                diagnostics.image_response(
                    flow,
                    load_token,
                    candidate_ordinal,
                    surface_ordinal,
                    source,
                    response_mime,
                    signature,
                    image_bytes.bytes.len(),
                    diagnostic_preflight(preflight),
                );
                if preflight == policy::MediaPreflight::Accepted {
                    let format = policy::fetched_image_format(
                        &image.mime_type,
                        &image_bytes.mime_type,
                        &image_bytes.bytes,
                    )
                    .expect("accepted image preflight has a format");
                    resident_bytes = resident_bytes.saturating_add(image_bytes.bytes.len());
                    diagnostics.image_fetch_result(
                        flow,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageFetchResult::Succeeded,
                    );
                    diagnostics.image_state(
                        flow,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Ready,
                    );
                    states.insert_with_context(
                        image.attachment_id,
                        RichImageRenderState::Ready(Arc::new(Image::from_bytes(
                            gpui_image_format(format),
                            image_bytes.bytes,
                        ))),
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                    );
                } else {
                    diagnostics.image_fetch_result(
                        flow,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageFetchResult::Failed(DiagnosticErrorKind::InvalidInput),
                    );
                    diagnostics.image_state(
                        flow,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Failed,
                    );
                    states.insert_with_context(
                        image.attachment_id,
                        RichImageRenderState::Failed,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                    );
                }
            }
            Err(error) => {
                let error_kind = DiagnosticErrorKind::from(error.kind());
                if let Some(attachment_diagnostic) = error.attachment_diagnostic() {
                    diagnostics.attachment_read_diagnostic(
                        flow,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        attachment_diagnostic,
                    );
                }
                diagnostics.image_fetch_result(
                    flow,
                    load_token,
                    candidate_ordinal,
                    surface_ordinal,
                    source,
                    ImageFetchResult::Failed(error_kind),
                );
                diagnostics.image_state(
                    flow,
                    load_token,
                    candidate_ordinal,
                    surface_ordinal,
                    source,
                    if error.kind() == jira_application::ErrorKind::Cancelled {
                        ImageStateReason::Cancelled
                    } else {
                        ImageStateReason::Failed
                    },
                );
                states.insert_with_context(
                    image.attachment_id,
                    RichImageRenderState::Failed,
                    candidate_ordinal,
                    surface_ordinal,
                    source,
                );
            }
        }
    }
    Ok(states)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, fs, path::PathBuf, sync::Mutex};

    use futures_lite::future::block_on;

    use super::*;
    use crate::diagnostics::{ImagePreflight, ImageSignature, ResponseMime};

    #[derive(Default)]
    struct FakeImageService {
        requests: Mutex<Vec<AttachmentImageRequest>>,
        responses: Mutex<VecDeque<Result<AttachmentImage, jira_application::ApplicationError>>>,
        cached_responses:
            Mutex<VecDeque<Result<Option<AttachmentImage>, jira_application::ApplicationError>>>,
        cancel_after_first: bool,
    }

    impl FakeImageService {
        fn push_response(
            &self,
            response: Result<AttachmentImage, jira_application::ApplicationError>,
        ) {
            self.responses
                .lock()
                .expect("response lock")
                .push_back(response);
        }

        fn push_cached_response(
            &self,
            response: Result<Option<AttachmentImage>, jira_application::ApplicationError>,
        ) {
            self.cached_responses
                .lock()
                .expect("cached response lock")
                .push_back(response);
        }

        fn request_ids(&self) -> Vec<String> {
            self.requests
                .lock()
                .expect("request lock")
                .iter()
                .map(|request| request.attachment_id.clone())
                .collect()
        }

        fn call_count(&self) -> usize {
            self.requests.lock().expect("request lock").len()
        }
    }

    impl AuthenticatedImageService for FakeImageService {
        fn fetch_attachment_image<'a>(
            &'a self,
            request: AttachmentImageRequest,
            cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, AttachmentImage> {
            let first = self.requests.lock().expect("request lock").is_empty();
            self.requests.lock().expect("request lock").push(request);
            if first && self.cancel_after_first {
                cancellation.cancel();
            }
            let response = self
                .responses
                .lock()
                .expect("response lock")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(jira_application::ApplicationError::new(
                        jira_application::ErrorKind::Internal,
                        "missing fake response",
                    ))
                });
            Box::pin(async move { response })
        }

        fn cached_attachment_image<'a>(
            &'a self,
            _request: AttachmentImageRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Option<AttachmentImage>> {
            let response = self
                .cached_responses
                .lock()
                .expect("cached response lock")
                .pop_front()
                .unwrap_or(Ok(None));
            Box::pin(async move { response })
        }
    }

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "jira-gpui-loader-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("diagnostics temp directory");
            Self(path)
        }

        fn lines(&self) -> Vec<String> {
            fs::read_to_string(self.0.join("diagnostics.jsonl"))
                .expect("diagnostics JSONL")
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn image(id: &str) -> RichImage {
        RichImage {
            attachment_id: id.to_owned(),
            filename: format!("{id}.png"),
            mime_type: "image/png".to_owned(),
            alt_text: None,
            width: None,
            height: None,
        }
    }

    fn png_image(id: &str, byte_count: usize) -> AttachmentImage {
        let mut bytes = vec![0; byte_count];
        if byte_count >= 8 {
            bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        }
        AttachmentImage {
            attachment_id: id.to_owned(),
            mime_type: "image/png".to_owned(),
            bytes,
        }
    }

    fn run_loader(
        fake: &FakeImageService,
        images: Vec<(RichImage, usize, ImageSource)>,
        cancellation: CancellationToken,
        diagnostics: DiagnosticsSink,
    ) -> Result<RichImageRenderStates, ()> {
        block_on(fetch_rich_image_states_with_loader(
            fake,
            JiraSiteId::new("site").expect("site"),
            IssueId::new("issue").expect("issue"),
            images,
            cancellation,
            diagnostics,
            DiagnosticFlow::SelectedDetail,
            7,
        ))
    }

    fn event_names(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.split("\"event\":\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .expect("event name")
                    .to_owned()
            })
            .collect()
    }

    fn assert_context(line: &str, candidate: usize, surface: usize, source: &str) {
        assert!(line.contains(&format!(r#""candidate":{candidate}"#)));
        assert!(line.contains(&format!(r#""surface":{surface}"#)));
        assert!(line.contains(&format!(r#""source":"{source}"#)));
    }

    #[test]
    fn loader_requests_multiple_successful_candidates_in_exact_order() {
        let fake = FakeImageService::default();
        for id in ["first", "second", "third"] {
            fake.push_response(Ok(png_image(id, 16)));
        }
        let states = run_loader(
            &fake,
            vec![
                (image("first"), 0, ImageSource::ResolvedAdf),
                (image("second"), 1, ImageSource::FallbackCandidate),
                (image("third"), 2, ImageSource::ResolvedAdf),
            ],
            CancellationToken::new(),
            DiagnosticsSink::disabled(),
        )
        .expect("all candidates succeed");

        assert_eq!(fake.call_count(), 3);
        assert_eq!(fake.request_ids(), ["first", "second", "third"]);
        let requests = fake.requests.lock().expect("request lock");
        assert!(requests.iter().all(|request| {
            request.site_id == JiraSiteId::new("site").expect("site")
                && request.issue_id == IssueId::new("issue").expect("issue")
                && request.width == policy::MAX_IMAGE_REQUEST_WIDTH
                && request.height == policy::MAX_IMAGE_REQUEST_HEIGHT
                && request.max_bytes == policy::MAX_IMAGE_REQUEST_BYTES
        }));
        for id in ["first", "second", "third"] {
            assert!(matches!(
                states.get(id),
                Some(RichImageRenderState::Ready(_))
            ));
        }
    }

    #[test]
    fn cached_loader_hydrates_ready_state_without_using_authenticated_fetch() {
        let fake = FakeImageService::default();
        fake.push_cached_response(Ok(Some(png_image("cached", 16))));
        let states = block_on(fetch_cached_rich_image_states_with_loader(
            &fake,
            JiraSiteId::new("site").expect("site"),
            IssueId::new("issue").expect("issue"),
            vec![(image("cached"), 0, ImageSource::ResolvedAdf)],
            CancellationToken::new(),
            DiagnosticsSink::disabled(),
            DiagnosticFlow::SelectedDetail,
            11,
        ))
        .expect("cached hydration");
        assert_eq!(fake.call_count(), 0);
        assert!(matches!(
            states.get("cached"),
            Some(RichImageRenderState::Ready(_))
        ));
    }

    #[test]
    fn first_service_failure_is_recorded_once_then_later_candidate_succeeds() {
        let fake = FakeImageService::default();
        fake.push_response(Err(jira_application::ApplicationError::new(
            jira_application::ErrorKind::Offline,
            "secret service failure payload",
        )));
        fake.push_response(Ok(png_image("later", 16)));

        let states = run_loader(
            &fake,
            vec![
                (image("failed"), 3, ImageSource::ResolvedAdf),
                (image("later"), 3, ImageSource::FallbackCandidate),
            ],
            CancellationToken::new(),
            DiagnosticsSink::disabled(),
        )
        .expect("later candidate succeeds");

        assert_eq!(fake.call_count(), 2);
        assert_eq!(fake.request_ids(), ["failed", "later"]);
        assert!(matches!(
            states.get("failed"),
            Some(RichImageRenderState::Failed)
        ));
        assert!(matches!(
            states.get("later"),
            Some(RichImageRenderState::Ready(_))
        ));
    }

    #[test]
    fn cancellation_before_first_request_makes_zero_calls_and_records_cancelled_state() {
        let fake = FakeImageService::default();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let directory = TempDirectory::new("cancel-before-first");
        let result = run_loader(
            &fake,
            vec![(image("cancelled"), 5, ImageSource::FallbackCandidate)],
            cancellation,
            DiagnosticsSink::for_directory(&directory.0),
        );

        assert!(result.is_err());
        assert_eq!(fake.call_count(), 0);
        let lines = directory.lines();
        assert_eq!(event_names(&lines), ["image_fetch_result", "image_state"]);
        assert_context(&lines[0], 0, 5, "fallback_candidate");
        assert!(lines[0].contains(r#""error":"cancelled"#));
        assert_context(&lines[1], 0, 5, "fallback_candidate");
        assert!(lines[1].contains(r#""reason":"cancelled"#));
    }

    #[test]
    fn cancellation_after_first_success_stops_later_requests() {
        let fake = FakeImageService {
            cancel_after_first: true,
            ..Default::default()
        };
        fake.push_response(Ok(png_image("first", 16)));
        fake.push_response(Ok(png_image("second", 16)));
        let result = run_loader(
            &fake,
            vec![
                (image("first"), 0, ImageSource::ResolvedAdf),
                (image("second"), 1, ImageSource::FallbackCandidate),
            ],
            CancellationToken::new(),
            DiagnosticsSink::disabled(),
        );

        assert!(result.is_err());
        assert_eq!(fake.call_count(), 1);
        assert_eq!(fake.request_ids(), ["first"]);
    }

    #[test]
    fn aggregate_cap_accepts_exact_boundary_then_rejects_next_sequential_response() {
        let fake = FakeImageService::default();
        let per_image = 2 * 1024 * 1024;
        for index in 0..16 {
            fake.push_response(Ok(png_image(&format!("image-{index}"), per_image)));
        }
        fake.push_response(Ok(png_image("over-cap", 1)));
        let images = (0..17)
            .map(|index| {
                let id = if index == 16 {
                    "over-cap".to_owned()
                } else {
                    format!("image-{index}")
                };
                (
                    image(&id),
                    index,
                    if index % 2 == 0 {
                        ImageSource::ResolvedAdf
                    } else {
                        ImageSource::FallbackCandidate
                    },
                )
            })
            .collect();

        let states = run_loader(
            &fake,
            images,
            CancellationToken::new(),
            DiagnosticsSink::disabled(),
        )
        .expect("aggregate rejection is nonfatal");
        assert_eq!(fake.call_count(), 17);
        assert_eq!(
            fake.request_ids().last().map(String::as_str),
            Some("over-cap")
        );
        for index in 0..16 {
            assert!(matches!(
                states.get(&format!("image-{index}")),
                Some(RichImageRenderState::Ready(_))
            ));
        }
        assert!(matches!(
            states.get("over-cap"),
            Some(RichImageRenderState::Failed)
        ));
    }

    #[test]
    fn diagnostics_have_exact_order_safe_categories_and_aligned_context() {
        let cases = [
            (
                "unsupported",
                vec![
                    "image_fetch_started",
                    "image_response",
                    "image_fetch_result",
                    "image_state",
                ],
            ),
            (
                "failure",
                vec!["image_fetch_started", "image_fetch_result", "image_state"],
            ),
            (
                "success",
                vec![
                    "image_fetch_started",
                    "image_response",
                    "image_fetch_result",
                    "image_state",
                ],
            ),
            (
                "rejection",
                vec![
                    "image_fetch_started",
                    "image_response",
                    "image_fetch_result",
                    "image_state",
                ],
            ),
            ("cancel", vec!["image_fetch_result", "image_state"]),
        ];
        for (label, expected_events) in cases {
            let (fake, _directory, lines) = run_diagnostic_case(label);
            assert_diagnostic_case(label, &expected_events, &fake, &lines);
        }
    }

    fn run_diagnostic_case(label: &str) -> (FakeImageService, TempDirectory, Vec<String>) {
        let directory = TempDirectory::new(label);
        let fake = FakeImageService::default();
        let mut candidate = image(label);
        if label == "unsupported" {
            candidate.mime_type = "application/pdf".to_owned();
        }
        if label == "failure" {
            fake.push_response(Err(jira_application::ApplicationError::new(
                jira_application::ErrorKind::Offline,
                "secret raw service message",
            )));
        } else if label == "success" {
            fake.push_response(Ok(png_image(label, 16)));
        } else if label == "rejection" {
            let mut response = png_image(label, 16);
            response.mime_type = "text/html".to_owned();
            fake.push_response(Ok(response));
        }
        let cancellation = CancellationToken::new();
        if label == "cancel" {
            cancellation.cancel();
        }
        let _ = run_loader(
            &fake,
            vec![(candidate, 9, ImageSource::FallbackCandidate)],
            cancellation,
            DiagnosticsSink::for_directory(&directory.0),
        );
        let lines = directory.lines();
        (fake, directory, lines)
    }

    fn assert_diagnostic_case(
        label: &str,
        expected_events: &[&str],
        fake: &FakeImageService,
        lines: &[String],
    ) {
        assert_eq!(event_names(lines), expected_events);
        assert!(lines.iter().all(|line| {
            !line.contains("secret")
                && !line.contains("raw service")
                && !line.contains("attachment-")
        }));
        for line in lines {
            assert_context(line, 0, 9, "fallback_candidate");
        }
        match label {
            "unsupported" => assert_unsupported_diagnostic_case(fake, lines),
            "failure" => assert_failure_diagnostic_case(fake, lines),
            "success" => assert_success_diagnostic_case(fake, lines),
            "rejection" => assert_rejection_diagnostic_case(fake, lines),
            "cancel" => assert_cancel_diagnostic_case(fake, lines),
            _ => unreachable!(),
        }
    }

    fn assert_unsupported_diagnostic_case(fake: &FakeImageService, lines: &[String]) {
        assert!(lines[1].contains(r#""preflight":"unsupported_cached_mime"#));
        assert!(lines[2].contains(r#""error":"invalid_input"#));
        assert!(lines[3].contains(r#""reason":"unsupported"#));
        assert_eq!(fake.call_count(), 0);
    }

    fn assert_failure_diagnostic_case(fake: &FakeImageService, lines: &[String]) {
        assert!(lines[1].contains(r#""error":"offline"#));
        assert!(lines[2].contains(r#""reason":"failed"#));
        assert_eq!(fake.call_count(), 1);
    }

    fn assert_success_diagnostic_case(fake: &FakeImageService, lines: &[String]) {
        assert!(lines[1].contains(r#""preflight":"accepted"#));
        assert!(lines[2].contains(r#""outcome":"succeeded"#));
        assert!(lines[3].contains(r#""reason":"ready"#));
        assert_eq!(fake.call_count(), 1);
    }

    fn assert_rejection_diagnostic_case(fake: &FakeImageService, lines: &[String]) {
        assert!(lines[1].contains(r#""preflight":"response_mime_rejected"#));
        assert!(lines[2].contains(r#""error":"invalid_input"#));
        assert!(lines[3].contains(r#""reason":"failed"#));
        assert_eq!(fake.call_count(), 1);
    }

    fn assert_cancel_diagnostic_case(fake: &FakeImageService, lines: &[String]) {
        assert!(lines[0].contains(r#""error":"cancelled"#));
        assert!(lines[1].contains(r#""reason":"cancelled"#));
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn diagnostics_adapters_map_every_neutral_policy_variant_losslessly() {
        for (policy, diagnostic) in [
            (policy::MediaMime::Png, ResponseMime::Png),
            (policy::MediaMime::Jpeg, ResponseMime::Jpeg),
            (policy::MediaMime::Gif, ResponseMime::Gif),
            (policy::MediaMime::Webp, ResponseMime::Webp),
            (policy::MediaMime::OctetStream, ResponseMime::OctetStream),
            (policy::MediaMime::Unsupported, ResponseMime::Unsupported),
        ] {
            assert_eq!(diagnostic_mime(policy), diagnostic);
        }
        for (policy, diagnostic) in [
            (policy::MediaSignature::Png, ImageSignature::Png),
            (policy::MediaSignature::Jpeg, ImageSignature::Jpeg),
            (policy::MediaSignature::Gif, ImageSignature::Gif),
            (policy::MediaSignature::Webp, ImageSignature::Webp),
            (policy::MediaSignature::Unknown, ImageSignature::Unknown),
        ] {
            assert_eq!(diagnostic_signature(policy), diagnostic);
        }
        for (policy, diagnostic) in [
            (policy::MediaPreflight::Accepted, ImagePreflight::Accepted),
            (policy::MediaPreflight::Empty, ImagePreflight::Empty),
            (
                policy::MediaPreflight::UnsupportedCachedMime,
                ImagePreflight::UnsupportedCachedMime,
            ),
            (
                policy::MediaPreflight::ResponseMimeRejected,
                ImagePreflight::ResponseMimeRejected,
            ),
            (
                policy::MediaPreflight::SignatureRejected,
                ImagePreflight::SignatureRejected,
            ),
            (
                policy::MediaPreflight::AggregateRejected,
                ImagePreflight::AggregateRejected,
            ),
        ] {
            assert_eq!(diagnostic_preflight(policy), diagnostic);
        }
    }
}
