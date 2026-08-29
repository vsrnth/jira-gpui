//! Inline rich-text rendering, mark styling, and attachment cards.

use super::{
    MAX_ATTACHMENT_FILENAME_BYTES, MAX_ATTACHMENT_LOOKAHEAD_BYTES, MAX_RENDER_CHILDREN,
    RenderBudget, RenderContext, RichAttachmentCard, RichInline, RichMark, RichTextPalette,
    presentation_placeholder_label, render_element_ordinal,
};
use gpui::{
    AnyElement, ElementId, FontStyle, FontWeight, HighlightStyle, IntoElement as _,
    ParentElement as _, SharedString, StrikethroughStyle, Styled as _, StyledText, UnderlineStyle,
    div, rems,
};
use gpui_component::{Icon, IconName, StyledExt as _, button::Button, h_flex};
use std::ops::Range;

pub(super) fn render_inline_line(
    content: &[RichInline],
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    div()
        .min_w_0()
        .text_sm()
        .text_color(context.palette.foreground)
        .child(render_inlines(content, context, depth, budget))
        .into_any_element()
}

pub(super) fn render_inlines(
    content: &[RichInline],
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let (content, content_was_capped) = bounded_inline_content(content);
    let rendered = if !content
        .iter()
        .any(|inline| matches!(inline, RichInline::AttachmentCard(_)))
    {
        let flow = inline_text_flow(content, context.palette, depth, budget)
            .expect("text-only inline content should produce a text flow");
        render_inline_text_flow(flow)
    } else {
        // Attachment cards are real GPUI elements, so keep them as flex children while
        // grouping all surrounding text into a single wrapping surface. This preserves
        // inline text flow without making cards part of StyledText's text runs.
        let mut children = Vec::new();
        let mut text_start = 0;
        for (index, inline) in content.iter().enumerate() {
            if !matches!(inline, RichInline::AttachmentCard(_)) {
                continue;
            }
            if text_start < index
                && let Some(flow) =
                    inline_text_flow(&content[text_start..index], context.palette, depth, budget)
            {
                children.push(render_inline_text_flow(flow));
            }
            if !budget.enter(depth.saturating_add(1)) {
                break;
            }
            children.push(render_inline(inline, context, budget));
            text_start = index.saturating_add(1);
        }
        if !budget.omitted
            && text_start < content.len()
            && let Some(flow) =
                inline_text_flow(&content[text_start..], context.palette, depth, budget)
        {
            children.push(render_inline_text_flow(flow));
        }

        h_flex()
            .min_w_0()
            .flex_wrap()
            .gap_0()
            .children(children)
            .into_any_element()
    };
    if content_was_capped {
        budget.omitted = true;
    }
    rendered
}

pub(super) fn bounded_inline_content(content: &[RichInline]) -> (&[RichInline], bool) {
    let bounded_len = content.len().min(MAX_RENDER_CHILDREN);
    (&content[..bounded_len], content.len() > bounded_len)
}

pub(super) struct InlineTextFlow {
    pub(super) text: String,
    pub(super) highlights: Vec<(Range<usize>, HighlightStyle)>,
    pub(super) font_family_overrides: Vec<(Range<usize>, SharedString)>,
}

pub(super) fn inline_text_flow(
    content: &[RichInline],
    palette: RichTextPalette,
    depth: usize,
    budget: &mut RenderBudget,
) -> Option<InlineTextFlow> {
    let mut flow = InlineTextFlow {
        text: String::new(),
        highlights: Vec::new(),
        font_family_overrides: Vec::new(),
    };

    for inline in content {
        if !budget.enter(depth.saturating_add(1)) {
            break;
        }
        match inline {
            RichInline::Text { text, marks } => {
                append_inline_text(
                    &mut flow,
                    &budget.text(text),
                    inline_mark_style(marks, palette),
                    marks.iter().any(|mark| matches!(mark, RichMark::Code)),
                );
            }
            RichInline::Mention { label, .. } => {
                let label = if label.trim().is_empty() {
                    "Mention".to_owned()
                } else {
                    budget.text(label)
                };
                append_inline_text(
                    &mut flow,
                    &label,
                    HighlightStyle {
                        color: Some(palette.info),
                        ..Default::default()
                    },
                    false,
                );
            }
            RichInline::Placeholder { label } => append_inline_text(
                &mut flow,
                &budget.text(presentation_placeholder_label(label)),
                HighlightStyle {
                    color: Some(palette.muted),
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
                false,
            ),
            RichInline::HardBreak => flow.text.push('\n'),
            RichInline::AttachmentCard(_) => return None,
        }
        if budget.omitted {
            break;
        }
    }

    Some(flow)
}

fn append_inline_text(
    flow: &mut InlineTextFlow,
    text: &str,
    style: HighlightStyle,
    monospace: bool,
) {
    if text.is_empty() {
        return;
    }
    let start = flow.text.len();
    flow.text.push_str(text);
    let end = flow.text.len();
    if style != HighlightStyle::default() {
        flow.highlights.push((start..end, style));
    }
    if monospace {
        flow.font_family_overrides
            .push((start..end, "monospace".into()));
    }
}

fn inline_mark_style(marks: &[RichMark], palette: RichTextPalette) -> HighlightStyle {
    let mut style = HighlightStyle::default();
    for mark in marks {
        style = style.highlight(match mark {
            RichMark::Code => HighlightStyle {
                background_color: Some(palette.code_surface),
                ..Default::default()
            },
            RichMark::Emphasis => HighlightStyle {
                font_style: Some(FontStyle::Italic),
                ..Default::default()
            },
            RichMark::Strong => HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
            RichMark::Strike => HighlightStyle {
                strikethrough: Some(StrikethroughStyle::default()),
                ..Default::default()
            },
            RichMark::Link { .. } => HighlightStyle {
                color: Some(palette.link),
                underline: Some(UnderlineStyle {
                    color: Some(palette.link),
                    ..Default::default()
                }),
                ..Default::default()
            },
        });
    }
    style
}

fn render_inline_text_flow(flow: InlineTextFlow) -> AnyElement {
    let mut text = StyledText::new(flow.text);
    if !flow.highlights.is_empty() {
        text = text.with_highlights(flow.highlights);
    }
    if !flow.font_family_overrides.is_empty() {
        text = text.with_font_family_overrides(flow.font_family_overrides);
    }
    // StyledText performs wrapping in the enclosing constrained block, so a
    // marked/unmarked ADF run cannot become an independently shrinking flex item.
    div()
        .min_w_0()
        .whitespace_normal()
        .child(text)
        .into_any_element()
}

#[cfg(test)]
pub(super) fn inline_line_count(content: &[RichInline]) -> usize {
    content
        .iter()
        .filter(|inline| matches!(inline, RichInline::HardBreak))
        .count()
        .saturating_add(1)
}

fn render_inline(
    inline: &RichInline,
    context: &RenderContext<'_>,
    budget: &mut RenderBudget,
) -> AnyElement {
    match inline {
        RichInline::Text { text, marks } => {
            let mut element = div().min_w_0().whitespace_normal().child(budget.text(text));
            for mark in marks {
                element = match mark {
                    RichMark::Code => element
                        .font_family("monospace")
                        .bg(context.palette.code_surface)
                        .px_1()
                        .rounded(rems(0.125)),
                    RichMark::Emphasis => element.italic(),
                    RichMark::Strong => element.font_bold(),
                    RichMark::Strike => element.line_through(),
                    // Safe hrefs are intentionally not activated here: this
                    // adapter has no existing opener contract to delegate to.
                    RichMark::Link { .. } => element
                        .text_color(context.palette.link)
                        .underline()
                        .text_decoration_color(context.palette.link),
                };
            }
            element.into_any_element()
        }
        RichInline::Mention { label, .. } => div()
            .text_color(context.palette.info)
            .child(if label.trim().is_empty() {
                "Mention".to_owned()
            } else {
                budget.text(label)
            })
            .into_any_element(),
        RichInline::AttachmentCard(card) => render_attachment_card(card, context, budget),
        RichInline::Placeholder { label } => div()
            .italic()
            .text_color(context.palette.muted)
            .child(budget.text(presentation_placeholder_label(label)))
            .into_any_element(),
        RichInline::HardBreak => div().into_any_element(),
    }
}

/// Render an ADF inline attachment card as either an inert chip or an action button.
///
/// The domain model deliberately carries only attachment metadata. The optional action
/// keeps authenticated download behavior in the presentation layer while making the
/// inline card discoverable and keyboard-accessible when that layer supplies one.
fn render_attachment_card(
    card: &RichAttachmentCard,
    context: &RenderContext<'_>,
    budget: &mut RenderBudget,
) -> AnyElement {
    let filename = budget.text(&bounded_attachment_filename(&card.filename));
    let attachment_id = card.attachment_id.clone();
    if let Some(action) = context.attachment_action.clone() {
        return Button::new(ElementId::named_usize(
            "rich-attachment-card",
            render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal()),
        ))
        .compact()
        .outline()
        .max_w_full()
        .max_w(rems(32.5))
        .overflow_hidden()
        .icon(IconName::File)
        .label(filename)
        .tooltip("Download attachment")
        .on_click(move |_, window, cx| action.invoke(&attachment_id, window, cx))
        .into_any_element();
    }

    h_flex()
        .min_w_0()
        .max_w(rems(32.5))
        .gap_1()
        .px_1()
        .py(rems(0.0625))
        .rounded(rems(0.1875))
        .border_1()
        .border_color(context.palette.border)
        .bg(context.palette.code_surface)
        .child(Icon::new(IconName::File).text_color(context.palette.muted))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_sm()
                .child(filename),
        )
        .into_any_element()
}

pub(super) fn bounded_attachment_filename(value: &str) -> String {
    let (value, truncated) = normalize_attachment_filename(value);
    if value.is_empty() {
        return "Unnamed attachment".to_owned();
    }
    if !truncated && value.len() <= MAX_ATTACHMENT_FILENAME_BYTES {
        return value;
    }

    let ellipsis = '…';
    let mut end = MAX_ATTACHMENT_FILENAME_BYTES.saturating_sub(ellipsis.len_utf8());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = value[..end].to_owned();
    bounded.push(ellipsis);
    bounded
}

/// Keep filenames readable and layout-safe without dropping printable Unicode. Jira can
/// legally return unusual metadata, and a newline or control character must not turn an
/// inline chip into a multi-line/broken description surface.
pub(super) fn normalize_attachment_filename(value: &str) -> (String, bool) {
    let mut normalized = String::with_capacity(value.len().min(MAX_ATTACHMENT_FILENAME_BYTES));
    let mut pending_space = false;
    let mut scanned_bytes = 0usize;
    let mut truncated = false;
    for character in value.chars() {
        let character_bytes = character.len_utf8();
        if scanned_bytes.saturating_add(character_bytes)
            > MAX_ATTACHMENT_FILENAME_BYTES.saturating_add(MAX_ATTACHMENT_LOOKAHEAD_BYTES)
            || normalized.len() >= MAX_ATTACHMENT_FILENAME_BYTES
        {
            truncated = true;
            break;
        }
        scanned_bytes += character_bytes;
        if character.is_whitespace() {
            if !normalized.is_empty() {
                pending_space = true;
            }
        } else if character.is_control() {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.push('\u{FFFD}');
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.push(character);
        }
    }
    (normalized, truncated)
}
