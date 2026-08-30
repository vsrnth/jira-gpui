//! Small, bounded rich-text renderer for the GPUI adapter.
//!
//! The domain layer has already discarded raw ADF/JSON and untrusted mention
//! identifiers, projecting media to bounded image metadata. This module only
//! turns that safe projection into ordinary GPUI elements; links remain visibly
//! styled but inert.

use std::rc::Rc;

use gpui::{AnyElement, App, Hsla, IntoElement as _, ParentElement as _, Styled as _, Window, div};
use gpui_component::{StyledExt as _, v_flex};
use jira_domain::{
    PanelKind, RichAttachmentCard, RichBlock, RichImage, RichInline, RichListItem, RichMark,
    RichStatusColor, RichTextDocument,
};

use crate::diagnostics::{
    DecodeFallbackReason, DiagnosticEvent, ImageSource as DiagnosticImageSource, ImageStateReason,
};

mod state;
pub(crate) use state::{RichImageRenderState, RichImageRenderStates};

// Cached models can be deserialized without passing through the Jira ADF
// parser. Keep rendering bounded independently of the domain projection's
// plain-text limit, including for adversarially deep nested lists/panels.
const MAX_RENDER_DEPTH: usize = 32;
const MAX_RENDER_NODES: usize = 4_096;
const MAX_RENDER_CHILDREN: usize = 1_024;
const MAX_RENDER_TEXT_BYTES: usize = 1_000_000;
const MAX_IMAGE_LABEL_BYTES: usize = 512;
const MAX_ATTACHMENT_FILENAME_BYTES: usize = 512;
const MAX_ATTACHMENT_LOOKAHEAD_BYTES: usize = 16;
const MAX_IMAGE_HEIGHT: f32 = 720.;
const RENDER_SURFACE_STRIDE: usize = MAX_RENDER_NODES + 1;
const RENDER_OMITTED_LABEL: &str = "Some content was omitted by Jira Desk.";
const FALLBACK_IMAGE_GALLERY_LABEL: &str = "Image attachments";
const FALLBACK_IMAGE_GALLERY_NOTE: &str = "Candidate attachments · exact placement unavailable.";
const UNSUPPORTED_CONTENT_SENTINEL: &str = "[unsupported Jira content]";
const UNSUPPORTED_CONTENT_LABEL: &str = "Some Jira content isn't supported yet.";
const UNAVAILABLE_IMAGE_SENTINEL: &str = "[Jira image unavailable]";
const UNAVAILABLE_IMAGE_LABEL: &str = "Image unavailable.";

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RichTextPalette {
    pub foreground: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub code_surface: Hsla,
    pub link: Hsla,
    pub info: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
}

/// Callback used by the presentation layer to perform a safe attachment action.
///
/// The renderer deliberately emits only the attachment ID. The caller owns the
/// authenticated download flow and resolves that ID against the loaded issue detail.
#[derive(Clone)]
pub(crate) struct RichAttachmentCardAction(Rc<RichAttachmentActionHandler>);

type RichAttachmentActionHandler = dyn Fn(&str, &mut Window, &mut App);

impl RichAttachmentCardAction {
    pub(crate) fn new(handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        Self(Rc::new(handler))
    }

    fn invoke(&self, attachment_id: &str, window: &mut Window, cx: &mut App) {
        (self.0)(attachment_id, window, cx);
    }
}

#[derive(Clone)]
struct RenderContext<'a> {
    palette: RichTextPalette,
    image_states: &'a RichImageRenderStates,
    surface_ordinal: usize,
    source: DiagnosticImageSource,
    attachment_action: Option<RichAttachmentCardAction>,
}

impl<'a> RenderContext<'a> {
    fn with_source(&self, source: DiagnosticImageSource) -> Self {
        Self {
            source,
            ..self.clone()
        }
    }
}

pub(crate) fn render_rich_text(
    document: &RichTextDocument,
    palette: RichTextPalette,
    image_states: &RichImageRenderStates,
    surface_ordinal: usize,
    source: DiagnosticImageSource,
) -> AnyElement {
    render_rich_text_with_actions(
        document,
        palette,
        image_states,
        surface_ordinal,
        source,
        None,
    )
}

pub(crate) fn render_rich_text_with_actions(
    document: &RichTextDocument,
    palette: RichTextPalette,
    image_states: &RichImageRenderStates,
    surface_ordinal: usize,
    source: DiagnosticImageSource,
    attachment_action: Option<RichAttachmentCardAction>,
) -> AnyElement {
    let context = RenderContext {
        palette,
        image_states,
        surface_ordinal,
        source,
        attachment_action,
    };
    let mut budget = RenderBudget::default();
    let mut blocks = Vec::new();
    for block in document.blocks.iter().take(MAX_RENDER_CHILDREN) {
        blocks.push(render_block(block, &context, 0, &mut budget));
        if budget.omitted {
            break;
        }
    }
    if document.blocks.len() > MAX_RENDER_CHILDREN {
        budget.omitted = true;
    }

    let mut content = v_flex().min_w_0().gap_3().children(blocks);
    if document.truncated || budget.omitted {
        content = content.child(div().text_xs().text_color(context.palette.muted).child(
            if document.truncated {
                "Content truncated by Jira Desk."
            } else {
                RENDER_OMITTED_LABEL
            },
        ));
    }
    if !document.fallback_images.is_empty() && !budget.omitted {
        let mut gallery = Vec::new();
        for image in document
            .fallback_images
            .iter()
            .take(RichTextDocument::MAX_FALLBACK_IMAGES)
        {
            if !budget.enter(0) {
                break;
            }
            gallery.push(render_image(
                image,
                &context.with_source(DiagnosticImageSource::FallbackCandidate),
                &mut budget,
            ));
        }
        if !gallery.is_empty() {
            content = content.child(
                v_flex()
                    .min_w_0()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(context.palette.foreground)
                            .child(FALLBACK_IMAGE_GALLERY_LABEL),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(context.palette.muted)
                            .child(FALLBACK_IMAGE_GALLERY_NOTE),
                    )
                    .children(gallery),
            );
        }
    }
    content.into_any_element()
}

mod block;
mod budget;
mod image;
mod inline;

use block::render_block;
use budget::{RenderBudget, render_element_ordinal};
use image::render_image;

#[cfg(test)]
use budget::insert_soft_wraps;
#[cfg(test)]
use image::{image_render_state, rich_image_name};
#[cfg(test)]
use inline::{
    bounded_attachment_filename, bounded_inline_content, inline_line_count, inline_text_flow,
    normalize_attachment_filename, render_inlines,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HeadingSize {
    TwoXl,
    Xl,
    Lg,
    Base,
    Sm,
}

fn omitted_element(palette: RichTextPalette) -> AnyElement {
    div()
        .min_w_0()
        .text_xs()
        .italic()
        .text_color(palette.muted)
        .child(RENDER_OMITTED_LABEL)
        .into_any_element()
}

/// Translate transport-layer sentinels into calm presentation copy without changing
/// generic placeholder labels or the domain model's durable representation.
fn presentation_placeholder_label(label: &str) -> &str {
    match label {
        UNSUPPORTED_CONTENT_SENTINEL => UNSUPPORTED_CONTENT_LABEL,
        UNAVAILABLE_IMAGE_SENTINEL => UNAVAILABLE_IMAGE_LABEL,
        _ => label,
    }
}

fn heading_size(level: u8) -> HeadingSize {
    match level {
        1 => HeadingSize::TwoXl,
        2 => HeadingSize::Xl,
        3 => HeadingSize::Lg,
        4 => HeadingSize::Base,
        _ => HeadingSize::Sm,
    }
}

fn panel_accent(kind: PanelKind, palette: RichTextPalette) -> Hsla {
    match kind {
        PanelKind::Info => palette.info,
        PanelKind::Note => palette.muted,
        PanelKind::Warning => palette.warning,
        PanelKind::Success => palette.success,
        PanelKind::Error => palette.danger,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use jira_domain::{
        RichAttachmentCard, RichBlock, RichImage, RichInline, RichMark, RichTextDocument,
    };

    use super::{
        HeadingSize, MAX_RENDER_CHILDREN, MAX_RENDER_DEPTH, MAX_RENDER_NODES,
        MAX_RENDER_TEXT_BYTES, RenderBudget, RichAttachmentCardAction, RichImageRenderState,
        RichImageRenderStates, RichTextPalette, UNAVAILABLE_IMAGE_LABEL,
        UNAVAILABLE_IMAGE_SENTINEL, UNSUPPORTED_CONTENT_LABEL, UNSUPPORTED_CONTENT_SENTINEL,
        bounded_attachment_filename, bounded_inline_content, heading_size, image_render_state,
        inline_line_count, inline_text_flow, normalize_attachment_filename,
        presentation_placeholder_label, render_element_ordinal, render_rich_text,
        render_rich_text_with_actions, rich_image_name,
    };
    use crate::diagnostics::{
        DecodeFallbackReason, DiagnosticEvent, DiagnosticFlow, DiagnosticsSink, ImageSource,
        ImageStateReason,
    };

    #[test]
    fn heading_levels_have_stable_visual_scale() {
        assert_eq!(heading_size(1), HeadingSize::TwoXl);
        assert_eq!(heading_size(2), HeadingSize::Xl);
        assert_eq!(heading_size(3), HeadingSize::Lg);
        assert_eq!(heading_size(4), HeadingSize::Base);
        assert_eq!(heading_size(5), HeadingSize::Sm);
        assert_eq!(heading_size(6), HeadingSize::Sm);
        assert_eq!(heading_size(0), HeadingSize::Sm);
    }

    #[test]
    fn presentation_placeholder_labels_translate_domain_sentinels() {
        assert_eq!(
            presentation_placeholder_label(UNSUPPORTED_CONTENT_SENTINEL),
            UNSUPPORTED_CONTENT_LABEL
        );
        assert_eq!(
            presentation_placeholder_label(UNAVAILABLE_IMAGE_SENTINEL),
            UNAVAILABLE_IMAGE_LABEL
        );
        assert!(!presentation_placeholder_label(UNSUPPORTED_CONTENT_SENTINEL).contains('['));
        assert!(!presentation_placeholder_label(UNAVAILABLE_IMAGE_SENTINEL).contains('['));
    }

    #[test]
    fn presentation_placeholder_labels_preserve_generic_labels() {
        for label in ["Status", "", "[custom placeholder]"] {
            assert_eq!(presentation_placeholder_label(label), label);
        }
    }

    #[test]
    fn render_element_ordinals_are_distinct_per_surface_and_bounded() {
        assert_ne!(render_element_ordinal(0, 12), render_element_ordinal(1, 12));
        assert_eq!(
            render_element_ordinal(0, MAX_RENDER_NODES + 1),
            MAX_RENDER_NODES
        );
        assert_eq!(render_element_ordinal(usize::MAX, usize::MAX), usize::MAX);
    }

    #[test]
    fn hard_breaks_form_distinct_inline_lines() {
        let content = [
            RichInline::Text {
                text: "before".to_owned(),
                marks: Vec::new(),
            },
            RichInline::HardBreak,
            RichInline::Text {
                text: "after".to_owned(),
                marks: Vec::new(),
            },
        ];
        assert_eq!(inline_line_count(&content), 2);
    }

    #[test]
    fn text_runs_keep_marked_adf_children_on_one_wrapping_surface() {
        let content = [
            RichInline::Text {
                text: "Ticket: ".to_owned(),
                marks: Vec::new(),
            },
            RichInline::Text {
                text: "IX-2247 (Task)".to_owned(),
                marks: vec![RichMark::Strong],
            },
            RichInline::Text {
                text: "  Epic: IX-898 — IX Platform - Triage".to_owned(),
                marks: vec![RichMark::Link {
                    href: "https://example.invalid/epic".to_owned(),
                    title: None,
                }],
            },
            RichInline::HardBreak,
            RichInline::Text {
                text: "Agency FMO upline and downline (existing orgs).".to_owned(),
                marks: Vec::new(),
            },
        ];

        let mut budget = RenderBudget::default();
        let flow = inline_text_flow(&content, RichTextPalette::default(), 0, &mut budget)
            .expect("text-only ADF paragraph should use one text flow");

        assert_eq!(
            flow.text,
            "Ticket: IX-2247 (Task)  Epic: IX-898 — IX Platform - Triage\nAgency FMO upline and downline (existing orgs)."
        );
        assert_eq!(flow.highlights.len(), 2);
        assert_eq!(flow.font_family_overrides.len(), 0);
    }

    #[test]
    fn inline_content_caps_work_and_omits_tail_nodes() {
        let content = (0..=MAX_RENDER_CHILDREN)
            .map(|index| RichInline::Text {
                text: if index == MAX_RENDER_CHILDREN {
                    "tail-must-not-render".to_owned()
                } else {
                    "x".to_owned()
                },
                marks: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut budget = RenderBudget::default();
        let (bounded, capped) = bounded_inline_content(&content);
        let flow = inline_text_flow(bounded, RichTextPalette::default(), 0, &mut budget)
            .expect("bounded text content should produce a flow");

        assert_eq!(bounded.len(), MAX_RENDER_CHILDREN);
        assert!(capped);
        assert!(!budget.omitted);
        assert_eq!(flow.text.len(), MAX_RENDER_CHILDREN);
        assert!(!flow.text.contains("tail-must-not-render"));

        let image_states = RichImageRenderStates::default();
        let context = super::RenderContext {
            palette: RichTextPalette::default(),
            image_states: &image_states,
            surface_ordinal: 0,
            source: ImageSource::ResolvedAdf,
            attachment_action: None,
        };
        let mut render_budget = RenderBudget::default();
        let _ = super::render_inlines(&content, &context, 0, &mut render_budget);
        assert!(render_budget.omitted);
    }

    #[test]
    fn attachment_filename_is_bounded_without_splitting_utf8() {
        let filename = format!("{}終", "x".repeat(super::MAX_ATTACHMENT_FILENAME_BYTES));
        let bounded = bounded_attachment_filename(&filename);

        assert!(bounded.len() <= super::MAX_ATTACHMENT_FILENAME_BYTES);
        assert!(bounded.ends_with('…'));
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn attachment_filename_normalizes_layout_breaking_metadata() {
        assert_eq!(
            normalize_attachment_filename("  App\n\tkey\u{0000} data 💾  ").0,
            "App key� data 💾"
        );
    }

    #[test]
    fn consecutive_inline_elements_consume_distinct_ordinals() {
        let mut budget = RenderBudget::default();
        let first = render_element_ordinal(2, budget.next_element_ordinal());
        let second = render_element_ordinal(2, budget.next_element_ordinal());
        assert_ne!(first, second);
    }

    #[test]
    fn very_large_attachment_filename_has_bounded_normalization_work_surface() {
        let filename = "x".repeat(8 * 1024 * 1024);
        let normalized = normalize_attachment_filename(&filename).0;
        let bounded = bounded_attachment_filename(&filename);

        assert!(normalized.len() <= super::MAX_ATTACHMENT_FILENAME_BYTES + 3);
        assert!(bounded.len() <= super::MAX_ATTACHMENT_FILENAME_BYTES);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn attachment_card_renders_through_rich_text_path() {
        let document = RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::AttachmentCard(
                RichAttachmentCard {
                    attachment_id: "10002".to_owned(),
                    filename: "App key data.csv".to_owned(),
                    mime_type: Some("text/csv".to_owned()),
                    size_bytes: Some(128),
                },
            )])],
            false,
        );
        let palette = RichTextPalette {
            foreground: gpui::Hsla::default(),
            muted: gpui::Hsla::default(),
            border: gpui::Hsla::default(),
            code_surface: gpui::Hsla::default(),
            link: gpui::Hsla::default(),
            info: gpui::Hsla::default(),
            warning: gpui::Hsla::default(),
            success: gpui::Hsla::default(),
            danger: gpui::Hsla::default(),
        };

        let rendered = render_rich_text(
            &document,
            palette,
            &RichImageRenderStates::default(),
            0,
            ImageSource::ResolvedAdf,
        );

        let _ = rendered;
    }

    #[test]
    fn attachment_card_renders_with_download_action() {
        let document = RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::AttachmentCard(
                RichAttachmentCard {
                    attachment_id: "10002".to_owned(),
                    filename: "App key data.csv".to_owned(),
                    mime_type: Some("text/csv".to_owned()),
                    size_bytes: Some(128),
                },
            )])],
            false,
        );
        let palette = RichTextPalette {
            foreground: gpui::Hsla::default(),
            muted: gpui::Hsla::default(),
            border: gpui::Hsla::default(),
            code_surface: gpui::Hsla::default(),
            link: gpui::Hsla::default(),
            info: gpui::Hsla::default(),
            warning: gpui::Hsla::default(),
            success: gpui::Hsla::default(),
            danger: gpui::Hsla::default(),
        };
        let action = RichAttachmentCardAction::new(|attachment_id, _, _| {
            assert_eq!(attachment_id, "10002");
        });
        let rendered = render_rich_text_with_actions(
            &document,
            palette,
            &RichImageRenderStates::default(),
            0,
            ImageSource::ResolvedAdf,
            Some(action.clone()),
        );

        let _ = rendered;
        assert!(std::mem::size_of_val(&action) > 0);
    }

    #[test]
    fn renderer_budget_limits_text_and_depth() {
        let mut budget = RenderBudget::default();
        let bounded = budget.text_nowrap(&"x".repeat(MAX_RENDER_TEXT_BYTES + 1));
        assert_eq!(bounded.len(), MAX_RENDER_TEXT_BYTES);
        assert!(budget.omitted);
        assert!(!budget.enter(MAX_RENDER_DEPTH + 1));
    }

    #[test]
    fn soft_wraps_split_long_unbroken_tokens_without_changing_visible_text() {
        let token = "x".repeat(65);
        let wrapped = super::insert_soft_wraps(&token);
        assert!(wrapped.contains('\u{200b}'));
        assert_eq!(wrapped.replace('\u{200b}', ""), token);
    }

    fn image(attachment_id: &str, filename: &str, alt_text: Option<&str>) -> RichImage {
        RichImage {
            attachment_id: attachment_id.to_owned(),
            filename: filename.to_owned(),
            mime_type: "image/png".to_owned(),
            alt_text: alt_text.map(str::to_owned),
            width: Some(640),
            height: Some(480),
        }
    }

    #[test]
    fn image_name_prefers_nonempty_alt_text_and_falls_back_to_filename() {
        assert_eq!(
            rich_image_name(&image("1", "diagram.png", Some("Architecture"))),
            "Architecture"
        );
        assert_eq!(
            rich_image_name(&image("1", "diagram.png", Some("  "))),
            "diagram.png"
        );
        assert_eq!(
            rich_image_name(&image("1", "diagram.png", None)),
            "diagram.png"
        );
    }

    #[test]
    fn image_state_lookup_is_scoped_to_attachment_id() {
        let first = image("first", "one.png", None);
        let second = image("second", "two.png", None);
        let states = RichImageRenderStates::from([(
            first.attachment_id.clone(),
            RichImageRenderState::Loading,
        )]);

        assert!(matches!(
            image_render_state(&states, &first),
            Some(RichImageRenderState::Loading)
        ));
        assert!(image_render_state(&states, &second).is_none());
    }

    #[test]
    fn image_nodes_consume_render_budget() {
        let mut budget = RenderBudget::default();
        assert!(budget.enter(0));
        assert_eq!(budget.nodes, 1);
    }

    #[test]
    fn ready_image_state_can_hold_decoded_in_memory_image() {
        let image = Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, Vec::new()));
        let state = RichImageRenderState::Ready(image);
        assert!(matches!(state, RichImageRenderState::Ready(_)));
    }

    #[test]
    fn clearing_image_states_preserves_missing_diagnostic_context() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("jira-rich-image-context-{nonce}"));
        let sink = DiagnosticsSink::for_directory(&root);
        let load_token = sink.begin_image_load();
        let mut states = RichImageRenderStates::with_context(
            sink.clone(),
            DiagnosticFlow::SelectedDetail,
            load_token,
        );
        states.clear();
        let context = states
            .context_for("missing", 0, 0, ImageSource::ResolvedAdf)
            .expect("default context");
        context.sink.record_once(DiagnosticEvent::image_state(
            context.flow,
            context.load_token,
            context.candidate_ordinal,
            context.surface_ordinal,
            context.source,
            ImageStateReason::Missing,
        ));
        let second_surface = states
            .context_for("missing", 0, 1, ImageSource::ResolvedAdf)
            .expect("second surface context");
        second_surface
            .sink
            .record_once(DiagnosticEvent::image_state(
                second_surface.flow,
                second_surface.load_token,
                second_surface.candidate_ordinal,
                second_surface.surface_ordinal,
                second_surface.source,
                ImageStateReason::Missing,
            ));
        second_surface
            .sink
            .record_once(DiagnosticEvent::image_state(
                second_surface.flow,
                second_surface.load_token,
                second_surface.candidate_ordinal,
                second_surface.surface_ordinal,
                second_surface.source,
                ImageStateReason::Missing,
            ));
        context.sink.record_once(DiagnosticEvent::image_state(
            context.flow,
            context.load_token,
            context.candidate_ordinal,
            context.surface_ordinal,
            context.source,
            ImageStateReason::Missing,
        ));
        context
            .sink
            .record_once(DiagnosticEvent::gpui_decode_fallback(
                context.flow,
                context.load_token,
                context.candidate_ordinal,
                context.surface_ordinal,
                context.source,
                DecodeFallbackReason::DecodeFailed,
            ));
        context
            .sink
            .record_once(DiagnosticEvent::gpui_decode_fallback(
                context.flow,
                context.load_token,
                context.candidate_ordinal,
                context.surface_ordinal,
                context.source,
                DecodeFallbackReason::DecodeFailed,
            ));
        let next_load_token = sink.begin_image_load();
        states.set_context(
            sink.clone(),
            DiagnosticFlow::SelectedDetail,
            next_load_token,
        );
        states.clear();
        let next_context = states
            .context_for("missing", 0, 0, ImageSource::ResolvedAdf)
            .expect("next default context");
        next_context.sink.record_once(DiagnosticEvent::image_state(
            next_context.flow,
            next_context.load_token,
            next_context.candidate_ordinal,
            next_context.surface_ordinal,
            next_context.source,
            ImageStateReason::Missing,
        ));
        next_context.sink.record_once(DiagnosticEvent::image_state(
            next_context.flow,
            next_context.load_token,
            next_context.candidate_ordinal,
            next_context.surface_ordinal,
            next_context.source,
            ImageStateReason::Missing,
        ));
        next_context
            .sink
            .record_once(DiagnosticEvent::gpui_decode_fallback(
                next_context.flow,
                next_context.load_token,
                next_context.candidate_ordinal,
                next_context.surface_ordinal,
                next_context.source,
                DecodeFallbackReason::DecodeFailed,
            ));
        next_context
            .sink
            .record_once(DiagnosticEvent::gpui_decode_fallback(
                next_context.flow,
                next_context.load_token,
                next_context.candidate_ordinal,
                next_context.surface_ordinal,
                next_context.source,
                DecodeFallbackReason::DecodeFailed,
            ));
        let log = fs::read_to_string(root.join("diagnostics.jsonl")).expect("diagnostics");
        assert_eq!(log.lines().count(), 5);
        assert!(log.contains("\"reason\":\"missing\""));
        assert!(log.contains(&format!("\"load_token\":{}", load_token)));
        assert!(log.contains(&format!("\"load_token\":{}", next_load_token)));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn fallback_gallery_is_labeled_as_unresolved_candidates() {
        assert_eq!(super::FALLBACK_IMAGE_GALLERY_LABEL, "Image attachments");
        assert!(super::FALLBACK_IMAGE_GALLERY_NOTE.contains("exact placement unavailable"));
        assert!(
            super::RichTextDocument::new(Vec::new(), false)
                .fallback_images
                .is_empty()
        );
    }
}
