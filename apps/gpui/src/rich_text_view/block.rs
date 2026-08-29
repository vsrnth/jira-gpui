//! Block-level rich-text rendering: paragraphs, lists, panels, and code blocks.

use super::image::render_image;
use super::inline::render_inline_line;
use super::inline::render_inlines;
use super::{
    HeadingSize, MAX_RENDER_CHILDREN, RenderBudget, RenderContext, RichBlock, RichListItem,
    heading_size, omitted_element, panel_accent, presentation_placeholder_label,
};
use gpui::{
    AnyElement, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, rems,
};
use gpui_component::{StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use jira_domain::{HORIZONTAL_RULE_LABEL, RichTable, RichTableCell};

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
                .rounded(rems(0.25))
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
                .rounded(rems(0.25))
                .border_1()
                .border_color(accent)
                .bg(accent.opacity(0.08))
                .children(render_blocks(content, context, depth, budget))
                .into_any_element()
        }
        RichBlock::Table(table) => render_table(table, context, depth, budget),
        RichBlock::Image(image) => render_image(image, context, budget),
        RichBlock::Placeholder { label } if label == HORIZONTAL_RULE_LABEL => div()
            .id("rich-text-horizontal-rule")
            .debug_selector(|| "rich-text-horizontal-rule".to_owned())
            .accessibility_id("rich-text-horizontal-rule")
            .role(gpui::accesskit::Role::Group)
            .aria_label("Horizontal rule")
            .w_full()
            .h(rems(0.0625))
            .my_2()
            .bg(context.palette.border)
            .into_any_element(),
        RichBlock::Placeholder { label } => div()
            .min_w_0()
            .text_sm()
            .italic()
            .text_color(context.palette.muted)
            .child(budget.text(presentation_placeholder_label(label)))
            .into_any_element(),
    }
}

pub(super) fn render_table(
    table: &RichTable,
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let mut rows = Vec::new();
    for (row_index, row) in table.rows.iter().take(MAX_RENDER_CHILDREN).enumerate() {
        if !budget.enter(depth.saturating_add(1)) {
            break;
        }
        let mut cells = Vec::new();
        for (column_index, cell) in row.cells.iter().take(MAX_RENDER_CHILDREN).enumerate() {
            if !budget.enter(depth.saturating_add(2)) {
                break;
            }
            cells.push(render_table_cell(
                cell,
                row_index,
                column_index,
                context,
                depth,
                budget,
            ));
        }
        if row.cells.len() > MAX_RENDER_CHILDREN {
            budget.omitted = true;
        }
        rows.push(
            h_flex()
                .id(format!("rich-text-table-row-{row_index}"))
                .debug_selector(move || format!("rich-text-table-row-{row_index}"))
                .accessibility_id(format!("rich-text-table-row-{row_index}"))
                .role(gpui::accesskit::Role::Row)
                .min_w_0()
                .w_full()
                .gap_1()
                .items_stretch()
                .children(cells)
                .into_any_element(),
        );
        if budget.omitted {
            break;
        }
    }
    if table.rows.len() > MAX_RENDER_CHILDREN {
        budget.omitted = true;
    }
    v_flex()
        .id("rich-text-table")
        .debug_selector(|| "rich-text-table".to_owned())
        .accessibility_id("rich-text-table")
        .role(gpui::accesskit::Role::Table)
        .aria_label("Rich text table")
        .min_w_0()
        .w_full()
        .gap_1()
        .children(rows)
        .into_any_element()
}

fn render_table_cell(
    cell: &RichTableCell,
    row_index: usize,
    column_index: usize,
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let cell_id = format!("rich-text-table-cell-{row_index}-{column_index}");
    let debug_cell_id = cell_id.clone();
    let mut element = v_flex()
        .id(cell_id.clone())
        .debug_selector(move || debug_cell_id.clone())
        .accessibility_id(cell_id)
        .role(if cell.header {
            gpui::accesskit::Role::ColumnHeader
        } else {
            gpui::accesskit::Role::Cell
        })
        .min_w_0()
        .flex_1()
        .gap_1()
        .p_2()
        .border_1()
        .border_color(context.palette.border)
        .text_sm();
    if cell.header {
        element = element
            .font_semibold()
            .bg(context.palette.code_surface.opacity(0.12));
    }
    element
        .children(render_blocks(
            &cell.content,
            context,
            depth.saturating_add(1),
            budget,
        ))
        .into_any_element()
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
                        .w(rems(1.125))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::ImageSource;
    use crate::rich_text_view::{RichImageRenderStates, RichTextPalette};
    use gpui::{Context, Render, VisualTestContext, Window};
    use jira_domain::{RichInline, RichTable, RichTableCell, RichTableRow, RichTextDocument};

    struct RichTextFixture {
        document: RichTextDocument,
    }

    impl Render for RichTextFixture {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            super::super::render_rich_text(
                &self.document,
                RichTextPalette::default(),
                &RichImageRenderStates::default(),
                0,
                ImageSource::ResolvedAdf,
            )
        }
    }

    #[gpui::test]
    fn horizontal_rule_renders_as_a_bounded_divider(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let document = RichTextDocument::new(
            vec![
                RichBlock::Heading {
                    level: 2,
                    content: vec![RichInline::Text {
                        text: "Blocked by".to_owned(),
                        marks: Vec::new(),
                    }],
                },
                RichBlock::Paragraph(vec![RichInline::Text {
                    text: "This issue is waiting on another ticket.".to_owned(),
                    marks: Vec::new(),
                }]),
                RichBlock::horizontal_rule(),
            ],
            false,
        );
        assert!(document.blocks[2].is_horizontal_rule());

        let window = cx.open_window(gpui::size(gpui::px(480.), gpui::px(240.)), |_, _| {
            RichTextFixture { document }
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let divider = visual
            .debug_bounds("rich-text-horizontal-rule")
            .expect("horizontal rule should render as a divider");
        assert!(divider.size.width > gpui::px(0.));
        assert!(divider.size.height > gpui::px(0.));
        assert!(divider.size.height <= gpui::px(4.));
    }

    #[gpui::test]
    fn table_renders_as_a_bounded_themed_grid(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let document = RichTextDocument::new(
            vec![RichBlock::Table(RichTable {
                rows: vec![
                    RichTableRow {
                        cells: vec![
                            RichTableCell {
                                header: true,
                                content: vec![RichBlock::Paragraph(vec![RichInline::Text {
                                    text: "Given".to_owned(),
                                    marks: Vec::new(),
                                }])],
                            },
                            RichTableCell {
                                header: false,
                                content: vec![RichBlock::Paragraph(vec![RichInline::Text {
                                    text: "Then".to_owned(),
                                    marks: Vec::new(),
                                }])],
                            },
                        ],
                    },
                    RichTableRow {
                        cells: vec![
                            RichTableCell {
                                header: false,
                                content: vec![RichBlock::Paragraph(vec![RichInline::Text {
                                    text: "Status".to_owned(),
                                    marks: Vec::new(),
                                }])],
                            },
                            RichTableCell {
                                header: false,
                                content: vec![
                                    RichBlock::Paragraph(vec![RichInline::Text {
                                        text: "Ready".to_owned(),
                                        marks: Vec::new(),
                                    }]),
                                    RichBlock::Paragraph(vec![RichInline::Text {
                                        text: "Cache is preloaded".to_owned(),
                                        marks: Vec::new(),
                                    }]),
                                ],
                            },
                        ],
                    },
                ],
            })],
            false,
        );
        let window = cx.open_window(gpui::size(gpui::px(480.), gpui::px(240.)), |_, _| {
            RichTextFixture { document }
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let table = visual
            .debug_bounds("rich-text-table")
            .expect("table should render as a grid");
        assert!(table.size.width > gpui::px(0.));
        assert!(table.size.height > gpui::px(0.));
        assert!(table.size.height < gpui::px(240.));

        let short_cell = visual
            .debug_bounds("rich-text-table-cell-1-0")
            .expect("short table cell should expose stable geometry");
        let multiline_cell = visual
            .debug_bounds("rich-text-table-cell-1-1")
            .expect("multiline table cell should expose stable geometry");
        assert!(
            (f32::from(short_cell.origin.y) - f32::from(multiline_cell.origin.y)).abs() <= 2.,
            "table cells should share a top edge: short={short_cell:?}, multiline={multiline_cell:?}"
        );
        assert!(
            (f32::from(short_cell.origin.y + short_cell.size.height)
                - f32::from(multiline_cell.origin.y + multiline_cell.size.height))
            .abs()
                <= 2.,
            "table cells should share a bottom edge: short={short_cell:?}, multiline={multiline_cell:?}"
        );
    }
}
