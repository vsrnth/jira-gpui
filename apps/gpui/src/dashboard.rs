use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Image, ImageFormat, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, StyledExt as _, WindowExt as _,
    button::Button,
    button::ButtonVariants as _,
    combobox::{Combobox, ComboboxEvent, ComboboxState},
    dialog::Cancel,
    h_flex, h_resizable,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    notification::Notification,
    resizable_panel,
    scroll::ScrollableElement as _,
    searchable_list::{SearchableListItem, SearchableVec},
    spinner::Spinner,
    v_flex,
};
use jira_application::{
    ApplicationError, AttachmentDownloadRequest, CancellationToken, DefaultPollingPolicy,
    IssueLocator, JiraCommentWritePort, JiraReadPort, SyncMode,
};

use jira_domain::{
    AccountId, Issue, IssueId, IssueKey, JiraSiteId, RichBlock, RichImage, RichTextDocument, User,
};

use crate::{
    config::{LiveSession, StartupError, ensure_authenticated_user},
    diagnostics::{
        DiagnosticErrorKind, DiagnosticFlow, DiagnosticsSink, ImageFetchResult, ImagePreflight,
        ImageSignature, ImageSource, ImageStateReason, ResponseMime,
    },
    live_workspace::{CachedWorkspace, LiveWorkspace, RefreshResult},
    presentation::{
        IssueDetailViewModel, IssueStatusFilter, IssueStatusSelection, IssueViewModel,
        UpdateViewModel, issue_views_for_filter,
    },
    responsive::{IssuesPaneMode, LayoutMode, issues_pane_mode, layout_for_width},
    rich_text_view::{
        RichAttachmentCardAction, RichImageRenderState, RichImageRenderStates, RichTextPalette,
        render_rich_text, render_rich_text_with_actions,
    },
    sample_data::{sample_issues, sample_updates, sample_users},
    semantic_icons::{PriorityTone, issue_type_icon, priority_semantics},
};

fn safe_sync_error(error: &ApplicationError) -> &'static str {
    match error.kind() {
        jira_application::ErrorKind::Authentication => {
            "Refresh failed · Jira authentication was rejected"
        }
        jira_application::ErrorKind::Authorization => {
            "Refresh failed · Jira authorization was denied"
        }
        jira_application::ErrorKind::RateLimited => "Refresh paused · Jira rate limit reached",
        jira_application::ErrorKind::Offline => "Refresh failed · Jira is unreachable",
        jira_application::ErrorKind::Cancelled => "Refresh cancelled",
        jira_application::ErrorKind::InvalidInput => "Refresh failed · invalid request",
        jira_application::ErrorKind::NotFound => "Refresh failed · Jira site was not found",
        jira_application::ErrorKind::Upstream => "Refresh failed · Jira returned an error",
        jira_application::ErrorKind::Storage
        | jira_application::ErrorKind::Notification
        | jira_application::ErrorKind::Internal
        | jira_application::ErrorKind::UnknownOutcome => "Refresh failed · local application error",
    }
}

fn safe_detail_error(error: &ApplicationError) -> &'static str {
    match error.kind() {
        jira_application::ErrorKind::Authentication => {
            "Issue details unavailable · Jira authentication was rejected"
        }
        jira_application::ErrorKind::Authorization => {
            "Issue details unavailable · Jira authorization was denied"
        }
        jira_application::ErrorKind::NotFound => {
            "Issue details unavailable · Jira issue was not found"
        }
        jira_application::ErrorKind::RateLimited => {
            "Issue details unavailable · Jira rate limit reached"
        }
        jira_application::ErrorKind::Offline => "Issue details unavailable · Jira is unreachable",
        jira_application::ErrorKind::Cancelled => "Issue details request cancelled",
        jira_application::ErrorKind::InvalidInput
        | jira_application::ErrorKind::Upstream
        | jira_application::ErrorKind::Storage
        | jira_application::ErrorKind::Notification
        | jira_application::ErrorKind::Internal
        | jira_application::ErrorKind::UnknownOutcome => {
            "Issue details unavailable · Jira returned an error"
        }
    }
}

fn safe_lookup_error(error: &ApplicationError) -> &'static str {
    match error.kind() {
        jira_application::ErrorKind::Authentication => {
            "Jira lookup failed · authentication was rejected"
        }
        jira_application::ErrorKind::Authorization => {
            "Jira lookup failed · authorization was denied"
        }
        jira_application::ErrorKind::NotFound => "Jira lookup · issue was not found",
        jira_application::ErrorKind::RateLimited => "Jira lookup paused · rate limit reached",
        jira_application::ErrorKind::Offline => "Jira lookup failed · Jira is unreachable",
        jira_application::ErrorKind::Cancelled => "Jira lookup cancelled",
        jira_application::ErrorKind::InvalidInput
        | jira_application::ErrorKind::Upstream
        | jira_application::ErrorKind::Storage
        | jira_application::ErrorKind::Notification
        | jira_application::ErrorKind::Internal
        | jira_application::ErrorKind::UnknownOutcome => {
            "Jira lookup failed · request was not completed"
        }
    }
}

const MAX_RICH_IMAGES: usize = RichTextDocument::MAX_FALLBACK_IMAGES;
const MAX_RICH_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_ATTACHMENT_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
struct CollectedRichImage {
    image: RichImage,
    surface_ordinal: usize,
    source: ImageSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AttachmentDownloadState {
    Idle,
    Saving { attachment_id: String },
}

fn image_format_for_mime(mime_type: &str) -> Option<ImageFormat> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::Webp),
        _ => None,
    }
}

fn image_format_from_bytes(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// Cached metadata must identify an allowlisted image. Authenticated responses
/// may use an allowlisted image MIME or the original-content
/// `application/octet-stream` MIME; in either case, the bytes must carry a
/// strict image signature before GPUI decodes them.
fn fetched_image_format(
    cached_mime_type: &str,
    response_mime_type: &str,
    bytes: &[u8],
) -> Option<ImageFormat> {
    image_format_for_mime(cached_mime_type)?;
    if !response_mime_type
        .trim()
        .eq_ignore_ascii_case("application/octet-stream")
    {
        image_format_for_mime(response_mime_type)?;
    }
    image_format_from_bytes(bytes)
}

fn image_response_preflight(
    cached_mime_type: &str,
    response_mime_type: &str,
    bytes: &[u8],
    resident_bytes: usize,
) -> ImagePreflight {
    if image_format_for_mime(cached_mime_type).is_none() {
        ImagePreflight::UnsupportedCachedMime
    } else if bytes.is_empty() {
        ImagePreflight::Empty
    } else if !image_bytes_fit_aggregate(resident_bytes, bytes.len()) {
        ImagePreflight::AggregateRejected
    } else if ResponseMime::classify(response_mime_type) == ResponseMime::Unsupported {
        ImagePreflight::ResponseMimeRejected
    } else if fetched_image_format(cached_mime_type, response_mime_type, bytes).is_none() {
        ImagePreflight::SignatureRejected
    } else {
        ImagePreflight::Accepted
    }
}

/// Keep only a leaf filename suitable for a portal suggestion. The selected
/// destination remains controlled by the user and is never derived from Jira.
fn sanitized_attachment_filename(filename: &str) -> String {
    let candidate = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let candidate = candidate.trim().trim_matches('.').trim();
    if candidate.is_empty() {
        "jira-attachment".to_owned()
    } else {
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
}

fn choose_portal_download_directory(
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

fn portal_download_directory() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let downloads = std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|path| path.join("Downloads")));
    choose_portal_download_directory(home.as_deref(), downloads.as_deref(), Path::new("."))
}

fn attachment_temp_path(destination: &Path, unique_token: &str) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("jira-attachment");
    parent.join(format!(".{filename}.jira-desk-{unique_token}.part"))
}

fn attachment_temp_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

fn write_attachment_temp(
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

fn cleanup_attachment_temp(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn attachment_download_is_current(
    current_generation: u64,
    expected_generation: u64,
    cancellation: &CancellationToken,
) -> bool {
    current_generation == expected_generation && !cancellation.is_cancelled()
}

fn image_bytes_fit_aggregate(current: usize, next: usize) -> bool {
    next <= MAX_RICH_IMAGE_BYTES && current <= MAX_RICH_IMAGE_BYTES.saturating_sub(next)
}

fn image_result_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    detail_result_is_current(
        selected_issue,
        expected_issue,
        generation,
        expected_generation,
    )
}

fn attachment_download_button_label(active: bool) -> &'static str {
    if active { "Saving…" } else { "Download" }
}

fn attachment_issue_id(
    selected_issue: Option<&IssueId>,
    remote_issue: Option<&IssueId>,
) -> Option<IssueId> {
    remote_issue.cloned().or_else(|| selected_issue.cloned())
}

fn unique_attachment_for_id(
    attachments: &[crate::presentation::AttachmentViewModel],
    attachment_id: &str,
) -> Option<crate::presentation::AttachmentViewModel> {
    if attachment_id.trim().is_empty() {
        return None;
    }

    let mut matches = attachments
        .iter()
        .filter(|attachment| !attachment.id.trim().is_empty() && attachment.id == attachment_id);
    let attachment = matches.next()?.clone();
    matches.next().is_none().then_some(attachment)
}

fn inline_attachment_for_download(
    expected_issue_id: &IssueId,
    active_issue_id: &IssueId,
    attachments: &[crate::presentation::AttachmentViewModel],
    attachment_id: &str,
) -> Option<crate::presentation::AttachmentViewModel> {
    (expected_issue_id == active_issue_id)
        .then(|| unique_attachment_for_id(attachments, attachment_id))
        .flatten()
}

fn should_close_status_filter_after_change(
    previous: IssueStatusSelection,
    next: IssueStatusSelection,
) -> bool {
    previous == IssueStatusSelection::All
        && next.values().len() == 1
        && next != IssueStatusSelection::All
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
        .map(|image| image.image)
        .collect()
}

fn collect_detail_images_with_context(detail: &IssueDetailViewModel) -> Vec<CollectedRichImage> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    if let Some(document) = detail.rich_description.as_ref() {
        for image in collect_rich_images_with_context(document, 0) {
            if seen.insert(image.image.attachment_id.clone()) {
                images.push(image);
            }
        }
    }
    for (comment_index, comment) in detail.comments.iter().enumerate() {
        if let Some(document) = comment.rich_body.as_ref() {
            for image in collect_rich_images_with_context(document, comment_index + 1) {
                if seen.insert(image.image.attachment_id.clone()) {
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

fn collect_rich_images_with_context(
    document: &RichTextDocument,
    surface_ordinal: usize,
) -> Vec<CollectedRichImage> {
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
            images.push(CollectedRichImage {
                image: image.clone(),
                surface_ordinal,
                source: ImageSource::FallbackCandidate,
            });
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
    images: &mut Vec<CollectedRichImage>,
    surface_ordinal: usize,
    source: ImageSource,
) {
    if images.len() == MAX_RICH_IMAGES {
        return;
    }
    match block {
        RichBlock::Image(image) => {
            if seen.insert(image.attachment_id.clone()) {
                images.push(CollectedRichImage {
                    image: image.clone(),
                    surface_ordinal,
                    source,
                });
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

fn loading_image_states(
    images: &[CollectedRichImage],
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
            image.surface_ordinal,
            image.source,
            ImageStateReason::Loading,
        );
        states.insert_with_context(
            image.image.attachment_id.clone(),
            RichImageRenderState::Loading,
            candidate_ordinal,
            image.surface_ordinal,
            image.source,
        );
    }
    states
}

#[allow(clippy::too_many_arguments)]
async fn fetch_rich_image_states(
    workspace: Arc<LiveWorkspace>,
    site_id: JiraSiteId,
    issue_id: IssueId,
    images: Vec<CollectedRichImage>,
    cancellation: CancellationToken,
    diagnostics: DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> Result<RichImageRenderStates, ()> {
    let mut states = RichImageRenderStates::with_context(diagnostics.clone(), flow, load_token);
    let mut resident_bytes = 0usize;
    for (candidate_ordinal, collected) in images.into_iter().enumerate() {
        let image = collected.image;
        let surface_ordinal = collected.surface_ordinal;
        let source = collected.source;
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
        if image_format_for_mime(&image.mime_type).is_none() {
            diagnostics.image_response(
                flow,
                load_token,
                candidate_ordinal,
                surface_ordinal,
                source,
                ResponseMime::classify(&image.mime_type),
                ImageSignature::Unknown,
                0,
                ImagePreflight::UnsupportedCachedMime,
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
                    width: 1_600,
                    height: 1_200,
                    max_bytes: 8 * 1024 * 1024,
                },
                &cancellation,
            )
            .await;
        match result {
            Ok(image_bytes) => {
                let response_mime = ResponseMime::classify(&image_bytes.mime_type);
                let signature = ImageSignature::classify(&image_bytes.bytes);
                let preflight = image_response_preflight(
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
                    preflight,
                );
                if preflight == ImagePreflight::Accepted {
                    let format = fetched_image_format(
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
                            format,
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

fn refresh_complete_message(result: &RefreshResult) -> String {
    let mode = match result.outcome.mode {
        SyncMode::Baseline => "baseline",
        SyncMode::Incremental => "incremental",
        SyncMode::Reconciliation => "reconciliation",
    };
    format!(
        "Refresh complete · {} issues · {} new local updates · {} local updates loaded · desktop notifications: {} delivered, {} unavailable · {mode}",
        result.cached.issues.len(),
        result.outcome.events_inserted,
        result.cached.events.len(),
        result.outcome.notifications_delivered,
        result.outcome.notification_failures,
    )
}

fn authenticated_identity(user: Option<User>) -> (Vec<User>, Option<AccountId>) {
    let account = user.as_ref().map(|user| user.account_id.clone());
    (user.into_iter().collect(), account)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Issues,
    Updates,
}

struct RefreshNotification;
struct CommentNotification;
struct AttachmentNotification;

#[derive(Clone, Debug)]
struct StatusOption(IssueStatusSelection);

impl SearchableListItem for StatusOption {
    type Value = IssueStatusSelection;

    fn title(&self) -> gpui::SharedString {
        self.0.label().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

fn status_filter_trigger_label(selection: IssueStatusSelection) -> String {
    let count = selection.values().len();
    if count > 1 {
        format!("{count} statuses")
    } else {
        selection.label().to_owned()
    }
}

fn status_options() -> SearchableVec<StatusOption> {
    SearchableVec::new([
        StatusOption(IssueStatusSelection::ToDo),
        StatusOption(IssueStatusSelection::InProgress),
        StatusOption(IssueStatusSelection::Done),
        StatusOption(IssueStatusSelection::Uncategorized),
    ])
}

fn status_filter_indices(selection: IssueStatusSelection) -> Vec<gpui_component::IndexPath> {
    let selected = selection.values();
    [
        IssueStatusSelection::ToDo,
        IssueStatusSelection::InProgress,
        IssueStatusSelection::Done,
        IssueStatusSelection::Uncategorized,
    ]
    .into_iter()
    .enumerate()
    .filter(|(_, value)| selected.contains(value))
    .map(|(index, _)| gpui_component::IndexPath::new(index))
    .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DetailState {
    Empty,
    Loading { issue_id: IssueId },
    RemoteLoading { query: String },
    Loaded(IssueDetailViewModel),
    Error { issue_id: IssueId, message: String },
    RemoteError { query: String, message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CommentPostState {
    Idle,
    Confirming {
        issue_id: IssueId,
        issue_key: String,
        body: String,
        chars: usize,
        bytes: usize,
    },
    Posting {
        issue_id: IssueId,
    },
    Error {
        issue_id: IssueId,
        message: String,
        unknown_outcome: bool,
    },
}

fn comment_error_message(error: &ApplicationError) -> (&'static str, bool) {
    match error.kind() {
        jira_application::ErrorKind::Authentication => (
            "Comment not posted · Jira authentication was rejected",
            false,
        ),
        jira_application::ErrorKind::Authorization => {
            ("Comment not posted · Jira denied comment permission", false)
        }
        jira_application::ErrorKind::NotFound => {
            ("Comment not posted · the Jira issue was not found", false)
        }
        jira_application::ErrorKind::RateLimited => (
            "Comment not posted · Jira rate limit reached; try later",
            false,
        ),
        jira_application::ErrorKind::InvalidInput => {
            ("Comment not posted · the comment text is invalid", false)
        }
        jira_application::ErrorKind::UnknownOutcome => (
            "Jira may have accepted this comment. Refresh comments before retrying.",
            true,
        ),
        _ => ("Comment not posted · Jira returned an error", false),
    }
}

fn confirmed_comment_snapshot(
    state: &CommentPostState,
    selected_issue: Option<&IssueId>,
) -> Option<(IssueId, String)> {
    let CommentPostState::Confirming { issue_id, body, .. } = state else {
        return None;
    };
    (selected_issue == Some(issue_id)).then(|| (issue_id.clone(), body.clone()))
}

fn comment_target_is_current(
    remote_issue_id: Option<&IssueId>,
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
) -> bool {
    remote_issue_id.or(selected_issue) == Some(expected_issue)
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum RemoteLookupState {
    Idle,
    Loading {
        query: String,
    },
    Loaded {
        query: String,
        issue: Issue,
        detail: IssueDetailViewModel,
    },
    Error {
        query: String,
        message: String,
    },
}

fn detail_result_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    generation == expected_generation && selected_issue == Some(expected_issue)
}

fn remote_lookup_result_is_current(
    current_query: &str,
    expected_query: &str,
    generation: u64,
    expected_generation: u64,
) -> bool {
    generation == expected_generation
        && current_query
            .trim()
            .eq_ignore_ascii_case(expected_query.trim())
}

fn local_issue_id_for_key(issues: &[Issue], key: &IssueKey) -> Option<IssueId> {
    issues
        .iter()
        .find(|issue| issue.key.as_str().eq_ignore_ascii_case(key.as_str()))
        .map(|issue| issue.id.clone())
}

pub struct Dashboard {
    diagnostics: DiagnosticsSink,
    section: Section,
    domain_issues: Vec<Issue>,
    issues: Vec<IssueViewModel>,
    updates: Vec<UpdateViewModel>,
    selected_issue: Option<IssueId>,
    mobile_detail_open: bool,
    sync_message: String,
    workspace: Option<Arc<LiveWorkspace>>,
    users: Vec<User>,
    workspace_name: String,
    workspace_members: String,
    site_label: String,
    mode_label: String,
    operation_in_progress: bool,
    polling_task: Option<gpui::Task<()>>,
    automatic_polling_paused: bool,
    authenticated_account: Option<AccountId>,
    status_filter: IssueStatusFilter,
    status_combobox: Option<Entity<ComboboxState<SearchableVec<StatusOption>>>>,
    status_subscriptions: Vec<Subscription>,
    search_query: String,
    search_input: Option<Entity<InputState>>,
    search_subscriptions: Vec<Subscription>,
    detail_state: DetailState,
    detail_generation: u64,
    detail_cancellation: Option<CancellationToken>,
    detail_task: Option<gpui::Task<()>>,
    selected_image_states: RichImageRenderStates,
    remote_image_states: RichImageRenderStates,
    remote_lookup: RemoteLookupState,
    remote_lookup_generation: u64,
    remote_lookup_cancellation: Option<CancellationToken>,
    remote_lookup_task: Option<gpui::Task<()>>,
    comment_input: Option<Entity<TextareaState>>,
    comment_subscriptions: Vec<Subscription>,
    comment_state: CommentPostState,
    comment_generation: u64,
    comment_cancellation: Option<CancellationToken>,
    comment_task: Option<gpui::Task<()>>,
    attachment_download_state: AttachmentDownloadState,
    attachment_download_generation: u64,
    attachment_download_cancellation: Option<CancellationToken>,
    attachment_download_task: Option<gpui::Task<()>>,
}

impl Dashboard {
    pub fn from_sample_data() -> Self {
        Self::from_sample_data_with_diagnostics(DiagnosticsSink::disabled())
    }

    fn from_sample_data_with_diagnostics(diagnostics: DiagnosticsSink) -> Self {
        let domain_issues = sample_issues();
        let users = sample_users();
        let updates = sample_updates()
            .iter()
            .map(|event| {
                let issue = domain_issues
                    .iter()
                    .find(|issue| issue.id == event.issue_id);
                UpdateViewModel::from_domain(event, issue, &users)
            })
            .collect();
        let issues = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::All, "");
        let selected_issue = issues.first().map(|issue| issue.id.clone());

        Self {
            diagnostics: diagnostics.clone(),
            section: Section::Issues,
            domain_issues,
            issues,
            updates,
            selected_issue,
            mobile_detail_open: false,
            sync_message: "Preview data · Jira connection not configured".to_owned(),
            workspace: None,
            users,
            workspace_name: "Platform team".to_owned(),
            workspace_members: "Amina, Devon, Marco".to_owned(),
            site_label: "sample.atlassian.net".to_owned(),
            mode_label: "Local preview mode".to_owned(),
            operation_in_progress: false,
            polling_task: None,
            automatic_polling_paused: false,
            authenticated_account: None,
            status_filter: IssueStatusFilter::All,
            status_combobox: None,
            status_subscriptions: Vec::new(),
            search_query: String::new(),
            search_input: None,
            search_subscriptions: Vec::new(),
            detail_state: DetailState::Empty,
            detail_generation: 0,
            detail_cancellation: None,
            detail_task: None,
            selected_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                0,
            ),
            remote_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::RemoteLookup,
                0,
            ),
            remote_lookup: RemoteLookupState::Idle,
            remote_lookup_generation: 0,
            remote_lookup_cancellation: None,
            remote_lookup_task: None,
            comment_input: None,
            comment_subscriptions: Vec::new(),
            comment_state: CommentPostState::Idle,
            comment_generation: 0,
            comment_cancellation: None,
            comment_task: None,
            attachment_download_state: AttachmentDownloadState::Idle,
            attachment_download_generation: 0,
            attachment_download_cancellation: None,
            attachment_download_task: None,
        }
    }

    pub(crate) fn from_live(
        session: LiveSession,
        diagnostics: DiagnosticsSink,
        cx: &mut Context<Self>,
    ) -> Self {
        let (users, initial_authenticated_account) =
            authenticated_identity(session.authenticated_user.clone());
        let dashboard = Self {
            diagnostics: diagnostics.clone(),
            section: Section::Issues,
            domain_issues: Vec::new(),
            issues: Vec::new(),
            updates: Vec::new(),
            selected_issue: None,
            mobile_detail_open: false,
            sync_message: "Opening local cache…".to_owned(),
            workspace: None,
            users,
            workspace_name: "Jira Project".to_owned(),
            workspace_members: if initial_authenticated_account.is_some() {
                "Authenticated Jira account".to_owned()
            } else {
                "Environment bootstrap · My issues unavailable".to_owned()
            },
            site_label: session.site_label,
            mode_label:
                "Live Jira sync · explicit comments only · best-effort desktop notifications"
                    .to_owned(),
            operation_in_progress: true,
            polling_task: None,
            automatic_polling_paused: false,
            authenticated_account: initial_authenticated_account,
            status_filter: IssueStatusFilter::All,
            status_combobox: None,
            status_subscriptions: Vec::new(),
            search_query: String::new(),
            search_input: None,
            search_subscriptions: Vec::new(),
            detail_state: DetailState::Empty,
            detail_generation: 0,
            detail_cancellation: None,
            detail_task: None,
            selected_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                0,
            ),
            remote_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::RemoteLookup,
                0,
            ),
            remote_lookup: RemoteLookupState::Idle,
            remote_lookup_generation: 0,
            remote_lookup_cancellation: None,
            remote_lookup_task: None,
            comment_input: None,
            comment_subscriptions: Vec::new(),
            comment_state: CommentPostState::Idle,
            comment_generation: 0,
            comment_cancellation: None,
            comment_task: None,
            attachment_download_state: AttachmentDownloadState::Idle,
            attachment_download_generation: 0,
            attachment_download_cancellation: None,
            attachment_download_task: None,
        };

        let site_id = session.site_id;
        let initial_authenticated_user = session.authenticated_user;
        let jira = session.jira;
        let cache = session.cache;
        cx.spawn(async move |this, cx| {
            let result = match ensure_authenticated_user(
                initial_authenticated_user,
                jira.as_ref(),
                &site_id,
            )
            .await
            {
                Ok(authenticated_user) => {
                    let authenticated_account = authenticated_user.account_id.clone();
                    let jira_read: Arc<dyn JiraReadPort> = jira.clone();
                    let jira_write: Arc<dyn JiraCommentWritePort> = jira.clone();
                    match LiveWorkspace::initialize_with_comment_writer(
                        site_id,
                        Some(authenticated_account),
                        jira_read,
                        jira_write,
                        cache,
                    )
                    .await
                    {
                        Ok(workspace) => {
                            let workspace = Arc::new(workspace);
                            workspace
                                .load_cached_for_authenticated_account()
                                .await
                                .map(|cached| (workspace, cached, authenticated_user))
                                .map_err(|error| safe_sync_error(&error).to_owned())
                        }
                        Err(error) => Err(safe_sync_error(&error).to_owned()),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok((workspace, cached, authenticated_user)) => {
                        let issue_count = cached.issues.len();
                        let update_count = cached.events.len();
                        this.users = vec![authenticated_user.clone()];
                        this.authenticated_account = Some(authenticated_user.account_id.clone());
                        this.workspace_members = "Authenticated Jira account".to_owned();
                        this.workspace = Some(workspace);
                        this.apply_cached(cached, cx);
                        this.start_automatic_polling(cx);
                        this.sync_message =
                            format!("Ready · cached {issue_count} issues · {update_count} updates");
                    }
                    Err(error) => this.sync_message = format!("Startup error · {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        dashboard
    }

    fn start_automatic_polling(&mut self, cx: &mut Context<Self>) {
        if self.polling_task.is_some() && !self.automatic_polling_paused {
            return;
        }
        self.polling_task.take();
        self.automatic_polling_paused = false;
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let policy = DefaultPollingPolicy;
        let task = cx.spawn(async move |this, cx| {
            let mut delay = policy.next_delay_after_success();
            let mut consecutive_failures: u32 = 0;
            loop {
                cx.background_executor().timer(delay).await;
                let should_refresh = match this.update(cx, |this, cx| {
                    if this.operation_in_progress {
                        false
                    } else {
                        this.operation_in_progress = true;
                        this.sync_message = "Automatic refresh…".to_owned();
                        cx.notify();
                        true
                    }
                }) {
                    Ok(should_refresh) => should_refresh,
                    Err(_) => break,
                };
                if !should_refresh {
                    continue;
                }

                let cancellation = CancellationToken::new();
                let result = workspace.refresh_automatically(&cancellation).await;
                let next_delay = match this.update(cx, |this, cx| {
                    this.operation_in_progress = false;
                    match result {
                        Ok(result) => {
                            consecutive_failures = 0;
                            this.sync_message = refresh_complete_message(&result);
                            this.apply_cached(result.cached, cx);
                            cx.notify();
                            Some(policy.next_delay_after_success())
                        }
                        Err(error) => {
                            let next = policy.next_delay_after_failure(
                                &error,
                                consecutive_failures.saturating_add(1),
                            );
                            if next.is_some() {
                                consecutive_failures = consecutive_failures.saturating_add(1);
                            }
                            this.sync_message = if next.is_none() {
                                format!("{} · automatic polling paused", safe_sync_error(&error))
                            } else {
                                safe_sync_error(&error).to_owned()
                            };
                            if next.is_none() {
                                this.automatic_polling_paused = true;
                            }
                            cx.notify();
                            next
                        }
                    }
                }) {
                    Ok(next) => next,
                    Err(_) => break,
                };
                let Some(next_delay) = next_delay else {
                    break;
                };
                delay = next_delay;
            }
        });
        self.polling_task = Some(task);
    }

    pub fn from_configuration_error(error: StartupError) -> Self {
        let mut dashboard = Self::from_sample_data();
        dashboard.domain_issues.clear();
        dashboard.issues.clear();
        dashboard.updates.clear();
        dashboard.selected_issue = None;
        dashboard.invalidate_detail_selection();
        dashboard.users.clear();
        dashboard.workspace_name = "Jira Project".to_owned();
        dashboard.workspace_members = "Connect Jira to load this view".to_owned();
        dashboard.site_label = "Jira site unavailable".to_owned();
        dashboard.mode_label = "Startup configuration error".to_owned();
        dashboard.sync_message = format!("Configuration error · {error}");
        dashboard
    }

    fn apply_live_issues(
        &mut self,
        issues: Vec<Issue>,
        refresh_detail: bool,
        cx: &mut Context<Self>,
    ) {
        self.domain_issues = issues;
        self.rebuild_issue_views(refresh_detail, cx);
    }

    fn rebuild_issue_views(&mut self, refresh_detail: bool, cx: &mut Context<Self>) {
        self.issues = issue_views_for_filter(
            &self.domain_issues,
            &self.users,
            self.status_filter,
            &self.search_query,
        );
        let selected_visible = self
            .selected_issue
            .as_ref()
            .is_some_and(|selected| self.issues.iter().any(|issue| &issue.id == selected));
        if self.selected_issue.is_some() && !selected_visible {
            self.invalidate_detail_selection();
        }
        if self.selected_issue.is_none() {
            if let Some(issue_id) = self.issues.first().map(|issue| issue.id.clone()) {
                self.select_issue(issue_id, cx, true);
            }
        } else if refresh_detail {
            self.reload_selected_detail(cx);
        }
    }

    fn set_status_filter(&mut self, filter: IssueStatusFilter, cx: &mut Context<Self>) {
        if self.status_filter == filter {
            return;
        }
        self.status_filter = filter;
        self.rebuild_issue_views(false, cx);
        cx.notify();
    }

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_owned();
        if self.search_query == query {
            return;
        }
        self.clear_remote_lookup();
        self.invalidate_comment_selection();
        self.invalidate_attachment_download();
        self.search_query = query;
        self.rebuild_issue_views(false, cx);
        cx.notify();
    }

    fn clear_remote_lookup(&mut self) {
        self.invalidate_attachment_download();
        self.remote_image_states.clear();
        if let Some(cancellation) = self.remote_lookup_cancellation.take() {
            cancellation.cancel();
        }
        self.remote_lookup_task.take();
        self.remote_lookup_generation = self.remote_lookup_generation.wrapping_add(1);
        self.remote_lookup = RemoteLookupState::Idle;
    }

    fn search_jira(&mut self, cx: &mut Context<Self>) {
        let query = self.search_query.trim().to_owned();
        let Some(key) = crate::presentation::normalized_issue_key(&query) else {
            self.clear_remote_lookup();
            self.sync_message = "Jira lookup · enter a valid issue key such as IX-123".to_owned();
            cx.notify();
            return;
        };

        if let Some(issue_id) = local_issue_id_for_key(&self.domain_issues, &key) {
            self.clear_remote_lookup();
            self.select_issue(issue_id, cx, true);
            return;
        }

        let load_token = self.diagnostics.begin_image_load();
        self.invalidate_comment_selection();
        let Some(workspace) = self.workspace.clone() else {
            self.remote_lookup = RemoteLookupState::Error {
                query,
                message: "Jira lookup unavailable · live workspace is not ready".to_owned(),
            };
            cx.notify();
            return;
        };

        if let Some(cancellation) = self.remote_lookup_cancellation.take() {
            cancellation.cancel();
        }
        self.remote_lookup_task.take();
        self.remote_lookup_generation = self.remote_lookup_generation.wrapping_add(1);
        let generation = self.remote_lookup_generation;
        let cancellation = CancellationToken::new();
        self.remote_lookup_cancellation = Some(cancellation.clone());
        self.remote_image_states.set_context(
            self.diagnostics.clone(),
            DiagnosticFlow::RemoteLookup,
            load_token,
        );
        self.remote_lookup = RemoteLookupState::Loading {
            query: query.clone(),
        };
        let expected_query = query.clone();
        let users = self.users.clone();
        let diagnostics = self.diagnostics.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = workspace.lookup_issue(key, &cancellation).await;
            let detail = match result {
                Ok(detail) => detail,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if !remote_lookup_result_is_current(
                            &this.search_query,
                            &expected_query,
                            this.remote_lookup_generation,
                            generation,
                        ) {
                            return;
                        }
                        this.remote_lookup_cancellation = None;
                        this.remote_lookup_task = None;
                        this.remote_image_states.clear();
                        this.remote_lookup = RemoteLookupState::Error {
                            query: expected_query.clone(),
                            message: safe_lookup_error(&error).to_owned(),
                        };
                        cx.notify();
                    });
                    return;
                }
            };
            let issue = detail.core.issue.clone();
            let view = IssueDetailViewModel::from_domain(&detail, &users);
            let images = collect_detail_images_with_context(&view);
            let image_contexts = images
                .iter()
                .map(|image| (image.surface_ordinal, image.source))
                .collect::<Vec<_>>();
            let loading = loading_image_states(
                &images,
                &diagnostics,
                DiagnosticFlow::RemoteLookup,
                load_token,
            );
            let site_id = workspace.site_id().clone();
            let applied = this
                .update(cx, |this, cx| {
                    if !remote_lookup_result_is_current(
                        &this.search_query,
                        &expected_query,
                        this.remote_lookup_generation,
                        generation,
                    ) {
                        return false;
                    }
                    this.remote_lookup = RemoteLookupState::Loaded {
                        query: expected_query.clone(),
                        issue: issue.clone(),
                        detail: view.clone(),
                    };
                    this.remote_image_states = loading;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !applied {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::RemoteLookup,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
                return;
            }
            let states = fetch_rich_image_states(
                workspace,
                site_id,
                issue.id.clone(),
                images,
                cancellation,
                diagnostics.clone(),
                DiagnosticFlow::RemoteLookup,
                load_token,
            )
            .await;
            let applied = this
                .update(cx, |this, cx| {
                    if !remote_lookup_result_is_current(
                        &this.search_query,
                        &expected_query,
                        this.remote_lookup_generation,
                        generation,
                    ) {
                        return false;
                    }
                    this.remote_lookup_cancellation = None;
                    this.remote_lookup_task = None;
                    if let Ok(states) = states {
                        this.remote_image_states = states;
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !applied {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::RemoteLookup,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
            }
        });
        self.remote_lookup_task = Some(task);
        cx.notify();
    }

    fn invalidate_detail_selection(&mut self) {
        self.invalidate_attachment_download();
        self.selected_image_states.clear();
        if let Some(cancellation) = self.detail_cancellation.take() {
            cancellation.cancel();
        }
        self.detail_task.take();
        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.selected_issue = None;
        self.detail_state = DetailState::Empty;
    }

    fn invalidate_comment_selection(&mut self) {
        self.comment_generation = self.comment_generation.wrapping_add(1);
        self.comment_input = None;
        self.comment_subscriptions.clear();
        if !matches!(&self.comment_state, CommentPostState::Posting { .. }) {
            if let Some(cancellation) = self.comment_cancellation.take() {
                cancellation.cancel();
            }
            self.comment_task.take();
            self.comment_state = CommentPostState::Idle;
        } else {
            // A dispatched POST may have succeeded even if its UI selection is
            // gone; its completion is ignored by the generation guard.
            self.comment_state = CommentPostState::Idle;
        }
    }

    fn select_issue(&mut self, issue_id: IssueId, cx: &mut Context<Self>, force: bool) {
        if self.selected_issue.as_ref() == Some(&issue_id)
            && !force
            && matches!(
                self.detail_state,
                DetailState::Loading { .. } | DetailState::Loaded(_)
            )
        {
            return;
        }
        let load_token = self.diagnostics.begin_image_load();
        if let Some(cancellation) = self.detail_cancellation.take() {
            cancellation.cancel();
        }
        self.invalidate_attachment_download();
        self.selected_image_states.set_context(
            self.diagnostics.clone(),
            DiagnosticFlow::SelectedDetail,
            load_token,
        );
        self.invalidate_comment_selection();
        self.detail_task.take();
        self.detail_generation = self.detail_generation.wrapping_add(1);
        let generation = self.detail_generation;
        self.selected_issue = Some(issue_id.clone());

        let Some(workspace) = self.workspace.clone() else {
            self.detail_state = DetailState::Empty;
            cx.notify();
            return;
        };

        let cancellation = CancellationToken::new();
        self.detail_cancellation = Some(cancellation.clone());
        self.detail_state = DetailState::Loading {
            issue_id: issue_id.clone(),
        };
        let users = self.users.clone();
        let diagnostics = self.diagnostics.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = workspace
                .fetch_issue_detail(IssueLocator::Id(issue_id.clone()), &cancellation)
                .await;
            let detail = match result {
                Ok(detail) => detail,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if !detail_result_is_current(
                            this.selected_issue.as_ref(),
                            &issue_id,
                            this.detail_generation,
                            generation,
                        ) {
                            return;
                        }
                        this.detail_cancellation = None;
                        this.detail_task = None;
                        this.selected_image_states.clear();
                        this.detail_state = DetailState::Error {
                            issue_id: issue_id.clone(),
                            message: safe_detail_error(&error).to_owned(),
                        };
                        cx.notify();
                    });
                    return;
                }
            };
            let view = IssueDetailViewModel::from_domain(&detail, &users);
            let images = collect_detail_images_with_context(&view);
            let image_contexts = images
                .iter()
                .map(|image| (image.surface_ordinal, image.source))
                .collect::<Vec<_>>();
            let loading = loading_image_states(
                &images,
                &diagnostics,
                DiagnosticFlow::SelectedDetail,
                load_token,
            );
            let site_id = workspace.site_id().clone();
            let applied = this
                .update(cx, |this, cx| {
                    if !image_result_is_current(
                        this.selected_issue.as_ref(),
                        &issue_id,
                        this.detail_generation,
                        generation,
                    ) {
                        return false;
                    }
                    this.detail_state = DetailState::Loaded(view.clone());
                    this.selected_image_states = loading;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !applied {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::SelectedDetail,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
                return;
            }
            let states = fetch_rich_image_states(
                workspace,
                site_id,
                issue_id.clone(),
                images,
                cancellation,
                diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                load_token,
            )
            .await;
            let applied = this
                .update(cx, |this, cx| {
                    if !image_result_is_current(
                        this.selected_issue.as_ref(),
                        &issue_id,
                        this.detail_generation,
                        generation,
                    ) {
                        return false;
                    }
                    this.detail_cancellation = None;
                    this.detail_task = None;
                    if let Ok(states) = states {
                        this.selected_image_states = states;
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !applied {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::SelectedDetail,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
            }
        });
        self.detail_task = Some(task);
        cx.notify();
    }

    fn reload_selected_detail(&mut self, cx: &mut Context<Self>) {
        let Some(issue_id) = self.selected_issue.clone() else {
            return;
        };
        self.select_issue(issue_id, cx, true);
    }

    fn download_attachment(
        &mut self,
        attachment: crate::presentation::AttachmentViewModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.attachment_download_state,
            AttachmentDownloadState::Idle
        ) {
            return;
        }
        if attachment.size_bytes > MAX_ATTACHMENT_DOWNLOAD_BYTES as u64 {
            window.push_notification(
                Notification::error("Attachment is larger than the 64 MiB download limit")
                    .id::<AttachmentNotification>(),
                cx,
            );
            return;
        }
        let remote_issue = match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(&issue.id),
            _ => None,
        };
        let Some(issue_id) = attachment_issue_id(self.selected_issue.as_ref(), remote_issue) else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            window.push_notification(
                Notification::error(
                    "Attachment download unavailable · live workspace is not ready",
                )
                .id::<AttachmentNotification>(),
                cx,
            );
            return;
        };

        let filename = sanitized_attachment_filename(&attachment.filename);
        let picker = cx.prompt_for_new_path(&portal_download_directory(), Some(&filename));
        let generation = self.attachment_download_generation.wrapping_add(1);
        self.attachment_download_generation = generation;
        let cancellation = CancellationToken::new();
        self.attachment_download_cancellation = Some(cancellation.clone());
        self.attachment_download_state = AttachmentDownloadState::Saving {
            attachment_id: attachment.id.clone(),
        };
        let request = AttachmentDownloadRequest {
            site_id: workspace.site_id().clone(),
            issue_id,
            attachment_id: attachment.id.clone(),
            max_bytes: MAX_ATTACHMENT_DOWNLOAD_BYTES,
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let destination = picker
                    .await
                    .map_err(|_| "File picker unavailable".to_owned())?
                    .map_err(|_| "File picker unavailable".to_owned())?
                    .ok_or_else(|| "Download cancelled".to_owned())?;
                cancellation
                    .check()
                    .map_err(|_| "Download cancelled".to_owned())?;
                let content = workspace
                    .download_attachment(request, &cancellation)
                    .await
                    .map_err(|_| "Attachment download failed".to_owned())?;
                cancellation
                    .check()
                    .map_err(|_| "Download cancelled".to_owned())?;
                if content.bytes.len() > MAX_ATTACHMENT_DOWNLOAD_BYTES {
                    return Err("Attachment exceeded the 64 MiB download limit".to_owned());
                }
                let temporary = attachment_temp_path(&destination, &attachment_temp_token());
                let write_destination = temporary.clone();
                let write_cancellation = cancellation.clone();
                let write = cx.background_executor().spawn(async move {
                    write_attachment_temp(&write_destination, &content.bytes, &write_cancellation)
                });
                if let Err(error) = write.await {
                    cleanup_attachment_temp(&temporary);
                    return Err(error);
                }
                if cancellation.is_cancelled() {
                    cleanup_attachment_temp(&temporary);
                    return Err("Download cancelled".to_owned());
                }
                Ok::<(PathBuf, PathBuf), String>((destination, temporary))
            }
            .await;
            let temporary = result.as_ref().ok().map(|(_, temporary)| temporary.clone());
            let update_result = this.update_in(cx, |this, window, cx| {
                if !attachment_download_is_current(
                    this.attachment_download_generation,
                    generation,
                    &cancellation,
                ) {
                    if let Some(temporary) = temporary.as_deref() {
                        cleanup_attachment_temp(temporary);
                    }
                    return;
                }
                this.attachment_download_cancellation = None;
                this.attachment_download_task = None;
                this.attachment_download_state = AttachmentDownloadState::Idle;
                match result {
                    Ok((destination, temporary)) => match std::fs::rename(&temporary, &destination)
                    {
                        Ok(()) => window.push_notification(
                            Notification::success(format!(
                                "Attachment saved · {}",
                                destination.display()
                            ))
                            .id::<AttachmentNotification>(),
                            cx,
                        ),
                        Err(_) => {
                            cleanup_attachment_temp(&temporary);
                            window.push_notification(
                                Notification::error("Could not save the attachment")
                                    .id::<AttachmentNotification>(),
                                cx,
                            );
                        }
                    },
                    Err(message) if message == "Download cancelled" => {}
                    Err(message) => {
                        if let Some(temporary) = temporary.as_deref() {
                            cleanup_attachment_temp(temporary);
                        }
                        window.push_notification(
                            Notification::error(message).id::<AttachmentNotification>(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
            if update_result.is_err()
                && let Some(temporary) = temporary.as_deref()
            {
                cleanup_attachment_temp(temporary);
            }
        });
        self.attachment_download_task = Some(task);
        cx.notify();
    }

    fn download_inline_attachment(
        &mut self,
        expected_issue_id: &IssueId,
        attachment_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (active_issue_id, attachments) = match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, detail, .. } => {
                (issue.id.clone(), detail.attachments.as_slice())
            }
            _ => match (&self.selected_issue, &self.detail_state) {
                (Some(issue_id), DetailState::Loaded(detail)) => {
                    (issue_id.clone(), detail.attachments.as_slice())
                }
                _ => return,
            },
        };
        let Some(attachment) = inline_attachment_for_download(
            expected_issue_id,
            &active_issue_id,
            attachments,
            attachment_id,
        ) else {
            return;
        };
        self.download_attachment(attachment, window, cx);
    }

    fn invalidate_attachment_download(&mut self) {
        if let Some(cancellation) = self.attachment_download_cancellation.take() {
            cancellation.cancel();
        }
        self.attachment_download_task.take();
        self.attachment_download_generation = self.attachment_download_generation.wrapping_add(1);
        self.attachment_download_state = AttachmentDownloadState::Idle;
    }

    fn begin_comment_confirmation(&mut self, cx: &mut Context<Self>) {
        if matches!(
            &self.comment_state,
            CommentPostState::Error {
                unknown_outcome: true,
                ..
            }
        ) {
            self.sync_message =
                "Refresh comments before retrying a comment with an unknown outcome".to_owned();
            cx.notify();
            return;
        }
        let Some(input) = self.comment_input.as_ref() else {
            return;
        };
        let Some(issue) = self.comment_target_issue() else {
            return;
        };
        let body = input.read(cx).value().to_string().trim().to_owned();
        if body.trim().is_empty() {
            self.comment_state = CommentPostState::Error {
                issue_id: issue.id.clone(),
                message: "Comment not posted · enter a non-empty comment".to_owned(),
                unknown_outcome: false,
            };
        } else if body.len() > jira_application::MAX_COMMENT_BYTES {
            self.comment_state = CommentPostState::Error {
                issue_id: issue.id.clone(),
                message: "Comment not posted · comment exceeds the byte limit".to_owned(),
                unknown_outcome: false,
            };
        } else if body.chars().count() > jira_application::MAX_COMMENT_CHARS {
            self.comment_state = CommentPostState::Error {
                issue_id: issue.id.clone(),
                message: "Comment not posted · comment exceeds the character limit".to_owned(),
                unknown_outcome: false,
            };
        } else {
            let chars = body.chars().count();
            let bytes = body.len();
            self.comment_state = CommentPostState::Confirming {
                issue_id: issue.id.clone(),
                issue_key: issue.key.as_str().to_owned(),
                body,
                chars,
                bytes,
            };
        }
        cx.notify();
    }

    fn cancel_comment_confirmation(&mut self, cx: &mut Context<Self>) {
        self.comment_state = CommentPostState::Idle;
        cx.notify();
    }

    fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target_issue_id) = self.comment_target_issue().map(|issue| issue.id.clone())
        else {
            return;
        };
        let Some((issue_id, body)) =
            confirmed_comment_snapshot(&self.comment_state, Some(&target_issue_id))
        else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            self.comment_state = CommentPostState::Error {
                issue_id,
                message: "Comment not posted · live Jira workspace is not ready".to_owned(),
                unknown_outcome: false,
            };
            window.push_notification(
                Notification::error("Comment not posted · live Jira workspace is not ready")
                    .id::<CommentNotification>(),
                cx,
            );
            cx.notify();
            return;
        };
        let generation = self.comment_generation.wrapping_add(1);
        self.comment_generation = generation;
        let cancellation = CancellationToken::new();
        self.comment_cancellation = Some(cancellation.clone());
        self.comment_state = CommentPostState::Posting {
            issue_id: issue_id.clone(),
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = workspace
                .create_comment(IssueLocator::Id(issue_id.clone()), body, &cancellation)
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.comment_generation != generation
                    || !comment_target_is_current(
                        match &this.remote_lookup {
                            RemoteLookupState::Loaded { issue, .. } => Some(&issue.id),
                            RemoteLookupState::Idle
                            | RemoteLookupState::Loading { .. }
                            | RemoteLookupState::Error { .. } => None,
                        },
                        this.selected_issue.as_ref(),
                        &issue_id,
                    )
                {
                    return;
                }
                this.comment_cancellation = None;
                this.comment_task = None;
                match result {
                    Ok(_) => {
                        this.comment_input = None;
                        this.comment_subscriptions.clear();
                        this.comment_state = CommentPostState::Idle;
                        window.push_notification(
                            Notification::success("Comment posted to Jira.")
                                .id::<CommentNotification>(),
                            cx,
                        );
                        if matches!(
                            &this.remote_lookup,
                            RemoteLookupState::Loaded { issue, .. } if issue.id == issue_id
                        ) {
                            this.search_jira(cx);
                        } else {
                            this.reload_selected_detail(cx);
                        }
                    }
                    Err(error) => {
                        let (message, unknown_outcome) = comment_error_message(&error);
                        window.push_notification(
                            Notification::error(message).id::<CommentNotification>(),
                            cx,
                        );
                        this.comment_state = CommentPostState::Error {
                            issue_id: issue_id.clone(),
                            message: message.to_owned(),
                            unknown_outcome,
                        };
                    }
                }
                cx.notify();
            });
        });
        self.comment_task = Some(task);
        cx.notify();
    }

    fn refresh_comments(&mut self, cx: &mut Context<Self>) {
        if !matches!(&self.comment_state, CommentPostState::Posting { .. }) {
            if matches!(
                &self.comment_state,
                CommentPostState::Error {
                    unknown_outcome: true,
                    ..
                }
            ) {
                self.comment_state = CommentPostState::Idle;
            }
            if matches!(&self.remote_lookup, RemoteLookupState::Loaded { .. }) {
                self.search_jira(cx);
            } else {
                self.reload_selected_detail(cx);
            }
        }
    }

    fn apply_cached(&mut self, cached: CachedWorkspace, cx: &mut Context<Self>) {
        let CachedWorkspace { issues, events } = cached;
        let updates = events
            .iter()
            .map(|event| {
                let issue = issues.iter().find(|issue| issue.id == event.issue_id);
                UpdateViewModel::from_domain(event, issue, &self.users)
            })
            .collect();
        self.apply_live_issues(issues, true, cx);
        self.updates = updates;
    }

    fn begin_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.sync_message = "Refresh unavailable · local workspace is not ready".to_owned();
            window.push_notification(
                Notification::error("Refresh unavailable · local workspace is not ready")
                    .id::<RefreshNotification>(),
                cx,
            );
            cx.notify();
            return;
        };
        let cancellation = CancellationToken::new();
        self.operation_in_progress = true;
        self.sync_message = "Refreshing Jira…".to_owned();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = workspace.refresh(&cancellation).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(outcome) => {
                        let issue_count = outcome.cached.issues.len();
                        window.push_notification(
                            Notification::success(format!(
                                "Refresh complete · {issue_count} issues"
                            ))
                            .id::<RefreshNotification>(),
                            cx,
                        );
                        this.sync_message = refresh_complete_message(&outcome);
                        this.apply_cached(outcome.cached, cx);
                        this.start_automatic_polling(cx);
                    }
                    Err(error) => {
                        let message = safe_sync_error(&error);
                        window.push_notification(
                            Notification::error(message).id::<RefreshNotification>(),
                            cx,
                        );
                        this.sync_message = message.to_owned();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            for update in &mut self.updates {
                update.unread = false;
            }
            cx.notify();
            return;
        };
        if self.operation_in_progress {
            return;
        }
        self.operation_in_progress = true;
        self.sync_message = "Marking updates read…".to_owned();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = workspace.mark_all_read().await;
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(result) => {
                        this.apply_cached(result.cached, cx);
                        this.sync_message = format!("Marked {} updates read", result.changed);
                    }
                    Err(error) => this.sync_message = safe_sync_error(&error).to_owned(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn unread_count(&self) -> usize {
        self.updates.iter().filter(|update| update.unread).count()
    }

    fn rich_text_palette(&self, cx: &mut Context<Self>) -> RichTextPalette {
        RichTextPalette {
            foreground: cx.theme().foreground,
            muted: cx.theme().muted_foreground,
            border: cx.theme().border,
            code_surface: cx.theme().muted.opacity(0.18),
            link: cx.theme().link,
            info: cx.theme().link,
            warning: cx.theme().warning,
            success: cx.theme().success,
            danger: cx.theme().danger,
        }
    }

    fn issue_key_with_icon(
        &self,
        key: impl Into<String>,
        issue_type: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .min_w_0()
            .gap_1()
            .child(Icon::new(issue_type_icon(issue_type)).text_color(cx.theme().link))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().link)
                    .child(key.into()),
            )
            .into_any_element()
    }

    fn priority_badge(&self, label: String, cx: &mut Context<Self>) -> AnyElement {
        let (icon, tone) = priority_semantics(&label);
        let color = self.priority_color(tone, cx);
        h_flex()
            .min_w_0()
            .gap_1()
            .child(Icon::new(icon).text_color(color))
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    fn priority_color(&self, tone: PriorityTone, cx: &mut Context<Self>) -> gpui::Hsla {
        match tone {
            PriorityTone::Critical => cx.theme().danger,
            PriorityTone::Elevated => cx.theme().warning,
            PriorityTone::Neutral | PriorityTone::Unknown => cx.theme().muted_foreground,
            PriorityTone::Low | PriorityTone::Minimal => cx.theme().link,
        }
    }

    fn render_sidebar(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let rail = layout.is_rail();
        v_flex()
            .h_full()
            .w(px(layout.sidebar_width()))
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                h_flex()
                    .h(px(72.))
                    .px_4()
                    .gap_3()
                    .when(rail, |this| this.justify_center().px_2().gap_0())
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div()
                            .flex()
                            .size_9()
                            .items_center()
                            .justify_center()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().sidebar_primary)
                            .text_color(cx.theme().sidebar_primary_foreground)
                            .font_bold()
                            .child("JD"),
                    )
                    .child(v_flex().min_w_0().gap_0p5().when(!rail, |this| {
                        this.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_semibold()
                                .child("Jira Desk"),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Read-only sync · explicit comments"),
                        )
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_3()
                    .gap_1()
                    .when(!rail, |this| {
                        this.child(
                            div()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("WORKSPACE"),
                        )
                    })
                    .child(self.nav_item(
                        "Issues",
                        self.issues.len(),
                        self.section == Section::Issues,
                        Section::Issues,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Local updates",
                        self.unread_count(),
                        self.section == Section::Updates,
                        Section::Updates,
                        rail,
                        cx,
                    ))
                    .when(!rail, |this| {
                        this.child(
                            div()
                                .mt_5()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("Jira Project VIEW"),
                        )
                    })
                    .when(!rail, |this| {
                        this.child(
                            v_flex()
                                .mx_1()
                                .mt_1()
                                .p_3()
                                .gap_1()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().sidebar_border)
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(self.workspace_name.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.workspace_members.clone()),
                                ),
                        )
                    }),
            )
            .when(!rail, |this| {
                this.child(
                    v_flex()
                        .p_4()
                        .gap_1()
                        .border_t_1()
                        .border_color(cx.theme().sidebar_border)
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(self.site_label.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.mode_label.clone()),
                        ),
                )
            })
    }

    fn nav_item(
        &self,
        label: &'static str,
        count: usize,
        selected: bool,
        section: Section,
        rail: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon = match section {
            Section::Issues => IconName::LayoutDashboard,
            Section::Updates => IconName::Inbox,
        };
        let visual = if rail {
            Icon::new(icon).into_any_element()
        } else {
            div()
                .text_sm()
                .font_semibold()
                .child(label)
                .into_any_element()
        };
        h_flex()
            .id(label)
            .w_full()
            .px_3()
            .py_2()
            .justify_between()
            .when(rail, |this| this.justify_center().px_1())
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .aria_label(label)
            .when(selected, |this| {
                this.bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().sidebar_accent))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.section = section;
                cx.notify();
            }))
            .child(visual)
            .when(!rail, |this| {
                this.child(
                    div()
                        .min_w(px(26.))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(cx.theme().muted)
                        .text_center()
                        .text_xs()
                        .child(count.to_string()),
                )
            })
    }

    fn render_header(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        v_flex()
            .h(px(if mobile { 84. } else { 72. }))
            .px(px(if mobile { 12. } else { 20. }))
            .py(px(if mobile { 10. } else { 12. }))
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_w_0()
                    .justify_between()
                    .child(div().min_w_0().truncate().text_lg().font_semibold().child(
                        match self.section {
                            Section::Issues => "Jira Project issues",
                            Section::Updates => "Local updates",
                        },
                    ))
                    .child(
                        Button::new("refresh")
                            .compact()
                            .primary()
                            .label(if self.operation_in_progress {
                                "Refreshing…"
                            } else {
                                "Refresh"
                            })
                            .loading(self.operation_in_progress)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.begin_refresh(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.sync_message.clone()),
            )
    }

    fn render_issues(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let issue_list = v_flex()
            .h_full()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(if mobile {
                v_flex()
                    .h(px(58.))
                    .px_3()
                    .justify_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().min_w_0().truncate().child(format!(
                        "{} matching Jira Project issues · My issues",
                        self.issues.len(),
                    )))
                    .into_any_element()
            } else {
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().min_w_0().truncate().child(format!(
                        "{} matching Jira Project issues · My issues",
                        self.issues.len(),
                    )))
                    .child(div().flex_shrink_0().child("Updated newest first"))
                    .into_any_element()
            })
            .when_some(self.search_input.clone(), |this, input| {
                if mobile {
                    this.child(
                        v_flex()
                            .gap_1()
                            .px_2()
                            .py_2()
                            .min_w_0()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                Input::new(&input)
                                    .cleanable(true)
                                    .aria_label("Issue key or summary")
                                    .min_w_0()
                                    .w_full(),
                            )
                            .child(
                                Button::new("search-jira")
                                    .compact()
                                    .w_full()
                                    .label("Search Jira")
                                    .on_click(cx.listener(|this, _, _, cx| this.search_jira(cx))),
                            ),
                    )
                } else {
                    this.child(
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .min_w_0()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                Input::new(&input)
                                    .cleanable(true)
                                    .aria_label("Issue key or summary")
                                    .min_w_0()
                                    .flex_1(),
                            )
                            .child(
                                Button::new("search-jira")
                                    .compact()
                                    .label("Search Jira")
                                    .on_click(cx.listener(|this, _, _, cx| this.search_jira(cx))),
                            ),
                    )
                }
            })
            .child(
                h_flex()
                    .h(px(44.))
                    .px_3()
                    .gap_1()
                    .flex_shrink_0()
                    .min_w_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(self.status_filter_dropdown()),
            )
            .child(
                v_flex()
                    .id("issue-list")
                    .min_h_0()
                    .flex_1()
                    .when(mobile, |this| this.w_full())
                    .overflow_y_scrollbar()
                    .when_some(self.remote_lookup_view(), |this, issue| {
                        this.child(self.issue_row_with_label(
                            &issue,
                            "Jira lookup result",
                            layout,
                            cx,
                        ))
                    })
                    .children(
                        self.issues
                            .iter()
                            .map(|issue| self.issue_row(issue, layout, cx)),
                    ),
            )
            .into_any_element();

        let panes = match issues_pane_mode(layout, self.mobile_detail_open) {
            IssuesPaneMode::ListAndDetail => {
                let (list_min, list_max) = layout.issue_list_range();
                let detail = v_flex()
                    .size_full()
                    .min_w_0()
                    .child(self.issue_detail(layout, cx));
                h_resizable(layout.resizable_id())
                    .child(
                        resizable_panel()
                            .size(px(layout.issue_list_width()))
                            .size_range(px(list_min)..px(list_max))
                            .flex_none()
                            .child(issue_list),
                    )
                    .child(
                        resizable_panel()
                            .size_range(px(layout.detail_min_width())..px(4_096.))
                            .child(detail),
                    )
                    .into_any_element()
            }
            IssuesPaneMode::ListOnly => issue_list,
            IssuesPaneMode::DetailOnly => v_flex()
                .size_full()
                .min_w_0()
                .child(
                    h_flex()
                        .h(px(44.))
                        .px_3()
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("mobile-detail-back")
                                .compact()
                                .ghost()
                                .label("Back to issues")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.mobile_detail_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(self.issue_detail(layout, cx))
                .into_any_element(),
        };

        h_flex().size_full().min_w_0().child(panes)
    }

    fn status_filter_dropdown(&self) -> impl IntoElement {
        let state = self
            .status_combobox
            .as_ref()
            .expect("status combobox initialized before issue rendering");
        Combobox::new(state)
            .w_full()
            .cleanable(true)
            .footer(|_, cx| {
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Select one or more statuses"),
                    )
                    .child(
                        Button::new("status-filter-done")
                            .secondary()
                            .outline()
                            .compact()
                            .label("Done")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(Cancel), cx);
                            }),
                    )
            })
            .render_trigger(|trigger, _, _| {
                let selection = IssueStatusSelection::from_values(
                    trigger.selection().iter().map(|(_, item)| *item.value()),
                );
                div()
                    .min_w_0()
                    .w_full()
                    .truncate()
                    .child(status_filter_trigger_label(selection))
            })
    }

    fn remote_lookup_view(&self) -> Option<IssueViewModel> {
        match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => {
                Some(IssueViewModel::from_domain(issue, &self.users))
            }
            RemoteLookupState::Idle
            | RemoteLookupState::Loading { .. }
            | RemoteLookupState::Error { .. } => None,
        }
    }

    fn selected_issue_view(&self) -> Option<IssueViewModel> {
        let selected = self.selected_issue.as_ref()?;
        self.issues
            .iter()
            .find(|issue| &issue.id == selected)
            .cloned()
            .or_else(|| {
                self.domain_issues
                    .iter()
                    .find(|issue| &issue.id == selected)
                    .map(|issue| IssueViewModel::from_domain(issue, &self.users))
            })
    }

    fn comment_target_issue(&self) -> Option<&Issue> {
        match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(issue),
            RemoteLookupState::Idle
            | RemoteLookupState::Loading { .. }
            | RemoteLookupState::Error { .. } => self
                .selected_issue
                .as_ref()
                .and_then(|id| self.domain_issues.iter().find(|issue| &issue.id == id)),
        }
    }

    fn issue_row(
        &self,
        issue: &IssueViewModel,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.issue_row_with_label(issue, "", layout, cx)
    }

    fn issue_row_with_label(
        &self,
        issue: &IssueViewModel,
        label: &str,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_issue.as_ref() == Some(&issue.id)
            || matches!(
                &self.remote_lookup,
                RemoteLookupState::Loaded { issue: remote, .. } if remote.id == issue.id
            );
        let issue_id = issue.id.clone();
        let is_remote_result = !label.is_empty();
        let mobile = layout.is_mobile();
        v_flex()
            .id(issue.id.to_string())
            .w_full()
            .p_4()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .when(selected, |this| this.bg(cx.theme().list_active))
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().list_hover))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if !is_remote_result {
                    this.clear_remote_lookup();
                    this.select_issue(issue_id.clone(), cx, false);
                }
                this.mobile_detail_open = mobile;
                cx.notify();
            }))
            .child(
                h_flex()
                    .min_w_0()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(self.issue_key_with_icon(
                                issue.key.clone(),
                                &issue.issue_type,
                                cx,
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(issue.issue_type.clone()),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink_0()
                            .truncate()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(cx.theme().secondary)
                            .text_color(cx.theme().secondary_foreground)
                            .text_xs()
                            .child(issue.status.clone()),
                    ),
            )
            .when(!label.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().link)
                        .child(label.to_owned()),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .line_clamp(2)
                    .text_sm()
                    .font_semibold()
                    .child(issue.summary.clone()),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(format!("{} ·", issue.assignee)),
                            )
                            .child(self.priority_badge(issue.priority.clone(), cx)),
                    )
                    .child(div().flex_shrink_0().child(issue.updated.clone())),
            )
            .into_any_element()
    }

    fn active_image_states(&self) -> &RichImageRenderStates {
        if matches!(self.remote_lookup, RemoteLookupState::Loaded { .. }) {
            &self.remote_image_states
        } else {
            &self.selected_image_states
        }
    }

    fn issue_detail(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let issue = match &self.remote_lookup {
            RemoteLookupState::Loaded { .. } => self.remote_lookup_view(),
            RemoteLookupState::Loading { .. } | RemoteLookupState::Error { .. } => None,
            RemoteLookupState::Idle => self.selected_issue_view(),
        };
        let lookup_query = match &self.remote_lookup {
            RemoteLookupState::Loading { query } | RemoteLookupState::Error { query, .. } => {
                Some(query.as_str())
            }
            RemoteLookupState::Idle | RemoteLookupState::Loaded { .. } => None,
        };
        let project = issue
            .as_ref()
            .map(|issue| issue.project.clone())
            .unwrap_or_else(|| "Jira".to_owned());
        let key = issue
            .as_ref()
            .map(|issue| issue.key.clone())
            .or_else(|| lookup_query.map(str::to_owned))
            .unwrap_or_else(|| "—".to_owned());
        let summary = issue
            .as_ref()
            .map(|issue| issue.summary.clone())
            .unwrap_or_else(|| {
                if lookup_query.is_some() {
                    "Jira lookup".to_owned()
                } else {
                    "No issues loaded".to_owned()
                }
            });
        let issue_type = issue
            .as_ref()
            .map(|issue| issue.issue_type.clone())
            .unwrap_or_else(|| "—".to_owned());
        let status = issue
            .as_ref()
            .map(|issue| issue.status.clone())
            .unwrap_or_else(|| "Ready to refresh".to_owned());
        let priority = issue
            .as_ref()
            .map(|issue| issue.priority.clone())
            .unwrap_or_else(|| "—".to_owned());
        let detail_state = match &self.remote_lookup {
            RemoteLookupState::Loaded { detail, .. } => DetailState::Loaded(detail.clone()),
            RemoteLookupState::Loading { query } => DetailState::RemoteLoading {
                query: query.clone(),
            },
            RemoteLookupState::Error { query, message } => DetailState::RemoteError {
                query: query.clone(),
                message: message.clone(),
            },
            RemoteLookupState::Idle => self.detail_state.clone(),
        };
        let description = match &detail_state {
            DetailState::Loaded(detail) => detail.description.clone(),
            _ => issue.as_ref().map_or_else(
                || "Select an issue to load its details.".to_owned(),
                |issue| issue.description.clone(),
            ),
        };
        let rich_description = match &detail_state {
            DetailState::Loaded(detail) => detail.rich_description.clone(),
            _ => issue
                .as_ref()
                .and_then(|issue| issue.rich_description.clone()),
        };
        let detail_issue_id = match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(issue.id.clone()),
            _ if matches!(&detail_state, DetailState::Loaded(_)) => self.selected_issue.clone(),
            _ => None,
        };
        let inline_attachment_action = detail_issue_id.map(|expected_issue_id| {
            let dashboard = cx.entity().downgrade();
            RichAttachmentCardAction::new(move |attachment_id, window, app| {
                if let Some(dashboard) = dashboard.upgrade() {
                    let expected_issue_id = expected_issue_id.clone();
                    dashboard.update(app, |this, cx| {
                        this.download_inline_attachment(
                            &expected_issue_id,
                            attachment_id,
                            window,
                            cx,
                        );
                    });
                }
            })
        });
        let description_content = rich_description
            .as_ref()
            .map(|document| {
                render_rich_text_with_actions(
                    document,
                    self.rich_text_palette(cx),
                    self.active_image_states(),
                    0,
                    ImageSource::ResolvedAdf,
                    inline_attachment_action.clone(),
                )
            })
            .unwrap_or_else(|| div().text_sm().child(description).into_any_element());
        let assignee = issue
            .as_ref()
            .map(|issue| issue.assignee.clone())
            .unwrap_or_else(|| "—".to_owned());
        let reporter = issue
            .as_ref()
            .map(|issue| issue.reporter.clone())
            .unwrap_or_else(|| "—".to_owned());
        let status_category = issue
            .as_ref()
            .map(|issue| issue.status_category.clone())
            .unwrap_or_else(|| "—".to_owned());
        let parent = issue
            .as_ref()
            .and_then(|issue| issue.parent.clone())
            .unwrap_or_else(|| "None".to_owned());
        let created = issue
            .as_ref()
            .map(|issue| issue.created.clone())
            .unwrap_or_else(|| "—".to_owned());
        let updated = issue
            .as_ref()
            .map(|issue| issue.updated.clone())
            .unwrap_or_else(|| "—".to_owned());
        let due_date = issue
            .as_ref()
            .map(|issue| issue.due_date.clone())
            .unwrap_or_else(|| "—".to_owned());
        let labels = issue
            .as_ref()
            .map(|issue| issue.labels.clone())
            .unwrap_or_default();
        v_flex()
            .id("issue-detail")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_y_scrollbar()
            .p(px(layout.detail_padding()))
            .gap(px(if layout.is_mobile() { 16. } else { 20. }))
            .child(
                v_flex()
                    .min_w_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(div().min_w_0().truncate().child(project))
                            .child("/")
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        Icon::new(issue_type_icon(&issue_type))
                                            .text_color(cx.theme().link),
                                    )
                                    .child(div().min_w_0().truncate().child(key)),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .line_clamp(if layout.is_mobile() { 3 } else { 4 })
                            .text_2xl()
                            .font_semibold()
                            .child(summary),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .min_w_0()
                            .gap_2()
                            .child(self.pill(issue_type, cx))
                            .child(self.pill(status, cx))
                            .child(self.priority_badge(priority, cx)),
                    )
                    .when(
                        matches!(&self.remote_lookup, RemoteLookupState::Loaded { .. }),
                        |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().link)
                                    .child("Jira lookup result"),
                            )
                        },
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Description"))
                    .child(
                        div()
                            .p_4()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(description_content),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().font_semibold().child("Details"))
                    .child(self.detail_field("Assignee", assignee, layout, cx))
                    .child(self.detail_field("Reporter", reporter, layout, cx))
                    .child(self.detail_field("Status category", status_category, layout, cx))
                    .child(self.detail_field("Parent", parent, layout, cx))
                    .child(self.detail_field("Created", created, layout, cx))
                    .child(self.detail_field("Updated", updated, layout, cx))
                    .child(self.detail_field("Due date", due_date, layout, cx)),
            )
            .when(!labels.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_semibold().child("Labels"))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .min_w_0()
                                .gap_2()
                                .children(labels.iter().cloned().map(|label| self.pill(label, cx))),
                        ),
                )
            })
            .child(self.render_detail_state_for(&detail_state, layout, cx))
    }

    fn render_detail_state_for(
        &self,
        detail_state: &DetailState,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match detail_state {
            DetailState::Empty => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("Comments and attachments"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(if self.selected_issue.is_some() {
                            "Select the issue to load its Jira details."
                        } else {
                            "Select an issue to load comments and attachments."
                        }),
                )
                .into_any_element(),
            DetailState::Loading { .. } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("Comments and attachments"),
                )
                .child(
                    h_flex().gap_2().child(Spinner::new()).child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Loading issue details…"),
                    ),
                )
                .into_any_element(),
            DetailState::RemoteLoading { query } => v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Jira lookup"))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Looking up {query}…")),
                )
                .into_any_element(),
            DetailState::Error { message, .. } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("Comments and attachments"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(message.clone()),
                )
                .into_any_element(),
            DetailState::RemoteError { message, .. } => v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Jira lookup"))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(message.clone()),
                )
                .into_any_element(),
            DetailState::Loaded(detail) => {
                let palette = self.rich_text_palette(cx);
                let comments = if detail.comments.is_empty() {
                    v_flex()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No comments.")
                        .into_any_element()
                } else {
                    v_flex()
                        .min_w_0()
                        .gap_2()
                        .child(
                            h_flex()
                                .min_w_0()
                                .flex_wrap()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Issue"),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Jira exposes these comments at issue level"),
                                ),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap_3()
                                .border_l_1()
                                .border_color(cx.theme().border)
                                .pl_3()
                                .children(detail.comments.iter().enumerate().map(
                                    |(comment_index, comment)| {
                                        let body = comment
                                            .rich_body
                                            .as_ref()
                                            .map(|document| {
                                                render_rich_text(
                                                    document,
                                                    palette,
                                                    self.active_image_states(),
                                                    comment_index.saturating_add(1),
                                                    ImageSource::ResolvedAdf,
                                                )
                                            })
                                            .unwrap_or_else(|| {
                                                div()
                                                    .text_sm()
                                                    .child(comment.body.clone())
                                                    .into_any_element()
                                            });
                                        v_flex()
                                            .gap_1()
                                            .p_3()
                                            .rounded(cx.theme().radius)
                                            .border_1()
                                            .border_color(cx.theme().border)
                                            .child(
                                                h_flex()
                                                    .min_w_0()
                                                    .flex_wrap()
                                                    .justify_between()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_sm()
                                                            .font_semibold()
                                                            .child(comment.author.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_shrink_0()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(comment.created.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("On issue"),
                                            )
                                            .child(div().min_w_0().child(body))
                                            .when_some(comment.updated.clone(), |this, updated| {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(format!("Updated {updated}")),
                                                )
                                            })
                                    },
                                )),
                        )
                        .into_any_element()
                };
                let attachments = if detail.attachments.is_empty() {
                    v_flex()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No attachments.")
                        .into_any_element()
                } else {
                    v_flex()
                        .gap_2()
                        .children(detail.attachments.iter().map(|attachment| {
                            let attachment_for_click = attachment.clone();
                            let downloading = matches!(
                                &self.attachment_download_state,
                                AttachmentDownloadState::Saving { attachment_id }
                                    if attachment_id == &attachment.id
                            );
                            let download_active = !matches!(
                                self.attachment_download_state,
                                AttachmentDownloadState::Idle
                            );
                            h_flex()
                                .min_w_0()
                                .flex_wrap()
                                .justify_between()
                                .text_sm()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .child(attachment.filename.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} · {}",
                                            attachment.mime_type, attachment.size
                                        )),
                                )
                                .child(
                                    Button::new(format!("download-attachment-{}", attachment.id))
                                        .ghost()
                                        .label(attachment_download_button_label(downloading))
                                        .loading(downloading)
                                        .disabled(download_active)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.download_attachment(
                                                attachment_for_click.clone(),
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                        }))
                        .into_any_element()
                };
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_sm().font_semibold().child("Comments"))
                            .child(
                                Button::new("refresh-comments")
                                    .secondary()
                                    .outline()
                                    .compact()
                                    .label("Refresh comments")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.refresh_comments(cx)),
                                    ),
                            ),
                    )
                    .child(comments)
                    .child(div().text_sm().font_semibold().child("Attachments"))
                    .child(attachments)
                    .child(self.render_comment_composer(layout, cx))
                    .into_any_element()
            }
        }
    }

    fn render_comment_composer(&self, layout: LayoutMode, cx: &mut Context<Self>) -> AnyElement {
        let Some(input) = self.comment_input.as_ref() else {
            return div().into_any_element();
        };
        let state = self.comment_state.clone();
        let body = match &state {
            CommentPostState::Confirming { body, .. } => body.clone(),
            _ => input.read(cx).value().to_string(),
        };
        let posting = matches!(&state, CommentPostState::Posting { .. });
        let editing_confirmed = matches!(&state, CommentPostState::Confirming { .. });
        let mut composer = v_flex()
            .min_w_0()
            .gap_2()
            .child(div().text_sm().font_semibold().child("Add comment"))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Plain text accepted · sent as safe Jira ADF"),
            )
            .child(
                Textarea::new(input)
                    .w_full()
                    .aria_label("Comment text")
                    .disabled(posting || editing_confirmed),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} characters · {} bytes",
                        body.chars().count(),
                        body.len()
                    )),
            );
        composer = match state {
            CommentPostState::Confirming {
                issue_key,
                body: _,
                chars,
                bytes,
                ..
            } => composer
                .child(
                    div()
                        .min_w_0()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(format!(
                            "Post this comment to {issue_key}? {chars} characters · {bytes} bytes"
                        )),
                )
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("post-comment-now")
                                .primary()
                                .label("Post now")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.post_comment(window, cx)
                                    }),
                                ),
                        )
                        .child(Button::new("cancel-comment").label("Cancel").on_click(
                            cx.listener(|this, _, _, cx| this.cancel_comment_confirmation(cx)),
                        )),
                ),
            CommentPostState::Posting { .. } => composer.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Posting comment…"),
            ),
            CommentPostState::Error {
                message,
                unknown_outcome,
                ..
            } => composer
                .child(div().text_sm().text_color(cx.theme().danger).child(message))
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("post-comment")
                                .primary()
                                .label("Post comment")
                                .disabled(posting)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.begin_comment_confirmation(cx)
                                })),
                        )
                        .when(unknown_outcome, |this| {
                            this.child(
                                Button::new("refresh-comments-after-unknown")
                                    .secondary()
                                    .outline()
                                    .compact()
                                    .label("Refresh comments")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.refresh_comments(cx)),
                                    ),
                            )
                        }),
                ),
            CommentPostState::Idle => composer.child(
                Button::new("post-comment")
                    .primary()
                    .label("Post comment")
                    .disabled(posting)
                    .on_click(cx.listener(|this, _, _, cx| this.begin_comment_confirmation(cx))),
            ),
        };
        composer.into_any_element()
    }

    fn detail_field(
        &self,
        label: &'static str,
        value: String,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if layout.is_mobile() {
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(div().min_w_0().text_sm().child(value))
                .into_any_element()
        } else {
            h_flex()
                .min_w_0()
                .items_start()
                .child(
                    div()
                        .w(px(if layout.is_rail() { 108. } else { 132. }))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(div().min_w_0().text_sm().child(value))
                .into_any_element()
        }
    }

    fn pill(&self, label: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .px_2()
            .py_1()
            .rounded_full()
            .bg(cx.theme().secondary)
            .text_color(cx.theme().secondary_foreground)
            .text_xs()
            .child(label)
            .into_any_element()
    }

    fn ensure_status_combobox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status_combobox.is_some() {
            return;
        }

        let selected_indices = status_filter_indices(self.status_filter);
        let state = cx.new(|cx| {
            ComboboxState::new(status_options(), selected_indices, window, cx)
                .multiple(true)
                .searchable(false)
        });
        self.status_subscriptions.push(cx.subscribe_in(
            &state,
            window,
            |this, _, event: &ComboboxEvent<SearchableVec<StatusOption>>, window, cx| {
                let ComboboxEvent::Change(values) = event else {
                    return;
                };
                let next = IssueStatusSelection::from_values(values.iter().copied());
                let close_after_change =
                    should_close_status_filter_after_change(this.status_filter, next);
                this.set_status_filter(next, cx);
                if close_after_change {
                    window.dispatch_action(Box::new(Cancel), cx);
                }
            },
        ));
        self.status_combobox = Some(state);
    }

    fn ensure_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.is_some() {
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search issue key or summary"));
        self.search_subscriptions
            .push(cx.subscribe_in(&input, window, {
                let input = input.clone();
                move |this, _, event: &InputEvent, _window, cx| match event {
                    InputEvent::Change => {
                        this.set_search_query(input.read(cx).value().to_string(), cx);
                    }
                    InputEvent::PressEnter { .. } => this.search_jira(cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                }
            }));
        self.search_input = Some(input);
    }

    fn ensure_comment_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .placeholder("Write a Jira comment")
        });
        self.comment_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                    if matches!(&this.comment_state, CommentPostState::Error { .. }) {
                        this.comment_state = CommentPostState::Idle;
                    }
                }
            },
        ));
        self.comment_input = Some(input);
    }

    fn render_updates(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .h(px(if mobile { 80. } else { 54. }))
                    .px(px(if mobile { 12. } else { 20. }))
                    .py(px(if mobile { 8. } else { 0. }))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .min_w_0()
                            .justify_between()
                            .child(div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} unread local updates · Changes detected by Jira Desk, not Jira notifications",
                                self.unread_count()
                            )))
                            .child(
                                Button::new("mark-all-read")
                                    .compact()
                                    .ghost()
                                    .label("Mark all read")
                                    .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("update-list")
                    .flex_1()
                    .overflow_y_scrollbar()
                    .min_h_0()
                    .p(px(layout.list_padding()))
                    .gap_3()
                    .children(
                        self.updates
                            .iter()
                            .enumerate()
                            .map(|(index, update)| self.update_card(index, update, layout, cx)),
                    ),
            )
    }

    fn update_card(
        &self,
        index: usize,
        update: &UpdateViewModel,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let issue_type = self
            .domain_issues
            .iter()
            .find(|issue| issue.key.as_str().eq_ignore_ascii_case(&update.issue_key))
            .map(|issue| issue.issue_type.name.as_str())
            .unwrap_or("Unknown");
        h_flex()
            .id(("update-card", index))
            .w_full()
            .items_start()
            .min_w_0()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .when(update.unread, |this| this.bg(cx.theme().list_active))
            .child(
                div()
                    .mt_1()
                    .size_2()
                    .flex_shrink_0()
                    .rounded_full()
                    .when(update.unread, |this| this.bg(cx.theme().primary))
                    .when(!update.unread, |this| this.bg(cx.theme().muted)),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .min_w_0()
                            .justify_between()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .when(layout.is_mobile(), |this| this.flex_col())
                                    .gap_2()
                                    .child(self.issue_key_with_icon(
                                        update.issue_key.clone(),
                                        issue_type,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .line_clamp(2)
                                            .text_sm()
                                            .font_semibold()
                                            .child(update.issue_summary.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(update.occurred_at.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(update.change.clone()),
                    ),
            )
            .into_any_element()
    }

    fn render_mobile_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(48.))
            .flex_shrink_0()
            .px_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("mobile-issues")
                    .compact()
                    .when(self.section == Section::Issues, |this| this.primary())
                    .label(format!("Issues · {}", self.issues.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Issues;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("mobile-updates")
                    .compact()
                    .when(self.section == Section::Updates, |this| this.primary())
                    .label(format!("Updates · {}", self.unread_count()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Updates;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_status_combobox(window, cx);
        self.ensure_search_input(window, cx);
        self.ensure_comment_input(window, cx);
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let content = match self.section {
            Section::Issues => self.render_issues(layout, cx).into_any_element(),
            Section::Updates => self.render_updates(layout, cx).into_any_element(),
        };

        let main = v_flex()
            .h_full()
            .min_w_0()
            .flex_1()
            .child(self.render_header(layout, cx))
            .child(div().min_w_0().min_h_0().flex_1().child(content));

        if layout.is_mobile() {
            v_flex()
                .size_full()
                .min_w_0()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_mobile_nav(cx))
                .child(main)
        } else {
            h_flex()
                .size_full()
                .min_w_0()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_sidebar(layout, cx))
                .child(main)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::normalized_issue_key;
    use crate::sample_data::{sample_issues, sample_users};
    use gpui_component::searchable_list::SearchableListDelegate as _;

    #[test]
    fn status_filter_trigger_summary_is_deterministic() {
        assert_eq!(
            status_filter_trigger_label(IssueStatusSelection::All),
            "All statuses"
        );
        assert_eq!(
            status_filter_trigger_label(IssueStatusSelection::Done),
            "Done"
        );
        assert_eq!(
            status_filter_trigger_label(IssueStatusSelection::from_values([
                IssueStatusSelection::Done,
                IssueStatusSelection::ToDo,
            ])),
            "2 statuses"
        );
    }

    #[test]
    fn status_options_keep_combobox_values_and_labels_aligned() {
        let options = status_options();
        let values = [
            IssueStatusSelection::ToDo,
            IssueStatusSelection::InProgress,
            IssueStatusSelection::Done,
            IssueStatusSelection::Uncategorized,
        ];
        for (index, expected) in values.into_iter().enumerate() {
            let item = options
                .item(gpui_component::IndexPath::new(index))
                .expect("status option");
            assert_eq!(*item.value(), expected);
            assert_eq!(item.title(), expected.label());
        }
    }

    #[test]
    fn status_filter_initial_indices_follow_presentation_order() {
        assert_eq!(status_filter_indices(IssueStatusSelection::All), Vec::new());
        assert_eq!(
            status_filter_indices(IssueStatusSelection::from_values([
                IssueStatusSelection::Done,
                IssueStatusSelection::ToDo,
            ])),
            vec![
                gpui_component::IndexPath::new(0),
                gpui_component::IndexPath::new(2),
            ]
        );
    }

    #[test]
    fn status_filter_closes_only_for_first_single_selection() {
        assert!(should_close_status_filter_after_change(
            IssueStatusSelection::All,
            IssueStatusSelection::ToDo,
        ));
        assert!(!should_close_status_filter_after_change(
            IssueStatusSelection::All,
            IssueStatusSelection::from_values([
                IssueStatusSelection::ToDo,
                IssueStatusSelection::Done,
            ]),
        ));
        assert!(!should_close_status_filter_after_change(
            IssueStatusSelection::ToDo,
            IssueStatusSelection::from_values([
                IssueStatusSelection::ToDo,
                IssueStatusSelection::Done,
            ]),
        ));
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

    #[test]
    fn authenticated_user_is_seeded_for_display_mapping_and_account_filtering() {
        let authenticated_user = sample_users().into_iter().next().expect("sample user");
        let display_name = authenticated_user.display_name.clone();
        let account_id = authenticated_user.account_id.clone();
        let (users, account) = authenticated_identity(Some(authenticated_user));
        let views = issue_views_for_filter(&sample_issues(), &users, IssueStatusFilter::All, "");

        assert_eq!(account, Some(account_id));
        assert_eq!(views[0].assignee, display_name);
    }

    #[test]
    fn status_filter_rebuilds_from_loaded_domain_issues_without_remote_state() {
        let domain_issues = sample_issues();
        let users = sample_users();
        let all = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::All, "");
        let done = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::Done, "");

        assert_eq!(all.len(), 5);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].key, "DESK-163");
    }

    #[test]
    fn stale_detail_results_are_rejected_after_selection_changes() {
        let first = IssueId::new("10001").expect("issue");
        let second = IssueId::new("10002").expect("issue");

        assert!(!detail_result_is_current(Some(&second), &first, 2, 1));
        assert!(!detail_result_is_current(Some(&first), &first, 2, 1));
        assert!(detail_result_is_current(Some(&second), &second, 2, 2));
    }

    #[test]
    fn exact_local_key_hit_returns_id_without_remote_lookup() {
        let issues = sample_issues();
        let key = IssueKey::new("DESK-163").expect("key");
        let expected = issues
            .iter()
            .find(|issue| issue.key == key)
            .map(|issue| issue.id.clone());

        assert_eq!(local_issue_id_for_key(&issues, &key), expected);
    }

    #[test]
    fn invalid_key_is_rejected_before_a_remote_lookup() {
        assert!(normalized_issue_key("summary text").is_none());
        assert!(normalized_issue_key("IX-").is_none());
    }

    #[test]
    fn remote_result_can_be_present_even_when_local_status_filter_hides_it() {
        let issues = sample_issues();
        let users = sample_users();
        let remote = issues
            .iter()
            .find(|issue| issue.key.as_str() == "DESK-163")
            .expect("sample issue")
            .clone();
        let local_done = issue_views_for_filter(&issues, &users, IssueStatusFilter::ToDo, "");
        let remote_view = IssueViewModel::from_domain(&remote, &users);

        assert!(local_done.iter().all(|issue| issue.id != remote.id));
        assert_eq!(remote_view.key, "DESK-163");
    }

    #[test]
    fn stale_remote_results_are_rejected_after_query_changes() {
        assert!(!remote_lookup_result_is_current("IX-2", "IX-1", 2, 1));
        assert!(!remote_lookup_result_is_current("IX-2", "IX-1", 2, 2));
        assert!(remote_lookup_result_is_current(" ix-1 ", "IX-1", 2, 2));
    }

    #[test]
    fn clearing_search_cancels_and_removes_remote_result() {
        let mut dashboard = Dashboard::from_sample_data();
        dashboard.remote_lookup = RemoteLookupState::Error {
            query: "IX-404".to_owned(),
            message: "not found".to_owned(),
        };
        let generation = dashboard.remote_lookup_generation;

        dashboard.clear_remote_lookup();

        assert_eq!(dashboard.remote_lookup, RemoteLookupState::Idle);
        assert_eq!(dashboard.remote_lookup_generation, generation + 1);
    }

    #[test]
    fn comment_failures_have_definite_and_unknown_outcome_messages() {
        let definite = ApplicationError::new(
            jira_application::ErrorKind::Authorization,
            "server detail must not reach UI",
        );
        let (message, unknown) = comment_error_message(&definite);
        assert_eq!(
            message,
            "Comment not posted · Jira denied comment permission"
        );
        assert!(!unknown);
        assert!(!message.contains("server detail"));

        let uncertain = ApplicationError::new(
            jira_application::ErrorKind::UnknownOutcome,
            "secret response",
        );
        let (message, unknown) = comment_error_message(&uncertain);
        assert!(unknown);
        assert!(message.contains("Refresh comments"));
        assert!(!message.contains("secret response"));
    }

    #[test]
    fn comment_post_state_keeps_confirmation_issue_and_sizes() {
        let issue_id = IssueId::new("100").expect("issue");
        let state = CommentPostState::Confirming {
            issue_id: issue_id.clone(),
            issue_key: "IX-100".to_owned(),
            body: "hello".to_owned(),
            chars: 5,
            bytes: 7,
        };
        assert_eq!(
            state,
            CommentPostState::Confirming {
                issue_id,
                issue_key: "IX-100".to_owned(),
                body: "hello".to_owned(),
                chars: 5,
                bytes: 7,
            }
        );
    }

    #[test]
    fn confirmed_comment_snapshot_uses_original_body_and_rejects_other_issue() {
        let issue_a = IssueId::new("100").expect("issue");
        let issue_b = IssueId::new("200").expect("issue");
        let state = CommentPostState::Confirming {
            issue_id: issue_a.clone(),
            issue_key: "IX-100".to_owned(),
            body: "original body".to_owned(),
            chars: 13,
            bytes: 13,
        };

        let edited_editor_value = "edited after confirmation";
        let snapshot = confirmed_comment_snapshot(&state, Some(&issue_a));
        assert_eq!(
            snapshot,
            Some((issue_a.clone(), "original body".to_owned()))
        );
        assert_ne!(
            snapshot.as_ref().map(|(_, body)| body),
            Some(&edited_editor_value.to_owned())
        );
        assert_eq!(confirmed_comment_snapshot(&state, Some(&issue_b)), None);
        assert_eq!(confirmed_comment_snapshot(&state, None), None);
        assert_eq!(
            confirmed_comment_snapshot(
                &CommentPostState::Posting {
                    issue_id: issue_a.clone()
                },
                Some(&issue_a)
            ),
            None
        );
    }

    #[test]
    fn remote_lookup_identity_can_authorize_comment_independently_of_local_selection() {
        let remote_id = IssueId::new("remote-100").expect("issue");
        let local_id = IssueId::new("local-200").expect("issue");

        assert!(comment_target_is_current(
            Some(&remote_id),
            Some(&local_id),
            &remote_id
        ));
        assert!(!comment_target_is_current(
            Some(&remote_id),
            Some(&local_id),
            &local_id
        ));
        assert!(comment_target_is_current(None, Some(&local_id), &local_id));
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
        assert_eq!(images[0].surface_ordinal, 3);
        assert_eq!(images[0].source, ImageSource::ResolvedAdf);
        assert_eq!(images[1].surface_ordinal, 3);
        assert_eq!(images[1].source, ImageSource::FallbackCandidate);
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
    fn image_aggregate_limit_accepts_boundary_and_rejects_overflow() {
        assert!(image_bytes_fit_aggregate(0, MAX_RICH_IMAGE_BYTES));
        assert!(!image_bytes_fit_aggregate(1, MAX_RICH_IMAGE_BYTES));
        assert!(!image_bytes_fit_aggregate(MAX_RICH_IMAGE_BYTES, 1));
    }

    #[test]
    fn image_response_preflight_distinguishes_rejection_causes() {
        assert_eq!(
            image_response_preflight("application/pdf", "image/png", b"", 0),
            ImagePreflight::UnsupportedCachedMime
        );
        assert_eq!(
            image_response_preflight("image/png", "text/html", b"not image", 0),
            ImagePreflight::ResponseMimeRejected
        );
        assert_eq!(
            image_response_preflight("image/png", "image/png", b"not image", 0),
            ImagePreflight::SignatureRejected
        );
        assert_eq!(
            image_response_preflight(
                "image/png",
                "image/png",
                b"\x89PNG\r\n\x1a\n",
                MAX_RICH_IMAGE_BYTES,
            ),
            ImagePreflight::AggregateRejected
        );
    }

    #[test]
    fn image_response_preflight_accepts_authenticated_thumbnail_mime_variants() {
        assert_eq!(
            image_response_preflight(
                "image/png",
                "application/octet-stream",
                b"\x89PNG\r\n\x1a\nvalid png",
                0,
            ),
            ImagePreflight::Accepted,
            "authenticated original-content responses may be octet-stream when bytes are PNG"
        );
        assert_eq!(
            image_response_preflight("image/jpg", "image/jpg", b"\xff\xd8\xff\xe0valid jpeg", 0,),
            ImagePreflight::Accepted,
            "Jira's image/jpg response must be accepted when bytes are JPEG"
        );
    }

    #[test]
    fn image_response_preflight_rejects_unsupported_mime_or_bad_signature() {
        assert_eq!(
            image_response_preflight(
                "image/png",
                "application/pdf",
                b"\x89PNG\r\n\x1a\nvalid png",
                0,
            ),
            ImagePreflight::ResponseMimeRejected
        );
        assert_eq!(
            image_response_preflight("image/jpg", "image/jpg", b"not a jpeg", 0,),
            ImagePreflight::SignatureRejected
        );
    }

    #[test]
    fn image_mime_mapping_accepts_only_supported_render_formats() {
        assert_eq!(image_format_for_mime(" image/png "), Some(ImageFormat::Png));
        assert_eq!(image_format_for_mime("image/jpg"), Some(ImageFormat::Jpeg));
        assert_eq!(image_format_for_mime("image/jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(image_format_for_mime("image/svg+xml"), None);
        assert_eq!(image_format_for_mime("application/pdf"), None);
    }

    #[test]
    fn fetched_image_format_uses_bytes_after_mime_preflight() {
        assert_eq!(
            fetched_image_format("image/jpeg", " image/png ", b"\x89PNG\r\n\x1a\nrest of png",),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            fetched_image_format("image/png", "image/svg+xml", b"\x89PNG\r\n\x1a\n"),
            None,
            "unsupported authenticated responses must not be decoded"
        );
        assert_eq!(
            fetched_image_format("application/pdf", "image/png", b"\x89PNG\r\n\x1a\n"),
            None,
            "non-image ADF metadata must still fail preflight"
        );
    }

    #[test]
    fn image_bytes_are_strictly_signature_detected() {
        assert_eq!(
            image_format_from_bytes(b"\x89PNG\r\n\x1a\nfixture"),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            image_format_from_bytes(b"\xff\xd8\xff\xe0fixture"),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            image_format_from_bytes(b"GIF87afixture"),
            Some(ImageFormat::Gif)
        );
        assert_eq!(
            image_format_from_bytes(b"GIF89afixture"),
            Some(ImageFormat::Gif)
        );

        let mut webp = b"RIFFxxxxWEBP".to_vec();
        webp.extend_from_slice(b"fixture");
        assert_eq!(image_format_from_bytes(&webp), Some(ImageFormat::Webp));
        assert_eq!(image_format_from_bytes(b"not an image"), None);
    }

    #[test]
    fn fetched_image_format_accepts_bytes_when_response_mime_differs() {
        assert_eq!(
            fetched_image_format(
                "image/png",
                "image/jpeg",
                b"\x89PNG\r\n\x1a\nvalid signature",
            ),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            fetched_image_format("image/png", "image/png", b"not an image"),
            None
        );
    }

    #[test]
    fn fetched_image_format_allows_octet_stream_only_for_known_images() {
        assert_eq!(
            fetched_image_format(
                "image/png",
                " application/octet-stream ",
                b"\x89PNG\r\n\x1a\nvalid signature",
            ),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            fetched_image_format("image/png", "application/octet-stream", b"not an image"),
            None,
            "octet-stream responses must have a strict image signature"
        );
        assert_eq!(
            fetched_image_format(
                "application/octet-stream",
                "application/octet-stream",
                b"\x89PNG\r\n\x1a\nvalid signature",
            ),
            None,
            "cached attachment metadata must be an allowlisted image"
        );
        assert_eq!(
            fetched_image_format("image/png", "application/pdf", b"\x89PNG\r\n\x1a\n"),
            None,
            "only octet-stream may use the original-content MIME exception"
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
