//! Small, bounded rich-text renderer for the GPUI adapter.
//!
//! The domain layer has already discarded raw ADF/JSON and untrusted mention
//! identifiers, projecting media to bounded image metadata. This module only
//! turns that safe projection into ordinary GPUI elements; links remain visibly
//! styled but inert.

use std::{collections::HashMap, sync::Arc};

use gpui::{
    AnyElement, Hsla, Image, ImageSource, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, StyledImage as _, div, img,
    px,
};
use gpui_component::{
    StyledExt as _, h_flex, scroll::ScrollableElement as _, spinner::Spinner, v_flex,
};
use jira_domain::{
    PanelKind, RichBlock, RichImage, RichInline, RichListItem, RichMark, RichTextDocument,
};

// Cached models can be deserialized without passing through the Jira ADF
// parser. Keep rendering bounded independently of the domain projection's
// plain-text limit, including for adversarially deep nested lists/panels.
const MAX_RENDER_DEPTH: usize = 32;
const MAX_RENDER_NODES: usize = 4_096;
const MAX_RENDER_CHILDREN: usize = 1_024;
const MAX_RENDER_TEXT_BYTES: usize = 1_000_000;
const MAX_IMAGE_LABEL_BYTES: usize = 512;
const MAX_IMAGE_HEIGHT: f32 = 720.;
const RENDER_OMITTED_LABEL: &str = "Some content was omitted by Jira Desk.";

/// The application-owned state for a Jira attachment image.
///
/// The renderer only accepts already-authenticated, decoded in-memory images. It never
/// turns attachment metadata into a URI or performs a fetch itself.
#[derive(Clone)]
pub(crate) enum RichImageRenderState {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

pub(crate) type RichImageRenderStates = HashMap<String, RichImageRenderState>;

#[derive(Clone, Copy, Debug)]
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

pub(crate) fn render_rich_text(
    document: &RichTextDocument,
    palette: RichTextPalette,
    image_states: &RichImageRenderStates,
) -> AnyElement {
    let mut budget = RenderBudget::default();
    let mut blocks = Vec::new();
    for block in document.blocks.iter().take(MAX_RENDER_CHILDREN) {
        blocks.push(render_block(block, palette, image_states, 0, &mut budget));
        if budget.omitted {
            break;
        }
    }
    if document.blocks.len() > MAX_RENDER_CHILDREN {
        budget.omitted = true;
    }

    let mut content = v_flex().min_w_0().gap_3().children(blocks);
    if document.truncated || budget.omitted {
        content = content.child(div().text_xs().text_color(palette.muted).child(
            if document.truncated {
                "Content truncated by Jira Desk."
            } else {
                RENDER_OMITTED_LABEL
            },
        ));
    }
    content.into_any_element()
}

fn render_block(
    block: &RichBlock,
    palette: RichTextPalette,
    image_states: &RichImageRenderStates,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    if !budget.enter(depth) {
        return omitted_element(palette);
    }

    match block {
        RichBlock::Paragraph(content) => render_inline_line(content, palette, depth, budget),
        RichBlock::Heading { level, content } => {
            let element = div()
                .min_w_0()
                .font_semibold()
                .text_color(palette.foreground)
                .child(render_inlines(content, palette, depth, budget));
            match heading_size(*level) {
                HeadingSize::TwoXl => element.text_2xl().into_any_element(),
                HeadingSize::Xl => element.text_xl().into_any_element(),
                HeadingSize::Lg => element.text_lg().into_any_element(),
                HeadingSize::Base => element.text_base().into_any_element(),
                HeadingSize::Sm => element.text_sm().into_any_element(),
            }
        }
        RichBlock::BulletList(items) => {
            render_list(items, None, palette, image_states, depth, budget)
        }
        RichBlock::OrderedList { order, items } => {
            render_list(items, Some(*order), palette, image_states, depth, budget)
        }
        RichBlock::CodeBlock { language, text } => {
            let mut code = v_flex()
                .min_w_0()
                .gap_1()
                .p_3()
                .rounded(px(4.))
                .border_1()
                .border_color(palette.border)
                .bg(palette.code_surface)
                .text_sm()
                .text_color(palette.foreground)
                .font_family("monospace");
            if let Some(language) = language {
                code = code.child(
                    div()
                        .text_xs()
                        .text_color(palette.muted)
                        .child(budget.text(language)),
                );
            }
            code.child(div().whitespace_normal().child(budget.text_nowrap(text)))
                .overflow_x_scrollbar()
                .into_any_element()
        }
        RichBlock::BlockQuote(content) => v_flex()
            .min_w_0()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_color(palette.muted)
            .children(render_blocks(content, palette, image_states, depth, budget))
            .into_any_element(),
        RichBlock::Panel { kind, content } => {
            let accent = panel_accent(*kind, palette);
            v_flex()
                .min_w_0()
                .gap_2()
                .p_3()
                .rounded(px(4.))
                .border_1()
                .border_color(accent)
                .bg(accent.opacity(0.08))
                .children(render_blocks(content, palette, image_states, depth, budget))
                .into_any_element()
        }
        RichBlock::Image(image) => render_image(image, palette, image_states, budget),
        RichBlock::Placeholder { label } => div()
            .min_w_0()
            .text_sm()
            .italic()
            .text_color(palette.muted)
            .child(budget.text(label))
            .into_any_element(),
    }
}

fn render_image(
    image: &RichImage,
    palette: RichTextPalette,
    image_states: &RichImageRenderStates,
    budget: &mut RenderBudget,
) -> AnyElement {
    let name = bounded_image_name(rich_image_name(image));
    let accessible_label = budget.text(&format!("Image: {name}"));
    let mut frame = v_flex()
        .min_w_0()
        .max_w_full()
        .gap_2()
        .rounded(px(6.))
        .border_1()
        .border_color(palette.border)
        // The ID is internal GPUI bookkeeping, never rendered or exposed as an
        // accessibility label. The budget ordinal makes repeated attachments
        // unique within this render pass.
        .id(format!(
            "rich-image-{}-{}",
            image.attachment_id, budget.nodes
        ))
        .aria_label(accessible_label);

    match image_render_state(image_states, image) {
        Some(RichImageRenderState::Ready(image)) => {
            let unavailable = format!("Image unavailable · {name}");
            let loading_color = palette.muted;
            let fallback_color = palette.muted;
            frame = frame.child(
                img(ImageSource::Image(image.clone()))
                    .max_w_full()
                    .max_h(px(MAX_IMAGE_HEIGHT))
                    .object_fit(ObjectFit::Contain)
                    // ImageSource::Image is already in memory, but GPUI may
                    // still decode it on the render path. Keep that fallback
                    // visible without adding a second animated spinner beside
                    // the source-state Loading view.
                    .with_loading(move || {
                        div()
                            .text_xs()
                            .text_color(loading_color)
                            .child("Loading image…")
                            .into_any_element()
                    })
                    .with_fallback(move || {
                        div()
                            .text_xs()
                            .text_color(fallback_color)
                            .child(unavailable.clone())
                            .into_any_element()
                    }),
            );
        }
        Some(RichImageRenderState::Loading) => {
            frame = frame.child(
                h_flex()
                    .min_h(px(72.))
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Spinner::new())
                    .child("Loading image…"),
            );
        }
        Some(RichImageRenderState::Failed) | None => {
            let unavailable = format!("Image unavailable · {name}");
            frame = frame.child(
                div()
                    .min_h(px(72.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(palette.muted)
                    .child(budget.text(&unavailable)),
            );
        }
    }

    frame
        .child(
            div()
                .text_xs()
                .text_color(palette.muted)
                .child(budget.text(&name)),
        )
        .into_any_element()
}

fn rich_image_name(image: &RichImage) -> &str {
    image
        .alt_text
        .as_deref()
        .filter(|alt| !alt.trim().is_empty())
        .unwrap_or(image.filename.as_str())
}

fn bounded_image_name(value: &str) -> String {
    let mut end = value.len().min(MAX_IMAGE_LABEL_BYTES);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn image_render_state<'a>(
    image_states: &'a RichImageRenderStates,
    image: &RichImage,
) -> Option<&'a RichImageRenderState> {
    image_states.get(&image.attachment_id)
}

fn render_blocks(
    blocks: &[RichBlock],
    palette: RichTextPalette,
    image_states: &RichImageRenderStates,
    depth: usize,
    budget: &mut RenderBudget,
) -> Vec<AnyElement> {
    let mut rendered = Vec::new();
    for block in blocks.iter().take(MAX_RENDER_CHILDREN) {
        rendered.push(render_block(
            block,
            palette,
            image_states,
            depth.saturating_add(1),
            budget,
        ));
        if budget.omitted {
            break;
        }
    }
    if blocks.len() > MAX_RENDER_CHILDREN {
        budget.omitted = true;
    }
    rendered
}

fn render_list(
    items: &[RichListItem],
    order: Option<u32>,
    palette: RichTextPalette,
    image_states: &RichImageRenderStates,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let mut rows = Vec::new();
    for (index, item) in items.iter().take(MAX_RENDER_CHILDREN).enumerate() {
        if !budget.enter(depth.saturating_add(1)) {
            break;
        }
        let marker = match order {
            Some(order) => format!("{}.", order.saturating_add(index as u32)),
            None => "•".to_owned(),
        };
        rows.push(
            h_flex()
                .min_w_0()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .w(px(18.))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(palette.muted)
                        .text_right()
                        .child(marker),
                )
                .child(v_flex().min_w_0().flex_1().gap_2().children(render_blocks(
                    &item.blocks,
                    palette,
                    image_states,
                    depth,
                    budget,
                )))
                .into_any_element(),
        );
        if budget.omitted {
            break;
        }
    }
    if items.len() > MAX_RENDER_CHILDREN {
        budget.omitted = true;
    }
    v_flex().min_w_0().gap_2().children(rows).into_any_element()
}

fn render_inline_line(
    content: &[RichInline],
    palette: RichTextPalette,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    div()
        .min_w_0()
        .text_sm()
        .text_color(palette.foreground)
        .child(render_inlines(content, palette, depth, budget))
        .into_any_element()
}

fn render_inlines(
    content: &[RichInline],
    palette: RichTextPalette,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let mut lines: Vec<Vec<AnyElement>> =
        Vec::with_capacity(inline_line_count(content).min(MAX_RENDER_CHILDREN + 1));
    lines.push(Vec::new());
    for inline in content.iter().take(MAX_RENDER_CHILDREN) {
        if !budget.enter(depth.saturating_add(1)) {
            break;
        }
        if matches!(inline, RichInline::HardBreak) {
            lines.push(Vec::new());
        } else if let Some(line) = lines.last_mut() {
            line.push(render_inline(inline, palette, budget));
        }
        if budget.omitted {
            break;
        }
    }
    if content.len() > MAX_RENDER_CHILDREN {
        budget.omitted = true;
    }
    v_flex()
        .min_w_0()
        .children(
            lines
                .into_iter()
                .map(|line| h_flex().min_w_0().flex_wrap().gap_0().children(line)),
        )
        .into_any_element()
}

fn inline_line_count(content: &[RichInline]) -> usize {
    content
        .iter()
        .filter(|inline| matches!(inline, RichInline::HardBreak))
        .count()
        .saturating_add(1)
}

fn render_inline(
    inline: &RichInline,
    palette: RichTextPalette,
    budget: &mut RenderBudget,
) -> AnyElement {
    match inline {
        RichInline::Text { text, marks } => {
            let mut element = div().min_w_0().whitespace_normal().child(budget.text(text));
            for mark in marks {
                element = match mark {
                    RichMark::Code => element
                        .font_family("monospace")
                        .bg(palette.code_surface)
                        .px_1()
                        .rounded(px(2.)),
                    RichMark::Emphasis => element.italic(),
                    RichMark::Strong => element.font_bold(),
                    RichMark::Strike => element.line_through(),
                    // Safe hrefs are intentionally not activated here: this
                    // adapter has no existing opener contract to delegate to.
                    RichMark::Link { .. } => element
                        .text_color(palette.link)
                        .underline()
                        .text_decoration_color(palette.link),
                };
            }
            element.into_any_element()
        }
        RichInline::Mention { label, .. } => div()
            .text_color(palette.info)
            .child(if label.trim().is_empty() {
                "Mention".to_owned()
            } else {
                budget.text(label)
            })
            .into_any_element(),
        RichInline::Placeholder { label } => div()
            .italic()
            .text_color(palette.muted)
            .child(budget.text(label))
            .into_any_element(),
        RichInline::HardBreak => div().into_any_element(),
    }
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

#[derive(Default)]
struct RenderBudget {
    nodes: usize,
    text_bytes: usize,
    omitted: bool,
}

impl RenderBudget {
    fn enter(&mut self, depth: usize) -> bool {
        if depth > MAX_RENDER_DEPTH || self.nodes >= MAX_RENDER_NODES {
            self.omitted = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    fn text(&mut self, value: &str) -> String {
        self.text_with_wrap(value, true)
    }

    fn text_nowrap(&mut self, value: &str) -> String {
        self.text_with_wrap(value, false)
    }

    fn text_with_wrap(&mut self, value: &str, soft_wrap: bool) -> String {
        let remaining = MAX_RENDER_TEXT_BYTES.saturating_sub(self.text_bytes);
        let mut end = value.len().min(remaining);
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        let result = value[..end].to_owned();
        self.text_bytes += result.len();
        if end < value.len() {
            self.omitted = true;
        }
        if soft_wrap {
            insert_soft_wraps(&result)
        } else {
            result
        }
    }
}

fn insert_soft_wraps(value: &str) -> String {
    const SOFT_WRAP_AFTER: usize = 64;
    let mut wrapped = String::with_capacity(value.len());
    let mut run = 0;
    for character in value.chars() {
        if character.is_whitespace() {
            run = 0;
        } else if run >= SOFT_WRAP_AFTER {
            wrapped.push('\u{200b}');
            run = 0;
        }
        wrapped.push(character);
        run += 1;
    }
    wrapped
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeadingSize {
    TwoXl,
    Xl,
    Lg,
    Base,
    Sm,
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
    use std::sync::Arc;

    use jira_domain::{RichImage, RichInline};

    use super::{
        HeadingSize, MAX_RENDER_DEPTH, MAX_RENDER_TEXT_BYTES, RenderBudget, RichImageRenderState,
        RichImageRenderStates, heading_size, image_render_state, inline_line_count,
        rich_image_name,
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
}
