use jira_application::{
    ApplicationError, AttachmentBodyClass, AttachmentContent, AttachmentMimeClass,
    AttachmentReadAttempt, AttachmentReadDiagnostic, AttachmentTransportClass, CancellationToken,
    ErrorKind,
};
use reqwest::{Client, StatusCode, header};
use url::Url;

use super::{ApiTokenCredentials, read_response};

pub(super) struct AttachmentReadOptions {
    pub(super) attachment_id: String,
    pub(super) cancellation: CancellationToken,
    pub(super) max_bytes: usize,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) thumbnail: bool,
}

enum AttachmentMimeResolution {
    Declared(String),
    InferFromBody,
}

pub(super) fn attachment_request_builder(
    client: &Client,
    url: Url,
    credentials: &ApiTokenCredentials,
) -> reqwest::RequestBuilder {
    client
        .get(url)
        .basic_auth(&credentials.email, Some(&credentials.token))
        .header(header::ACCEPT, "*/*")
}

pub(super) async fn read_attachment(
    client: Client,
    url: Url,
    credentials: ApiTokenCredentials,
    options: AttachmentReadOptions,
) -> Result<AttachmentContent, ApplicationError> {
    if options.max_bytes == 0 {
        return Err(ApplicationError::invalid_input(
            "attachment response limit must be positive",
        ));
    }
    let url = attachment_url_with_query(url, options.width, options.height, options.thumbnail);

    options.cancellation.check()?;
    let response = attachment_request_builder(&client, url, &credentials)
        .send()
        .await
        .map_err(|error| attachment_transport_error(error, attachment_attempt(&options)))?;
    let attempt = attachment_attempt(&options);
    let status = response.status();
    if !status.is_success() {
        return Err(attachment_status_error(status, response.headers(), attempt));
    }
    let mime_type = if options.thumbnail {
        attachment_thumbnail_mime_type(response.headers(), attempt)?
    } else {
        AttachmentMimeResolution::Declared(attachment_mime_type(
            response.headers(),
            attempt,
            false,
        )?)
    };
    if response
        .content_length()
        .is_some_and(|length| length > options.max_bytes as u64)
    {
        return Err(attachment_body_error(
            attempt,
            AttachmentBodyClass::TooLarge,
        ));
    }

    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ApplicationError::new(ErrorKind::Offline, "could not read Jira attachment")
            .with_attachment_diagnostic(AttachmentReadDiagnostic::body(
                attempt,
                AttachmentBodyClass::ReadFailed,
            ))
    })? {
        options.cancellation.check()?;
        if body.len().saturating_add(chunk.len()) > options.max_bytes {
            return Err(attachment_body_error(
                attempt,
                AttachmentBodyClass::TooLarge,
            ));
        }
        body.extend_from_slice(&chunk);
    }
    options.cancellation.check()?;
    if body.is_empty() {
        return Err(attachment_body_error(attempt, AttachmentBodyClass::Empty));
    }
    let body = finish_attachment_body(body, options.max_bytes, &options.cancellation)?;
    let mime_type = match mime_type {
        AttachmentMimeResolution::Declared(mime_type) => mime_type,
        AttachmentMimeResolution::InferFromBody => image_mime_from_signature(&body)
            .map(str::to_owned)
            .ok_or_else(|| attachment_signature_error(attempt))?,
    };
    Ok(AttachmentContent {
        attachment_id: options.attachment_id,
        mime_type,
        bytes: body,
    })
}

pub(super) fn attachment_attempt(options: &AttachmentReadOptions) -> AttachmentReadAttempt {
    if options.thumbnail {
        AttachmentReadAttempt::Thumbnail
    } else {
        AttachmentReadAttempt::ExplicitDownload
    }
}

pub(super) fn attachment_transport_error(
    error: reqwest::Error,
    attempt: AttachmentReadAttempt,
) -> ApplicationError {
    let transport_class = if error.is_timeout() {
        AttachmentTransportClass::TimedOut
    } else if error.is_connect() {
        AttachmentTransportClass::ConnectFailed
    } else {
        AttachmentTransportClass::RequestFailed
    };
    read_response::transport_error(error).with_attachment_diagnostic(
        AttachmentReadDiagnostic::transport(attempt, transport_class),
    )
}

pub(super) fn attachment_status_error(
    status: StatusCode,
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
) -> ApplicationError {
    read_response::status_error(status, headers)
        .with_attachment_diagnostic(AttachmentReadDiagnostic::status(attempt, status.as_u16()))
}

pub(super) fn attachment_body_error(
    attempt: AttachmentReadAttempt,
    body_class: AttachmentBodyClass,
) -> ApplicationError {
    ApplicationError::new(
        ErrorKind::Upstream,
        match body_class {
            AttachmentBodyClass::Empty => "Jira returned an empty attachment",
            AttachmentBodyClass::TooLarge => "Jira attachment exceeded the size limit",
            AttachmentBodyClass::ReadFailed => "could not read Jira attachment",
        },
    )
    .with_attachment_diagnostic(AttachmentReadDiagnostic::body(attempt, body_class))
}

pub(super) fn attachment_mime_type(
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
    thumbnail: bool,
) -> Result<String, ApplicationError> {
    let mime_type = parsed_attachment_mime_type(headers, attempt)?;
    if thumbnail && !is_allowed_image_mime(&mime_type) {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response was not an image",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Other,
        )));
    }
    Ok(mime_type)
}

fn attachment_thumbnail_mime_type(
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
) -> Result<AttachmentMimeResolution, ApplicationError> {
    let mime_type = parsed_attachment_mime_type(headers, attempt)?;
    if is_allowed_image_mime(&mime_type) {
        Ok(AttachmentMimeResolution::Declared(mime_type))
    } else {
        Ok(AttachmentMimeResolution::InferFromBody)
    }
}

fn parsed_attachment_mime_type(
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
) -> Result<String, ApplicationError> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response had an invalid media type",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Missing,
        )));
    };
    let raw_value = value.to_str().map_err(|_| {
        ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response had an invalid media type",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Malformed,
        ))
    })?;
    let mime_type = media_type(raw_value).ok_or_else(|| {
        ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response had an invalid media type",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Malformed,
        ))
    })?;
    Ok(mime_type)
}

pub(super) fn attachment_signature_error(attempt: AttachmentReadAttempt) -> ApplicationError {
    ApplicationError::new(
        ErrorKind::NotFound,
        "Jira attachment response bytes did not match an image format",
    )
    .with_attachment_diagnostic(AttachmentReadDiagnostic::validation(attempt))
}

pub(super) fn image_mime_from_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

pub(super) fn media_type(value: &str) -> Option<String> {
    let media_type = value.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, subtype) = media_type.split_once('/')?;
    if kind.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !kind.bytes().all(is_media_type_token)
        || !subtype.bytes().all(is_media_type_token)
    {
        return None;
    }
    Some(media_type)
}

fn is_media_type_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

pub(super) fn attachment_url_with_query(
    mut url: Url,
    width: usize,
    height: usize,
    thumbnail: bool,
) -> Url {
    if thumbnail {
        url.query_pairs_mut()
            .append_pair("redirect", "false")
            .append_pair("width", &width.to_string())
            .append_pair("height", &height.to_string())
            .append_pair("fallbackToDefault", "false");
    } else {
        url.query_pairs_mut().append_pair("redirect", "false");
    }
    url
}

pub(super) fn is_allowed_image_mime(value: &str) -> bool {
    matches!(
        value,
        "application/octet-stream"
            | "image/gif"
            | "image/jpg"
            | "image/jpeg"
            | "image/png"
            | "image/webp"
    )
}

pub(super) fn finish_attachment_body(
    body: Vec<u8>,
    max_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, ApplicationError> {
    cancellation.check()?;
    if body.is_empty() {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira returned an empty attachment",
        ));
    }
    if body.len() > max_bytes {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment exceeded the size limit",
        ));
    }
    Ok(body)
}
