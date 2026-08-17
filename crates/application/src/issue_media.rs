use std::sync::Arc;

use crate::{
    ApplicationError, AttachmentContent, AttachmentDownloadRequest, AttachmentImage,
    AttachmentImageRequest, CancellationToken, ErrorKind, JiraReadPort,
};

pub const DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_ATTACHMENT_IMAGE_WIDTH: usize = 1_600;
pub const DEFAULT_ATTACHMENT_IMAGE_HEIGHT: usize = 1_200;
pub const DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_ATTACHMENT_ID_BYTES: usize = 255;

/// Bounds used by the application when requesting and accepting attachment thumbnails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IssueMediaConfig {
    pub max_bytes: usize,
    pub width: usize,
    pub height: usize,
}

impl Default for IssueMediaConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES,
            width: DEFAULT_ATTACHMENT_IMAGE_WIDTH,
            height: DEFAULT_ATTACHMENT_IMAGE_HEIGHT,
        }
    }
}

/// Read-only application orchestration for one Jira issue image attachment.
#[derive(Clone)]
pub struct IssueMediaService {
    jira: Arc<dyn JiraReadPort>,
    config: IssueMediaConfig,
}

impl IssueMediaService {
    pub fn new(jira: Arc<dyn JiraReadPort>, config: IssueMediaConfig) -> Self {
        Self { jira, config }
    }

    pub async fn fetch(
        &self,
        request: AttachmentImageRequest,
        cancellation: &CancellationToken,
    ) -> Result<AttachmentImage, ApplicationError> {
        self.validate_config()?;
        validate_attachment_id(&request.attachment_id)?;
        cancellation.check()?;
        let requested_attachment_id = request.attachment_id.clone();

        let port_request = AttachmentImageRequest {
            width: self.config.width,
            height: self.config.height,
            max_bytes: self.config.max_bytes,
            ..request
        };

        let image = match self
            .jira
            .fetch_attachment_image(&port_request, cancellation)
            .await
        {
            Ok(image) => image,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                cancellation.check()?;
                let content_request = AttachmentDownloadRequest {
                    site_id: port_request.site_id.clone(),
                    issue_id: port_request.issue_id.clone(),
                    attachment_id: requested_attachment_id.clone(),
                    max_bytes: self
                        .config
                        .max_bytes
                        .min(DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES),
                };
                let content = self
                    .jira
                    .fetch_attachment_content(&content_request, cancellation)
                    .await?;
                cancellation.check()?;
                AttachmentImage {
                    attachment_id: content.attachment_id,
                    mime_type: content.mime_type,
                    bytes: content.bytes,
                }
            }
            Err(error) => return Err(error),
        };

        validate_image(&image, &requested_attachment_id, self.config.max_bytes)?;

        cancellation.check()?;
        Ok(image)
    }

    pub async fn load(
        &self,
        request: AttachmentImageRequest,
        cancellation: &CancellationToken,
    ) -> Result<AttachmentImage, ApplicationError> {
        self.fetch(request, cancellation).await
    }

    pub async fn fetch_attachment_image(
        &self,
        request: AttachmentImageRequest,
        cancellation: &CancellationToken,
    ) -> Result<AttachmentImage, ApplicationError> {
        self.fetch(request, cancellation).await
    }

    pub async fn download(
        &self,
        request: AttachmentDownloadRequest,
        cancellation: &CancellationToken,
    ) -> Result<AttachmentContent, ApplicationError> {
        self.validate_config()?;
        validate_attachment_id(&request.attachment_id)?;
        if request.max_bytes == 0 || request.max_bytes > DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES {
            return Err(ApplicationError::invalid_input(
                "attachment download limit is invalid",
            ));
        }
        cancellation.check()?;
        let requested_attachment_id = request.attachment_id.clone();

        let port_request = request;

        let attachment = self
            .jira
            .fetch_attachment_content(&port_request, cancellation)
            .await?;
        cancellation.check()?;

        if attachment.attachment_id != requested_attachment_id {
            return Err(upstream("Jira returned a different attachment"));
        }
        validate_attachment_id(&attachment.attachment_id)?;
        if attachment.mime_type.trim().is_empty() {
            return Err(upstream("Jira returned an attachment without a media type"));
        }
        if attachment.bytes.is_empty() {
            return Err(upstream("Jira returned an empty attachment"));
        }
        if attachment.bytes.len() > port_request.max_bytes {
            return Err(upstream("Jira attachment exceeded the size limit"));
        }

        cancellation.check()?;
        Ok(attachment)
    }

    pub async fn fetch_attachment(
        &self,
        request: AttachmentDownloadRequest,
        cancellation: &CancellationToken,
    ) -> Result<AttachmentContent, ApplicationError> {
        self.download(request, cancellation).await
    }

    fn validate_config(&self) -> Result<(), ApplicationError> {
        if self.config.max_bytes == 0
            || self.config.max_bytes > DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES
            || self.config.width == 0
            || self.config.width > DEFAULT_ATTACHMENT_IMAGE_WIDTH
            || self.config.height == 0
            || self.config.height > DEFAULT_ATTACHMENT_IMAGE_HEIGHT
        {
            return Err(ApplicationError::invalid_input(
                "issue media configuration is invalid",
            ));
        }
        Ok(())
    }
}

fn validate_attachment_id(value: &str) -> Result<(), ApplicationError> {
    if value.trim().is_empty() {
        return Err(ApplicationError::invalid_input(
            "attachment ID must not be empty",
        ));
    }
    if value.len() > MAX_ATTACHMENT_ID_BYTES {
        return Err(ApplicationError::invalid_input(
            "attachment ID exceeds the maximum length",
        ));
    }
    Ok(())
}

fn is_allowed_image_mime(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "application/octet-stream"
            | "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
    )
}

fn validate_image(
    image: &AttachmentImage,
    requested_attachment_id: &str,
    max_bytes: usize,
) -> Result<(), ApplicationError> {
    if image.attachment_id != requested_attachment_id {
        return Err(upstream("Jira returned a different attachment"));
    }
    validate_attachment_id(&image.attachment_id)?;
    if !is_allowed_image_mime(&image.mime_type) {
        return Err(upstream("Jira returned an unsupported image type"));
    }
    if image.bytes.is_empty() {
        return Err(upstream("Jira returned an empty attachment image"));
    }
    if image.bytes.len() > max_bytes {
        return Err(upstream("Jira attachment image exceeded the size limit"));
    }
    Ok(())
}

fn upstream(message: &'static str) -> ApplicationError {
    ApplicationError::new(ErrorKind::Upstream, message)
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        sync::{Arc, Mutex},
        task::{Context, Poll, Wake, Waker},
    };

    use jira_domain::{IssueId, JiraSiteId, User};

    use super::*;
    use crate::{IssueFetchRequest, IssuePage, PortFuture, UserSearchRequest};

    struct FakeJira {
        image: Mutex<Option<Result<AttachmentImage, ApplicationError>>>,
        download: Mutex<Option<Result<AttachmentContent, ApplicationError>>>,
        image_request: Mutex<Option<AttachmentImageRequest>>,
        download_request: Mutex<Option<AttachmentDownloadRequest>>,
        cancellation: Mutex<Option<CancellationToken>>,
    }

    impl JiraReadPort for FakeJira {
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
            let result = self
                .download
                .lock()
                .expect("download lock")
                .take()
                .expect("download");
            Box::pin(async move { result })
        }

        fn fetch_current_user<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, User> {
            Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "not used")) })
        }

        fn search_users<'a>(
            &'a self,
            _request: &'a UserSearchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<User>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn fetch_issue_page<'a>(
            &'a self,
            _request: &'a IssueFetchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            Box::pin(async {
                Ok(IssuePage {
                    issues: Vec::new(),
                    next_cursor: None,
                    server_time: None,
                })
            })
        }

        fn fetch_issues_by_id<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _issue_ids: &'a [IssueId],
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<jira_domain::Issue>> {
            Box::pin(async { Ok(Vec::new()) })
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
            Arc::new(FakeJira {
                image: Mutex::new(Some(image)),
                download: Mutex::new(None),
                image_request: Mutex::new(None),
                download_request: Mutex::new(None),
                cancellation: Mutex::new(None),
            }),
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
            Arc::new(FakeJira {
                image: Mutex::new(Some(Ok(image("att-1", "application/octet-stream", bytes)))),
                download: Mutex::new(None),
                image_request: Mutex::new(None),
                download_request: Mutex::new(None),
                cancellation: Mutex::new(None),
            }),
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
            Arc::new(FakeJira {
                image: Mutex::new(Some(Ok(image("att-1", "image/jpg", bytes)))),
                download: Mutex::new(None),
                image_request: Mutex::new(None),
                download_request: Mutex::new(None),
                cancellation: Mutex::new(None),
            }),
            IssueMediaConfig::default(),
        );

        let result = block_on(service.fetch(request("att-1"), &CancellationToken::new()))
            .expect("direct image/jpg thumbnail");

        assert_eq!(result.mime_type, "image/jpg");
        assert_eq!(result.bytes, bytes);
    }

    #[test]
    fn falls_back_to_bounded_original_content_when_thumbnail_is_not_found() {
        let fake = Arc::new(FakeJira {
            image: Mutex::new(Some(Err(ApplicationError::new(
                ErrorKind::NotFound,
                "thumbnail unavailable",
            )))),
            download: Mutex::new(Some(Ok(content("att-1", "IMAGE/PNG", b"png")))),
            image_request: Mutex::new(None),
            download_request: Mutex::new(None),
            cancellation: Mutex::new(None),
        });
        let fallback_service = IssueMediaService::new(
            Arc::clone(&fake) as Arc<dyn JiraReadPort>,
            IssueMediaConfig {
                max_bytes: 4,
                ..IssueMediaConfig::default()
            },
        );
        let image_request = request("att-1");

        let result =
            block_on(fallback_service.fetch(image_request.clone(), &CancellationToken::new()))
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
    }

    #[test]
    fn allows_octet_stream_for_original_content_fallback() {
        let fake = Arc::new(FakeJira {
            image: Mutex::new(Some(Err(ApplicationError::new(
                ErrorKind::NotFound,
                "thumbnail unavailable",
            )))),
            download: Mutex::new(Some(Ok(content(
                "att-1",
                "application/octet-stream",
                b"original bytes",
            )))),
            image_request: Mutex::new(None),
            download_request: Mutex::new(None),
            cancellation: Mutex::new(None),
        });
        let fallback_service = IssueMediaService::new(
            Arc::clone(&fake) as Arc<dyn JiraReadPort>,
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
        let fake = Arc::new(FakeJira {
            image: Mutex::new(Some(Err(ApplicationError::new(
                ErrorKind::NotFound,
                "thumbnail unavailable",
            )))),
            download: Mutex::new(Some(Err(ApplicationError::cancelled()))),
            image_request: Mutex::new(None),
            download_request: Mutex::new(None),
            cancellation: Mutex::new(None),
        });
        let service = IssueMediaService::new(
            Arc::clone(&fake) as Arc<dyn JiraReadPort>,
            IssueMediaConfig::default(),
        );

        let error = block_on(service.fetch(request("att-1"), &CancellationToken::new()))
            .expect_err("cancelled original content must be rejected");

        assert_eq!(error.kind(), ErrorKind::Cancelled);
    }

    #[test]
    fn service_materializes_non_default_thumbnail_bounds_for_the_port() {
        let fake = Arc::new(FakeJira {
            image: Mutex::new(Some(Ok(image("att-1", "image/png", b"png")))),
            download: Mutex::new(None),
            image_request: Mutex::new(None),
            download_request: Mutex::new(None),
            cancellation: Mutex::new(None),
        });
        let service = IssueMediaService::new(
            Arc::clone(&fake) as Arc<dyn JiraReadPort>,
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
            let error = block_on(
                service(Ok(response)).fetch(request(request_id), &CancellationToken::new()),
            )
            .expect_err("invalid attachment image should be rejected");
            assert_eq!(error.kind(), kind);
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
        let fake = Arc::new(FakeJira {
            image: Mutex::new(Some(Ok(image("att-1", "image/png", b"x")))),
            download: Mutex::new(None),
            image_request: Mutex::new(None),
            download_request: Mutex::new(None),
            cancellation: Mutex::new(None),
        });
        let service = IssueMediaService::new(
            Arc::clone(&fake) as Arc<dyn JiraReadPort>,
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
            Arc::new(FakeJira {
                image: Mutex::new(None),
                download: Mutex::new(Some(Ok(content("att-1", "application/pdf", b"pdf")))),
                image_request: Mutex::new(None),
                download_request: Mutex::new(None),
                cancellation: Mutex::new(None),
            }),
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
