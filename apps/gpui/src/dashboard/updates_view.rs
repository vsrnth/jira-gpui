use super::*;

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
                    .h(px(if mobile { 104. } else { 78. }))
                    .px(px(if mobile { 12. } else { 20. }))
                    .py(px(8.))
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
        let accessible_label = format!("Open {}: {}", group.issue_key, group.issue_summary);
        let open_area = div()
            .id(("update-open", index))
            .role(gpui::accesskit::Role::Button)
            .aria_label(accessible_label)
            .tab_index(0)
            .flex()
            .flex_1()
            .h_auto()
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
                                    .child(self.issue_key_with_icon(
                                        group.issue_key.clone(),
                                        issue_type,
                                        cx,
                                    ))
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
                                    .when(!mobile, |this| this.flex_shrink_0())
                                    .when(mobile, |this| this.w_full())
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
            .id(("update-card", index))
            .debug_selector(move || format!("update-card-{index}"))
            .w_full()
            .min_w_0()
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
}
