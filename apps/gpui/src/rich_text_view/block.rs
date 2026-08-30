//! Block-level rich-text rendering: paragraphs, lists, panels, and code blocks.

use super::image::render_image;
use super::inline::render_inline_line;
use super::inline::render_inlines;
use super::{
    HeadingSize, MAX_RENDER_CHILDREN, RenderBudget, RenderContext, RichBlock, RichListItem,
    heading_size, omitted_element, panel_accent, presentation_placeholder_label,
};
use gpui::{
    AnyElement, ElementId, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, rems,
};
use gpui_component::{StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use jira_domain::{
    HORIZONTAL_RULE_LABEL, RichDecisionItem, RichDecisionState, RichTable, RichTableCell,
    RichTaskItem, RichTaskState,
};

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
        RichBlock::TaskList(items) => render_task_list(items, context, depth, budget),
        RichBlock::DecisionList(items) => render_decision_list(items, context, depth, budget),
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
        RichBlock::Expand { title, content } => {
            render_expand(title.as_deref(), content, false, context, depth, budget)
        }
        RichBlock::NestedExpand { title, content } => {
            render_expand(title.as_deref(), content, true, context, depth, budget)
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

fn render_task_list(
    items: &[RichTaskItem],
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let list_ordinal =
        super::render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal());
    let mut rows = Vec::new();
    for (index, item) in items.iter().take(MAX_RENDER_CHILDREN).enumerate() {
        if !budget.enter(depth.saturating_add(1)) {
            break;
        }
        let (marker, state_label) = match item.state {
            RichTaskState::Todo => ("☐", "Todo"),
            RichTaskState::Done => ("☑", "Done"),
        };
        let row_id = format!("rich-text-task-item-{list_ordinal}-{index}");
        let row_label = format!("{state_label} task item");
        rows.push(
            h_flex()
                .id(ElementId::named_usize(
                    "rich-text-task-item",
                    list_ordinal.saturating_add(index),
                ))
                .debug_selector(move || row_id.clone())
                .accessibility_id(format!("rich-text-task-item-{list_ordinal}-{index}"))
                .role(gpui::accesskit::Role::ListItem)
                .aria_label(row_label)
                .aria_value(format!("{state_label} task"))
                .min_w_0()
                .w_full()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .id(ElementId::named_usize(
                            "rich-text-task-marker",
                            list_ordinal.saturating_add(index),
                        ))
                        .debug_selector(move || {
                            format!("rich-text-task-marker-{list_ordinal}-{index}")
                        })
                        .accessibility_id(format!("rich-text-task-marker-{list_ordinal}-{index}"))
                        .aria_label(format!("{state_label} task"))
                        .aria_value(marker)
                        .w(rems(1.25))
                        .flex_shrink_0()
                        .items_start()
                        .text_sm()
                        .text_color(context.palette.muted)
                        .child(marker),
                )
                .child(v_flex().min_w_0().flex_1().gap_2().children(render_blocks(
                    &item.content,
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
    v_flex()
        .id(ElementId::named_usize("rich-text-task-list", list_ordinal))
        .debug_selector(|| "rich-text-task-list".to_owned())
        .accessibility_id(format!("rich-text-task-list-{list_ordinal}"))
        .role(gpui::accesskit::Role::List)
        .aria_label("Task list")
        .min_w_0()
        .w_full()
        .gap_2()
        .children(rows)
        .into_any_element()
}

fn render_decision_list(
    items: &[RichDecisionItem],
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let list_ordinal =
        super::render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal());
    let mut rows = Vec::new();
    for (index, item) in items.iter().take(MAX_RENDER_CHILDREN).enumerate() {
        if !budget.enter(depth.saturating_add(1)) {
            break;
        }
        let (marker, state_label) = match item.state {
            RichDecisionState::Decided => ("✓", "Decided"),
            RichDecisionState::Undecided => ("?", "Undecided"),
            RichDecisionState::Unknown => ("•", "Unknown"),
        };
        let row_id = format!("rich-text-decision-item-{list_ordinal}-{index}");
        rows.push(
            h_flex()
                .id(ElementId::named_usize(
                    "rich-text-decision-item",
                    list_ordinal.saturating_add(index),
                ))
                .debug_selector(move || row_id.clone())
                .accessibility_id(format!("rich-text-decision-item-{list_ordinal}-{index}"))
                .role(gpui::accesskit::Role::ListItem)
                .aria_label(format!("{state_label} decision item"))
                .aria_value(format!("{state_label} decision"))
                .min_w_0()
                .w_full()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .id(ElementId::named_usize(
                            "rich-text-decision-marker",
                            list_ordinal.saturating_add(index),
                        ))
                        .debug_selector(move || {
                            format!("rich-text-decision-marker-{list_ordinal}-{index}")
                        })
                        .accessibility_id(format!(
                            "rich-text-decision-marker-{list_ordinal}-{index}"
                        ))
                        .aria_label(state_label)
                        .aria_value(marker)
                        .w(rems(1.25))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(context.palette.muted)
                        .child(marker),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .children(vec![render_inline_line(
                            &item.content,
                            context,
                            depth,
                            budget,
                        )]),
                )
                .into_any_element(),
        );
        if budget.omitted {
            break;
        }
    }
    if items.len() > MAX_RENDER_CHILDREN {
        budget.omitted = true;
    }
    v_flex()
        .id(ElementId::named_usize(
            "rich-text-decision-list",
            list_ordinal,
        ))
        .debug_selector(|| "rich-text-decision-list".to_owned())
        .accessibility_id(format!("rich-text-decision-list-{list_ordinal}"))
        .role(gpui::accesskit::Role::List)
        .aria_label("Decision list")
        .min_w_0()
        .w_full()
        .gap_2()
        .children(rows)
        .into_any_element()
}

fn render_expand(
    title: Option<&str>,
    content: &[RichBlock],
    nested: bool,
    context: &RenderContext<'_>,
    depth: usize,
    budget: &mut RenderBudget,
) -> AnyElement {
    let ordinal =
        super::render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal());
    let title_label = title
        .filter(|title| !title.trim().is_empty())
        .map(|title| budget.text(title));
    let label = title_label
        .clone()
        .unwrap_or_else(|| "Expanded content".to_owned());
    let debug_id = if nested {
        "rich-text-nested-expand"
    } else {
        "rich-text-expand"
    };
    let accessibility_id = format!("{debug_id}-{ordinal}");
    let mut container = v_flex()
        .id(ElementId::named_usize(debug_id, ordinal))
        .debug_selector(move || debug_id.to_owned())
        .accessibility_id(accessibility_id)
        .role(gpui::accesskit::Role::Group)
        .aria_label(label.clone())
        .aria_value("Expanded")
        .min_w_0()
        .w_full()
        .gap_2()
        .p_3()
        .pl(if nested { rems(4.) } else { rems(3.) })
        .border_1()
        .border_color(context.palette.border);
    if let Some(title) = title_label {
        container = container.child(
            div()
                .id(ElementId::named_usize("rich-text-expand-title", ordinal))
                .accessibility_id(format!("rich-text-expand-title-{ordinal}"))
                .role(gpui::accesskit::Role::Heading)
                .aria_label(label)
                .font_semibold()
                .text_sm()
                .text_color(context.palette.foreground)
                .child(title),
        );
    }
    container
        .children(render_blocks(content, context, depth, budget))
        .into_any_element()
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
    use jira_domain::{
        RichInline, RichTable, RichTableCell, RichTableRow, RichTaskItem, RichTaskState,
        RichTextDocument,
    };

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

    #[gpui::test]
    fn task_rows_keep_state_markers_visible_and_aligned(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let paragraph = |text: &str| {
            RichBlock::Paragraph(vec![RichInline::Text {
                text: text.to_owned(),
                marks: Vec::new(),
            }])
        };
        let document = RichTextDocument::new(
            vec![RichBlock::TaskList(vec![
                RichTaskItem {
                    state: RichTaskState::Todo,
                    content: vec![paragraph("Short task")],
                },
                RichTaskItem {
                    state: RichTaskState::Done,
                    content: vec![paragraph(
                        "A completed task with enough text to wrap inside the bounded surface.",
                    )],
                },
            ])],
            false,
        );
        let window = cx.open_window(gpui::size(gpui::px(480.), gpui::px(240.)), |_, _| {
            RichTextFixture { document }
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let list = visual
            .debug_bounds("rich-text-task-list")
            .expect("task list should expose stable geometry");
        let first = visual
            .debug_bounds("rich-text-task-item-0-0")
            .expect("todo row should expose stable geometry");
        let second = visual
            .debug_bounds("rich-text-task-item-0-1")
            .expect("done row should expose stable geometry");
        let first_marker = visual
            .debug_bounds("rich-text-task-marker-0-0")
            .expect("todo marker should be visible");
        let second_marker = visual
            .debug_bounds("rich-text-task-marker-0-1")
            .expect("done marker should be visible");

        assert!(list.size.width > gpui::px(0.));
        assert!(first.size.width > gpui::px(0.));
        assert!(second.size.width > gpui::px(0.));
        assert!((f32::from(first.origin.x) - f32::from(second.origin.x)).abs() <= 1.);
        assert!((f32::from(first_marker.origin.x) - f32::from(second_marker.origin.x)).abs() <= 1.);
        assert!(first_marker.size.width > gpui::px(0.));
        assert!(second_marker.size.width > gpui::px(0.));
    }

    #[gpui::test]
    fn nested_expand_is_read_only_visible_and_indented(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let paragraph = |text: &str| {
            RichBlock::Paragraph(vec![RichInline::Text {
                text: text.to_owned(),
                marks: Vec::new(),
            }])
        };
        let document = RichTextDocument::new(
            vec![RichBlock::Expand {
                title: Some("Details".to_owned()),
                content: vec![RichBlock::NestedExpand {
                    title: Some("More details".to_owned()),
                    content: vec![paragraph("Nested content remains visible")],
                }],
            }],
            false,
        );
        let window = cx.open_window(gpui::size(gpui::px(480.), gpui::px(240.)), |_, _| {
            RichTextFixture { document }
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let outer = visual
            .debug_bounds("rich-text-expand")
            .expect("expand should expose stable group geometry");
        let nested = visual
            .debug_bounds("rich-text-nested-expand")
            .expect("nested expand should remain visible");
        assert!(outer.size.width > gpui::px(0.));
        assert!(outer.size.height > gpui::px(0.));
        assert!(nested.size.width > gpui::px(0.));
        assert!(nested.size.height > gpui::px(0.));
        assert!(nested.origin.x > outer.origin.x);
    }
}
