use std::path::{Path, PathBuf};

use jira_domain::{IssueId, RichTextDocument};

pub(super) const MAX_RICH_IMAGES: usize = RichTextDocument::MAX_FALLBACK_IMAGES;
pub(super) const MAX_RICH_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_IMAGE_REQUEST_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_IMAGE_REQUEST_WIDTH: usize = 1_600;
pub(super) const MAX_IMAGE_REQUEST_HEIGHT: usize = 1_200;
pub(super) const MAX_ATTACHMENT_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum MediaFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum MediaMime {
    Png,
    Jpeg,
    Gif,
    Webp,
    OctetStream,
    Unsupported,
}

impl MediaMime {
    /// Classifies a MIME value using the exact trimming and case rules used by
    /// Jira image metadata and authenticated responses.
    pub(super) fn classify(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "image/png" => Self::Png,
            "image/jpeg" | "image/jpg" => Self::Jpeg,
            "image/gif" => Self::Gif,
            "image/webp" => Self::Webp,
            "application/octet-stream" => Self::OctetStream,
            _ => Self::Unsupported,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum MediaSignature {
    Png,
    Jpeg,
    Gif,
    Webp,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum MediaPreflight {
    Accepted,
    Empty,
    UnsupportedCachedMime,
    ResponseMimeRejected,
    SignatureRejected,
    AggregateRejected,
}

pub(super) fn image_format_for_mime(mime_type: &str) -> Option<MediaFormat> {
    match MediaMime::classify(mime_type) {
        MediaMime::Png => Some(MediaFormat::Png),
        MediaMime::Jpeg => Some(MediaFormat::Jpeg),
        MediaMime::Gif => Some(MediaFormat::Gif),
        MediaMime::Webp => Some(MediaFormat::Webp),
        MediaMime::OctetStream | MediaMime::Unsupported => None,
    }
}

pub(super) fn image_signature(bytes: &[u8]) -> MediaSignature {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        MediaSignature::Png
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        MediaSignature::Jpeg
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        MediaSignature::Gif
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        MediaSignature::Webp
    } else {
        MediaSignature::Unknown
    }
}

pub(super) fn image_format_from_bytes(bytes: &[u8]) -> Option<MediaFormat> {
    match image_signature(bytes) {
        MediaSignature::Png => Some(MediaFormat::Png),
        MediaSignature::Jpeg => Some(MediaFormat::Jpeg),
        MediaSignature::Gif => Some(MediaFormat::Gif),
        MediaSignature::Webp => Some(MediaFormat::Webp),
        MediaSignature::Unknown => None,
    }
}

/// Cached metadata must identify an allowlisted image. Authenticated
/// responses may use an allowlisted image MIME or the original-content
/// `application/octet-stream` MIME; in either case, the bytes must carry a
/// strict image signature before decoding.
pub(super) fn fetched_image_format(
    cached_mime_type: &str,
    response_mime_type: &str,
    bytes: &[u8],
) -> Option<MediaFormat> {
    image_format_for_mime(cached_mime_type)?;
    match MediaMime::classify(response_mime_type) {
        MediaMime::Png
        | MediaMime::Jpeg
        | MediaMime::Gif
        | MediaMime::Webp
        | MediaMime::OctetStream => {}
        MediaMime::Unsupported => return None,
    }
    image_format_from_bytes(bytes)
}

/// The order here is part of the observable response contract.
pub(super) fn image_response_preflight(
    cached_mime_type: &str,
    response_mime_type: &str,
    bytes: &[u8],
    resident_bytes: usize,
) -> MediaPreflight {
    if image_format_for_mime(cached_mime_type).is_none() {
        MediaPreflight::UnsupportedCachedMime
    } else if bytes.is_empty() {
        MediaPreflight::Empty
    } else if !image_bytes_fit_aggregate(resident_bytes, bytes.len()) {
        MediaPreflight::AggregateRejected
    } else if matches!(
        MediaMime::classify(response_mime_type),
        MediaMime::Unsupported
    ) {
        MediaPreflight::ResponseMimeRejected
    } else if fetched_image_format(cached_mime_type, response_mime_type, bytes).is_none() {
        MediaPreflight::SignatureRejected
    } else {
        MediaPreflight::Accepted
    }
}

pub(super) fn image_bytes_fit_aggregate(current: usize, next: usize) -> bool {
    next <= MAX_RICH_IMAGE_BYTES && current <= MAX_RICH_IMAGE_BYTES.saturating_sub(next)
}

pub(super) fn attachment_issue_id(
    selected_issue: Option<&IssueId>,
    remote_issue: Option<&IssueId>,
) -> Option<IssueId> {
    remote_issue.cloned().or_else(|| selected_issue.cloned())
}

pub(super) fn unique_exact_id_index<T>(
    items: &[T],
    requested_id: &str,
    id_of: impl Fn(&T) -> &str,
) -> Option<usize> {
    if requested_id.trim().is_empty() {
        return None;
    }
    let mut matches = items.iter().enumerate().filter(|(_, item)| {
        let id = id_of(item);
        !id.trim().is_empty() && id == requested_id
    });
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

pub(super) fn inline_attachment_index<T>(
    expected_issue_id: &IssueId,
    active_issue_id: &IssueId,
    attachments: &[T],
    attachment_id: &str,
    id_of: impl Fn(&T) -> &str,
) -> Option<usize> {
    (expected_issue_id == active_issue_id)
        .then(|| unique_exact_id_index(attachments, attachment_id, id_of))
        .flatten()
}

pub(super) fn attachment_generation_is_current(
    current_generation: u64,
    expected_generation: u64,
) -> bool {
    current_generation == expected_generation
}

/// Keep only a leaf filename suitable for a portal suggestion. The selected
/// destination remains controlled by the user and is never derived from Jira.
pub(super) fn sanitized_attachment_filename(filename: &str) -> String {
    let candidate = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let candidate = candidate.trim().trim_matches('.').trim();
    if candidate.is_empty() {
        return "jira-attachment".to_owned();
    }

    let mut bounded = String::new();
    for character in candidate.chars() {
        if bounded.len().saturating_add(character.len_utf8()) > 255 {
            break;
        }
        bounded.push(character);
    }
    if bounded.is_empty() {
        "jira-attachment".to_owned()
    } else {
        bounded
    }
}

pub(super) fn choose_portal_download_directory(
    home: Option<&Path>,
    downloads: Option<&Path>,
    current: &Path,
) -> PathBuf {
    downloads
        .filter(|path| path.is_dir())
        .or_else(|| home.filter(|path| path.is_dir()))
        .unwrap_or(current)
        .to_path_buf()
}

pub(super) fn attachment_temp_path(destination: &Path, unique_token: &str) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("jira-attachment");
    parent.join(format!(".{filename}.jira-desk-{unique_token}.part"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webp_bytes() -> Vec<u8> {
        let mut bytes = b"RIFFxxxxWEBP".to_vec();
        bytes.extend_from_slice(b"fixture");
        bytes
    }

    #[test]
    fn mime_table_preserves_allowlists_trim_case_jpg_and_octet_exception() {
        let cases = [
            (" image/png ", MediaMime::Png, Some(MediaFormat::Png)),
            ("IMAGE/JPEG", MediaMime::Jpeg, Some(MediaFormat::Jpeg)),
            ("image/jpg", MediaMime::Jpeg, Some(MediaFormat::Jpeg)),
            ("image/gif", MediaMime::Gif, Some(MediaFormat::Gif)),
            ("image/webp", MediaMime::Webp, Some(MediaFormat::Webp)),
            (" application/octet-stream ", MediaMime::OctetStream, None),
            ("image/svg+xml", MediaMime::Unsupported, None),
            ("application/pdf", MediaMime::Unsupported, None),
            ("", MediaMime::Unsupported, None),
        ];
        for (mime, classification, format) in cases {
            assert_eq!(MediaMime::classify(mime), classification, "{mime:?}");
            assert_eq!(image_format_for_mime(mime), format, "{mime:?}");
        }
    }

    #[test]
    fn signatures_are_strict_and_formats_follow_actual_bytes() {
        let webp = webp_bytes();
        let cases = [
            (b"\x89PNG\r\n\x1a\nfixture".as_slice(), MediaSignature::Png),
            (b"\xff\xd8\xff\xe0fixture".as_slice(), MediaSignature::Jpeg),
            (b"GIF87afixture".as_slice(), MediaSignature::Gif),
            (b"GIF89afixture".as_slice(), MediaSignature::Gif),
            (webp.as_slice(), MediaSignature::Webp),
            (b"RIFFxxxxPNG!".as_slice(), MediaSignature::Unknown),
            (b"not an image".as_slice(), MediaSignature::Unknown),
        ];
        for (bytes, signature) in cases {
            assert_eq!(image_signature(bytes), signature);
        }
        assert_eq!(
            fetched_image_format("image/jpeg", " image/png ", b"\x89PNG\r\n\x1a\nrest"),
            Some(MediaFormat::Png)
        );
        assert_eq!(
            fetched_image_format("image/png", "image/png", b"not an image"),
            None
        );
    }

    #[test]
    fn preflight_precedence_and_boundaries_are_exact() {
        assert_eq!(MAX_RICH_IMAGES, 16);
        assert_eq!(MAX_IMAGE_REQUEST_BYTES, 8 * 1024 * 1024);
        assert_eq!(MAX_IMAGE_REQUEST_WIDTH, 1_600);
        assert_eq!(MAX_IMAGE_REQUEST_HEIGHT, 1_200);
        assert_eq!(MAX_RICH_IMAGE_BYTES, 32 * 1024 * 1024);
        let png = b"\x89PNG\r\n\x1a\nvalid";
        let cases = [
            (
                "application/pdf",
                "image/png",
                b"".as_slice(),
                0,
                MediaPreflight::UnsupportedCachedMime,
            ),
            (
                "image/png",
                "text/html",
                b"".as_slice(),
                0,
                MediaPreflight::Empty,
            ),
            (
                "image/png",
                "text/html",
                png,
                MAX_RICH_IMAGE_BYTES,
                MediaPreflight::AggregateRejected,
            ),
            (
                "image/png",
                "text/html",
                png,
                0,
                MediaPreflight::ResponseMimeRejected,
            ),
            (
                "image/png",
                "image/png",
                b"not image",
                0,
                MediaPreflight::SignatureRejected,
            ),
            (
                "image/png",
                "image/png",
                png,
                MAX_RICH_IMAGE_BYTES - png.len(),
                MediaPreflight::Accepted,
            ),
        ];
        for (cached, response, bytes, resident, expected) in cases {
            assert_eq!(
                image_response_preflight(cached, response, bytes, resident),
                expected,
                "cached={cached} response={response} resident={resident}"
            );
        }
        assert!(!image_bytes_fit_aggregate(0, MAX_RICH_IMAGE_BYTES + 1));
        assert!(!image_bytes_fit_aggregate(usize::MAX, 1));
        assert!(!image_bytes_fit_aggregate(1, MAX_RICH_IMAGE_BYTES));
        assert!(image_bytes_fit_aggregate(0, MAX_RICH_IMAGE_BYTES));
    }

    #[test]
    fn response_allowlist_octet_stream_and_mime_mismatch_are_preserved() {
        let png = b"\x89PNG\r\n\x1a\nvalid";
        assert_eq!(
            image_response_preflight("image/png", "application/octet-stream", png, 0),
            MediaPreflight::Accepted
        );
        assert_eq!(
            image_response_preflight("image/jpg", "image/jpg", b"\xff\xd8\xff\xe0jpeg", 0),
            MediaPreflight::Accepted
        );
        assert_eq!(
            image_response_preflight("image/png", "application/pdf", png, 0),
            MediaPreflight::ResponseMimeRejected
        );
        assert_eq!(
            fetched_image_format("image/png", "application/octet-stream", b"not image"),
            None
        );
        assert_eq!(
            fetched_image_format("application/octet-stream", "image/png", png),
            None
        );
    }

    #[test]
    fn attachment_identity_generation_and_exact_uniqueness_are_pure() {
        #[derive(Clone, Debug, Eq, PartialEq)]
        struct Attachment(&'static str);
        let attachments = [Attachment("a"), Attachment("b")];
        assert_eq!(
            unique_exact_id_index(&attachments, "a", |item| item.0),
            Some(0)
        );
        assert_eq!(unique_exact_id_index(&attachments, "", |item| item.0), None);
        assert_eq!(
            unique_exact_id_index(&attachments, "missing", |item| item.0),
            None
        );
        assert_eq!(
            unique_exact_id_index(&[Attachment("a"), Attachment("a")], "a", |item| item.0),
            None
        );
        assert_eq!(
            inline_attachment_index(
                &IssueId::new("100").expect("issue"),
                &IssueId::new("100").expect("issue"),
                &attachments,
                "a",
                |item| item.0,
            ),
            Some(0)
        );
        assert!(!attachment_generation_is_current(2, 3));
        assert!(attachment_generation_is_current(3, 3));
    }

    #[test]
    fn filename_path_and_utf8_limits_are_exact() {
        assert_eq!(
            sanitized_attachment_filename("/private/..\\report.pdf\0"),
            "report.pdf"
        );
        assert_eq!(sanitized_attachment_filename(" .. "), "jira-attachment");
        assert_eq!(
            sanitized_attachment_filename("\u{7f}\u{80}"),
            "jira-attachment"
        );
        let exact = "é".repeat(127);
        assert_eq!(sanitized_attachment_filename(&exact).len(), 254);
        let bounded = sanitized_attachment_filename(&format!("{exact}é"));
        assert_eq!(bounded.len(), 254);
        assert!(bounded.is_char_boundary(bounded.len()));
        assert_eq!(sanitized_attachment_filename(&"a".repeat(255)).len(), 255);
        assert_eq!(sanitized_attachment_filename(&"a".repeat(256)).len(), 255);
    }

    #[test]
    fn portal_directory_preference_and_temp_sibling_path_are_pure_policies() {
        let current = std::env::current_dir().expect("current directory");
        let temp = std::env::temp_dir();
        let root = Path::new("/");
        assert_eq!(
            choose_portal_download_directory(Some(root), Some(&temp), &current),
            temp
        );
        assert_eq!(
            choose_portal_download_directory(Some(&temp), None, &current),
            temp
        );
        assert_eq!(
            choose_portal_download_directory(None, None, root),
            PathBuf::from("/")
        );
        let destination = Path::new("/tmp/downloads/report.pdf");
        assert_eq!(
            attachment_temp_path(destination, "current-generation"),
            PathBuf::from("/tmp/downloads/.report.pdf.jira-desk-current-generation.part")
        );
    }
}
