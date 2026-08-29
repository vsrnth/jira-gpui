use super::*;
use gpui_component::Selectable as _;

pub(super) fn update_filter_is_selected(current: UpdateFilter, option: UpdateFilter) -> bool {
    current == option
}

impl Dashboard {
    pub(super) fn render_updates(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let unread = self.unread_count();
        let visible_groups = filtered_update_group_indices(&self.update_groups, self.update_filter);
        let no_visible_groups = visible_groups.is_empty();
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .id("updates-header")
                    .debug_selector(|| "updates-header".to_owned())
                    .px(px(if mobile { 12. } else { 20. }))
                    .py(px(8.))
                    .flex_shrink_0()
                    .min_w_0()
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
                                    .child(
                                        div()
                                            .id("updates-heading")
                                            .debug_selector(|| "updates-heading".to_owned())
                                            .text_sm()
                                            .font_semibold()
                                            .child("Change ledger"),
                                    )
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
                            .id("updates-filters")
                            .debug_selector(|| "updates-filters".to_owned())
                            .min_w_0()
                            .w_full()
                            .gap_1()
                            .child(
                                Button::new("updates-filter-unread")
                                    .compact()
                                    .selected(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::Unread,
                                    ))
                                    .toggled(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::Unread,
                                    ))
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
                                    .selected(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::All,
                                    ))
                                    .toggled(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::All,
                                    ))
                                    .when(self.update_filter == UpdateFilter::All, |this| {
                                        this.primary()
                                    })
                                    .label(format!("All · {}", self.update_groups.len()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_update_filter(UpdateFilter::All, cx)
                                    })),
                            ),
                    )
                    .child(
                        h_flex().min_w_0().child(
                            div()
                                .id("updates-description")
                                .debug_selector(|| "updates-description".to_owned())
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
                    .debug_selector(|| "update-list".to_owned())
                    .flex_1()
                    .overflow_y_scrollbar()
                    .overflow_x_hidden()
                    .min_h_0()
                    .w_full()
                    .justify_start()
                    .items_start()
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
                                        .id(if self.update_filter == UpdateFilter::Unread {
                                            "updates-empty-unread"
                                        } else {
                                            "updates-empty-all"
                                        })
                                        .debug_selector(|| {
                                            if self.update_filter == UpdateFilter::Unread {
                                                "updates-empty-unread".to_owned()
                                            } else {
                                                "updates-empty-all".to_owned()
                                            }
                                        })
                                        .role(gpui::accesskit::Role::Status)
                                        .aria_label(if self.update_filter == UpdateFilter::Unread {
                                            "You are all caught up. New local updates will appear after refresh."
                                        } else {
                                            "No local updates yet. Refresh to check Jira activity."
                                        })
                                        .p_4()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if self.update_filter == UpdateFilter::Unread {
                                            "You are all caught up. New local updates will appear after refresh."
                                        } else {
                                            "No local updates yet. Refresh to check Jira activity."
                                        }),
                                )
                            }),
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
        let keyboard_issue_id = issue_id.clone();
        let expanded = self.expanded_update_groups.contains(&group.issue_id);
        let rows = compact_update_rows(&group.events);
        let visible_row_count = visible_update_row_count(rows.len(), expanded);
        let hidden_row_count = hidden_update_row_count(rows.len(), expanded);
        let read_state = if group.unread { "Unread" } else { "Read" };
        let accessible_label = format!(
            "{read_state} update. Open {}: {}",
            group.issue_key, group.issue_summary
        );
        let open_area =
            div()
                .id(("update-open", index))
                .role(gpui::accesskit::Role::Button)
                .aria_label(accessible_label)
                .tab_index(0)
                .flex()
                .flex_1()
                .h_auto()
                .when(mobile, |this| this.w_full())
                .items_start()
                .min_w_0()
                .gap_3()
                .p_2()
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().list_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_update_issue(clicked_issue_id.clone(), mobile, cx);
                }))
                .on_key_down(cx.listener(move |this, event, window, cx| {
                    if is_activation_key(event) {
                        window.prevent_default();
                        this.open_update_issue(keyboard_issue_id.clone(), mobile, cx);
                    }
                }))
                .focus(|style| style.border_1().border_color(cx.theme().primary))
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
                        .text_base()
                        .text_color(cx.theme().foreground)
                        .child(
                            h_flex()
                                .min_w_0()
                                .w_full()
                                .justify_between()
                                .when(mobile, |this| {
                                    this.flex_col().w_full().items_start().gap_1()
                                })
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .when(!mobile, |this| this.flex_1())
                                        .when(layout.is_mobile(), |this| this.flex_col())
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .min_w_0()
                                                .gap_2()
                                                .child(self.issue_key_with_icon(
                                                    group.issue_key.clone(),
                                                    issue_type,
                                                    cx,
                                                ))
                                                .child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .text_xs()
                                                        .text_color(if group.unread {
                                                            cx.theme().primary
                                                        } else {
                                                            cx.theme().muted_foreground
                                                        })
                                                        .child(read_state),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .when(!mobile, |this| this.flex_1())
                                                .when(mobile, |this| this.w_full())
                                                .line_clamp(2)
                                                .text_sm()
                                                .when(group.unread, |this| this.font_semibold())
                                                .when(!group.unread, |this| this.font_normal())
                                                .child(group.issue_summary.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .when(!mobile, |this| this.flex_shrink_0())
                                        .when(mobile, |this| this.w_full().truncate())
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(group.latest_occurred_at.clone()),
                                ),
                        )
                        .child(v_flex().gap_1().children(
                            rows.iter().take(visible_row_count).enumerate().map(
                                |(row_index, row)| {
                                    self.update_row_element(index, row_index, row, mobile, cx)
                                },
                            ),
                        )),
                )
                .into_any_element();
        h_flex()
            .id(("update-card", index))
            .debug_selector(move || format!("update-card-{index}"))
            .w_full()
            .min_w_0()
            .overflow_x_hidden()
            .items_start()
            .gap_2()
            .when(mobile, |this| this.flex_col())
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(open_area)
            .child(
                v_flex()
                    .id(("update-actions", index))
                    .debug_selector(move || format!("update-actions-{index}"))
                    .flex_shrink_0()
                    .when(!mobile, |this| this.items_end())
                    .when(mobile, |this| this.w_full().flex_wrap().items_start())
                    .gap_1()
                    .p_1()
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

    fn update_row_element(
        &self,
        group_index: usize,
        row_index: usize,
        row: &CompactedUpdateRow,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (change, occurred_at) = match row {
            CompactedUpdateRow::Event(event) => (event.change.clone(), event.occurred_at.clone()),
            CompactedUpdateRow::GenericSummary { count, occurred_at } => {
                (generic_summary_label(*count), occurred_at.clone())
            }
        };
        h_flex()
            .id(format!("update-row-{group_index}-{row_index}"))
            .debug_selector(move || format!("update-row-{group_index}-{row_index}"))
            .min_w_0()
            .when(mobile, |this| this.w_full().flex_col().items_start())
            .gap_2()
            .text_xs()
            .child(
                div()
                    .min_w_0()
                    .when(!mobile, |this| this.flex_1())
                    .when(mobile, |this| this.w_full().line_clamp(2))
                    .child(change),
            )
            .child(
                div()
                    .min_w_0()
                    .when(!mobile, |this| this.flex_shrink_0())
                    .when(mobile, |this| this.w_full().truncate())
                    .text_color(cx.theme().muted_foreground)
                    .child(occurred_at),
            )
            .into_any_element()
    }
}
