//! Block-level rich-text rendering: paragraphs, lists, panels, and code blocks.

use super::image::render_image;
use super::inline::render_inline_line;
use super::inline::render_inlines;
use super::{
    HeadingSize, MAX_RENDER_CHILDREN, RenderBudget, RenderContext, RichBlock, RichListItem,
    heading_size, omitted_element, panel_accent, presentation_placeholder_label,
};
use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div, px};
use gpui_component::{StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};

pub(super) fn render_block(
    block: &RichBlock,
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    if !budget.enter(depth) {
        return omitted_element(context.palette);
    }

    match block {
        RichBlock::Paragraph(content) => render_inline_line(content, context, depth, budget),
        RichBlock::Heading { level, content } => {
            let element = div()
                .min_w_0()
                .font_semibold()
                .text_color(context.palette.foreground)
                .child(render_inlines(content, context, depth, budget));
            match heading_size(*level) {
                HeadingSize::TwoXl => element.text_2xl().into_any_element(),
                HeadingSize::Xl => element.text_xl().into_any_element(),
                HeadingSize::Lg => element.text_lg().into_any_element(),
                HeadingSize::Base => element.text_base().into_any_element(),
                HeadingSize::Sm => element.text_sm().into_any_element(),
            }
        }
        RichBlock::BulletList(items) => render_list(items, None, context, depth, budget),
        RichBlock::OrderedList { order, items } => {
            render_list(items, Some(*order), context, depth, budget)
        }
        RichBlock::CodeBlock { language, text } => {
            let mut code = v_flex()
                .min_w_0()
                .gap_1()
                .p_3()
                .rounded(px(4.))
                .border_1()
                .border_color(context.palette.border)
                .bg(context.palette.code_surface)
                .text_sm()
                .text_color(context.palette.foreground)
                .font_family("monospace");
            if let Some(language) = language {
                code = code.child(
                    div()
                        .text_xs()
                        .text_color(context.palette.muted)
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
            .border_color(context.palette.muted)
            .children(render_blocks(content, context, depth, budget))
            .into_any_element(),
        RichBlock::Panel { kind, content } => {
            let accent = panel_accent(*kind, context.palette);
            v_flex()
                .min_w_0()
                .gap_2()
                .p_3()
                .rounded(px(4.))
                .border_1()
                .border_color(accent)
                .bg(accent.opacity(0.08))
                .children(render_blocks(content, context, depth, budget))
                .into_any_element()
        }
        RichBlock::Image(image) => render_image(image, context, budget),
        RichBlock::Placeholder { label } => div()
            .min_w_0()
            .text_sm()
            .italic()
            .text_color(context.palette.muted)
            .child(budget.text(presentation_placeholder_label(label)))
            .into_any_element(),
    }
}

fn render_blocks(
    blocks: &[RichBlock],
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> Vec<AnyElement> {
    let mut rendered = Vec::new();
    for block in blocks.iter().take(MAX_RENDER_CHILDREN) {
        rendered.push(render_block(
            block,
            context,
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
    context: &RenderContext<'_>,
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
                        .text_color(context.palette.muted)
                        .text_right()
                        .child(marker),
                )
                .child(v_flex().min_w_0().flex_1().gap_2().children(render_blocks(
                    &item.blocks,
                    context,
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
