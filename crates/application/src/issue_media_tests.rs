use std::sync::{Arc, Mutex};

use jira_domain::{IssueId, JiraSiteId};

use super::*;
use crate::{AttachmentReadAttempt, AttachmentReadStage, PortFuture, test_support::block_on};

struct FakeJira {
    image: Mutex<Option<Result<AttachmentImage, ApplicationError>>>,
    download: Mutex<Option<Result<AttachmentContent, ApplicationError>>>,
    image_request: Mutex<Option<AttachmentImageRequest>>,
    download_request: Mutex<Option<AttachmentDownloadRequest>>,
    download_calls: Mutex<usize>,
    cancellation: Mutex<Option<CancellationToken>>,
}

impl FakeJira {
    fn new(
        image: Option<Result<AttachmentImage, ApplicationError>>,
        download: Option<Result<AttachmentContent, ApplicationError>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            image: Mutex::new(image),
            download: Mutex::new(download),
            image_request: Mutex::new(None),
            download_request: Mutex::new(None),
            download_calls: Mutex::new(0),
            cancellation: Mutex::new(None),
        })
    }
}

impl JiraAttachmentReadPort for FakeJira {
    fn fetch_attachment_image<'a>(
        &'a self,
        request: &'a AttachmentImageRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentImage> {
        *self.image_request.lock().expect("image request lock") = Some(request.clone());
        *self.cancellation.lock().expect("cancel lock") = Some(cancellation.clone());
        let result = self
            .image
            .lock()
            .expect("image lock")
            .take()
            .expect("image");
        Box::pin(async move { result })
    }

    fn fetch_attachment_content<'a>(
        &'a self,
        request: &'a AttachmentDownloadRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentContent> {
        *self.download_request.lock().expect("download request lock") = Some(request.clone());
        *self.download_calls.lock().expect("download calls lock") += 1;
        let result = self
            .download
            .lock()
            .expect("download lock")
            .take()
            .expect("download");
        Box::pin(async move { result })
    }
}

fn request(id: &str) -> AttachmentImageRequest {
    AttachmentImageRequest {
        site_id: JiraSiteId::new("site").expect("site"),
        issue_id: IssueId::new("100").expect("issue"),
        attachment_id: id.to_owned(),
        width: DEFAULT_ATTACHMENT_IMAGE_WIDTH,
        height: DEFAULT_ATTACHMENT_IMAGE_HEIGHT,
        max_bytes: DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES,
    }
}

fn service(image: Result<AttachmentImage, ApplicationError>) -> IssueMediaService {
    IssueMediaService::new(
        FakeJira::new(Some(image), None),
        IssueMediaConfig {
            max_bytes: 4,
            ..IssueMediaConfig::default()
        },
    )
}

fn image(id: &str, mime: &str, bytes: &[u8]) -> AttachmentImage {
    AttachmentImage {
        attachment_id: id.to_owned(),
        mime_type: mime.to_owned(),
        bytes: bytes.to_vec(),
    }
}

fn content(id: &str, mime: &str, bytes: &[u8]) -> AttachmentContent {
    AttachmentContent {
        attachment_id: id.to_owned(),
        mime_type: mime.to_owned(),
        bytes: bytes.to_vec(),
    }
}

#[test]
fn defaults_bound_media_requests() {
    let config = IssueMediaConfig::default();
    assert_eq!(config.max_bytes, 8 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES, 64 * 1024 * 1024);
    assert_eq!(config.width, 1_600);
    assert_eq!(config.height, 1_200);
}

#[test]
fn accepts_allowlisted_nonempty_image_within_limit() {
    let service = service(Ok(image("att-1", "IMAGE/PNG", b"png")));
    let result = block_on(service.fetch(request("att-1"), &CancellationToken::new()));
    assert_eq!(result.expect("image").bytes, b"png");
}

#[test]
fn accepts_direct_thumbnail_with_octet_stream_mime_and_png_signature() {
    let bytes = b"\x89PNG\r\n\x1a\n";
    let service = IssueMediaService::new(
        FakeJira::new(
            Some(Ok(image("att-1", "application/octet-stream", bytes))),
            None,
        ),
        IssueMediaConfig::default(),
    );

    let result = block_on(service.fetch(request("att-1"), &CancellationToken::new()))
        .expect("direct octet-stream thumbnail");

    assert_eq!(result.mime_type, "application/octet-stream");
    assert_eq!(result.bytes, bytes);
}

#[test]
fn accepts_direct_thumbnail_with_image_jpg_mime_and_jpeg_signature() {
    let bytes = b"\xff\xd8\xff";
    let service = IssueMediaService::new(
        FakeJira::new(Some(Ok(image("att-1", "image/jpg", bytes))), None),
        IssueMediaConfig::default(),
    );

    let result = block_on(service.fetch(request("att-1"), &CancellationToken::new()))
        .expect("direct image/jpg thumbnail");

    assert_eq!(result.mime_type, "image/jpg");
    assert_eq!(result.bytes, bytes);
}

#[test]
fn falls_back_to_bounded_original_content_when_thumbnail_is_not_found() {
    let fake = FakeJira::new(
        Some(Err(ApplicationError::new(
            ErrorKind::NotFound,
            "thumbnail unavailable",
        )
        .with_attachment_diagnostic(
            AttachmentReadDiagnostic::validation(AttachmentReadAttempt::Thumbnail),
        ))),
        Some(Ok(content("att-1", "IMAGE/PNG", b"png"))),
    );
    let fallback_service = IssueMediaService::new(
        Arc::clone(&fake) as Arc<dyn JiraAttachmentReadPort>,
        IssueMediaConfig {
            max_bytes: 4,
            ..IssueMediaConfig::default()
        },
    );
    let image_request = request("att-1");

    let result = block_on(fallback_service.fetch(image_request.clone(), &CancellationToken::new()))
        .expect("original image content");

    assert_eq!(result, image("att-1", "IMAGE/PNG", b"png"));
    assert_eq!(
        fake.download_request
            .lock()
            .expect("download request lock")
            .clone()
            .expect("download request"),
        AttachmentDownloadRequest {
            site_id: image_request.site_id,
            issue_id: image_request.issue_id,
            attachment_id: "att-1".to_owned(),
            max_bytes: 4,
        }
    );
    assert_eq!(
        *fake.download_calls.lock().expect("download calls lock"),
        1,
        "thumbnail fallback must issue exactly one bounded original request"
    );
}

#[test]
fn allows_octet_stream_for_original_content_fallback() {
    let fake = FakeJira::new(
        Some(Err(ApplicationError::new(
            ErrorKind::NotFound,
            "thumbnail unavailable",
        ))),
        Some(Ok(content(
            "att-1",
            "application/octet-stream",
            b"original bytes",
        ))),
    );
    let fallback_service = IssueMediaService::new(
        Arc::clone(&fake) as Arc<dyn JiraAttachmentReadPort>,
        IssueMediaConfig::default(),
    );

    let result = block_on(fallback_service.fetch(request("att-1"), &CancellationToken::new()))
        .expect("octet-stream original fallback");
    assert_eq!(result.mime_type, "application/octet-stream");
}

#[test]
fn does_not_fallback_for_non_not_found_thumbnail_errors() {
    for kind in [
        ErrorKind::Authentication,
        ErrorKind::Authorization,
        ErrorKind::RateLimited,
        ErrorKind::Offline,
        ErrorKind::Cancelled,
        ErrorKind::InvalidInput,
        ErrorKind::UnknownOutcome,
        ErrorKind::Storage,
        ErrorKind::Upstream,
        ErrorKind::Notification,
        ErrorKind::Internal,
    ] {
        let error = block_on(
            service(Err(ApplicationError::new(kind, "thumbnail failed")))
                .fetch(request("att-1"), &CancellationToken::new()),
        )
        .expect_err("non-NotFound thumbnail errors must not fall back");
        assert_eq!(error.kind(), kind);
    }
}

#[test]
fn rejects_cancelled_original_content_fallback() {
    let fake = FakeJira::new(
        Some(Err(ApplicationError::new(
            ErrorKind::NotFound,
            "thumbnail unavailable",
        ))),
        Some(Err(ApplicationError::cancelled())),
    );
    let service = IssueMediaService::new(
        Arc::clone(&fake) as Arc<dyn JiraAttachmentReadPort>,
        IssueMediaConfig::default(),
    );

    let error = block_on(service.fetch(request("att-1"), &CancellationToken::new()))
        .expect_err("cancelled original content must be rejected");

    assert_eq!(error.kind(), ErrorKind::Cancelled);
}

#[test]
fn relabels_original_fallback_error_without_changing_error_semantics() {
    let fake = FakeJira::new(
        Some(Err(ApplicationError::new(
            ErrorKind::NotFound,
            "thumbnail unavailable",
        ))),
        Some(Err(ApplicationError::new(
            ErrorKind::Upstream,
            "content failed",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::body(
            AttachmentReadAttempt::ExplicitDownload,
            AttachmentBodyClass::ReadFailed,
        )))),
    );
    let service = IssueMediaService::new(
        Arc::clone(&fake) as Arc<dyn JiraAttachmentReadPort>,
        IssueMediaConfig::default(),
    );

    let error = block_on(service.fetch(request("att-1"), &CancellationToken::new()))
        .expect_err("failed original fallback");

    assert_eq!(error.kind(), ErrorKind::Upstream);
    assert_eq!(error.message(), "content failed");
    let diagnostic = error.attachment_diagnostic().expect("fallback diagnostic");
    assert_eq!(
        diagnostic.attempt(),
        AttachmentReadAttempt::OriginalFallback
    );
    assert_eq!(diagnostic.stage(), AttachmentReadStage::Body);
    assert_eq!(
        diagnostic.body_class(),
        Some(AttachmentBodyClass::ReadFailed)
    );
}

#[test]
fn service_materializes_non_default_thumbnail_bounds_for_the_port() {
    let fake = FakeJira::new(Some(Ok(image("att-1", "image/png", b"png"))), None);
    let service = IssueMediaService::new(
        Arc::clone(&fake) as Arc<dyn JiraAttachmentReadPort>,
        IssueMediaConfig {
            max_bytes: 3,
            width: 640,
            height: 480,
        },
    );
    block_on(service.fetch(request("att-1"), &CancellationToken::new())).expect("image");
    let request = fake
        .image_request
        .lock()
        .expect("image request lock")
        .clone()
        .expect("image request");
    assert_eq!(
        (request.width, request.height, request.max_bytes),
        (640, 480, 3)
    );
}

#[test]
fn rejects_invalid_request_response_mime_and_size() {
    for (request_id, response, kind) in [
        ("", image("", "image/png", b"x"), ErrorKind::InvalidInput),
        (
            "att-1",
            image("different", "image/png", b"x"),
            ErrorKind::Upstream,
        ),
        (
            "att-1",
            image("att-1", "text/plain", b"x"),
            ErrorKind::Upstream,
        ),
        (
            "att-1",
            image("att-1", "image/png", b""),
            ErrorKind::Upstream,
        ),
        (
            "att-1",
            image("att-1", "image/png", b"12345"),
            ErrorKind::Upstream,
        ),
    ] {
        let error =
            block_on(service(Ok(response)).fetch(request(request_id), &CancellationToken::new()))
                .expect_err("invalid attachment image should be rejected");
        assert_eq!(error.kind(), kind);
    }
}

#[test]
fn validation_failures_report_safe_attachment_context() {
    let cases = [
        (
            image("different", "image/png", b"x"),
            AttachmentReadStage::Validation,
            None,
            None,
        ),
        (
            image("att-1", "text/plain", b"x"),
            AttachmentReadStage::ContentType,
            Some(AttachmentMimeClass::Other),
            None,
        ),
        (
            image("att-1", "image/png", b""),
            AttachmentReadStage::Body,
            None,
            Some(AttachmentBodyClass::Empty),
        ),
        (
            image("att-1", "image/png", b"12345"),
            AttachmentReadStage::Body,
            None,
            Some(AttachmentBodyClass::TooLarge),
        ),
    ];

    for (response, stage, mime_class, body_class) in cases {
        let error =
            block_on(service(Ok(response)).fetch(request("att-1"), &CancellationToken::new()))
                .expect_err("invalid attachment image should be rejected");
        let diagnostic = error.attachment_diagnostic().expect("diagnostic");
        assert_eq!(diagnostic.attempt(), AttachmentReadAttempt::Thumbnail);
        assert_eq!(diagnostic.stage(), stage);
        assert_eq!(diagnostic.mime_class(), mime_class);
        assert_eq!(diagnostic.body_class(), body_class);
    }
}

#[test]
fn checks_cancellation_before_and_after_port_call() {
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let error = block_on(
        service(Ok(image("att-1", "image/png", b"x"))).fetch(request("att-1"), &cancelled),
    )
    .expect_err("cancelled attachment image should be rejected");
    assert_eq!(error.kind(), ErrorKind::Cancelled);

    let during = CancellationToken::new();
    let fake = FakeJira::new(Some(Ok(image("att-1", "image/png", b"x"))), None);
    let service = IssueMediaService::new(
        Arc::clone(&fake) as Arc<dyn JiraAttachmentReadPort>,
        IssueMediaConfig::default(),
    );
    fake.cancellation
        .lock()
        .expect("cancel lock")
        .replace(during.clone());
    during.cancel();
    let error = block_on(service.fetch(request("att-1"), &during))
        .expect_err("cancelled attachment image should be rejected");
    assert_eq!(error.kind(), ErrorKind::Cancelled);
}

#[test]
fn downloads_original_content_without_image_mime_filter() {
    let service = IssueMediaService::new(
        FakeJira::new(None, Some(Ok(content("att-1", "application/pdf", b"pdf")))),
        IssueMediaConfig::default(),
    );
    let result = block_on(service.download(
        AttachmentDownloadRequest {
            site_id: JiraSiteId::new("site").expect("site"),
            issue_id: IssueId::new("100").expect("issue"),
            attachment_id: "att-1".to_owned(),
            max_bytes: DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES,
        },
        &CancellationToken::new(),
    ))
    .expect("download");
    assert_eq!(result.mime_type, "application/pdf");
}
