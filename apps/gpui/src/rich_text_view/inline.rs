//! Inline rich-text rendering, mark styling, and attachment cards.

use super::{
    MAX_ATTACHMENT_FILENAME_BYTES, MAX_ATTACHMENT_LOOKAHEAD_BYTES, MAX_RENDER_CHILDREN,
    RenderBudget, RenderContext, RichAttachmentCard, RichInline, RichMark, RichStatusColor,
    RichTextPalette, presentation_placeholder_label, render_element_ordinal,
};
use gpui::{
    AnyElement, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    StrikethroughStyle, Styled as _, StyledText, UnderlineStyle, div, rems,
};
use gpui_component::{Icon, IconName, StyledExt as _, button::Button, h_flex, link::Link};
use std::ops::Range;

pub(super) fn render_inline_line(
    content: &[RichInline],
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let (bounded_content, content_was_capped) = bounded_inline_content(content);
    if !bounded_content.iter().any(inline_requires_element) {
        let flow = inline_text_flow(bounded_content, context.palette, depth, budget)
            .expect("text-only inline content should produce a text flow");
        let aria_label = flow.text.clone();
        if content_was_capped {
            budget.omitted = true;
        }
        let ordinal =
            render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal());
        return div()
            .min_w_0()
            .text_sm()
            .text_color(context.palette.foreground)
            .id(ElementId::named_usize("rich-text-paragraph", ordinal))
            .accessibility_id(format!("rich-text-paragraph-{ordinal}"))
            // AccessKit maps Label to macOS static text, which keeps the rendered paragraph
            // discoverable to native automation while retaining the bounded flow text.
            .role(gpui::accesskit::Role::Label)
            .aria_label(aria_label)
            .aria_value(flow.text.clone())
            .child(render_inline_text_flow(flow))
            .into_any_element();
    }

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
    let rendered = if !content.iter().any(inline_requires_element) {
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
            if !inline_requires_element(inline) {
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
            RichInline::Emoji { text } => append_inline_text(
                &mut flow,
                &budget.text(text),
                HighlightStyle::default(),
                false,
            ),
            RichInline::Date { date } => append_inline_text(
                &mut flow,
                &budget.text(date),
                HighlightStyle::default(),
                false,
            ),
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
            RichInline::Status { .. }
            | RichInline::AttachmentCard(_)
            | RichInline::JiraIssueLink(_) => return None,
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
            RichMark::Link { .. } | RichMark::JiraIssueLink { .. } => HighlightStyle {
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

fn inline_requires_element(inline: &RichInline) -> bool {
    match inline {
        RichInline::Status { .. }
        | RichInline::AttachmentCard(_)
        | RichInline::JiraIssueLink(_) => true,
        RichInline::Text { marks, .. } => marks
            .iter()
            .any(|mark| matches!(mark, RichMark::Link { .. } | RichMark::JiraIssueLink { .. })),
        _ => false,
    }
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

fn link_mark(mark: &RichMark) -> Option<&RichMark> {
    matches!(mark, RichMark::Link { .. } | RichMark::JiraIssueLink { .. }).then_some(mark)
}

fn render_link(
    text: &str,
    mark: &RichMark,
    marks: &[RichMark],
    context: &RenderContext<'_>,
    budget: &mut RenderBudget,
) -> AnyElement {
    let (href, accessibility_label) = match mark {
        RichMark::Link { href, title } => (
            href,
            title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Open link: {text}")),
        ),
        RichMark::JiraIssueLink {
            issue_key, href, ..
        } => (href, format!("Open Jira issue {issue_key}")),
        _ => unreachable!("render_link requires a parser-approved link mark"),
    };
    // Cached RichTextDocument values can be deserialized without the Jira parser;
    // re-check the browser target at the activation boundary before constructing
    // the GPUI Link element.
    if !super::safe_browser_url(href) {
        return div()
            .min_w_0()
            .whitespace_normal()
            .text_color(context.palette.link)
            .underline()
            .text_decoration_color(context.palette.link)
            .child(budget.text(text))
            .into_any_element();
    }
    let ordinal = render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal());
    let a11y_href = href.clone();
    let mut link = Link::new(ElementId::named_usize("rich-text-link", ordinal))
        .href(href.clone())
        .min_w_0()
        .whitespace_normal()
        .text_color(context.palette.link)
        .underline()
        .text_decoration_color(context.palette.link)
        .child(budget.text(text));
    for mark in marks {
        link = match mark {
            RichMark::Code => link
                .font_family("monospace")
                .bg(context.palette.code_surface)
                .px_1()
                .rounded(rems(0.125)),
            RichMark::Emphasis => link.italic(),
            RichMark::Strong => link.font_bold(),
            RichMark::Strike => link.line_through(),
            RichMark::Link { .. } | RichMark::JiraIssueLink { .. } => link,
        };
    }
    div()
        .id(ElementId::named_usize("rich-text-link-semantic", ordinal))
        .accessibility_id(format!("rich-text-link-{ordinal}"))
        .role(gpui::accesskit::Role::Link)
        .aria_label(accessibility_label)
        .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
            cx.open_url(&a11y_href);
        })
        .min_w_0()
        .child(link)
        .into_any_element()
}

fn render_inline(
    inline: &RichInline,
    context: &RenderContext<'_>,
    budget: &mut RenderBudget,
) -> AnyElement {
    match inline {
        RichInline::Text { text, marks } => {
            if let Some(link) = marks.iter().find_map(link_mark) {
                return render_link(text, link, marks, context, budget);
            }
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
                    RichMark::Link { .. } => element
                        .text_color(context.palette.link)
                        .underline()
                        .text_decoration_color(context.palette.link),
                    RichMark::JiraIssueLink { .. } => element,
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
        RichInline::Emoji { text } => div().min_w_0().child(budget.text(text)).into_any_element(),
        RichInline::Date { date } => div().min_w_0().child(budget.text(date)).into_any_element(),
        RichInline::Status { text, color } => render_status(text, *color, context, budget),
        RichInline::AttachmentCard(card) => render_attachment_card(card, context, budget),
        RichInline::JiraIssueLink(link) => render_link(
            &link.issue_key,
            &RichMark::JiraIssueLink {
                issue_key: link.issue_key.clone(),
                href: link.href.clone(),
                title: None,
            },
            &[],
            context,
            budget,
        ),
        RichInline::Placeholder { label } => div()
            .italic()
            .text_color(context.palette.muted)
            .child(budget.text(presentation_placeholder_label(label)))
            .into_any_element(),
        RichInline::HardBreak => div().into_any_element(),
    }
}

fn render_status(
    text: &str,
    color: RichStatusColor,
    context: &RenderContext<'_>,
    budget: &mut RenderBudget,
) -> AnyElement {
    let text = budget.text(text);
    let ordinal = render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal());
    let tone = status_tone(color, context.palette);
    h_flex()
        .id(ElementId::named_usize("rich-text-status", ordinal))
        .accessibility_id(format!("rich-text-status-{ordinal}"))
        .role(gpui::accesskit::Role::Label)
        .aria_label(text.clone())
        .aria_value(text.clone())
        .min_w_0()
        .gap_0p5()
        .px_1()
        .py(rems(0.0625))
        .rounded(rems(0.25))
        .border_1()
        .border_color(tone.opacity(0.4))
        .bg(tone.opacity(0.14))
        .text_color(tone)
        .text_xs()
        .child(text)
        .into_any_element()
}

fn status_tone(color: RichStatusColor, palette: RichTextPalette) -> gpui::Hsla {
    match color {
        RichStatusColor::Neutral => palette.muted,
        RichStatusColor::Purple | RichStatusColor::Blue => palette.info,
        RichStatusColor::Red => palette.danger,
        RichStatusColor::Yellow => palette.warning,
        RichStatusColor::Green => palette.success,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_same_color(actual: gpui::Hsla, expected: gpui::Hsla) {
        assert_eq!(actual.h, expected.h);
        assert_eq!(actual.s, expected.s);
        assert_eq!(actual.l, expected.l);
        assert_eq!(actual.a, expected.a);
    }

    #[test]
    fn status_colors_use_their_bounded_semantic_palette_tones() {
        let palette = RichTextPalette {
            muted: gpui::Hsla {
                h: 0.1,
                s: 0.2,
                l: 0.3,
                a: 1.0,
            },
            info: gpui::Hsla {
                h: 0.2,
                s: 0.3,
                l: 0.4,
                a: 1.0,
            },
            warning: gpui::Hsla {
                h: 0.3,
                s: 0.4,
                l: 0.5,
                a: 1.0,
            },
            success: gpui::Hsla {
                h: 0.4,
                s: 0.5,
                l: 0.6,
                a: 1.0,
            },
            danger: gpui::Hsla {
                h: 0.5,
                s: 0.6,
                l: 0.7,
                a: 1.0,
            },
            ..RichTextPalette::default()
        };
        for (color, expected) in [
            (RichStatusColor::Neutral, palette.muted),
            (RichStatusColor::Purple, palette.info),
            (RichStatusColor::Blue, palette.info),
            (RichStatusColor::Red, palette.danger),
            (RichStatusColor::Yellow, palette.warning),
            (RichStatusColor::Green, palette.success),
        ] {
            assert_same_color(status_tone(color, palette), expected);
        }
    }

    #[test]
    fn emoji_and_date_join_the_surrounding_text_flow() {
        let content = [
            RichInline::Text {
                text: "Release ".to_owned(),
                marks: Vec::new(),
            },
            RichInline::Emoji {
                text: "🚀".to_owned(),
            },
            RichInline::Text {
                text: " on ".to_owned(),
                marks: Vec::new(),
            },
            RichInline::Date {
                date: "2026-08-30".to_owned(),
            },
            RichInline::Text {
                text: "".to_owned(),
                marks: Vec::new(),
            },
        ];
        let mut budget = RenderBudget::default();
        let flow = inline_text_flow(&content, RichTextPalette::default(), 0, &mut budget)
            .expect("emoji/date content should stay in the text flow");

        assert_eq!(flow.text, "Release 🚀 on 2026-08-30");
        assert!(flow.highlights.is_empty());
        assert!(!budget.omitted);
    }
}
