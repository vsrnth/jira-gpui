use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{Local, SecondsFormat};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, AnyElement, AppContext as _, Context, Entity, Image, ImageFormat,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use gpui_component::table::{DataTable, TableEvent, TableState};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, StyledExt as _, WindowExt as _,
    button::Button,
    button::ButtonVariants as _,
    combobox::{Combobox, ComboboxEvent, ComboboxState},
    dialog::Cancel,
    h_flex, h_resizable,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    notification::Notification,
    popover::Popover,
    resizable_panel,
    scroll::ScrollableElement as _,
    searchable_list::{SearchableListItem, SearchableVec},
    spinner::Spinner,
    v_flex,
};
use jira_application::{
    ApplicationError, AttachmentDownloadRequest, CancellationToken, DEFAULT_JQL_SCOPE,
    DefaultPollingPolicy, IssueLocator, IssueTransition, JiraCommentWritePort, JiraIssueEditPort,
    JiraReadPort, MAX_JQL_SCOPE_LENGTH, SyncMode,
};

use jira_desktop_notifications::{TEST_NOTIFICATION_BODY, TEST_NOTIFICATION_SUMMARY};
use jira_domain::{
    AccountId, Issue, IssueId, IssueKey, JiraSiteId, RichBlock, RichImage, RichTextDocument, User,
};

use crate::{
    config::{LiveSession, StartupError, ensure_authenticated_user},
    credential_store::{self, DeleteOutcome},
    diagnostics::{
        DesktopNotificationTestResult as DiagnosticDesktopNotificationTestResult,
        DiagnosticErrorKind, DiagnosticFlow, DiagnosticsSink, ImageFetchResult, ImagePreflight,
        ImageSignature, ImageSource, ImageStateReason, ResponseMime,
    },
    live_workspace::{CachedWorkspace, LiveWorkspace, RefreshResult},
    local_data::{
        LocalPreferences, MAX_TEAM_MEMBERS, PersistedTeamMember, load_preferences,
        normalize_issue_jql_scope, normalize_team_members, save_preferences,
    },
    presentation::{
        IssueDetailViewModel, IssueStatusFilter, IssueStatusSelection, IssueViewModel,
        UpdateGroupViewModel, UpdateViewModel, issue_views_for_filter, update_groups_for_events,
    },
    responsive::{IssuesPaneMode, LayoutMode, issues_pane_mode, layout_for_width},
    rich_text_view::{
        RichAttachmentCardAction, RichImageRenderState, RichImageRenderStates, RichTextPalette,
        render_rich_text, render_rich_text_with_actions,
    },
    sample_data::{sample_issues, sample_updates, sample_users},
    semantic_icons::{PriorityTone, issue_type_icon, priority_semantics},
    team_table::{TeamTicketTableDelegate, TeamTicketTableStateExt},
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
        "Refresh complete · {} issues · {} new local updates · {} local updates loaded · desktop notifications: {} accepted by desktop service, {} unavailable · {mode}",
        result.cached.issues.len(),
        result.outcome.events_inserted,
        result.cached.events.len(),
        result.outcome.notifications_delivered,
        result.outcome.notification_failures,
    )
}

fn refresh_notification_message(result: &RefreshResult) -> String {
    let updates = match result.outcome.events_inserted {
        0 => "no new local updates".to_owned(),
        1 => "1 new local update".to_owned(),
        count => format!("{count} new local updates"),
    };
    format!(
        "Refresh complete · {} issues · {updates}",
        result.cached.issues.len()
    )
}

fn desktop_notification_error_category(error: DiagnosticErrorKind) -> &'static str {
    match error {
        DiagnosticErrorKind::Authentication => "authentication",
        DiagnosticErrorKind::Authorization => "authorization",
        DiagnosticErrorKind::RateLimited => "rate_limited",
        DiagnosticErrorKind::Offline => "offline",
        DiagnosticErrorKind::Cancelled => "cancelled",
        DiagnosticErrorKind::InvalidInput => "invalid_input",
        DiagnosticErrorKind::NotFound => "not_found",
        DiagnosticErrorKind::Upstream => "upstream",
        DiagnosticErrorKind::Storage => "storage",
        DiagnosticErrorKind::Notification => "notification",
        DiagnosticErrorKind::Internal => "internal",
        DiagnosticErrorKind::UnknownOutcome => "unknown_outcome",
    }
}

fn authenticated_identity(user: Option<User>) -> (Vec<User>, Option<AccountId>) {
    let account = user.as_ref().map(|user| user.account_id.clone());
    (user.into_iter().collect(), account)
}

fn project_label(issues: &[Issue]) -> String {
    let mut projects = issues
        .iter()
        .map(|issue| issue.project.name.clone())
        .collect::<Vec<_>>();
    projects.sort();
    projects.dedup();
    match projects.as_slice() {
        [] => "Jira projects".to_owned(),
        [project] => project.clone(),
        projects => format!("{} Jira projects", projects.len()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Issues,
    Updates,
    Team,
    Settings,
}

fn team_identifier_lines(value: &str) -> Result<Vec<String>, &'static str> {
    let lines = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.len() > MAX_TEAM_MEMBERS {
        return Err("Team tracker accepts at most 100 members");
    }
    for line in &lines {
        if line.chars().any(char::is_control) || line.len() > 320 {
            return Err("Team tracker entries must be short, single-line values");
        }
        if !line.contains('@') {
            let account = AccountId::new(line.clone())
                .map_err(|_| "Enter a valid Jira account ID or Atlassian email")?;
            if account
                .as_str()
                .chars()
                .any(|character| matches!(character, '"' | '\\'))
            {
                return Err("Jira account IDs cannot contain quote or backslash characters");
            }
        }
    }
    Ok(lines)
}

fn team_issue_counts(issues: &[Issue]) -> (usize, usize) {
    let displayed = issues
        .iter()
        .filter(|issue| {
            issue
                .status
                .category
                .as_deref()
                .is_some_and(|category| category.trim().eq_ignore_ascii_case("in progress"))
        })
        .count();
    (issues.len(), displayed)
}

fn team_refresh_feedback(prefix: &str, issues: &[Issue]) -> String {
    let (fetched, displayed) = team_issue_counts(issues);
    format!(
        "{prefix} · fetched {fetched} · displaying {displayed} in-progress {}",
        if displayed == 1 { "ticket" } else { "tickets" }
    )
}

fn team_summary(displayed: usize, configured_members: usize) -> String {
    format!(
        "{displayed} in-progress {} displayed · {configured_members} configured team {} · cached updates remain isolated from Jira issues",
        if displayed == 1 { "ticket" } else { "tickets" },
        if configured_members == 1 {
            "member"
        } else {
            "members"
        },
    )
}

fn team_email_resolution_message(candidate_count: usize) -> Option<&'static str> {
    match candidate_count {
        1 => None,
        0 => Some("Email did not resolve to one active Jira user"),
        _ => Some("Email matched multiple active Jira users; enter an account ID instead"),
    }
}

fn persisted_direct_team_member(identifier: String) -> Result<PersistedTeamMember, &'static str> {
    let account_id = AccountId::new(identifier.clone())
        .map_err(|_| "Enter a valid Jira account ID or Atlassian email")?;
    if account_id
        .as_str()
        .chars()
        .any(|character| matches!(character, '"' | '\\'))
    {
        return Err("Jira account IDs cannot contain quote or backslash characters");
    }
    Ok(PersistedTeamMember {
        identifier,
        account_id: account_id.into_inner(),
        display_name: "Unknown user".to_owned(),
    })
}

fn persisted_team_member_has_display_name(member: &PersistedTeamMember) -> bool {
    !member.display_name.trim().is_empty()
        && !member.display_name.eq_ignore_ascii_case("unknown user")
        && member.display_name != member.account_id
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DesktopNotificationTestOutcome {
    Accepted { notification_id: u32 },
    Failed(DiagnosticErrorKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DesktopNotificationTestReport {
    timestamp: String,
    outcome: DesktopNotificationTestOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DesktopNotificationTestState {
    Idle,
    Sending,
    Completed(DesktopNotificationTestReport),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SavedLoginDeleteOutcome {
    Deleted,
    Absent,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SavedLoginDeleteState {
    Idle,
    Deleting,
    Completed(SavedLoginDeleteOutcome),
}

fn saved_login_delete_feedback(outcome: SavedLoginDeleteOutcome) -> (&'static str, bool) {
    match outcome {
        SavedLoginDeleteOutcome::Deleted => (
            "Saved Jira login forgotten. This session remains connected.",
            false,
        ),
        SavedLoginDeleteOutcome::Absent => (
            "No saved Jira login was present. This session remains connected.",
            false,
        ),
        SavedLoginDeleteOutcome::Error => (
            "Saved Jira login could not be removed from the system keyring.",
            true,
        ),
    }
}

fn can_start_saved_login_delete(state: SavedLoginDeleteState) -> bool {
    !matches!(state, SavedLoginDeleteState::Deleting)
}

struct RefreshNotification;
struct CommentNotification;
struct IssueEditNotification;
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum IssueEditState {
    Idle,
    LoadingAssignees {
        issue_id: IssueId,
        query: String,
    },
    AssigneeChooser {
        issue_id: IssueId,
        issue_key: String,
        query: String,
        users: Vec<User>,
    },
    LoadingTransitions {
        issue_id: IssueId,
    },
    TransitionChooser {
        issue_id: IssueId,
        issue_key: String,
        transitions: Vec<IssueTransition>,
    },
    ConfirmingAssignee {
        issue_id: IssueId,
        issue_key: String,
        account_id: Option<AccountId>,
        display_name: String,
    },
    ConfirmingTransition {
        issue_id: IssueId,
        issue_key: String,
        transition_id: String,
        transition_name: String,
        target_status: String,
    },
    Submitting {
        issue_id: IssueId,
        target: String,
    },
    Error {
        issue_id: IssueId,
        message: String,
        unknown_outcome: bool,
        operation: IssueEditOperation,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IssueEditOperation {
    Assignee,
    Transition,
}

fn issue_edit_error_message(error: &ApplicationError, operation: &str) -> (&'static str, bool) {
    match error.kind() {
        jira_application::ErrorKind::UnknownOutcome => (
            "Jira may have accepted this change. Refresh Jira before another attempt.",
            true,
        ),
        jira_application::ErrorKind::Authentication => (
            "Change not applied · Jira authentication was rejected",
            false,
        ),
        jira_application::ErrorKind::Authorization => {
            ("Change not applied · Jira denied permission", false)
        }
        jira_application::ErrorKind::NotFound => {
            ("Change not applied · the Jira issue was not found", false)
        }
        jira_application::ErrorKind::RateLimited => (
            "Change not applied · Jira rate limit reached; try later",
            false,
        ),
        jira_application::ErrorKind::Offline => ("Change not applied · Jira is unreachable", false),
        jira_application::ErrorKind::InvalidInput => (
            "Change not applied · Jira rejected the requested change",
            false,
        ),
        jira_application::ErrorKind::Cancelled => ("Change cancelled", false),
        _ if operation == "lookup" => (
            "Jira options unavailable · request was not completed",
            false,
        ),
        _ => ("Change not applied · Jira returned an error", false),
    }
}

fn status_control_is_editable(
    has_workspace: bool,
    is_selected_issue: bool,
    is_remote_lookup: bool,
    operation_in_progress: bool,
    issue_edit_state: &IssueEditState,
) -> bool {
    has_workspace
        && is_selected_issue
        && !is_remote_lookup
        && !operation_in_progress
        && matches!(
            issue_edit_state,
            IssueEditState::Idle
                | IssueEditState::LoadingTransitions { .. }
                | IssueEditState::TransitionChooser { .. }
        )
}

fn transition_option_label(transition: &IssueTransition) -> &str {
    transition.to.name.as_str()
}

const STATUS_TRANSITION_ROW_HEIGHT: f32 = 32.;
const STATUS_TRANSITION_GAP: f32 = 4.;
const STATUS_TRANSITION_LIST_MAX_HEIGHT: f32 = 280.;

fn status_transition_list_height(transition_count: usize) -> Pixels {
    let rows_height = transition_count as f32 * STATUS_TRANSITION_ROW_HEIGHT;
    let gaps_height = transition_count.saturating_sub(1) as f32 * STATUS_TRANSITION_GAP;
    px((rows_height + gaps_height).min(STATUS_TRANSITION_LIST_MAX_HEIGHT))
}

fn issue_edit_target_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    selected_issue == Some(expected_issue) && generation == expected_generation
}

fn update_group_event_ids(group: &UpdateGroupViewModel) -> Vec<jira_domain::EventId> {
    group
        .events
        .iter()
        .map(|event| event.event_id.clone())
        .collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum UpdateFilter {
    #[default]
    All,
    Unread,
}

fn filtered_update_group_indices(
    groups: &[UpdateGroupViewModel],
    filter: UpdateFilter,
) -> Vec<usize> {
    groups
        .iter()
        .enumerate()
        .filter(|(_, group)| filter == UpdateFilter::All || group.unread)
        .map(|(index, _)| index)
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CompactedUpdateRow {
    Event(UpdateViewModel),
    GenericSummary { count: usize, occurred_at: String },
}

fn compact_update_rows(events: &[UpdateViewModel]) -> Vec<CompactedUpdateRow> {
    let generic_count = events
        .iter()
        .filter(|event| event.change == "Issue activity changed")
        .count();
    let mut summary_inserted = false;
    events
        .iter()
        .filter_map(|event| {
            if event.change == "Issue activity changed" {
                if summary_inserted {
                    None
                } else {
                    summary_inserted = true;
                    Some(CompactedUpdateRow::GenericSummary {
                        count: generic_count,
                        occurred_at: event.occurred_at.clone(),
                    })
                }
            } else {
                Some(CompactedUpdateRow::Event(event.clone()))
            }
        })
        .collect()
}

fn generic_summary_label(count: usize) -> String {
    if count == 1 {
        "Other Jira activity · exact field not available from sync".to_owned()
    } else {
        format!("Other Jira activity · {count} events · exact field not available from sync")
    }
}

const UPDATE_PREVIEW_LIMIT: usize = 3;

fn visible_update_row_count(row_count: usize, expanded: bool) -> usize {
    if expanded {
        row_count
    } else {
        row_count.min(UPDATE_PREVIEW_LIMIT)
    }
}

fn hidden_update_row_count(row_count: usize, expanded: bool) -> usize {
    row_count.saturating_sub(visible_update_row_count(row_count, expanded))
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

fn selected_issue_view_from_sources(
    selected_issue: Option<&IssueId>,
    visible_issues: &[IssueViewModel],
    domain_issues: &[Issue],
    selected_issue_core: Option<&Issue>,
    users: &[User],
) -> Option<IssueViewModel> {
    let selected = selected_issue?;
    visible_issues
        .iter()
        .find(|issue| &issue.id == selected)
        .cloned()
        .or_else(|| {
            domain_issues
                .iter()
                .find(|issue| &issue.id == selected)
                .map(|issue| IssueViewModel::from_domain(issue, users))
        })
        .or_else(|| {
            selected_issue_core
                .filter(|issue| &issue.id == selected)
                .map(|issue| IssueViewModel::from_domain(issue, users))
        })
}

fn selected_issue_from_sources<'a>(
    selected_issue: Option<&IssueId>,
    domain_issues: &'a [Issue],
    selected_issue_core: Option<&'a Issue>,
) -> Option<&'a Issue> {
    let selected = selected_issue?;
    domain_issues
        .iter()
        .find(|issue| &issue.id == selected)
        .or_else(|| selected_issue_core.filter(|issue| &issue.id == selected))
}

fn selection_after_issue_view_rebuild(
    selected_issue: Option<IssueId>,
    visible_issues: &[IssueViewModel],
) -> Option<IssueId> {
    selected_issue.or_else(|| visible_issues.first().map(|issue| issue.id.clone()))
}

fn should_defer_detail_refresh(
    selected_issue: Option<&IssueId>,
    domain_issues: &[Issue],
    detail_state: &DetailState,
    refresh_detail: bool,
) -> bool {
    let Some(selected_issue) = selected_issue else {
        return false;
    };
    refresh_detail
        && !domain_issues
            .iter()
            .any(|issue| &issue.id == selected_issue)
        && matches!(
            detail_state,
            DetailState::Loading { issue_id } | DetailState::Error { issue_id, .. }
                if issue_id == selected_issue
        )
}

pub struct Dashboard {
    diagnostics: DiagnosticsSink,
    section: Section,
    domain_issues: Vec<Issue>,
    issues: Vec<IssueViewModel>,
    update_groups: Vec<UpdateGroupViewModel>,
    update_filter: UpdateFilter,
    expanded_update_groups: HashSet<IssueId>,
    selected_issue: Option<IssueId>,
    selected_issue_core: Option<Issue>,
    mobile_detail_open: bool,
    sync_message: String,
    workspace: Option<Arc<LiveWorkspace>>,
    users: Vec<User>,
    workspace_name: String,
    workspace_members: String,
    team_issues: Vec<Issue>,
    team_events: Vec<jira_domain::UpdateEvent>,
    team_members: Vec<PersistedTeamMember>,
    team_table: Option<Entity<TableState<TeamTicketTableDelegate>>>,
    team_table_subscriptions: Vec<Subscription>,
    team_input: Option<Entity<TextareaState>>,
    team_text: String,
    team_feedback: Option<String>,
    team_task: Option<gpui::Task<()>>,
    team_age_task: Option<gpui::Task<()>>,
    team_automatic_polling_paused: bool,
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
    issue_edit_state: IssueEditState,
    status_popover_open: bool,
    issue_edit_generation: u64,
    issue_edit_cancellation: Option<CancellationToken>,
    issue_edit_task: Option<gpui::Task<()>>,
    assignee_input: Option<Entity<InputState>>,
    assignee_subscriptions: Vec<Subscription>,
    issue_edit_reconciliation_pending: bool,
    attachment_download_state: AttachmentDownloadState,
    attachment_download_generation: u64,
    attachment_download_cancellation: Option<CancellationToken>,
    attachment_download_task: Option<gpui::Task<()>>,
    settings_input: Option<Entity<TextareaState>>,
    settings_subscriptions: Vec<Subscription>,
    settings_scope_text: String,
    settings_warning: Option<String>,
    settings_feedback: Option<String>,
    settings_task: Option<gpui::Task<()>>,
    saved_login_delete_state: SavedLoginDeleteState,
    saved_login_delete_task: Option<gpui::Task<()>>,
    desktop_notification_test_state: DesktopNotificationTestState,
    desktop_notification_test_task: Option<gpui::Task<()>>,
}

impl Dashboard {
    pub fn from_sample_data() -> Self {
        Self::from_sample_data_with_diagnostics(DiagnosticsSink::disabled())
    }

    fn from_sample_data_with_diagnostics(diagnostics: DiagnosticsSink) -> Self {
        let domain_issues = sample_issues();
        let users = sample_users();
        let sample_events = sample_updates();
        let update_groups = update_groups_for_events(&sample_events, &domain_issues, &users);
        let issues = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::All, "");
        let selected_issue = issues.first().map(|issue| issue.id.clone());

        Self {
            diagnostics: diagnostics.clone(),
            section: Section::Issues,
            domain_issues,
            issues,
            update_groups,
            update_filter: UpdateFilter::All,
            expanded_update_groups: HashSet::new(),
            selected_issue,
            selected_issue_core: None,
            mobile_detail_open: false,
            sync_message: "Preview data · Jira connection not configured".to_owned(),
            workspace: None,
            users,
            workspace_name: "Platform team".to_owned(),
            workspace_members: "Amina, Devon, Marco".to_owned(),
            team_issues: Vec::new(),
            team_events: Vec::new(),
            team_members: Vec::new(),
            team_table: None,
            team_table_subscriptions: Vec::new(),
            team_input: None,
            team_text: String::new(),
            team_feedback: None,
            team_task: None,
            team_age_task: None,
            team_automatic_polling_paused: false,
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
            issue_edit_state: IssueEditState::Idle,
            status_popover_open: false,
            issue_edit_generation: 0,
            issue_edit_cancellation: None,
            issue_edit_task: None,
            assignee_input: None,
            assignee_subscriptions: Vec::new(),
            issue_edit_reconciliation_pending: false,
            attachment_download_state: AttachmentDownloadState::Idle,
            attachment_download_generation: 0,
            attachment_download_cancellation: None,
            attachment_download_task: None,
            settings_input: None,
            settings_subscriptions: Vec::new(),
            settings_scope_text: DEFAULT_JQL_SCOPE.to_owned(),
            settings_warning: None,
            settings_feedback: None,
            settings_task: None,
            saved_login_delete_state: SavedLoginDeleteState::Idle,
            saved_login_delete_task: None,
            desktop_notification_test_state: DesktopNotificationTestState::Idle,
            desktop_notification_test_task: None,
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
            update_groups: Vec::new(),
            update_filter: UpdateFilter::All,
            expanded_update_groups: HashSet::new(),
            selected_issue: None,
            selected_issue_core: None,
            mobile_detail_open: false,
            sync_message: "Opening local cache…".to_owned(),
            workspace: None,
            users,
            workspace_name: "Jira projects".to_owned(),
            workspace_members: if initial_authenticated_account.is_some() {
                "Authenticated Jira account".to_owned()
            } else {
                "Environment bootstrap · Assigned or watched view unavailable".to_owned()
            },
            team_issues: Vec::new(),
            team_events: Vec::new(),
            team_members: Vec::new(),
            team_table: None,
            team_table_subscriptions: Vec::new(),
            team_input: None,
            team_text: String::new(),
            team_feedback: None,
            team_task: None,
            team_age_task: None,
            team_automatic_polling_paused: false,
            site_label: session.site_label,
            mode_label:
                "Live Jira sync · confirmed comments, assignee changes, and status transitions · best-effort desktop notifications"
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
            issue_edit_state: IssueEditState::Idle,
            status_popover_open: false,
            issue_edit_generation: 0,
            issue_edit_cancellation: None,
            issue_edit_task: None,
            assignee_input: None,
            assignee_subscriptions: Vec::new(),
            issue_edit_reconciliation_pending: false,
            attachment_download_state: AttachmentDownloadState::Idle,
            attachment_download_generation: 0,
            attachment_download_cancellation: None,
            attachment_download_task: None,
            settings_input: None,
            settings_subscriptions: Vec::new(),
            settings_scope_text: DEFAULT_JQL_SCOPE.to_owned(),
            settings_warning: None,
            settings_feedback: None,
            settings_task: None,
            saved_login_delete_state: SavedLoginDeleteState::Idle,
            saved_login_delete_task: None,
            desktop_notification_test_state: DesktopNotificationTestState::Idle,
            desktop_notification_test_task: None,
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
                    let (saved_scope, saved_team, preference_warning) = match load_preferences() {
                        Ok(preferences) => {
                            let (scope, scope_warning) = match normalize_issue_jql_scope(
                                preferences.issue_jql_scope.clone(),
                            ) {
                                Ok(scope) => (scope, None),
                                Err(_) => (
                                    None,
                                    Some(
                                        "Saved Jira scope was invalid; using the default scope"
                                            .to_owned(),
                                    ),
                                ),
                            };
                            let (team, team_warning) = match normalize_team_members(preferences.team_members) {
                                Ok(team) => (team, None),
                                Err(_) => (Vec::new(), Some("Saved team tracker settings were invalid; using an empty team".to_owned())),
                            };
                            let warning = scope_warning.or(team_warning);
                            (scope, team, warning)
                        }
                        Err(_) => (
                            None,
                            Vec::new(),
                            Some(
                                "Saved Jira settings could not be read; using default Jira and team settings"
                                    .to_owned(),
                            ),
                        ),
                    };
                    let authenticated_account = authenticated_user.account_id.clone();
                    let jira_read: Arc<dyn JiraReadPort> = jira.clone();
                    let jira_comment_write: Arc<dyn JiraCommentWritePort> = jira.clone();
                    let jira_issue_edit: Arc<dyn JiraIssueEditPort> = jira.clone();
                    match LiveWorkspace::initialize_with_writers_and_scope(
                        site_id,
                        Some(authenticated_account),
                        jira_read,
                        jira_comment_write,
                        jira_issue_edit,
                        cache,
                        saved_scope,
                    )
                    .await
                    {
                        Ok(workspace) => {
                            let workspace = Arc::new(workspace);
                            let team_accounts = saved_team
                                .iter()
                                .filter_map(|member| AccountId::new(member.account_id.clone()).ok())
                                .collect::<Vec<_>>();
                            match workspace.configure_team_members(team_accounts).await {
                                Err(error) => Err(safe_sync_error(&error).to_owned()),
                                Ok(()) => match workspace.load_cached_for_authenticated_account().await {
                                    Err(error) => Err(safe_sync_error(&error).to_owned()),
                                    Ok(cached) => match workspace.load_cached_team().await {
                                        Err(error) => Err(safe_sync_error(&error).to_owned()),
                                        Ok(team_cached) => Ok((workspace, cached, team_cached, authenticated_user, saved_team, preference_warning)),
                                    },
                                },
                            }
                        }
                        Err(error) => Err(safe_sync_error(&error).to_owned()),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok((workspace, cached, team_cached, authenticated_user, saved_team, preference_warning)) => {
                        let issue_count = cached.issues.len();
                        let update_count = cached.events.len();
                        this.users = vec![authenticated_user.clone()];
                        this.users.extend(saved_team.iter().filter_map(|member| {
                            if !persisted_team_member_has_display_name(member) {
                                return None;
                            }
                            AccountId::new(member.account_id.clone()).ok().map(|account_id| {
                                User::new(workspace.site_id().clone(), account_id, member.display_name.clone(), None, true)
                            })
                        }));
                        this.team_members = saved_team;
                        this.team_text = this.team_members.iter().map(|member| member.identifier.clone()).collect::<Vec<_>>().join("\n");
                        if let Some(input) = this.team_input.clone() {
                            input.update(cx, |input, cx| {
                                input.set_value(&this.team_text, window, cx)
                            });
                        }
                        this.authenticated_account = Some(authenticated_user.account_id.clone());
                        this.settings_scope_text = workspace
                            .jql_scope()
                            .unwrap_or_else(|| DEFAULT_JQL_SCOPE.to_owned());
                        if let Some(input) = this.settings_input.clone() {
                            input.update(cx, |input, cx| {
                                input.set_value(&this.settings_scope_text, window, cx)
                            });
                        }
                        this.settings_warning = preference_warning;
                        this.workspace_members = "Authenticated Jira account".to_owned();
                        this.workspace = Some(workspace);
                        this.apply_cached(cached, cx);
                        this.apply_team_cached(team_cached, cx);
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
                let team_polling_allowed =
                    match this.update(cx, |this, _| !this.team_automatic_polling_paused) {
                        Ok(allowed) => allowed,
                        Err(_) => break,
                    };
                let team_result = if result.is_ok() && team_polling_allowed {
                    Some(
                        workspace
                            .refresh_team_automatically(&CancellationToken::new())
                            .await,
                    )
                } else {
                    None
                };
                let next_delay = match this.update(cx, |this, cx| {
                    this.operation_in_progress = false;
                    match result {
                        Ok(result) => {
                            consecutive_failures = 0;
                            this.sync_message = refresh_complete_message(&result);
                            this.apply_cached(result.cached, cx);
                            if let Some(team_result) = team_result {
                                match team_result {
                                    Ok(team_result) => {
                                        this.apply_team_cached(team_result.cached, cx);
                                        this.team_feedback = Some(team_refresh_feedback(
                                            "Team tracker refreshed",
                                            &this.team_issues,
                                        ));
                                    }
                                    Err(error) => {
                                        this.team_feedback =
                                            Some(safe_sync_error(&error).to_owned())
                                    }
                                }
                            }
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
        dashboard.update_groups.clear();
        dashboard.selected_issue = None;
        dashboard.invalidate_detail_selection();
        dashboard.users.clear();
        dashboard.workspace_name = "Jira projects".to_owned();
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
        self.workspace_name = project_label(&self.domain_issues);
        self.rebuild_issue_views(refresh_detail, cx);
    }

    fn rebuild_issue_views(&mut self, refresh_detail: bool, cx: &mut Context<Self>) {
        self.issues = issue_views_for_filter(
            &self.domain_issues,
            &self.users,
            self.status_filter,
            &self.search_query,
        );
        let retained_selection =
            selection_after_issue_view_rebuild(self.selected_issue.clone(), &self.issues);
        if self.selected_issue.is_none() {
            if let Some(issue_id) = retained_selection {
                self.select_issue(issue_id, cx, true);
            }
        } else if refresh_detail
            && !should_defer_detail_refresh(
                self.selected_issue.as_ref(),
                &self.domain_issues,
                &self.detail_state,
                refresh_detail,
            )
        {
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
        self.status_popover_open = false;
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
        self.selected_issue_core = None;
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

    fn invalidate_issue_edit_selection(&mut self) {
        self.issue_edit_generation = self.issue_edit_generation.wrapping_add(1);
        self.assignee_input = None;
        self.assignee_subscriptions.clear();
        if !matches!(self.issue_edit_state, IssueEditState::Submitting { .. }) {
            if let Some(cancellation) = self.issue_edit_cancellation.take() {
                cancellation.cancel();
            }
            self.issue_edit_task.take();
        }
        // A dispatched write is never cancelled or converted into a retryable
        // state; its completion is simply ignored after this generation bump.
        self.issue_edit_state = IssueEditState::Idle;
        self.status_popover_open = false;
    }

    fn select_issue(&mut self, issue_id: IssueId, cx: &mut Context<Self>, force: bool) {
        if matches!(self.issue_edit_state, IssueEditState::Submitting { .. })
            && self.selected_issue.as_ref() != Some(&issue_id)
        {
            self.sync_message =
                "Finish the confirmed Jira change before changing issues".to_owned();
            cx.notify();
            return;
        }
        let selection_changed = self.selected_issue.as_ref() != Some(&issue_id);
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
        if !matches!(self.issue_edit_state, IssueEditState::Submitting { .. }) {
            self.invalidate_issue_edit_selection();
        }
        self.detail_task.take();
        self.detail_generation = self.detail_generation.wrapping_add(1);
        if selection_changed {
            self.selected_issue_core = None;
        }
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
                    this.selected_issue_core = Some(issue.clone());
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

    fn open_update_issue(&mut self, issue_id: IssueId, mobile: bool, cx: &mut Context<Self>) {
        self.clear_remote_lookup();
        self.select_issue(issue_id, cx, false);
        self.section = Section::Issues;
        self.mobile_detail_open = mobile;
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

    fn ensure_assignee_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assignee_input.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter assignees"));
        self.assignee_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |_this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));
        self.assignee_input = Some(input);
    }

    fn begin_assignee_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(issue) = selected_issue_from_sources(
            self.selected_issue.as_ref(),
            &self.domain_issues,
            self.selected_issue_core.as_ref(),
        )
        .cloned() else {
            return;
        };
        self.ensure_assignee_input(window, cx);
        if let Some(input) = &self.assignee_input {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.start_assignee_search_for_issue(issue.id, issue.key.to_string(), String::new(), cx);
    }

    fn start_assignee_search(&mut self, query: String, cx: &mut Context<Self>) {
        let (issue_id, issue_key) = match &self.issue_edit_state {
            IssueEditState::AssigneeChooser {
                issue_id,
                issue_key,
                ..
            } => (issue_id.clone(), issue_key.clone()),
            IssueEditState::LoadingAssignees { issue_id, .. } => {
                let key = self
                    .selected_issue_view()
                    .filter(|issue| issue.id == *issue_id)
                    .map(|issue| issue.key)
                    .unwrap_or_default();
                (issue_id.clone(), key)
            }
            _ => return,
        };
        self.start_assignee_search_for_issue(issue_id, issue_key, query, cx);
    }

    fn start_assignee_search_for_issue(
        &mut self,
        issue_id: IssueId,
        issue_key: String,
        query: String,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        if let Some(cancellation) = self.issue_edit_cancellation.take() {
            cancellation.cancel();
        }
        self.issue_edit_task.take();
        self.issue_edit_generation = self.issue_edit_generation.wrapping_add(1);
        let generation = self.issue_edit_generation;
        let cancellation = CancellationToken::new();
        self.issue_edit_cancellation = Some(cancellation.clone());
        self.issue_edit_state = IssueEditState::LoadingAssignees {
            issue_id: issue_id.clone(),
            query: query.clone(),
        };
        let task = cx.spawn(async move |this, cx| {
            let result = workspace
                .search_assignable_users(
                    IssueLocator::Id(issue_id.clone()),
                    query.clone(),
                    100,
                    &cancellation,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if !issue_edit_target_is_current(
                    this.selected_issue.as_ref(),
                    &issue_id,
                    this.issue_edit_generation,
                    generation,
                ) {
                    return;
                }
                this.issue_edit_cancellation = None;
                this.issue_edit_task = None;
                match result {
                    Ok(users) => {
                        this.issue_edit_state = IssueEditState::AssigneeChooser {
                            issue_id: issue_id.clone(),
                            issue_key: issue_key.clone(),
                            query: query.clone(),
                            users,
                        };
                    }
                    Err(error) => {
                        let (message, unknown_outcome) = issue_edit_error_message(&error, "lookup");
                        this.issue_edit_state = IssueEditState::Error {
                            issue_id: issue_id.clone(),
                            message: message.to_owned(),
                            unknown_outcome,
                            operation: IssueEditOperation::Assignee,
                        };
                    }
                }
                cx.notify();
            });
        });
        self.issue_edit_task = Some(task);
        cx.notify();
    }

    fn choose_assignee(
        &mut self,
        account_id: Option<AccountId>,
        display_name: String,
        cx: &mut Context<Self>,
    ) {
        let IssueEditState::AssigneeChooser {
            issue_id,
            issue_key,
            ..
        } = &self.issue_edit_state
        else {
            return;
        };
        if let Some(cancellation) = self.issue_edit_cancellation.take() {
            cancellation.cancel();
        }
        self.issue_edit_task.take();
        self.issue_edit_generation = self.issue_edit_generation.wrapping_add(1);
        self.issue_edit_state = IssueEditState::ConfirmingAssignee {
            issue_id: issue_id.clone(),
            issue_key: issue_key.clone(),
            account_id,
            display_name,
        };
        cx.notify();
    }

    fn begin_transition_chooser(&mut self, cx: &mut Context<Self>) {
        let Some(issue) = selected_issue_from_sources(
            self.selected_issue.as_ref(),
            &self.domain_issues,
            self.selected_issue_core.as_ref(),
        )
        .cloned() else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        self.status_popover_open = true;
        if let Some(cancellation) = self.issue_edit_cancellation.take() {
            cancellation.cancel();
        }
        self.issue_edit_task.take();
        self.issue_edit_generation = self.issue_edit_generation.wrapping_add(1);
        let generation = self.issue_edit_generation;
        let cancellation = CancellationToken::new();
        self.issue_edit_cancellation = Some(cancellation.clone());
        self.issue_edit_state = IssueEditState::LoadingTransitions {
            issue_id: issue.id.clone(),
        };
        let issue_id = issue.id.clone();
        let issue_key = issue.key.to_string();
        let task = cx.spawn(async move |this, cx| {
            let result = workspace
                .available_transitions(IssueLocator::Id(issue_id.clone()), &cancellation)
                .await;
            let _ = this.update(cx, |this, cx| {
                if !issue_edit_target_is_current(
                    this.selected_issue.as_ref(),
                    &issue_id,
                    this.issue_edit_generation,
                    generation,
                ) {
                    return;
                }
                this.issue_edit_cancellation = None;
                this.issue_edit_task = None;
                match result {
                    Ok(transitions) => {
                        this.issue_edit_state = IssueEditState::TransitionChooser {
                            issue_id: issue_id.clone(),
                            issue_key: issue_key.clone(),
                            transitions,
                        };
                    }
                    Err(error) => {
                        let (message, unknown_outcome) = issue_edit_error_message(&error, "lookup");
                        this.status_popover_open = false;
                        this.issue_edit_state = IssueEditState::Error {
                            issue_id: issue_id.clone(),
                            message: message.to_owned(),
                            unknown_outcome,
                            operation: IssueEditOperation::Transition,
                        };
                    }
                }
                cx.notify();
            });
        });
        self.issue_edit_task = Some(task);
        cx.notify();
    }

    fn choose_transition(&mut self, transition: IssueTransition, cx: &mut Context<Self>) {
        let IssueEditState::TransitionChooser {
            issue_id,
            issue_key,
            ..
        } = &self.issue_edit_state
        else {
            return;
        };
        self.issue_edit_generation = self.issue_edit_generation.wrapping_add(1);
        self.issue_edit_state = IssueEditState::ConfirmingTransition {
            issue_id: issue_id.clone(),
            issue_key: issue_key.clone(),
            transition_id: transition.id,
            transition_name: transition.name,
            target_status: transition.to.name,
        };
        self.status_popover_open = false;
        cx.notify();
    }

    fn cancel_issue_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(cancellation) = self.issue_edit_cancellation.take() {
            cancellation.cancel();
        }
        self.issue_edit_task.take();
        self.issue_edit_generation = self.issue_edit_generation.wrapping_add(1);
        self.issue_edit_state = IssueEditState::Idle;
        self.status_popover_open = false;
        cx.notify();
    }

    fn submit_assignee(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let IssueEditState::ConfirmingAssignee {
            issue_id,
            account_id,
            display_name,
            ..
        } = &self.issue_edit_state
        else {
            return;
        };
        let issue_id = issue_id.clone();
        let dispatch_issue_id = issue_id.clone();
        let account_id = account_id.clone();
        let display_name = display_name.clone();
        self.submit_issue_write(
            window,
            cx,
            issue_id,
            IssueEditOperation::Assignee,
            format!("assignee {display_name}"),
            move |workspace, cancellation| {
                let account_id = account_id.clone();
                let issue_id = dispatch_issue_id.clone();
                async move {
                    workspace
                        .assign_issue(
                            IssueLocator::Id(issue_id.clone()),
                            account_id,
                            &cancellation,
                        )
                        .await
                }
            },
        );
    }

    fn submit_transition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let IssueEditState::ConfirmingTransition {
            issue_id,
            transition_id,
            target_status,
            ..
        } = &self.issue_edit_state
        else {
            return;
        };
        let issue_id = issue_id.clone();
        let dispatch_issue_id = issue_id.clone();
        let transition_id = transition_id.clone();
        let target_status = target_status.clone();
        self.submit_issue_write(
            window,
            cx,
            issue_id,
            IssueEditOperation::Transition,
            format!("status {target_status}"),
            move |workspace, cancellation| {
                let transition_id = transition_id.clone();
                let issue_id = dispatch_issue_id.clone();
                async move {
                    workspace
                        .transition_issue(
                            IssueLocator::Id(issue_id.clone()),
                            transition_id,
                            &cancellation,
                        )
                        .await
                }
            },
        );
    }

    fn submit_issue_write<F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        issue_id: IssueId,
        operation: IssueEditOperation,
        target: String,
        dispatch: F,
    ) where
        F: FnOnce(Arc<LiveWorkspace>, CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), ApplicationError>> + Send + 'static,
    {
        if self.operation_in_progress {
            self.sync_message =
                "Finish the current Jira operation before applying another change".to_owned();
            cx.notify();
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.issue_edit_state = IssueEditState::Error {
                issue_id,
                message: "Change unavailable · live Jira workspace is not ready".to_owned(),
                unknown_outcome: false,
                operation,
            };
            cx.notify();
            return;
        };
        if matches!(self.issue_edit_state, IssueEditState::Submitting { .. }) {
            return;
        }
        let cancellation = CancellationToken::new();
        self.operation_in_progress = true;
        self.issue_edit_state = IssueEditState::Submitting {
            issue_id: issue_id.clone(),
            target: target.clone(),
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = dispatch(workspace, cancellation.clone()).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        this.operation_in_progress = false;
                        this.issue_edit_state = IssueEditState::Idle;
                        window.push_notification(
                            Notification::success(format!("Jira change applied · {target}"))
                                .id::<IssueEditNotification>(),
                            cx,
                        );
                        this.issue_edit_reconciliation_pending = true;
                        this.sync_message = "Change applied · reconciling with Jira…".to_owned();
                        this.begin_refresh(window, cx);
                    }
                    Err(error) => {
                        this.operation_in_progress = false;
                        let (message, unknown_outcome) = issue_edit_error_message(&error, "write");
                        this.issue_edit_state = IssueEditState::Error {
                            issue_id: issue_id.clone(),
                            message: message.to_owned(),
                            unknown_outcome,
                            operation,
                        };
                        window.push_notification(
                            Notification::error(message).id::<IssueEditNotification>(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        });
        // The dispatched write owns its cancellation token and runs detached.
        // Selection changes must not abort or turn this uncertain attempt into
        // a retryable operation.
        task.detach();
        cx.notify();
    }

    fn apply_cached(&mut self, cached: CachedWorkspace, cx: &mut Context<Self>) {
        let CachedWorkspace { issues, events } = cached;
        let update_groups = update_groups_for_events(&events, &issues, &self.users);
        self.apply_live_issues(issues, true, cx);
        self.update_groups = update_groups;
    }

    fn apply_team_cached(&mut self, cached: CachedWorkspace, cx: &mut Context<Self>) {
        self.team_issues = cached.issues;
        self.team_events = cached.events;
        self.refresh_team_table(cx);
    }

    fn refresh_team_table(&mut self, cx: &mut Context<Self>) {
        let Some(table) = self.team_table.clone() else {
            return;
        };
        let issues = self.team_issues.clone();
        let events = self.team_events.clone();
        let users = self.users.clone();
        table.update(cx, |table, cx| {
            table.replace_team_ticket_rows(
                &issues,
                &events,
                &users,
                jira_domain::Timestamp::now_utc(),
                cx,
            );
        });
    }

    fn ensure_team_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.team_table.is_some() {
            return;
        }
        let delegate = TeamTicketTableDelegate::new(
            &self.team_issues,
            &self.team_events,
            &self.users,
            jira_domain::Timestamp::now_utc(),
        );
        let table = cx.new(|cx| TableState::new(delegate, window, cx));
        self.team_table_subscriptions.push(cx.subscribe_in(
            &table,
            window,
            |this, table, event: &TableEvent, window, cx| {
                if let TableEvent::SelectRow(_) = event
                    && let Some(issue_id) = table.read(cx).selected_team_ticket_issue_id()
                {
                    this.select_issue(issue_id, cx, true);
                    this.section = Section::Team;
                    this.mobile_detail_open =
                        layout_for_width(f32::from(window.viewport_size().width)).is_mobile();
                    cx.notify();
                }
            },
        ));
        self.team_table = Some(table);
        let table = self.team_table.clone().expect("team table created");
        self.team_age_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(60))
                    .await;
                let result = this.update(cx, |this, cx| {
                    if this.team_table.is_none() {
                        return false;
                    }
                    let issues = this.team_issues.clone();
                    let events = this.team_events.clone();
                    let users = this.users.clone();
                    table.update(cx, |table, cx| {
                        table.replace_team_ticket_rows(
                            &issues,
                            &events,
                            &users,
                            jira_domain::Timestamp::now_utc(),
                            cx,
                        );
                    });
                    true
                });
                if !matches!(result, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn begin_team_refresh(&mut self, cx: &mut Context<Self>) {
        if self.team_task.is_some() || self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.team_feedback =
                Some("Team tracker is unavailable until Jira is connected".to_owned());
            cx.notify();
            return;
        };
        self.operation_in_progress = true;
        self.team_feedback = Some("Refreshing team tracker…".to_owned());
        let task = cx.spawn(async move |this, cx| {
            let result = workspace.refresh_team(&CancellationToken::new()).await;
            let _ = this.update(cx, |this, cx| {
                this.team_task = None;
                this.operation_in_progress = false;
                match result {
                    Ok(result) => {
                        this.apply_team_cached(result.cached, cx);
                        this.team_feedback = Some(team_refresh_feedback(
                            "Team tracker refreshed",
                            &this.team_issues,
                        ));
                    }
                    Err(error) => this.team_feedback = Some(safe_sync_error(&error).to_owned()),
                }
                cx.notify();
            });
        });
        self.team_task = Some(task);
        cx.notify();
    }

    fn begin_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation_in_progress
            || matches!(self.issue_edit_state, IssueEditState::Submitting { .. })
        {
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
            let team_result = workspace.refresh_team(&CancellationToken::new()).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(outcome) => {
                        window.push_notification(
                            Notification::success(refresh_notification_message(&outcome))
                                .id::<RefreshNotification>(),
                            cx,
                        );
                        this.sync_message = refresh_complete_message(&outcome);
                        this.apply_cached(outcome.cached, cx);
                        match team_result {
                            Ok(team) => {
                                this.apply_team_cached(team.cached, cx);
                                this.team_feedback = Some(team_refresh_feedback(
                                    "Team tracker refreshed",
                                    &this.team_issues,
                                ));
                            }
                            Err(error) => {
                                this.team_feedback = Some(safe_sync_error(&error).to_owned())
                            }
                        }
                        this.issue_edit_reconciliation_pending = false;
                        if matches!(
                            this.issue_edit_state,
                            IssueEditState::Error {
                                unknown_outcome: true,
                                ..
                            }
                        ) {
                            this.issue_edit_state = IssueEditState::Idle;
                        }
                        this.start_automatic_polling(cx);
                    }
                    Err(error) => {
                        let message = safe_sync_error(&error);
                        window.push_notification(
                            Notification::error(message).id::<RefreshNotification>(),
                            cx,
                        );
                        this.sync_message = if this.issue_edit_reconciliation_pending {
                            "Change applied · refresh still needed".to_owned()
                        } else {
                            message.to_owned()
                        };
                        match team_result {
                            Ok(team) => {
                                this.apply_team_cached(team.cached, cx);
                                this.team_feedback = Some(team_refresh_feedback(
                                    "Team tracker refreshed",
                                    &this.team_issues,
                                ));
                            }
                            Err(error) => {
                                this.team_feedback = Some(safe_sync_error(&error).to_owned())
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            for group in &mut self.update_groups {
                group.unread = false;
                group.unread_count = 0;
                for event in &mut group.events {
                    event.unread = false;
                }
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
        self.update_groups
            .iter()
            .map(|group| group.unread_count)
            .sum()
    }

    fn set_update_filter(&mut self, filter: UpdateFilter, cx: &mut Context<Self>) {
        if self.update_filter != filter {
            self.update_filter = filter;
            cx.notify();
        }
    }

    fn toggle_update_group_expanded(&mut self, issue_id: IssueId, cx: &mut Context<Self>) {
        if !self.expanded_update_groups.remove(&issue_id) {
            self.expanded_update_groups.insert(issue_id);
        }
        cx.notify();
    }

    fn mark_group_read(&mut self, issue_id: IssueId, cx: &mut Context<Self>) {
        let Some(group) = self
            .update_groups
            .iter()
            .find(|group| group.issue_id == issue_id)
            .cloned()
        else {
            return;
        };
        if !group.unread {
            return;
        }
        let event_ids = update_group_event_ids(&group);
        let Some(workspace) = self.workspace.clone() else {
            if let Some(group) = self
                .update_groups
                .iter_mut()
                .find(|group| group.issue_id == issue_id)
            {
                group.unread = false;
                group.unread_count = 0;
                for event in &mut group.events {
                    event.unread = false;
                }
            }
            cx.notify();
            return;
        };
        if self.operation_in_progress {
            return;
        }
        self.operation_in_progress = true;
        self.sync_message = "Marking ticket updates read…".to_owned();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = workspace.mark_read(&event_ids, true).await;
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(result) => {
                        this.apply_cached(result.cached, cx);
                        this.sync_message =
                            format!("Marked {} ticket updates read", result.changed);
                    }
                    Err(error) => this.sync_message = safe_sync_error(&error).to_owned(),
                }
                cx.notify();
            });
        })
        .detach();
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
                                .child("Synced workspace · confirmed comments and issue edits"),
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
                        Some(self.issues.len()),
                        self.section == Section::Issues,
                        Section::Issues,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Local updates",
                        Some(self.unread_count()),
                        self.section == Section::Updates,
                        Section::Updates,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Team tracker",
                        Some(self.team_issues.len()),
                        self.section == Section::Team,
                        Section::Team,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Settings",
                        None,
                        self.section == Section::Settings,
                        Section::Settings,
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
                                .child("JIRA ACCOUNT VIEW"),
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
        count: Option<usize>,
        selected: bool,
        section: Section,
        rail: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon = match section {
            Section::Issues => IconName::LayoutDashboard,
            Section::Updates => IconName::Inbox,
            Section::Team => IconName::LayoutDashboard,
            Section::Settings => IconName::Settings,
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
            .when(!rail && count.is_some(), |this| {
                this.child(
                    div()
                        .min_w(px(26.))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(cx.theme().muted)
                        .text_center()
                        .text_xs()
                        .child(count.unwrap_or_default().to_string()),
                )
            })
    }

    fn render_header(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        v_flex()
            .id("header")
            .debug_selector(|| "header".to_owned())
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
                            Section::Issues => "Jira issues",
                            Section::Updates => "Local updates",
                            Section::Team => "Team tracker",
                            Section::Settings => "Settings",
                        },
                    ))
                    .when(self.section != Section::Settings, |this| {
                        this.child(
                            Button::new("refresh")
                                .compact()
                                .primary()
                                .label(if self.operation_in_progress {
                                    "Refreshing…"
                                } else {
                                    "Refresh"
                                })
                                .loading(self.operation_in_progress)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.begin_refresh(window, cx)
                                })),
                        )
                    }),
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
                        "{} matching Jira issues · Assigned or watched",
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
                        "{} matching Jira issues · Assigned or watched",
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

    fn render_team(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let configured = !self.team_members.is_empty();
        let show_mobile_detail = mobile && self.mobile_detail_open;
        let table = self.team_table.clone();
        let (_, displayed_team_count) = team_issue_counts(&self.team_issues);
        v_flex()
            .size_full()
            .min_w_0()
            .p(px(layout.list_padding()))
            .gap_3()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_base().font_semibold().child("In-progress team tickets"))
                    .child(div().min_w_0().whitespace_normal().text_sm().text_color(cx.theme().muted_foreground).child(
                        "All visible Jira issues whose status category is In Progress and whose assignee is one of the configured accounts. Jira permissions still apply.",
                    ))
                    .child(div().min_w_0().whitespace_normal().text_xs().text_color(cx.theme().muted_foreground).child(
                        if configured { team_summary(displayed_team_count, self.team_members.len()) } else { "No team members configured · no Jira request will be made".to_owned() },
                    ))
                    .when_some(self.team_feedback.clone(), |this, feedback| {
                        this.child(
                            div()
                                .min_w_0()
                                .whitespace_normal()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(feedback),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h_full()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(mobile, |this| this.flex_col())
                    .child(if show_mobile_detail {
                        v_flex()
                            .h_full()
                            .flex_1()
                            .min_h_0()
                            .p_3()
                            .gap_2()
                            .child(
                                Button::new("team-back-to-table")
                                    .ghost()
                                    .label("Back to team tickets")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mobile_detail_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(self.render_detail_state_for(&self.detail_state, layout, cx))
                            .into_any_element()
                    } else {
                        v_flex()
                            .id("team-table")
                            .h_full()
                            .debug_selector(|| "team-table".to_owned())
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(if !configured {
                                div()
                                    .p_4()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Configure at least one Jira account ID or Atlassian email in Settings to load team tickets.")
                                    .into_any_element()
                            } else if let Some(table) = table {
                                DataTable::new(&table).into_any_element()
                            } else {
                                div().p_4().text_sm().text_color(cx.theme().muted_foreground).child(if configured { "Loading team tracker…" } else { "Configure at least one Jira account ID or Atlassian email in Settings to load team tickets." }).into_any_element()
                            })
                            .into_any_element()
                    })
                    .when(!mobile, |this| {
                        this.child(
                            v_flex()
                                .id("team-detail")
                                .h_full()
                                .min_h_0()
                                .min_w_0()
                                .debug_selector(|| "team-detail".to_owned())
                                .w(px(320.))
                                .ml_3()
                                .p_4()
                                .gap_2()
                                .overflow_y_scrollbar()
                                .overflow_x_hidden()
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(div().text_base().font_semibold().child("Issue detail"))
                                .child(self.render_detail_state_for(&self.detail_state, layout, cx)),
                        )
                    }),
            )
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
        selected_issue_view_from_sources(
            self.selected_issue.as_ref(),
            &self.issues,
            &self.domain_issues,
            self.selected_issue_core.as_ref(),
            &self.users,
        )
    }

    fn comment_target_issue(&self) -> Option<&Issue> {
        match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(issue),
            RemoteLookupState::Idle
            | RemoteLookupState::Loading { .. }
            | RemoteLookupState::Error { .. } => self.selected_issue.as_ref().and_then(|id| {
                selected_issue_from_sources(
                    Some(id),
                    &self.domain_issues,
                    self.selected_issue_core.as_ref(),
                )
            }),
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
                            .py_1()
                            .rounded_full()
                            .bg(cx.theme().secondary)
                            .border_1()
                            .border_color(cx.theme().link.opacity(0.4))
                            .text_color(cx.theme().secondary_foreground)
                            .text_xs()
                            .font_semibold()
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
                            .child(self.status_control(issue.as_ref(), status, cx))
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
            .child(self.render_issue_edit_controls(issue.as_ref(), layout, cx))
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

    fn status_control(
        &self,
        issue: Option<&IssueViewModel>,
        status: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_selected_issue = issue
            .map(|issue| self.selected_issue.as_ref() == Some(&issue.id))
            .unwrap_or(false);
        let is_remote_lookup = matches!(self.remote_lookup, RemoteLookupState::Loaded { .. });
        let editable = status_control_is_editable(
            self.workspace.is_some(),
            is_selected_issue,
            is_remote_lookup,
            self.operation_in_progress,
            &self.issue_edit_state,
        );
        let popover_open = self.status_popover_open
            && matches!(
                self.issue_edit_state,
                IssueEditState::LoadingTransitions { .. }
                    | IssueEditState::TransitionChooser { .. }
            );
        let button = Button::new("issue-status-control")
            .secondary()
            .label(status)
            .dropdown_caret(true)
            .tooltip(if editable {
                "Change issue status"
            } else {
                "Issue status · editing unavailable in this view"
            })
            .disabled(!editable);
        if !editable {
            return button.into_any_element();
        }
        Popover::new("issue-status-popover")
            .anchor(Anchor::TopLeft)
            .open(popover_open)
            .trigger(button)
            .on_open_change(cx.listener(|this, open, _, cx| {
                this.status_popover_open = *open;
                if *open && matches!(this.issue_edit_state, IssueEditState::Idle) {
                    this.begin_transition_chooser(cx);
                } else if !*open
                    && matches!(
                        this.issue_edit_state,
                        IssueEditState::LoadingTransitions { .. }
                            | IssueEditState::TransitionChooser { .. }
                    )
                {
                    this.cancel_issue_edit(cx);
                } else {
                    cx.notify();
                }
            }))
            .w(px(320.))
            .max_h(px(360.))
            .p_3()
            .gap_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_lg()
            .child(self.render_status_transition_popover(cx))
            .into_any_element()
    }

    fn render_status_transition_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.issue_edit_state.clone() {
            IssueEditState::LoadingTransitions { .. } => v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Change issue status"))
                .child(
                    h_flex().gap_2().child(Spinner::new()).child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Loading available transitions…"),
                    ),
                )
                .into_any_element(),
            IssueEditState::TransitionChooser { transitions, .. } => {
                let no_transitions = transitions.is_empty();
                v_flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Change issue status"))
                    .when(!no_transitions, |this| {
                        this.child(
                            v_flex()
                                .h(status_transition_list_height(transitions.len()))
                                .min_h_0()
                                .max_h(px(STATUS_TRANSITION_LIST_MAX_HEIGHT))
                                .gap_1()
                                .overflow_y_scrollbar()
                                .children(transitions.into_iter().map(|transition| {
                                    let transition_id = transition.id.clone();
                                    let label = transition_option_label(&transition).to_owned();
                                    Button::new(format!("status-transition-{}", transition.id))
                                        .w_full()
                                        .compact()
                                        .ghost()
                                        .justify_start()
                                        .debug_selector(move || {
                                            format!("status-transition-{transition_id}")
                                        })
                                        .label(label.clone())
                                        .tooltip(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.choose_transition(transition.clone(), cx)
                                        }))
                                })),
                        )
                    })
                    .when(no_transitions, |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No status transitions are currently available."),
                        )
                    })
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn render_issue_edit_controls(
        &self,
        issue: Option<&IssueViewModel>,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(issue) = issue else {
            return div().into_any_element();
        };
        if self.workspace.is_none() || self.selected_issue.as_ref() != Some(&issue.id) {
            return div().into_any_element();
        }
        let busy = self.operation_in_progress;
        let state = self.issue_edit_state.clone();
        let controls = match state {
            IssueEditState::Idle => h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("change-assignee")
                        .secondary()
                        .outline()
                        .compact()
                        .label("Change assignee")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.begin_assignee_chooser(window, cx)
                        })),
                )
                .into_any_element(),
            IssueEditState::LoadingAssignees { .. } => h_flex()
                .gap_2()
                .child(Spinner::new())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading assignable users…"),
                )
                .child(
                    Button::new("cancel-assignee-load")
                        .compact()
                        .label("Cancel")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                )
                .into_any_element(),
            IssueEditState::AssigneeChooser { users, .. } => {
                let no_users = users.is_empty();
                v_flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Choose assignee"))
                    .when_some(self.assignee_input.clone(), |this, input| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Input::new(&input)
                                        .cleanable(true)
                                        .aria_label("Filter assignees")
                                        .flex_1(),
                                )
                                .child(
                                    Button::new("search-assignees")
                                        .compact()
                                        .label("Search")
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let query = this
                                                .assignee_input
                                                .as_ref()
                                                .map(|input| input.read(cx).value().to_string())
                                                .unwrap_or_default();
                                            this.start_assignee_search(query, cx);
                                        })),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .child(
                                Button::new("assignee-unassigned")
                                    .compact()
                                    .label("Unassigned")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.choose_assignee(None, "Unassigned".to_owned(), cx)
                                    })),
                            )
                            .children(users.into_iter().enumerate().map(|(index, user)| {
                                let name = user.display_name.clone();
                                let account_id = user.account_id.clone();
                                Button::new(format!("assignee-{index}"))
                                    .compact()
                                    .label(name.clone())
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_assignee(
                                            Some(account_id.clone()),
                                            name.clone(),
                                            cx,
                                        )
                                    }))
                            })),
                    )
                    .when(no_users, |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No assignable Jira users match this search."),
                        )
                    })
                    .child(
                        Button::new("cancel-assignee")
                            .compact()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                    )
                    .into_any_element()
            }
            IssueEditState::LoadingTransitions { .. }
            | IssueEditState::TransitionChooser { .. } => div().into_any_element(),
            IssueEditState::ConfirmingAssignee {
                issue_key,
                display_name,
                ..
            } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(format!("Assign {issue_key} to {display_name}?")),
                )
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("confirm-assignee")
                                .primary()
                                .label("Confirm change")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_assignee(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cancel-assignee-confirmation")
                                .compact()
                                .label("Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                        ),
                )
                .into_any_element(),
            IssueEditState::ConfirmingTransition {
                issue_key,
                transition_name,
                target_status,
                ..
            } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(format!(
                            "Move {issue_key} via {transition_name} to {target_status}?"
                        )),
                )
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("confirm-transition")
                                .primary()
                                .label("Confirm change")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_transition(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cancel-transition-confirmation")
                                .compact()
                                .label("Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                        ),
                )
                .into_any_element(),
            IssueEditState::Submitting { target, .. } => h_flex()
                .gap_2()
                .child(Spinner::new())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Applying {target}…")),
                )
                .into_any_element(),
            IssueEditState::Error {
                message,
                unknown_outcome,
                operation,
                ..
            } => v_flex()
                .gap_2()
                .child(div().text_sm().text_color(cx.theme().danger).child(message))
                .child(
                    h_flex()
                        .gap_2()
                        .when(!unknown_outcome, |this| {
                            this.child(
                                Button::new("retry-issue-edit")
                                    .compact()
                                    .label("Choose again")
                                    .disabled(busy)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| match operation {
                                            IssueEditOperation::Assignee => {
                                                this.begin_assignee_chooser(window, cx)
                                            }
                                            IssueEditOperation::Transition => {
                                                this.begin_transition_chooser(cx)
                                            }
                                        },
                                    )),
                            )
                        })
                        .when(unknown_outcome, |this| {
                            this.child(
                                Button::new("refresh-after-issue-edit")
                                    .compact()
                                    .label("Refresh Jira")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.begin_refresh(window, cx)
                                    })),
                            )
                        }),
                )
                .into_any_element(),
        };
        v_flex()
            .gap_2()
            .child(div().text_sm().font_semibold().child("Jira issue actions"))
            .child(controls)
            .into_any_element()
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
        let unread = self.unread_count();
        let visible_groups = filtered_update_group_indices(&self.update_groups, self.update_filter);
        let no_visible_groups = visible_groups.is_empty();
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .h(px(if mobile { 104. } else { 86. }))
                    .px(px(if mobile { 12. } else { 20. }))
                    .py(px(10.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .gap_1()
                    .child(
                        h_flex()
                            .min_w_0()
                            .justify_between()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_2()
                                    .child(div().text_sm().font_semibold().child("Change ledger"))
                                    .child(
                                        div()
                                            .px_1()
                                            .rounded_full()
                                            .bg(cx.theme().secondary)
                                            .text_xs()
                                            .text_color(cx.theme().secondary_foreground)
                                            .child(format!("{unread} unread")),
                                    ),
                            )
                            .child(
                                Button::new("mark-all-read")
                                    .compact()
                                    .ghost()
                                    .disabled(unread == 0 || self.operation_in_progress)
                                    .label("Mark all read")
                                    .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                            ),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_1()
                            .child(
                                Button::new("updates-filter-unread")
                                    .compact()
                                    .when(self.update_filter == UpdateFilter::Unread, |this| {
                                        this.primary()
                                    })
                                    .label(format!("Unread · {unread}"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_update_filter(UpdateFilter::Unread, cx)
                                    })),
                            )
                            .child(
                                Button::new("updates-filter-all")
                                    .compact()
                                    .when(self.update_filter == UpdateFilter::All, |this| {
                                        this.primary()
                                    })
                                    .label(format!("All · {}", self.update_groups.len()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_update_filter(UpdateFilter::All, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Operational change ledger · local Jira activity"),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .id("update-list")
                    .flex_1()
                    .overflow_y_scrollbar()
                    .min_h_0()
                    .w_full()
                    .justify_center()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(1120.))
                            .p(px(layout.list_padding()))
                            .gap_3()
                            .children(visible_groups.into_iter().map(|index| {
                                self.update_group_card(
                                    index,
                                    &self.update_groups[index],
                                    layout,
                                    cx,
                                )
                            }))
                            .when(no_visible_groups, |this| {
                                this.child(
                                    div()
                                        .p_4()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if self.update_filter == UpdateFilter::Unread {
                                            "No unread local updates"
                                        } else {
                                            "No local updates yet"
                                        }),
                                )
                            }),
                    ),
            )
    }

    fn ensure_settings_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(7)
                .placeholder("project = YOUR_PROJECT")
        });
        input.update(cx, |input, cx| {
            input.set_value(&self.settings_scope_text, window, cx)
        });
        self.settings_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.settings_scope_text = input.read(cx).value().to_string();
                    this.settings_feedback = None;
                    cx.notify();
                }
            },
        ));
        self.settings_input = Some(input);
    }

    fn ensure_team_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.team_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(5)
                .placeholder("account-id-1\nuser@example.com")
        });
        input.update(cx, |input, cx| input.set_value(&self.team_text, window, cx));
        self.settings_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.team_text = input.read(cx).value().to_string();
                    this.team_feedback = None;
                    cx.notify();
                }
            },
        ));
        self.team_input = Some(input);
    }

    fn reset_settings_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_scope_text = DEFAULT_JQL_SCOPE.to_owned();
        self.settings_feedback = None;
        if let Some(input) = self.settings_input.clone() {
            input.update(cx, |input, cx| {
                input.set_value(DEFAULT_JQL_SCOPE, window, cx)
            });
        }
        cx.notify();
    }

    fn begin_save_settings(&mut self, cx: &mut Context<Self>) {
        if self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.settings_feedback = Some("Connect Jira before applying a scope".to_owned());
            cx.notify();
            return;
        };
        let entered = self.settings_scope_text.clone();
        let previous_scope = workspace.jql_scope();
        let team_members = self.team_members.clone();
        self.operation_in_progress = true;
        self.settings_feedback = Some("Applying scope and refreshing Jira…".to_owned());
        cx.notify();

        let task = cx.spawn(async move |this, cx| {
            let normalized = normalize_issue_jql_scope(Some(entered.clone()));
            let result = async {
                let normalized = normalized
                    .map_err(|_| "Scope is invalid; check the expression and ORDER BY rule")?;
                workspace
                    .set_jql_scope(normalized.clone())
                    .await
                    .map_err(|_| "Scope could not be prepared locally")?;
                let refreshed = match workspace.refresh(&CancellationToken::new()).await {
                    Ok(result) => result,
                    Err(_) => {
                        if workspace.set_jql_scope(previous_scope.clone()).await.is_err() {
                            return Err("Jira rejected the scope and the previous scope could not be restored");
                        }
                        let _ = workspace.load_cached_for_authenticated_account().await;
                        return Err("Jira rejected the scope; the previous scope remains active");
                    }
                };
                if save_preferences(&LocalPreferences {
                    issue_jql_scope: normalized.clone(),
                    team_members,
                })
                .is_err()
                {
                    if workspace.set_jql_scope(previous_scope.clone()).await.is_err() {
                        return Err("Settings could not be saved and the previous scope could not be restored");
                    }
                    let _ = workspace.load_cached_for_authenticated_account().await;
                    return Err("Scope applied remotely, but settings could not be saved locally");
                }
                Ok((refreshed, normalized))
            }
            .await;

            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok((refreshed, normalized)) => {
                        this.settings_scope_text = normalized
                            .clone()
                            .unwrap_or_else(|| DEFAULT_JQL_SCOPE.to_owned());
                        this.settings_warning = None;
                        this.settings_feedback = Some("Scope saved and Jira refreshed".to_owned());
                        this.sync_message = refresh_complete_message(&refreshed);
                        this.apply_cached(refreshed.cached, cx);
                        this.start_automatic_polling(cx);
                    }
                    Err(message) => {
                        if message.contains("could not be restored") {
                            // Never leave the old cache paired with an unknown active scope.
                            this.workspace = None;
                            this.polling_task.take();
                            this.automatic_polling_paused = true;
                            this.domain_issues.clear();
                            this.issues.clear();
                            this.update_groups.clear();
                            this.selected_issue = None;
                        }
                        this.settings_feedback = Some(message.to_owned());
                    }
                }
                cx.notify();
            });
        });
        self.settings_task = Some(task);
    }

    fn begin_save_team(&mut self, cx: &mut Context<Self>) {
        if self.team_task.is_some() || self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.team_feedback = Some("Connect Jira before saving the team tracker".to_owned());
            cx.notify();
            return;
        };
        let entered = self.team_text.clone();
        let previous_members = self.team_members.clone();
        let previous_text = self.team_text.clone();
        let previous_accounts = workspace.team_members();
        let issue_jql_scope = workspace.jql_scope();
        self.operation_in_progress = true;
        self.team_feedback = Some("Resolving team members and refreshing Jira…".to_owned());
        let task = cx.spawn(async move |this, cx| {
            let result = async {
                let identifiers = team_identifier_lines(&entered)?;
                let mut resolved = Vec::new();
                for identifier in identifiers {
                    let user = if identifier.contains('@') {
                        let users = workspace
                            .search_users(identifier.clone(), 5, &CancellationToken::new())
                            .await
                            .map_err(|_| "Jira user search failed; existing team remains active")?
                            .into_iter()
                            .filter(|user| user.active)
                            .collect::<Vec<_>>();
                        if let Some(message) = team_email_resolution_message(users.len()) {
                            return Err(message);
                        }
                        users.into_iter().next().expect("exactly one candidate")
                    } else {
                        let member = persisted_direct_team_member(identifier)?;
                        resolved.push(member);
                        continue;
                    };
                    resolved.push(PersistedTeamMember {
                        identifier,
                        account_id: user.account_id.to_string(),
                        display_name: user.display_name,
                    });
                }
                let normalized = normalize_team_members(resolved)
                    .map_err(|_| "Team tracker entries are invalid or exceed the member limit")?;
                let accounts = normalized
                    .iter()
                    .filter_map(|member| AccountId::new(member.account_id.clone()).ok())
                    .collect::<Vec<_>>();
                workspace.configure_team_members(accounts).await.map_err(
                    |_| "Team configuration could not be applied; existing team remains active",
                )?;
                let refreshed = match workspace.refresh_team(&CancellationToken::new()).await {
                    Ok(refreshed) => refreshed,
                    Err(_) => {
                        let restored = workspace
                            .configure_team_members(previous_accounts.clone())
                            .await
                            .is_ok();
                        return Err(if restored {
                            "Team refresh failed; existing team remains active"
                        } else {
                            "Team refresh failed and the previous team could not be restored; team tracker paused"
                        });
                    }
                };
                if save_preferences(&LocalPreferences {
                    issue_jql_scope,
                    team_members: normalized.clone(),
                })
                .is_err()
                {
                    let restored = workspace
                        .configure_team_members(previous_accounts.clone())
                        .await
                        .is_ok();
                    return Err(if restored {
                        "Team refreshed but could not be saved locally; existing team remains active"
                    } else {
                        "Team settings could not be saved and the previous team could not be restored; team tracker paused"
                    });
                }
                Ok((normalized, refreshed))
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.team_task = None;
                this.operation_in_progress = false;
                match result {
                    Ok((members, refreshed)) => {
                        this.team_automatic_polling_paused = false;
                        this.team_members = members;
                        this.team_text = this
                            .team_members
                            .iter()
                            .map(|member| member.identifier.clone())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let authenticated_account = this.authenticated_account.clone();
                        this.users.retain(|user| {
                            authenticated_account.as_ref() == Some(&user.account_id)
                        });
                        this.users
                            .extend(this.team_members.iter().filter_map(|member| {
                                if !persisted_team_member_has_display_name(member) {
                                    return None;
                                }
                                AccountId::new(member.account_id.clone())
                                    .ok()
                                    .map(|account_id| {
                                        User::new(
                                            workspace.site_id().clone(),
                                            account_id,
                                            member.display_name.clone(),
                                            None,
                                            true,
                                        )
                                    })
                            }));
                        this.apply_team_cached(refreshed.cached, cx);
                        this.team_feedback = Some(team_refresh_feedback(
                            "Team tracker saved and refreshed",
                            &this.team_issues,
                        ));
                    }
                    Err(message) => {
                        this.team_members = previous_members;
                        this.team_text = previous_text;
                        if message.contains("could not be restored") {
                            this.team_automatic_polling_paused = true;
                            this.team_members.clear();
                            this.team_issues.clear();
                            this.team_events.clear();
                            this.refresh_team_table(cx);
                        }
                        this.team_feedback = Some(message.to_owned());
                    }
                }
                cx.notify();
            });
        });
        self.team_task = Some(task);
        cx.notify();
    }

    fn begin_forget_saved_login(&mut self, cx: &mut Context<Self>) {
        if self.saved_login_delete_task.is_some()
            || !can_start_saved_login_delete(self.saved_login_delete_state)
        {
            return;
        }
        self.saved_login_delete_state = SavedLoginDeleteState::Deleting;
        let task = cx.spawn(async move |this, cx| {
            let outcome = match credential_store::delete_saved_credentials().await {
                Ok(DeleteOutcome::Deleted) => SavedLoginDeleteOutcome::Deleted,
                Ok(DeleteOutcome::Absent) => SavedLoginDeleteOutcome::Absent,
                Err(_) => SavedLoginDeleteOutcome::Error,
            };
            let _ = this.update(cx, |this, cx| {
                this.saved_login_delete_task = None;
                this.saved_login_delete_state = SavedLoginDeleteState::Completed(outcome);
                cx.notify();
            });
        });
        self.saved_login_delete_task = Some(task);
        cx.notify();
    }

    fn begin_test_desktop_notification(&mut self, cx: &mut Context<Self>) {
        if !self.can_start_desktop_notification_test() {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        self.desktop_notification_test_state = DesktopNotificationTestState::Sending;
        self.diagnostics.desktop_notification_test_started();
        let diagnostics = self.diagnostics.clone();
        let task = cx.spawn(async move |this, cx| {
            let outcome = match workspace.test_desktop_notification().await {
                Ok(receipt) => {
                    let result = DiagnosticDesktopNotificationTestResult::Accepted {
                        notification_id: receipt.notification_id(),
                    };
                    diagnostics.desktop_notification_test_result(result);
                    DesktopNotificationTestOutcome::Accepted {
                        notification_id: receipt.notification_id(),
                    }
                }
                Err(error) => {
                    let result = DiagnosticDesktopNotificationTestResult::Failed(
                        DiagnosticErrorKind::from(error.kind()),
                    );
                    diagnostics.desktop_notification_test_result(result);
                    DesktopNotificationTestOutcome::Failed(DiagnosticErrorKind::from(error.kind()))
                }
            };
            let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Secs, true);
            let _ = this.update(cx, |this, cx| {
                this.desktop_notification_test_task = None;
                this.desktop_notification_test_state =
                    DesktopNotificationTestState::Completed(DesktopNotificationTestReport {
                        timestamp,
                        outcome,
                    });
                cx.notify();
            });
        });
        self.desktop_notification_test_task = Some(task);
        cx.notify();
    }

    fn can_start_desktop_notification_test(&self) -> bool {
        self.workspace.is_some()
            && matches!(
                self.desktop_notification_test_state,
                DesktopNotificationTestState::Idle | DesktopNotificationTestState::Completed(_)
            )
    }

    fn render_settings(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.settings_input.clone();
        let team_input = self.team_input.clone();
        let text = self.settings_scope_text.clone();
        let chars = text.chars().count();
        let bytes = text.len();
        let validation = normalize_issue_jql_scope(Some(text.clone())).err();
        let live = self.workspace.is_some();
        let test_running = matches!(
            self.desktop_notification_test_state,
            DesktopNotificationTestState::Sending
        );
        let saved_login_deleting = matches!(
            self.saved_login_delete_state,
            SavedLoginDeleteState::Deleting
        );
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                h_flex()
                    .id("settings-scroll")
                    .flex_1()
                    .overflow_y_scrollbar()
                    .justify_center()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(820.))
                            .p(px(layout.list_padding()))
                            .gap_3()
                            .child(div().text_xl().font_semibold().child("Jira settings"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Choose the Jira scope used for your assigned or watched account view."),
                            )
                            .when_some(input, |this, input| {
                                this.child(
                                    Textarea::new(&input)
                                        .w_full()
                                        .h(px(if layout.is_mobile() { 128. } else { 160. }))
                                        .aria_label("JQL scope")
                                        .disabled(!live || self.operation_in_progress),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if validation.is_some() {
                                        cx.theme().danger
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(format!("{chars} characters · {bytes} bytes · maximum {MAX_JQL_SCOPE_LENGTH} bytes")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("This is a scope expression. Jira Desk appends assigned-or-watched account membership, incremental updated overlap, and ORDER BY updated DESC. Do not include ORDER BY."),
                            )
                            .when(!live, |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().warning)
                                        .child("Settings become available after a live Jira workspace is connected."),
                                )
                            })
                            .when_some(self.settings_warning.clone(), |this, warning| {
                                this.child(div().text_sm().text_color(cx.theme().warning).child(warning))
                            })
                            .when_some(validation.map(|_| format!("Scope is invalid: it must be non-empty, within {MAX_JQL_SCOPE_LENGTH} bytes, and contain no ORDER BY")), |this, message| {
                                this.child(div().text_sm().text_color(cx.theme().danger).child(message))
                            })
                            .when_some(self.settings_feedback.clone(), |this, feedback| {
                                this.child(div().text_sm().text_color(cx.theme().muted_foreground).child(feedback))
                            })
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(div().text_base().font_semibold().child("Desktop notifications"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Send a local test through the same Freedesktop service configuration used by Jira updates. This never calls Jira or changes the local update feed."),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("App name: Jira Desk · Icon: dev.jiradesk.JiraDesk · Desktop-entry: dev.jiradesk.JiraDesk · Summary: {TEST_NOTIFICATION_SUMMARY} · Body: {TEST_NOTIFICATION_BODY}")),
                                    )
                                    .child(
                                        Button::new("test-desktop-notification")
                                            .label(if test_running {
                                                "Sending test notification…"
                                            } else {
                                                "Send test notification"
                                            })
                                            .disabled(!live || test_running)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.begin_test_desktop_notification(cx)
                                            })),
                                    )
                                    .when_some(
                                        match &self.desktop_notification_test_state {
                                            DesktopNotificationTestState::Completed(report) => {
                                                Some(report.clone())
                                            }
                                            _ => None,
                                        },
                                        |this, report| {
                                            let result = match report.outcome {
                                                DesktopNotificationTestOutcome::Accepted {
                                                    notification_id,
                                                } => format!(
                                                    "Accepted by desktop service · notification ID {notification_id}"
                                                ),
                                                DesktopNotificationTestOutcome::Failed(error) => {
                                                    format!("Failed · error category {}", desktop_notification_error_category(error))
                                                }
                                            };
                                            this.child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(div().text_sm().child(format!("Last test · {} · {result}", report.timestamp)))
                                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Accepted by the desktop service does not prove GNOME displayed a banner."))
                                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Diagnostic events are written to diagnostics.jsonl.")),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(div().text_base().font_semibold().child("Saved Jira login"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Credentials are kept in the desktop system keyring, reused automatically across Jira Desk/AppImage versions, and never written to SQLite, preferences, or logs."),
                                    )
                                    .child(
                                        Button::new("forget-saved-jira-login")
                                            .label(if saved_login_deleting {
                                                "Forgetting saved Jira login…"
                                            } else {
                                                "Forget saved Jira login"
                                            })
                                            .when(layout.is_mobile(), |this| this.w_full())
                                            .disabled(saved_login_deleting)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.begin_forget_saved_login(cx)
                                            })),
                                    )
                                    .when_some(
                                        match self.saved_login_delete_state {
                                            SavedLoginDeleteState::Completed(outcome) => {
                                                Some(outcome)
                                            }
                                            SavedLoginDeleteState::Idle
                                            | SavedLoginDeleteState::Deleting => None,
                                        },
                                        |this, outcome| {
                                            let (message, is_error) =
                                                saved_login_delete_feedback(outcome);
                                            this.child(
                                                div()
                                                    .text_sm()
                                                    .text_color(if is_error {
                                                        cx.theme().danger
                                                    } else {
                                                        cx.theme().muted_foreground
                                                    })
                                                    .child(message),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                h_flex()
                                    .when(layout.is_mobile(), |this| this.flex_col())
                                    .gap_2()
                                    .child(
                                        Button::new("save-settings")
                                            .primary()
                                            .label("Save and refresh")
                                            .disabled(!live || self.operation_in_progress || validation.is_some())
                                            .on_click(cx.listener(|this, _, _, cx| this.begin_save_settings(cx))),
                                    )
                                    .child(
                                        Button::new("reset-settings")
                                            .ghost()
                                            .label("Use default scope")
                                            .disabled(!live || self.operation_in_progress)
                                        .on_click(cx.listener(|this, _, window, cx| this.reset_settings_editor(window, cx))),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(div().text_base().font_semibold().child("Team tracker"))
                                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("One Jira account ID or Atlassian email per line. This shows in-progress tickets assigned to those accounts; Jira permissions still apply."))
                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Email resolution requires exactly one active Jira user because the User search domain does not retain email. Uses existing read:jira-user/read:jira-work scopes; no new scope is needed."))
                                    .when_some(team_input, |this, input| this.child(Textarea::new(&input).w_full().h(px(if layout.is_mobile() { 110. } else { 140. })).aria_label("Team tracker members").disabled(!live || self.team_task.is_some() || self.operation_in_progress)))
                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(format!("{} configured · maximum {}", self.team_members.len(), MAX_TEAM_MEMBERS)))
                                    .when_some(self.team_feedback.clone(), |this, message| this.child(div().text_sm().text_color(cx.theme().muted_foreground).child(message)))
                                    .child(h_flex().gap_2().when(layout.is_mobile(), |this| this.flex_col()).child(Button::new("save-team").primary().label(if self.team_task.is_some() { "Saving team…" } else { "Save team" }).disabled(!live || self.team_task.is_some() || self.operation_in_progress).on_click(cx.listener(|this, _, _, cx| this.begin_save_team(cx)))).child(Button::new("refresh-team").ghost().label("Refresh team").disabled(!live || self.team_task.is_some() || self.operation_in_progress || self.team_automatic_polling_paused).on_click(cx.listener(|this, _, _, cx| this.begin_team_refresh(cx))))),
                            )
                    ),
            )
    }

    fn update_group_card(
        &self,
        index: usize,
        group: &UpdateGroupViewModel,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let issue_type = self
            .domain_issues
            .iter()
            .find(|issue| issue.id == group.issue_id)
            .map(|issue| issue.issue_type.name.as_str())
            .unwrap_or("Unknown");
        let mobile = layout.is_mobile();
        let issue_id = group.issue_id.clone();
        let clicked_issue_id = issue_id.clone();
        let expanded = self.expanded_update_groups.contains(&group.issue_id);
        let rows = compact_update_rows(&group.events);
        let visible_row_count = visible_update_row_count(rows.len(), expanded);
        let hidden_row_count = hidden_update_row_count(rows.len(), expanded);
        let open_area = Button::new(("update-open", index))
            .ghost()
            .flex_1()
            .h_auto()
            .items_start()
            .min_w_0()
            .gap_3()
            .p_3()
            .hover(|style| style.bg(cx.theme().list_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_update_issue(clicked_issue_id.clone(), mobile, cx);
            }))
            .child(
                div()
                    .mt_1()
                    .size_2()
                    .flex_shrink_0()
                    .rounded_full()
                    .when(group.unread, |this| this.bg(cx.theme().primary))
                    .when(!group.unread, |this| this.bg(cx.theme().muted)),
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
                                        group.issue_key.clone(),
                                        issue_type,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .line_clamp(2)
                                            .text_sm()
                                            .font_semibold()
                                            .child(group.issue_summary.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(group.latest_occurred_at.clone()),
                            ),
                    )
                    .child(
                        v_flex().gap_1().children(
                            rows.iter()
                                .take(visible_row_count)
                                .map(|row| self.update_row_element(row, cx)),
                        ),
                    ),
            )
            .into_any_element();
        h_flex()
            .w_full()
            .min_w_0()
            .items_start()
            .gap_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .when(group.unread, |this| this.border_color(cx.theme().primary))
            .child(open_area)
            .child(
                v_flex()
                    .flex_shrink_0()
                    .items_end()
                    .gap_1()
                    .p_2()
                    .when(hidden_row_count > 0, |this| {
                        let issue_id = issue_id.clone();
                        this.child(
                            Button::new(("update-expand", index))
                                .compact()
                                .ghost()
                                .label(format!("Show {hidden_row_count} more"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_update_group_expanded(issue_id.clone(), cx);
                                })),
                        )
                    })
                    .when(expanded && rows.len() > UPDATE_PREVIEW_LIMIT, |this| {
                        let issue_id = issue_id.clone();
                        this.child(
                            Button::new(("update-collapse", index))
                                .compact()
                                .ghost()
                                .label("Show less")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_update_group_expanded(issue_id.clone(), cx);
                                })),
                        )
                    })
                    .when(group.unread, |this| {
                        this.child(
                            Button::new(("update-mark-read", index))
                                .ghost()
                                .compact()
                                .label("Mark read")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.mark_group_read(issue_id.clone(), cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn update_row_element(&self, row: &CompactedUpdateRow, cx: &mut Context<Self>) -> AnyElement {
        let (change, occurred_at) = match row {
            CompactedUpdateRow::Event(event) => (event.change.clone(), event.occurred_at.clone()),
            CompactedUpdateRow::GenericSummary { count, occurred_at } => {
                (generic_summary_label(*count), occurred_at.clone())
            }
        };
        h_flex()
            .min_w_0()
            .gap_2()
            .text_xs()
            .child(div().min_w_0().flex_1().child(change))
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(occurred_at),
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
            .child(
                Button::new("mobile-team")
                    .compact()
                    .when(self.section == Section::Team, |this| this.primary())
                    .label(format!("Team · {}", self.team_issues.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Team;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("mobile-settings")
                    .compact()
                    .when(self.section == Section::Settings, |this| this.primary())
                    .label("Settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Settings;
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
        self.ensure_settings_input(window, cx);
        self.ensure_team_input(window, cx);
        self.ensure_team_table(window, cx);
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let content = match self.section {
            Section::Issues => self.render_issues(layout, cx).into_any_element(),
            Section::Updates => self.render_updates(layout, cx).into_any_element(),
            Section::Team => self.render_team(layout, cx).into_any_element(),
            Section::Settings => self.render_settings(layout, cx).into_any_element(),
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
    use crate::presentation::{UpdateViewModel, normalized_issue_key};
    use crate::sample_data::{sample_issues, sample_users};
    use gpui::VisualTestContext;
    use gpui_component::searchable_list::SearchableListDelegate as _;
    use jira_application::{
        ErrorKind, IssueFetchRequest, IssuePage, PortFuture, UserSearchRequest,
    };
    use time::macros::datetime;

    struct EmptyJira;

    impl JiraReadPort for EmptyJira {
        fn fetch_current_user<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, User> {
            Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
        }

        fn search_users<'a>(
            &'a self,
            _request: &'a UserSearchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<User>> {
            Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
        }

        fn fetch_issue_page<'a>(
            &'a self,
            _request: &'a IssueFetchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
        }

        fn fetch_issues_by_id<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _issue_ids: &'a [IssueId],
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<Issue>> {
            Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
        }
    }

    fn update_view(event_id: &str, change: &str, occurred_at: &str) -> UpdateViewModel {
        UpdateViewModel {
            event_id: jira_domain::EventId::new(event_id).expect("event"),
            issue_id: IssueId::new("100").expect("issue"),
            issue_key: "IX-100".to_owned(),
            issue_summary: "Summary".to_owned(),
            change: change.to_owned(),
            occurred_at: occurred_at.to_owned(),
            unread: false,
        }
    }

    #[test]
    fn update_filter_returns_only_unread_ticket_groups() {
        let mut groups = vec![
            UpdateGroupViewModel {
                issue_id: IssueId::new("100").expect("issue"),
                issue_key: "IX-100".to_owned(),
                issue_summary: "Unread".to_owned(),
                events: Vec::new(),
                latest_occurred_at: "now".to_owned(),
                unread_count: 1,
                unread: true,
            },
            UpdateGroupViewModel {
                issue_id: IssueId::new("200").expect("issue"),
                issue_key: "IX-200".to_owned(),
                issue_summary: "Read".to_owned(),
                events: Vec::new(),
                latest_occurred_at: "earlier".to_owned(),
                unread_count: 0,
                unread: false,
            },
        ];

        assert_eq!(
            filtered_update_group_indices(&groups, UpdateFilter::All),
            vec![0, 1]
        );
        assert_eq!(
            filtered_update_group_indices(&groups, UpdateFilter::Unread),
            vec![0]
        );

        groups[0].unread = false;
        assert!(filtered_update_group_indices(&groups, UpdateFilter::Unread).is_empty());
    }

    #[test]
    fn status_control_editability_requires_live_selected_issue() {
        let idle = IssueEditState::Idle;
        assert!(status_control_is_editable(true, true, false, false, &idle));
        assert!(!status_control_is_editable(
            true, false, false, false, &idle
        ));
        assert!(!status_control_is_editable(
            false, true, false, false, &idle
        ));
        assert!(!status_control_is_editable(true, true, true, false, &idle));
        assert!(!status_control_is_editable(true, true, false, true, &idle));
    }

    #[test]
    fn status_control_allows_only_status_lookup_states() {
        let issue_id = IssueId::new("100").expect("issue");
        let loading = IssueEditState::LoadingTransitions {
            issue_id: issue_id.clone(),
        };
        let confirming = IssueEditState::ConfirmingAssignee {
            issue_id,
            issue_key: "IX-100".to_owned(),
            account_id: None,
            display_name: "Unassigned".to_owned(),
        };
        assert!(status_control_is_editable(
            true, true, false, false, &loading
        ));
        assert!(!status_control_is_editable(
            true,
            true,
            false,
            false,
            &confirming
        ));
    }

    #[test]
    fn transition_option_label_uses_destination_status_only() {
        let transition = IssueTransition {
            id: "31".to_owned(),
            name: "Move issue".to_owned(),
            to: jira_domain::Status {
                id: "3".to_owned(),
                name: "In Progress".to_owned(),
                category: None,
            },
        };

        assert_eq!(transition_option_label(&transition), "In Progress");
    }

    #[test]
    fn transition_list_height_is_compact_and_bounded() {
        assert_eq!(status_transition_list_height(1), px(32.));
        assert_eq!(status_transition_list_height(2), px(68.));
        assert_eq!(
            status_transition_list_height(12),
            px(STATUS_TRANSITION_LIST_MAX_HEIGHT)
        );
    }

    #[test]
    fn live_project_label_is_inferred_from_unique_projects() {
        assert_eq!(project_label(&[]), "Jira projects");
        let mut issues = sample_issues();
        issues.truncate(1);
        assert_eq!(project_label(&issues), issues[0].project.name);
        let mut second = issues[0].clone();
        second.project.name = "Another project".to_owned();
        issues.push(second);
        assert_eq!(project_label(&issues), "2 Jira projects");
    }

    #[test]
    fn compacts_generic_activity_rows_but_keeps_specific_changes() {
        let rows = compact_update_rows(&[
            update_view("event-1", "Issue activity changed", "latest"),
            update_view("event-2", "Status: To do → Done", "middle"),
            update_view("event-3", "Issue activity changed", "earlier"),
        ]);

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            CompactedUpdateRow::GenericSummary {
                count: 2,
                occurred_at: "latest".to_owned(),
            }
        );
        assert!(matches!(
            &rows[1],
            CompactedUpdateRow::Event(UpdateViewModel { change, .. })
                if change == "Status: To do → Done"
        ));
    }

    #[test]
    fn generic_summary_copy_is_honest_and_counted() {
        assert_eq!(
            generic_summary_label(1),
            "Other Jira activity · exact field not available from sync"
        );
        assert_eq!(
            generic_summary_label(3),
            "Other Jira activity · 3 events · exact field not available from sync"
        );
    }

    #[test]
    fn update_preview_limits_rows_without_discarding_audit_data() {
        assert_eq!(visible_update_row_count(5, false), 3);
        assert_eq!(hidden_update_row_count(5, false), 2);
        assert_eq!(visible_update_row_count(5, true), 5);
        assert_eq!(hidden_update_row_count(5, true), 0);
        assert_eq!(visible_update_row_count(2, false), 2);
    }

    #[test]
    fn update_filter_and_expansion_state_start_in_safe_defaults() {
        let dashboard = Dashboard::from_sample_data();

        assert_eq!(dashboard.update_filter, UpdateFilter::All);
        assert!(dashboard.expanded_update_groups.is_empty());
    }

    #[test]
    fn settings_editor_starts_with_the_default_scope() {
        let dashboard = Dashboard::from_sample_data();

        assert_eq!(dashboard.settings_scope_text, DEFAULT_JQL_SCOPE);
    }

    #[test]
    fn saved_login_delete_feedback_is_safe_and_explicit() {
        assert_eq!(
            saved_login_delete_feedback(SavedLoginDeleteOutcome::Deleted),
            (
                "Saved Jira login forgotten. This session remains connected.",
                false
            )
        );
        assert_eq!(
            saved_login_delete_feedback(SavedLoginDeleteOutcome::Absent),
            (
                "No saved Jira login was present. This session remains connected.",
                false
            )
        );
        assert_eq!(
            saved_login_delete_feedback(SavedLoginDeleteOutcome::Error),
            (
                "Saved Jira login could not be removed from the system keyring.",
                true
            )
        );
    }

    #[test]
    fn saved_login_delete_gates_repeated_clicks_only_while_deleting() {
        assert!(can_start_saved_login_delete(SavedLoginDeleteState::Idle));
        assert!(!can_start_saved_login_delete(
            SavedLoginDeleteState::Deleting
        ));
        assert!(can_start_saved_login_delete(
            SavedLoginDeleteState::Completed(SavedLoginDeleteOutcome::Deleted,)
        ));
    }

    #[test]
    fn desktop_notification_test_is_unavailable_without_live_workspace() {
        let dashboard = Dashboard::from_sample_data();

        assert!(!dashboard.can_start_desktop_notification_test());
        assert_eq!(
            dashboard.desktop_notification_test_state,
            DesktopNotificationTestState::Idle
        );
    }

    #[test]
    fn desktop_notification_error_copy_uses_stable_category() {
        assert_eq!(
            desktop_notification_error_category(DiagnosticErrorKind::Notification),
            "notification"
        );
        assert_eq!(
            desktop_notification_error_category(DiagnosticErrorKind::UnknownOutcome),
            "unknown_outcome"
        );
    }

    fn refresh_result_with_inserted_events(events_inserted: usize) -> RefreshResult {
        RefreshResult {
            cached: CachedWorkspace {
                issues: sample_issues(),
                events: Vec::new(),
            },
            outcome: jira_application::SyncOutcome {
                mode: SyncMode::Incremental,
                pages_fetched: 1,
                issues_fetched: 5,
                events_inserted,
                notifications_delivered: events_inserted,
                notification_failures: 0,
                cursor: datetime!(2026-08-18 00:00 UTC),
            },
        }
    }

    #[test]
    fn refresh_notification_reports_new_update_count() {
        let result = refresh_result_with_inserted_events(2);

        assert_eq!(
            refresh_notification_message(&result),
            "Refresh complete · 5 issues · 2 new local updates"
        );
    }

    #[test]
    fn refresh_notification_distinguishes_zero_new_updates() {
        let result = refresh_result_with_inserted_events(0);

        assert_eq!(
            refresh_notification_message(&result),
            "Refresh complete · 5 issues · no new local updates"
        );
    }

    #[test]
    fn refresh_status_uses_submission_wording_for_desktop_notifications() {
        let result = refresh_result_with_inserted_events(1);

        let message = refresh_complete_message(&result);
        assert!(message.contains("accepted by desktop service"));
        assert!(!message.contains("delivered"));
    }

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

    #[gpui::test]
    fn open_update_selects_issue_and_opens_mobile_detail(cx: &mut gpui::TestAppContext) {
        let issue = sample_issues()
            .into_iter()
            .find(|issue| issue.key.as_str() == "DESK-176")
            .expect("issue");
        let dashboard = cx.new(|_| Dashboard::from_sample_data());

        cx.update_entity(&dashboard, |dashboard, cx| {
            dashboard.open_update_issue(issue.id.clone(), true, cx);
        });
        let (selected_issue, section, mobile_detail_open) =
            cx.read_entity(&dashboard, |dashboard, _| {
                (
                    dashboard.selected_issue.clone(),
                    dashboard.section,
                    dashboard.mobile_detail_open,
                )
            });

        assert_eq!(selected_issue, Some(issue.id));
        assert_eq!(section, Section::Issues);
        assert!(mobile_detail_open);
    }

    #[gpui::test]
    fn transition_chooser_options_remain_visible_in_constrained_popover(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);

        let workspace = futures_lite::future::block_on(LiveWorkspace::initialize(
            JiraSiteId::new("site").expect("site"),
            None,
            Arc::new(EmptyJira),
            Arc::new(jira_storage::SqliteStore::in_memory().expect("store")),
        ))
        .expect("workspace");
        let mut dashboard = Dashboard::from_sample_data();
        dashboard.workspace = Some(Arc::new(workspace));
        let issue = dashboard
            .selected_issue
            .clone()
            .expect("sample issue selected");
        let transitions = (0..12)
            .map(|index| IssueTransition {
                id: (31 + index).to_string(),
                name: format!("Move issue {index}"),
                to: jira_domain::Status {
                    id: (3 + index).to_string(),
                    name: format!("Status {index}"),
                    category: None,
                },
            })
            .collect();
        dashboard.issue_edit_state = IssueEditState::TransitionChooser {
            issue_id: issue,
            issue_key: "DESK-176".to_owned(),
            transitions,
        };
        dashboard.status_popover_open = true;
        let window = cx.open_window(gpui::size(px(720.), px(600.)), |_, _| dashboard);
        let dashboard_entity = window.root(cx).expect("dashboard root");
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let option_bounds = visual
            .debug_bounds("status-transition-31")
            .expect("transition option should be laid out");
        assert!(
            option_bounds.size.height > px(0.),
            "transition option collapsed: {option_bounds:?}"
        );

        visual.simulate_click(
            gpui::point(
                option_bounds.origin.x + option_bounds.size.width / 2.,
                option_bounds.origin.y + option_bounds.size.height / 2.,
            ),
            Default::default(),
        );
        assert!(
            dashboard_entity.read_with(&visual, |dashboard, _| {
                matches!(
                    dashboard.issue_edit_state,
                    IssueEditState::ConfirmingTransition { .. }
                )
            }),
            "first transition should be clickable at {option_bounds:?}"
        );
    }

    #[gpui::test]
    fn team_tracker_table_and_detail_are_bounded_on_desktop(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);

        let mut dashboard = Dashboard::from_sample_data();
        dashboard.section = Section::Team;
        dashboard.team_members = vec![PersistedTeamMember {
            identifier: "amina".to_owned(),
            account_id: "amina".to_owned(),
            display_name: "Amina Yusuf".to_owned(),
        }];
        dashboard.team_issues = sample_issues();
        dashboard.team_feedback =
            Some("Team tracker refreshed · fetched 5 · displaying 3".to_owned());

        let window = cx.open_window(gpui::size(px(1370.), px(900.)), |_, _| dashboard);
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let table_bounds = visual
            .debug_bounds("team-table")
            .expect("team table should be laid out");
        let detail_bounds = visual
            .debug_bounds("team-detail")
            .expect("team detail should be laid out");
        let header_bounds = visual
            .debug_bounds("header")
            .expect("dashboard header should be laid out");
        assert!(
            table_bounds.size.height > px(300.),
            "team table body collapsed to header: {table_bounds:?}"
        );
        assert!(
            detail_bounds.origin.y >= header_bounds.origin.y + header_bounds.size.height,
            "team detail overlaps dashboard header: detail={detail_bounds:?}, header={header_bounds:?}"
        );
        assert!(
            detail_bounds.origin.y + detail_bounds.size.height <= px(900.) + px(1.),
            "team detail escapes the desktop content region: {detail_bounds:?}"
        );
        assert!(
            table_bounds.origin.x + table_bounds.size.width <= detail_bounds.origin.x,
            "team detail overlaps the team table: table={table_bounds:?}, detail={detail_bounds:?}"
        );
        assert!(
            detail_bounds.origin.x + detail_bounds.size.width <= px(1370.) + px(1.),
            "team detail escapes the desktop width: {detail_bounds:?}"
        );

        visual.simulate_resize(gpui::size(px(390.), px(800.)));
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let mobile_table_bounds = visual
            .debug_bounds("team-table")
            .expect("team table should remain visible on mobile");
        assert!(
            mobile_table_bounds.size.height > px(300.),
            "mobile team table body collapsed: {mobile_table_bounds:?}"
        );
    }

    #[test]
    fn filtered_selected_issue_resolves_header_from_domain_cache() {
        let domain_issues = sample_issues();
        let users = sample_users();
        let selected = domain_issues
            .iter()
            .find(|issue| issue.key.as_str() == "DESK-176")
            .expect("selected issue");
        let visible = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::ToDo, "");
        assert!(!visible.iter().any(|issue| issue.id == selected.id));

        let view = selected_issue_view_from_sources(
            Some(&selected.id),
            &visible,
            &domain_issues,
            None,
            &users,
        )
        .expect("filtered selected issue header");

        assert_eq!(view.id, selected.id);
        assert_eq!(view.key, selected.key.to_string());
    }

    #[test]
    fn absent_cache_selected_issue_uses_fetched_core_for_header_and_comments() {
        let issue = sample_issues()
            .into_iter()
            .find(|issue| issue.key.as_str() == "DESK-176")
            .expect("issue");
        let users = sample_users();
        let mut dashboard = Dashboard::from_sample_data();
        dashboard.domain_issues.clear();
        dashboard.issues.clear();
        dashboard.selected_issue = Some(issue.id.clone());
        dashboard.selected_issue_core = Some(issue.clone());

        let view = dashboard
            .selected_issue_view()
            .expect("fetched issue core header");
        let comment_target = dashboard.comment_target_issue().expect("comment target");

        assert_eq!(view.id, issue.id);
        assert_eq!(
            view.assignee,
            IssueViewModel::from_domain(&issue, &users).assignee
        );
        assert_eq!(comment_target.id, issue.id);
    }

    #[test]
    fn rebuild_retains_hidden_selection_and_defers_refresh_of_absent_cache_fetch() {
        let issue = sample_issues()
            .into_iter()
            .find(|issue| issue.key.as_str() == "DESK-176")
            .expect("issue");
        let visible = issue_views_for_filter(
            std::slice::from_ref(&issue),
            &sample_users(),
            IssueStatusFilter::ToDo,
            "",
        );
        assert!(visible.is_empty());
        assert_eq!(
            selection_after_issue_view_rebuild(Some(issue.id.clone()), &visible),
            Some(issue.id.clone())
        );

        let loading = DetailState::Loading {
            issue_id: issue.id.clone(),
        };
        assert!(should_defer_detail_refresh(
            Some(&issue.id),
            &[],
            &loading,
            true,
        ));
        assert!(!should_defer_detail_refresh(
            Some(&issue.id),
            std::slice::from_ref(&issue),
            &loading,
            true,
        ));
        assert!(!should_defer_detail_refresh(
            Some(&issue.id),
            &[],
            &DetailState::Empty,
            true,
        ));
        assert!(should_defer_detail_refresh(
            Some(&issue.id),
            &[],
            &DetailState::Error {
                issue_id: issue.id.clone(),
                message: "detail unavailable".to_owned(),
            },
            true,
        ));
        assert!(!should_defer_detail_refresh(
            Some(&issue.id),
            &[],
            &DetailState::Error {
                issue_id: IssueId::new("different").expect("issue"),
                message: "detail unavailable".to_owned(),
            },
            true,
        ));
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
    fn issue_edit_failures_have_safe_definite_and_unknown_copy() {
        let definite = ApplicationError::new(
            jira_application::ErrorKind::Authorization,
            "raw Jira response must not reach UI",
        );
        let (message, unknown) = issue_edit_error_message(&definite, "write");
        assert_eq!(message, "Change not applied · Jira denied permission");
        assert!(!unknown);
        assert!(!message.contains("raw Jira response"));

        let uncertain = ApplicationError::new(
            jira_application::ErrorKind::UnknownOutcome,
            "secret transport detail",
        );
        let (message, unknown) = issue_edit_error_message(&uncertain, "write");
        assert!(unknown);
        assert!(message.contains("Refresh Jira"));
        assert!(!message.contains("secret transport detail"));
    }

    #[test]
    fn issue_edit_target_snapshot_requires_current_issue_and_generation() {
        let issue = IssueId::new("100").expect("issue");
        assert!(issue_edit_target_is_current(Some(&issue), &issue, 4, 4));
        assert!(!issue_edit_target_is_current(Some(&issue), &issue, 5, 4));
        let other = IssueId::new("200").expect("issue");
        assert!(!issue_edit_target_is_current(Some(&other), &issue, 4, 4));
    }

    #[test]
    fn grouped_activity_dispatch_ids_include_read_and_unread_events() {
        let issue = IssueId::new("100").expect("issue");
        let first = jira_domain::EventId::new("event-1").expect("event");
        let second = jira_domain::EventId::new("event-2").expect("event");
        let group = UpdateGroupViewModel {
            issue_id: issue,
            issue_key: "IX-100".to_owned(),
            issue_summary: "Summary".to_owned(),
            latest_occurred_at: "now".to_owned(),
            unread_count: 1,
            unread: true,
            events: vec![
                UpdateViewModel {
                    event_id: first.clone(),
                    issue_id: IssueId::new("100").expect("issue"),
                    issue_key: "IX-100".to_owned(),
                    issue_summary: "Summary".to_owned(),
                    change: "first".to_owned(),
                    occurred_at: "earlier".to_owned(),
                    unread: false,
                },
                UpdateViewModel {
                    event_id: second.clone(),
                    issue_id: IssueId::new("100").expect("issue"),
                    issue_key: "IX-100".to_owned(),
                    issue_summary: "Summary".to_owned(),
                    change: "second".to_owned(),
                    occurred_at: "latest".to_owned(),
                    unread: true,
                },
            ],
        };
        assert_eq!(update_group_event_ids(&group), vec![first, second]);
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

    #[test]
    fn team_identifier_parser_trims_bounds_and_rejects_jql_metacharacters() {
        assert_eq!(
            team_identifier_lines("  account-a  \n\nuser@example.com").expect("valid"),
            vec!["account-a", "user@example.com"]
        );
        assert!(team_identifier_lines("account\"bad").is_err());
        assert!(team_identifier_lines(&"account-a\n".repeat(MAX_TEAM_MEMBERS + 1)).is_err());
    }

    #[test]
    fn team_email_resolution_requires_exactly_one_candidate() {
        assert_eq!(team_email_resolution_message(1), None);
        assert!(team_email_resolution_message(0).is_some());
        assert!(team_email_resolution_message(2).is_some());
    }

    #[test]
    fn direct_account_id_member_uses_persistable_unknown_display_name() {
        let member = persisted_direct_team_member("account-123".to_owned()).expect("valid ID");
        assert_eq!(member.display_name, "Unknown user");
        assert!(normalize_team_members(vec![member]).is_ok());
    }

    #[test]
    fn team_section_is_distinct_from_primary_issue_section() {
        assert_ne!(Section::Issues, Section::Team);
        assert_ne!(Section::Team, Section::Settings);
    }

    #[test]
    fn team_refresh_feedback_reports_fetched_and_displayed_counts() {
        assert_eq!(
            team_refresh_feedback("Team tracker refreshed", &sample_issues()),
            "Team tracker refreshed · fetched 5 · displaying 3 in-progress tickets"
        );
    }

    #[test]
    fn team_summary_is_cache_honest_and_pluralizes_counts() {
        assert_eq!(
            team_summary(1, 1),
            "1 in-progress ticket displayed · 1 configured team member · cached updates remain isolated from Jira issues"
        );
        assert_eq!(
            team_summary(3, 2),
            "3 in-progress tickets displayed · 2 configured team members · cached updates remain isolated from Jira issues"
        );
    }
}
