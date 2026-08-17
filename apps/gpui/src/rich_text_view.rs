//! Small, bounded rich-text renderer for the GPUI adapter.
//!
//! The domain layer has already discarded raw ADF/JSON, media nodes, and
//! untrusted mention identifiers. This module only turns that safe projection
//! into ordinary GPUI elements; links remain visibly styled but inert.

use gpui::{AnyElement, Hsla, IntoElement, ParentElement as _, Styled as _, div, px};
use gpui_component::{StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use jira_domain::{PanelKind, RichBlock, RichInline, RichListItem, RichMark, RichTextDocument};

// Cached models can be deserialized without passing through the Jira ADF
// parser. Keep rendering bounded independently of the domain projection's
// plain-text limit, including for adversarially deep nested lists/panels.
const MAX_RENDER_DEPTH: usize = 32;
const MAX_RENDER_NODES: usize = 4_096;
const MAX_RENDER_CHILDREN: usize = 1_024;
const MAX_RENDER_TEXT_BYTES: usize = 1_000_000;
const RENDER_OMITTED_LABEL: &str = "Some content was omitted by Jira Desk.";

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
) -> AnyElement {
    let mut budget = RenderBudget::default();
    let mut blocks = Vec::new();
    for block in document.blocks.iter().take(MAX_RENDER_CHILDREN) {
        blocks.push(render_block(block, palette, 0, &mut budget));
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
        RichBlock::BulletList(items) => render_list(items, None, palette, depth, budget),
        RichBlock::OrderedList { order, items } => {
            render_list(items, Some(*order), palette, depth, budget)
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
            .children(render_blocks(content, palette, depth, budget))
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
                .children(render_blocks(content, palette, depth, budget))
                .into_any_element()
        }
        RichBlock::Placeholder { label } => div()
            .min_w_0()
            .text_sm()
            .italic()
            .text_color(palette.muted)
            .child(budget.text(label))
            .into_any_element(),
    }
}

fn render_blocks(
    blocks: &[RichBlock],
    palette: RichTextPalette,
    depth: usize,
    budget: &mut RenderBudget,
) -> Vec<AnyElement> {
    let mut rendered = Vec::new();
    for block in blocks.iter().take(MAX_RENDER_CHILDREN) {
        rendered.push(render_block(
            block,
            palette,
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
    use jira_domain::RichInline;

    use super::{
        HeadingSize, MAX_RENDER_DEPTH, MAX_RENDER_TEXT_BYTES, RenderBudget, heading_size,
        inline_line_count,
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
}
