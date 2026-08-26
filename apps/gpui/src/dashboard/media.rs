use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{Image, ImageFormat};
use jira_application::CancellationToken;
use jira_domain::{IssueId, JiraSiteId, RichBlock, RichImage, RichTextDocument};

use crate::{
    diagnostics::{
        DiagnosticErrorKind, DiagnosticFlow, DiagnosticsSink, ImageFetchResult, ImageSource,
        ImageStateReason,
    },
    live_workspace::LiveWorkspace,
    presentation::{AttachmentViewModel, IssueDetailViewModel},
    rich_text_view::{RichImageRenderState, RichImageRenderStates},
};

mod policy;

pub(super) const MAX_RICH_IMAGES: usize = policy::MAX_RICH_IMAGES;
pub(super) const MAX_ATTACHMENT_DOWNLOAD_BYTES: usize = policy::MAX_ATTACHMENT_DOWNLOAD_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AttachmentDownloadState {
    Idle,
    Saving { attachment_id: String },
}

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

pub(super) fn sanitized_attachment_filename(filename: &str) -> String {
    policy::sanitized_attachment_filename(filename)
}

fn choose_portal_download_directory(
    home: Option<&Path>,
    downloads: Option<&Path>,
    current: &Path,
) -> PathBuf {
    policy::choose_portal_download_directory(home, downloads, current)
}

pub(super) fn portal_download_directory() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let downloads = std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|path| path.join("Downloads")));
    choose_portal_download_directory(home.as_deref(), downloads.as_deref(), Path::new("."))
}

pub(super) fn attachment_temp_path(destination: &Path, unique_token: &str) -> PathBuf {
    policy::attachment_temp_path(destination, unique_token)
}

pub(super) fn attachment_temp_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

pub(super) fn write_attachment_temp(
    path: &Path,
    bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("Download cancelled".to_owned());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Could not create a temporary attachment file".to_owned())?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| "Could not save the attachment".to_owned())?;
        file.sync_all()
            .map_err(|_| "Could not save the attachment".to_owned())?;
        if cancellation.is_cancelled() {
            return Err("Download cancelled".to_owned());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

pub(super) fn cleanup_attachment_temp(path: &Path) {
    let _ = std::fs::remove_file(path);
}

pub(super) fn attachment_download_is_current(
    current_generation: u64,
    expected_generation: u64,
    cancellation: &CancellationToken,
) -> bool {
    policy::attachment_generation_is_current(current_generation, expected_generation)
        && !cancellation.is_cancelled()
}

pub(super) fn image_result_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    super::detail_result_is_current(
        selected_issue,
        expected_issue,
        generation,
        expected_generation,
    )
}

pub(super) fn attachment_download_button_label(active: bool) -> &'static str {
    if active { "Saving…" } else { "Download" }
}

pub(super) fn attachment_issue_id(
    selected_issue: Option<&IssueId>,
    remote_issue: Option<&IssueId>,
) -> Option<IssueId> {
    policy::attachment_issue_id(selected_issue, remote_issue)
}

#[cfg(test)]
fn unique_attachment_for_id(
    attachments: &[AttachmentViewModel],
    attachment_id: &str,
) -> Option<AttachmentViewModel> {
    policy::unique_exact_id_index(attachments, attachment_id, |attachment| &attachment.id)
        .map(|index| attachments[index].clone())
}

pub(super) fn inline_attachment_for_download(
    expected_issue_id: &IssueId,
    active_issue_id: &IssueId,
    attachments: &[AttachmentViewModel],
    attachment_id: &str,
) -> Option<AttachmentViewModel> {
    policy::inline_attachment_index(
        expected_issue_id,
        active_issue_id,
        attachments,
        attachment_id,
        |attachment| &attachment.id,
    )
    .map(|index| attachments[index].clone())
}

#[cfg_attr(not(test), allow(dead_code))]
fn collect_rich_images(document: &RichTextDocument) -> Vec<RichImage> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for block in &document.blocks {
        collect_rich_images_from_block(block, &mut seen, &mut images);
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    for image in &document.fallback_images {
        if seen.insert(image.attachment_id.clone()) {
            images.push(image.clone());
        }
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    images
}

#[cfg_attr(not(test), allow(dead_code))]
fn collect_rich_images_from_block(
    block: &RichBlock,
    seen: &mut HashSet<String>,
    images: &mut Vec<RichImage>,
) {
    if images.len() == MAX_RICH_IMAGES {
        return;
    }
    match block {
        RichBlock::Image(image) => {
            if seen.insert(image.attachment_id.clone()) {
                images.push(image.clone());
            }
        }
        RichBlock::BlockQuote(children)
        | RichBlock::Panel {
            content: children, ..
        } => {
            for child in children {
                collect_rich_images_from_block(child, seen, images);
                if images.len() == MAX_RICH_IMAGES {
                    break;
                }
            }
        }
        RichBlock::BulletList(items) | RichBlock::OrderedList { items, .. } => {
            for item in items {
                for child in &item.blocks {
                    collect_rich_images_from_block(child, seen, images);
                    if images.len() == MAX_RICH_IMAGES {
                        return;
                    }
                }
            }
        }
        RichBlock::Paragraph(_)
        | RichBlock::Heading { .. }
        | RichBlock::CodeBlock { .. }
        | RichBlock::Placeholder { .. } => {}
    }
}

#[allow(dead_code)]
fn collect_detail_images(detail: &IssueDetailViewModel) -> Vec<RichImage> {
    collect_detail_images_with_context(detail)
        .into_iter()
        .map(|image| image.0)
        .collect()
}

pub(super) fn collect_detail_images_with_context(
    detail: &IssueDetailViewModel,
) -> Vec<(RichImage, usize, ImageSource)> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    if let Some(document) = detail.rich_description.as_ref() {
        for image in collect_rich_images_with_context(document, 0) {
            if seen.insert(image.0.attachment_id.clone()) {
                images.push(image);
            }
        }
    }
    for (comment_index, comment) in detail.comments.iter().enumerate() {
        if let Some(document) = comment.rich_body.as_ref() {
            for image in collect_rich_images_with_context(document, comment_index + 1) {
                if seen.insert(image.0.attachment_id.clone()) {
                    images.push(image);
                    if images.len() == MAX_RICH_IMAGES {
                        return images;
                    }
                }
            }
        }
    }
    images.truncate(MAX_RICH_IMAGES);
    images
}

pub(super) fn rich_image_contexts(
    images: &[(RichImage, usize, ImageSource)],
) -> Vec<(usize, ImageSource)> {
    images.iter().map(|image| (image.1, image.2)).collect()
}

fn collect_rich_images_with_context(
    document: &RichTextDocument,
    surface_ordinal: usize,
) -> Vec<(RichImage, usize, ImageSource)> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for block in &document.blocks {
        collect_rich_images_from_block_with_context(
            block,
            &mut seen,
            &mut images,
            surface_ordinal,
            ImageSource::ResolvedAdf,
        );
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    for image in &document.fallback_images {
        if seen.insert(image.attachment_id.clone()) {
            images.push((
                image.clone(),
                surface_ordinal,
                ImageSource::FallbackCandidate,
            ));
        }
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    images
}

fn collect_rich_images_from_block_with_context(
    block: &RichBlock,
    seen: &mut HashSet<String>,
    images: &mut Vec<(RichImage, usize, ImageSource)>,
    surface_ordinal: usize,
    source: ImageSource,
) {
    if images.len() == MAX_RICH_IMAGES {
        return;
    }
    match block {
        RichBlock::Image(image) => {
            if seen.insert(image.attachment_id.clone()) {
                images.push((image.clone(), surface_ordinal, source));
            }
        }
        RichBlock::BlockQuote(children)
        | RichBlock::Panel {
            content: children, ..
        } => {
            for child in children {
                collect_rich_images_from_block_with_context(
                    child,
                    seen,
                    images,
                    surface_ordinal,
                    source,
                );
                if images.len() == MAX_RICH_IMAGES {
                    break;
                }
            }
        }
        RichBlock::BulletList(items) | RichBlock::OrderedList { items, .. } => {
            for item in items {
                for child in &item.blocks {
                    collect_rich_images_from_block_with_context(
                        child,
                        seen,
                        images,
                        surface_ordinal,
                        source,
                    );
                    if images.len() == MAX_RICH_IMAGES {
                        return;
                    }
                }
            }
        }
        RichBlock::Paragraph(_)
        | RichBlock::Heading { .. }
        | RichBlock::CodeBlock { .. }
        | RichBlock::Placeholder { .. } => {}
    }
}

pub(super) fn loading_image_states(
    images: &[(RichImage, usize, ImageSource)],
    diagnostics: &DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> RichImageRenderStates {
    let mut states = RichImageRenderStates::with_context(diagnostics.clone(), flow, load_token);
    for (candidate_ordinal, image) in images.iter().enumerate() {
        diagnostics.image_state(
            flow,
            load_token,
            candidate_ordinal,
            image.1,
            image.2,
            ImageStateReason::Loading,
        );
        states.insert_with_context(
            image.0.attachment_id.clone(),
            RichImageRenderState::Loading,
            candidate_ordinal,
            image.1,
            image.2,
        );
    }
    states
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn fetch_rich_image_states(
    workspace: Arc<LiveWorkspace>,
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
        let result = workspace
            .fetch_attachment_image(
                jira_application::AttachmentImageRequest {
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
    use super::*;

    #[test]
    fn diagnostics_adapters_map_every_neutral_policy_variant_losslessly() {
        use crate::diagnostics::{ImagePreflight, ImageSignature, ResponseMime};

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

    #[test]
    fn inline_attachment_download_requires_one_nonempty_exact_id() {
        let attachment = |id: &str| crate::presentation::AttachmentViewModel {
            id: id.to_owned(),
            filename: "report.csv".to_owned(),
            mime_type: "text/csv".to_owned(),
            size_bytes: 12,
            size: "12 B".to_owned(),
        };
        let attachments = vec![attachment("attachment-1"), attachment("attachment-2")];
        let current_issue = IssueId::new("100").expect("issue");
        let stale_issue = IssueId::new("200").expect("issue");
        assert_eq!(
            inline_attachment_for_download(
                &current_issue,
                &current_issue,
                &attachments,
                "attachment-1",
            )
            .expect("current issue attachment")
            .id,
            "attachment-1"
        );
        assert!(
            inline_attachment_for_download(
                &current_issue,
                &stale_issue,
                &attachments,
                "attachment-1",
            )
            .is_none()
        );
        assert_eq!(
            unique_attachment_for_id(&attachments, "attachment-1")
                .expect("exact attachment")
                .id,
            "attachment-1"
        );
        assert!(unique_attachment_for_id(&attachments, "").is_none());
        assert!(unique_attachment_for_id(&attachments, "missing").is_none());
        let duplicate = vec![attachment("attachment-1"), attachment("attachment-1")];
        assert!(unique_attachment_for_id(&duplicate, "attachment-1").is_none());
        let empty_id = vec![attachment("")];
        assert!(unique_attachment_for_id(&empty_id, "").is_none());
    }

    fn test_image(id: &str) -> RichImage {
        RichImage {
            attachment_id: id.to_owned(),
            filename: format!("{id}.png"),
            mime_type: "image/png".to_owned(),
            alt_text: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn rich_image_collection_is_recursive_deduplicated_and_capped() {
        let mut blocks = vec![RichBlock::Panel {
            kind: jira_domain::PanelKind::Info,
            content: vec![RichBlock::BlockQuote(vec![RichBlock::Image(test_image(
                "nested",
            ))])],
        }];
        blocks.extend((0..20).map(|index| RichBlock::Image(test_image(&format!("image-{index}")))));
        blocks.push(RichBlock::Image(test_image("nested")));
        let document = RichTextDocument::new(blocks, false);
        let images = collect_rich_images(&document);

        assert_eq!(images.len(), MAX_RICH_IMAGES);
        assert_eq!(images[0].attachment_id, "nested");
        assert_eq!(
            images
                .iter()
                .filter(|image| image.attachment_id == "nested")
                .count(),
            1
        );
    }

    #[test]
    fn rich_image_collection_appends_fallback_candidates_after_resolved_images() {
        let document = RichTextDocument::new(vec![RichBlock::Image(test_image("resolved"))], false)
            .with_fallback_images(vec![
                test_image("resolved"),
                test_image("candidate-a"),
                test_image("candidate-b"),
            ]);

        let images = collect_rich_images(&document);
        assert_eq!(
            images
                .iter()
                .map(|image| image.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["resolved", "candidate-a", "candidate-b"]
        );
    }

    #[test]
    fn rich_image_context_preserves_surface_and_source_markers() {
        let document = RichTextDocument::new(vec![RichBlock::Image(test_image("resolved"))], false)
            .with_fallback_images(vec![test_image("candidate")]);
        let images = collect_rich_images_with_context(&document, 3);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].1, 3);
        assert_eq!(images[0].2, ImageSource::ResolvedAdf);
        assert_eq!(images[1].1, 3);
        assert_eq!(images[1].2, ImageSource::FallbackCandidate);
    }

    #[test]
    fn fallback_image_candidates_obey_the_global_image_cap() {
        let candidates = (0..(MAX_RICH_IMAGES + 4))
            .map(|index| test_image(&format!("candidate-{index}")))
            .collect();
        let document = RichTextDocument::new(Vec::new(), false).with_fallback_images(candidates);

        let images = collect_rich_images(&document);
        assert_eq!(images.len(), MAX_RICH_IMAGES);
        assert_eq!(images[0].attachment_id, "candidate-0");
        assert_eq!(
            images.last().map(|image| image.attachment_id.as_str()),
            Some("candidate-15")
        );
    }

    #[test]
    fn filename_sanitization_strips_paths_controls_and_bounds_utf8_bytes() {
        let filename = format!("/private/..\\{}\0", "é".repeat(200));
        let sanitized = sanitized_attachment_filename(&filename);
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.contains('\\'));
        assert!(!sanitized.chars().any(char::is_control));
        assert!(sanitized.len() <= 255);
        assert!(sanitized.is_char_boundary(sanitized.len()));
    }

    #[test]
    fn image_results_reject_stale_selection_generation() {
        let first = IssueId::new("first").expect("issue");
        let second = IssueId::new("second").expect("issue");
        assert!(!image_result_is_current(Some(&first), &first, 3, 2));
        assert!(!image_result_is_current(Some(&second), &first, 2, 2));
        assert!(image_result_is_current(Some(&first), &first, 2, 2));
    }

    #[test]
    fn attachment_download_state_and_labels_reflect_busy_operation() {
        assert_eq!(attachment_download_button_label(false), "Download");
        assert_eq!(attachment_download_button_label(true), "Saving…");
        let state = AttachmentDownloadState::Saving {
            attachment_id: "att-1".to_owned(),
        };
        assert!(!matches!(state, AttachmentDownloadState::Idle));
    }

    #[test]
    fn attachment_download_commit_requires_current_generation_and_live_token() {
        let cancellation = CancellationToken::new();
        assert!(attachment_download_is_current(4, 4, &cancellation));
        assert!(!attachment_download_is_current(5, 4, &cancellation));
        cancellation.cancel();
        assert!(!attachment_download_is_current(4, 4, &cancellation));
    }

    #[test]
    fn attachment_temp_path_is_unique_and_stays_with_destination() {
        let destination = Path::new("/tmp/downloads/report.pdf");
        let temporary = attachment_temp_path(destination, "unique-token");
        assert_eq!(temporary.parent(), destination.parent());
        assert_ne!(temporary, destination);
        assert_eq!(
            temporary.file_name().and_then(|name| name.to_str()),
            Some(".report.pdf.jira-desk-unique-token.part")
        );
    }

    #[test]
    fn attachment_temp_write_never_overwrites_an_existing_path() {
        let root = std::env::temp_dir().join(format!(
            "jira-gpui-temp-write-test-{}-{}",
            std::process::id(),
            attachment_temp_token()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let path = root.join("attachment.part");
        std::fs::write(&path, b"existing").expect("existing file");

        assert!(write_attachment_temp(&path, b"replacement", &CancellationToken::new()).is_err());
        assert_eq!(std::fs::read(&path).expect("existing bytes"), b"existing");

        cleanup_attachment_temp(&path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancelled_attachment_temp_write_leaves_no_file() {
        let root = std::env::temp_dir().join(format!(
            "jira-gpui-cancelled-temp-write-test-{}-{}",
            std::process::id(),
            attachment_temp_token()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let path = root.join("attachment.part");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert_eq!(
            write_attachment_temp(&path, b"should not be written", &cancellation),
            Err("Download cancelled".to_owned())
        );
        assert!(!path.exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn attachment_issue_id_prefers_loaded_remote_issue() {
        let local = IssueId::new("local").expect("issue");
        let remote = IssueId::new("remote").expect("issue");
        assert_eq!(
            attachment_issue_id(Some(&local), Some(&remote)),
            Some(remote.clone())
        );
        assert_eq!(attachment_issue_id(Some(&local), None), Some(local));
        assert_eq!(attachment_issue_id(None, None), None);
    }

    #[test]
    fn portal_directory_prefers_existing_downloads_then_home_then_current() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "jira-gpui-portal-test-{}-{nonce}",
            std::process::id()
        ));
        let downloads = root.join("Downloads");
        let home = root.join("home");
        std::fs::create_dir_all(&downloads).expect("downloads");
        std::fs::create_dir_all(&home).expect("home");
        assert_eq!(
            choose_portal_download_directory(Some(&home), Some(&downloads), Path::new(".")),
            downloads
        );
        assert_eq!(
            choose_portal_download_directory(Some(&home), None, Path::new(".")),
            home
        );
        assert_eq!(
            choose_portal_download_directory(None, None, Path::new(".")),
            PathBuf::from(".")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
