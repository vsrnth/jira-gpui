use std::sync::Arc;

use crate::{
    ApplicationError, AttachmentBodyClass, AttachmentContent, AttachmentDownloadRequest,
    AttachmentImage, AttachmentImageRequest, AttachmentMimeClass, AttachmentReadAttempt,
    AttachmentReadDiagnostic, CancellationToken, ErrorKind, JiraAttachmentReadPort,
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
    jira: Arc<dyn JiraAttachmentReadPort>,
    config: IssueMediaConfig,
}

impl IssueMediaService {
    pub fn new(jira: Arc<dyn JiraAttachmentReadPort>, config: IssueMediaConfig) -> Self {
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
