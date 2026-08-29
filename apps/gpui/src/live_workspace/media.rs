use jira_application::{
    ApplicationError, AttachmentContent, AttachmentDownloadRequest, AttachmentImage,
    AttachmentImageRequest, CancellationToken, IssueMediaService,
};

/// Private implementation seam for the workspace's two media facades. Keeping
/// the service opaque here preserves the workspace's one-call delegation and
/// leaves fallback, validation, and retry policy in the application service.
pub(super) async fn fetch_attachment_image(
    service: &IssueMediaService,
    request: AttachmentImageRequest,
    cancellation: &CancellationToken,
) -> Result<AttachmentImage, ApplicationError> {
    service.fetch(request, cancellation).await
}

pub(super) async fn cached_attachment_image(
    service: &IssueMediaService,
    request: AttachmentImageRequest,
    cancellation: &CancellationToken,
) -> Result<Option<AttachmentImage>, ApplicationError> {
    service.cached(request, cancellation).await
}

pub(super) async fn download_attachment(
    service: &IssueMediaService,
    request: AttachmentDownloadRequest,
    cancellation: &CancellationToken,
) -> Result<AttachmentContent, ApplicationError> {
    service.download(request, cancellation).await
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use futures_lite::future::block_on;
    use jira_application::{JiraAttachmentReadPort, PortFuture};
    use jira_domain::{IssueId, JiraSiteId};

    use super::*;

    struct FakeAttachmentPort {
        image_calls: AtomicUsize,
        download_calls: AtomicUsize,
    }

    impl JiraAttachmentReadPort for FakeAttachmentPort {
        fn fetch_attachment_image<'a>(
            &'a self,
            request: &'a AttachmentImageRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, AttachmentImage> {
            self.image_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(AttachmentImage {
                    attachment_id: request.attachment_id.clone(),
                    mime_type: "image/png".to_owned(),
                    bytes: b"\x89PNG\r\n\x1a\nvalid".to_vec(),
                })
            })
        }

        fn fetch_attachment_content<'a>(
            &'a self,
            request: &'a AttachmentDownloadRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, AttachmentContent> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(AttachmentContent {
                    attachment_id: request.attachment_id.clone(),
                    mime_type: "application/pdf".to_owned(),
                    bytes: b"pdf".to_vec(),
                })
            })
        }
    }

    #[test]
    fn media_service_facade_helpers_delegate_once_and_propagate_service_results() {
        let port = Arc::new(FakeAttachmentPort {
            image_calls: AtomicUsize::new(0),
            download_calls: AtomicUsize::new(0),
        });
        let service = IssueMediaService::new(port.clone(), Default::default());
        let site_id = JiraSiteId::new("site").expect("site");
        let issue_id = IssueId::new("issue").expect("issue");
        let cancellation = CancellationToken::new();

        let image = block_on(fetch_attachment_image(
            &service,
            AttachmentImageRequest {
                site_id: site_id.clone(),
                issue_id: issue_id.clone(),
                attachment_id: "image-1".to_owned(),
                width: 1,
                height: 1,
                max_bytes: 1,
            },
            &cancellation,
        ))
        .expect("image result");
        let content = block_on(download_attachment(
            &service,
            AttachmentDownloadRequest {
                site_id,
                issue_id,
                attachment_id: "file-1".to_owned(),
                max_bytes: 64 * 1024,
            },
            &cancellation,
        ))
        .expect("download result");

        assert_eq!(image.attachment_id, "image-1");
        assert_eq!(content.attachment_id, "file-1");
        assert_eq!(port.image_calls.load(Ordering::SeqCst), 1);
        assert_eq!(port.download_calls.load(Ordering::SeqCst), 1);
    }
}
