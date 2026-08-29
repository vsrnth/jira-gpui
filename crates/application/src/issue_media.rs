use std::sync::Arc;

use crate::{
    ApplicationError, AttachmentBodyClass, AttachmentContent, AttachmentDownloadRequest,
    AttachmentImage, AttachmentImageRequest, AttachmentMimeClass, AttachmentReadAttempt,
    AttachmentReadDiagnostic, CancellationToken, ErrorKind, IssueMediaCachePort,
    JiraAttachmentReadPort,
};

pub const DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_ATTACHMENT_IMAGE_WIDTH: usize = 1_600;
pub const DEFAULT_ATTACHMENT_IMAGE_HEIGHT: usize = 1_200;
pub const DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_ATTACHMENT_ID_BYTES: usize = 255;
pub const MAX_CACHED_ATTACHMENT_IMAGE_BYTES: usize = DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES;
pub const MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES: usize = 1_024;
pub const MAX_CACHED_ATTACHMENT_IMAGE_TOTAL_BYTES: usize = 32 * 1024 * 1024;

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
    jira: Arc<dyn JiraAttachmentReadPort>,
    cache: Option<Arc<dyn IssueMediaCachePort>>,
    config: IssueMediaConfig,
}

impl IssueMediaService {
    pub fn new(jira: Arc<dyn JiraAttachmentReadPort>, config: IssueMediaConfig) -> Self {
        Self {
            jira,
            cache: None,
            config,
        }
    }

    pub fn new_with_cache(
        jira: Arc<dyn JiraAttachmentReadPort>,
        cache: Arc<dyn IssueMediaCachePort>,
        config: IssueMediaConfig,
    ) -> Self {
        Self {
            jira,
            cache: Some(cache),
            config,
        }
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

        if let Some(cache) = &self.cache
            && let Ok(Some(image)) = cache
                .cached_attachment_image(
                    &request.site_id,
                    &request.issue_id,
                    &request.attachment_id,
                )
                .await
        {
            if validate_cached_image(&image, &requested_attachment_id, self.config.max_bytes)
                .is_ok()
            {
                cancellation.check()?;
                return Ok(image);
            }
            let _ = cache
                .remove_cached_attachment_image(
                    &request.site_id,
                    &request.issue_id,
                    &request.attachment_id,
                )
                .await;
        }

        let port_request = AttachmentImageRequest {
            width: self.config.width,
            height: self.config.height,
            max_bytes: self.config.max_bytes,
            ..request
        };

        let (attempt, image) = match self
            .jira
            .fetch_attachment_image(&port_request, cancellation)
            .await
        {
            Ok(image) => (AttachmentReadAttempt::Thumbnail, image),
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
                    .await
                    .map_err(|error| {
                        error.with_attachment_attempt(AttachmentReadAttempt::OriginalFallback)
                    })?;
                cancellation.check()?;
                (
                    AttachmentReadAttempt::OriginalFallback,
                    AttachmentImage {
                        attachment_id: content.attachment_id,
                        mime_type: content.mime_type,
                        bytes: content.bytes,
                    },
                )
            }
            Err(error) => {
                return Err(error.with_attachment_attempt(AttachmentReadAttempt::Thumbnail));
            }
        };

        validate_image(
            &image,
            &requested_attachment_id,
            self.config.max_bytes,
            attempt,
        )?;

        cancellation.check()?;
        if let Some(cache) = &self.cache {
            // The cache is an optimization only. A full or unavailable local
            // cache must never turn a valid Jira response into a UI failure.
            let _ = cache
                .cache_attachment_image(&port_request.site_id, &port_request.issue_id, &image)
                .await;
        }
        Ok(image)
    }

    /// Read one previously authenticated image without contacting Jira. This
    /// is intentionally a cache-only operation so callers can hydrate UI
    /// state before a detail refresh starts.
    pub async fn cached(
        &self,
        request: AttachmentImageRequest,
        cancellation: &CancellationToken,
    ) -> Result<Option<AttachmentImage>, ApplicationError> {
        self.validate_config()?;
        validate_attachment_id(&request.attachment_id)?;
        cancellation.check()?;
        let Some(cache) = &self.cache else {
            return Ok(None);
        };
        let result = cache
            .cached_attachment_image(&request.site_id, &request.issue_id, &request.attachment_id)
            .await;
        let Ok(Some(image)) = result else {
            return Ok(None);
        };
        if validate_cached_image(&image, &request.attachment_id, self.config.max_bytes).is_ok() {
            cancellation.check()?;
            Ok(Some(image))
        } else {
            let _ = cache
                .remove_cached_attachment_image(
                    &request.site_id,
                    &request.issue_id,
                    &request.attachment_id,
                )
                .await;
            Ok(None)
        }
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
            .await
            .map_err(|error| {
                error.with_attachment_attempt(AttachmentReadAttempt::ExplicitDownload)
            })?;
        cancellation.check()?;

        if attachment.attachment_id != requested_attachment_id {
            return Err(validation_error(
                "Jira returned a different attachment",
                AttachmentReadAttempt::ExplicitDownload,
            ));
        }
        validate_attachment_id(&attachment.attachment_id).map_err(|error| {
            error.with_attachment_diagnostic(AttachmentReadDiagnostic::validation(
                AttachmentReadAttempt::ExplicitDownload,
            ))
        })?;
        if attachment.mime_type.trim().is_empty() {
            return Err(content_type_error(
                "Jira returned an attachment without a media type",
                AttachmentReadAttempt::ExplicitDownload,
                AttachmentMimeClass::Missing,
            ));
        }
        if attachment.bytes.is_empty() {
            return Err(body_error(
                "Jira returned an empty attachment",
                AttachmentReadAttempt::ExplicitDownload,
                AttachmentBodyClass::Empty,
            ));
        }
        if attachment.bytes.len() > port_request.max_bytes {
            return Err(body_error(
                "Jira attachment exceeded the size limit",
                AttachmentReadAttempt::ExplicitDownload,
                AttachmentBodyClass::TooLarge,
            ));
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
    attempt: AttachmentReadAttempt,
) -> Result<(), ApplicationError> {
    if image.attachment_id != requested_attachment_id {
        return Err(validation_error(
            "Jira returned a different attachment",
            attempt,
        ));
    }
    validate_attachment_id(&image.attachment_id).map_err(|error| {
        error.with_attachment_diagnostic(AttachmentReadDiagnostic::validation(attempt))
    })?;
    if !is_allowed_image_mime(&image.mime_type) {
        return Err(content_type_error(
            "Jira returned an unsupported image type",
            attempt,
            classify_mime(&image.mime_type),
        ));
    }
    if image.bytes.is_empty() {
        return Err(body_error(
            "Jira returned an empty attachment image",
            attempt,
            AttachmentBodyClass::Empty,
        ));
    }
    if image.bytes.len() > max_bytes {
        return Err(body_error(
            "Jira attachment image exceeded the size limit",
            attempt,
            AttachmentBodyClass::TooLarge,
        ));
    }
    Ok(())
}

/// Validate bytes before they enter the durable authenticated-media cache.
/// This intentionally includes a signature check because cache contents are
/// trusted by neither the storage adapter nor a future process invocation.
pub fn validate_cached_image(
    image: &AttachmentImage,
    requested_attachment_id: &str,
    max_bytes: usize,
) -> Result<(), ApplicationError> {
    if image.attachment_id != requested_attachment_id {
        return Err(ApplicationError::invalid_input(
            "cached image belongs to a different attachment",
        ));
    }
    validate_attachment_id(&image.attachment_id)?;
    if !is_allowed_image_mime(&image.mime_type) {
        return Err(ApplicationError::invalid_input(
            "cached image has an unsupported media type",
        ));
    }
    if image.bytes.is_empty() || image.bytes.len() > max_bytes {
        return Err(ApplicationError::invalid_input(
            "cached image size is outside the configured bound",
        ));
    }
    let signature = image_signature(&image.bytes);
    if signature.is_none()
        || (!image
            .mime_type
            .trim()
            .eq_ignore_ascii_case("application/octet-stream")
            && !mime_matches_signature(&image.mime_type, signature.expect("checked above")))
    {
        return Err(ApplicationError::invalid_input(
            "cached image has an invalid signature",
        ));
    }
    Ok(())
}

fn mime_matches_signature(mime_type: &str, signature: &str) -> bool {
    let mime_type = mime_type.trim().to_ascii_lowercase();
    (mime_type == signature) || (mime_type == "image/jpg" && signature == "image/jpeg")
}

fn image_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn upstream(message: &'static str) -> ApplicationError {
    ApplicationError::new(ErrorKind::Upstream, message)
}

fn validation_error(message: &'static str, attempt: AttachmentReadAttempt) -> ApplicationError {
    upstream(message).with_attachment_diagnostic(AttachmentReadDiagnostic::validation(attempt))
}

fn content_type_error(
    message: &'static str,
    attempt: AttachmentReadAttempt,
    mime_class: AttachmentMimeClass,
) -> ApplicationError {
    upstream(message)
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(attempt, mime_class))
}

fn body_error(
    message: &'static str,
    attempt: AttachmentReadAttempt,
    body_class: AttachmentBodyClass,
) -> ApplicationError {
    upstream(message)
        .with_attachment_diagnostic(AttachmentReadDiagnostic::body(attempt, body_class))
}

fn classify_mime(value: &str) -> AttachmentMimeClass {
    let value = value.trim();
    if value.is_empty() {
        return AttachmentMimeClass::Missing;
    }

    match value.to_ascii_lowercase().as_str() {
        "image/png" => AttachmentMimeClass::Png,
        "image/jpeg" | "image/jpg" => AttachmentMimeClass::Jpeg,
        "image/gif" => AttachmentMimeClass::Gif,
        "image/webp" => AttachmentMimeClass::Webp,
        "application/octet-stream" => AttachmentMimeClass::OctetStream,
        value if value.contains('/') => AttachmentMimeClass::Other,
        _ => AttachmentMimeClass::Malformed,
    }
}

#[cfg(test)]
#[path = "issue_media_tests.rs"]
mod tests;
