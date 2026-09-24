use std::rc::Rc;

use super::virtual_rows::{
    RowMeasureKey, cached_height, measured_row, retain_identities, revision_hash,
};
use super::*;
use gpui_kit::component::Selectable as _;

fn update_group_height(group: &UpdateGroupViewModel, layout: LayoutMode, expanded: bool) -> f32 {
    let rows = compact_update_rows(&group.events);
    let visible = visible_update_row_count(rows.len(), expanded);
    // Event text is line-clamped on mobile and single-line on desktop. The
    // extra allowance covers the action row and card padding without clipping.
    if layout.is_mobile() {
        180. + visible as f32 * 40.
    } else {
        116. + visible as f32 * 22.
    }
}

pub(super) fn update_filter_is_selected(current: UpdateFilter, option: UpdateFilter) -> bool {
    current == option
}

fn should_show_row_timestamp(
    event_count: usize,
    latest_occurred_at: &str,
    row_occurred_at: &str,
) -> bool {
    event_count != 1 || row_occurred_at != latest_occurred_at
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
        let retained_update_ids = visible_groups
            .iter()
            .map(|index| self.update_groups[*index].issue_id.to_string())
            .collect();
        retain_identities(&self.updates_row_measurements, &retained_update_ids);
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .id("updates-header")
                    .debug_selector(|| "updates-header".to_owned())
                    .when(mobile, |this| this.px_3())
                    .when(!mobile, |this| this.px_5())
                    .py_2()
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
                                h_flex().min_w_0().gap_2().child(
                                    div()
                                        .id("updates-heading")
                                        .debug_selector(|| "updates-heading".to_owned())
                                        .text_sm()
                                        .font_semibold()
                                        .child("Local updates"),
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
                                    .label(format!("Unread events · {unread}"))
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
                                    .label(format!("All tickets · {}", self.update_groups.len()))
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
                                .child(format!(
                                    "{} tickets · {} unread events",
                                    visible_groups.len(),
                                    unread
                                )),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .id("updates-workspace")
                    .debug_selector(|| "updates-workspace".to_owned())
                    .flex_1()
                    .w_full()
                    .h_full()
                    .min_h_0()
                    .min_w_0()
                    .when(
                        layout.is_mobile() && self.mobile_update_detail_open,
                        |this| this.child(self.update_reading_pane(layout, cx)),
                    )
                    .when(
                        layout.is_mobile() && !self.mobile_update_detail_open,
                        |this| {
                            this.child(self.update_list(
                                layout,
                                no_visible_groups,
                                visible_groups.clone(),
                                cx,
                            ))
                        },
                    )
                    .when(!layout.is_mobile(), |this| {
                        this.child(self.update_list(
                            layout,
                            no_visible_groups,
                            visible_groups.clone(),
                            cx,
                        ))
                        .child(self.update_reading_pane(layout, cx))
                    }),
            )
    }

    fn update_list(
        &self,
        layout: LayoutMode,
        no_visible_groups: bool,
        visible_groups: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
                    .id("update-list")
                    .debug_selector(|| "update-list".to_owned())
                    .accessibility_id("update-list")
                    .role(gpui_kit::accesskit::Role::Group)
                    .aria_label("Local Jira activity")
                    .flex_1()
                    .overflow_x_hidden()
                    .vertical_scrollbar(&self.updates_scroll_handle)
                    .h_full()
                    .min_h_0()
                    .min_w_0()
                    .when(layout.is_mobile(), |this| this.w_full())
                    .justify_start()
                    .items_start()
                    .child(
                        v_flex()
                            .w_full()
                            .flex_1()
                            .h_full()
                            .min_h_0()
                            .max_w(gpui_kit::rems(70.))
                            .p(gpui_kit::rems(layout.list_padding() / 16.))
                            .gap_3()
                            .child(if no_visible_groups {
                                v_flex().into_any_element()
                            } else {
                                let heights = Rc::new(
                                    visible_groups
                                        .iter()
                                        .map(|index| {
                                            let group = &self.update_groups[*index];
                                            let expanded = self
                                                .expanded_update_groups
                                                .contains(&group.issue_id);
                                            gpui_kit::size(
                                                gpui_kit::px(0.),
                                                cached_height(
                                                    &self.updates_row_measurements,
                                                    &RowMeasureKey {
                                                        identity: group.issue_id.to_string(),
                                                        revision: revision_hash((
                                                            &group.issue_key,
                                                            &group.issue_summary,
                                                            &group.latest_occurred_at,
                                                            group.unread,
                                                            format!("{:?}", &group.events),
                                                        )),
                                                        layout: layout as u8,
                                                        expanded,
                                                    },
                                                    update_group_height(group, layout, expanded),
                                                ),
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                );
                                let visible_groups = visible_groups.clone();
                                gpui_kit::component::v_virtual_list(
                                    cx.entity(),
                                    "update-list-virtual",
                                    heights,
                                    move |this, range, _, cx| {
                                        range
                                            .map(|position| {
                                                let index = visible_groups[position];
                                                let group = &this.update_groups[index];
                                                measured_row(
                                                    RowMeasureKey {
                                                        identity: group.issue_id.to_string(),
                                                        revision: revision_hash((
                                                            &group.issue_key,
                                                            &group.issue_summary,
                                                            &group.latest_occurred_at,
                                                            group.unread,
                                                            format!("{:?}", &group.events),
                                                        )),
                                                        layout: layout as u8,
                                                        expanded: this.expanded_update_groups.contains(&group.issue_id),
                                                    },
                                                    this.updates_row_measurements.clone(),
                                                    cx.entity().downgrade(),
                                                    this.update_group_card(index, group, layout, cx),
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                )
                                .track_scroll(&self.updates_scroll_handle)
                                .into_any_element()
                            })
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
                                        .role(gpui_kit::accesskit::Role::Status)
                                        .aria_label(if self.update_filter == UpdateFilter::Unread {
                                            "You are all caught up. New local updates will appear after refresh."
                                        } else {
                                            "No local updates yet. Refresh to check Jira activity."
                                        })
                                        .child(gpui_kit::component::empty::Empty::new().header(
                                            gpui_kit::component::empty::EmptyHeader::new()
                                                .title(gpui_kit::component::empty::EmptyTitle::new().child(
                                                    if self.update_filter == UpdateFilter::Unread {
                                                        "You are all caught up"
                                                    } else {
                                                        "No local updates yet"
                                                    },
                                                ))
                                                .description(
                                                    gpui_kit::component::empty::EmptyDescription::new()
                                                        .child(if self.update_filter == UpdateFilter::Unread {
                                                            "New local updates will appear after refresh."
                                                        } else {
                                                            "Refresh to check Jira activity."
                                                        }),
                                                ),
                                        )),
                                )
                            }),
                    )
            .into_any_element()
    }

    fn update_reading_pane(&self, layout: LayoutMode, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected_update_issue.as_ref().and_then(|issue_id| {
            filtered_update_group_indices(&self.update_groups, self.update_filter)
                .into_iter()
                .map(|index| &self.update_groups[index])
                .find(|group| &group.issue_id == issue_id)
        });
        let Some(group) = selected else {
            return v_flex()
                .id("update-reading-pane-empty")
                .accessibility_id("update-reading-pane-empty")
                .role(gpui_kit::accesskit::Role::Status)
                .aria_label("Select a ticket to read its local updates")
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_center()
                .p(gpui_kit::rems(layout.detail_padding() / 16.))
                .when(layout.is_mobile(), |this| {
                    this.child(
                        Button::new("updates-reading-back")
                            .compact()
                            .ghost()
                            .debug_selector(|| "updates-reading-back".to_owned())
                            .h_11()
                            .label("Back to tickets")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.close_mobile_update_detail(cx)),
                            ),
                    )
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Select a ticket to read its updates"),
                )
                .into_any_element();
        };
        let issue = self
            .domain_issues
            .iter()
            .chain(self.team_issues.iter())
            .find(|issue| issue.id == group.issue_id)
            .or_else(|| {
                self.selected_issue_core
                    .as_ref()
                    .filter(|issue| issue.id == group.issue_id)
            });
        let has_cached_issue_detail = issue.is_some_and(issue_has_cached_detail);
        let mobile = layout.is_mobile();
        let issue_id = group.issue_id.clone();
        v_flex()
            .id("update-reading-pane")
            .debug_selector(|| "update-reading-pane".to_owned())
            .accessibility_id("update-reading-pane")
            .role(gpui_kit::accesskit::Role::Group)
            .aria_label(format!("{} local updates", group.issue_key))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_x_hidden()
            .overflow_y_scrollbar()
            .gap_3()
            .p(gpui_kit::rems(layout.detail_padding() / 16.))
            .border_l_1()
            .border_color(cx.theme().border)
            .when(mobile, |this| {
                this.child(
                    Button::new("updates-reading-back")
                        .compact()
                        .ghost()
                        .debug_selector(|| "updates-reading-back".to_owned())
                        .h_11()
                        .label("Back to tickets")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_mobile_update_detail(cx)
                        })),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("LOCAL ACTIVITY · TICKET"),
            )
            .child(
                div()
                    .text_xl()
                    .font_semibold()
                    .child(group.issue_key.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .whitespace_normal()
                    .text_base()
                    .font_medium()
                    .child(issue.map_or_else(
                        || group.issue_summary.clone(),
                        |issue| issue.summary.clone(),
                    )),
            )
            .when_some(issue, |this, issue| {
                this.child(
                    div()
                        .min_w_0()
                        .whitespace_normal()
                        .text_sm()
                        .child(format!(
                            "{} · {} · {}",
                            issue.issue_type.name,
                            issue.status.name,
                            issue.priority.name.as_deref().unwrap_or("No priority")
                        )),
                )
                .when(has_cached_issue_detail, |this| {
                    let description = issue.description_text.clone().unwrap_or_else(|| {
                        if issue.rich_description.is_some() {
                            "A rich description is available in full issue details.".to_owned()
                        } else {
                            "No description supplied.".to_owned()
                        }
                    });
                    this.child(
                        div()
                            .min_w_0()
                            .whitespace_normal()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    )
                })
            })
            .when(!has_cached_issue_detail, |this| {
                this.child(
                    div()
                        .id("update-issue-unavailable")
                        .accessibility_id("update-issue-unavailable")
                        .role(gpui_kit::accesskit::Role::Status)
                        .aria_label("Cached issue details are unavailable")
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Full issue details are not available in the local cache. The activity below is what was synced."),
                )
            })
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .child("Activity"),
            )
            .children(group.events.iter().map(|event| {
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(div().text_sm().child(event.change.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(event.occurred_at.clone()),
                    )
            }))
            .child(Button::new("update-open-full-details")
                .compact()
                .debug_selector(|| "update-open-full-details".to_owned())
                .when(mobile, |this| this.h_11())
                .label("Open full issue details")
                .accessibility_id("update-open-full-details")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_update_issue(issue_id.clone(), mobile, cx)
                })))
            .into_any_element()
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
            "{read_state} update. Open {} ({}): {}",
            group.issue_key, issue_type, group.issue_summary
        );
        let open_area =
            div()
                .id(("update-open", index))
                .debug_selector(move || format!("update-open-{index}"))
                .accessibility_id(format!("update-open-{index}"))
                .role(gpui_kit::accesskit::Role::Button)
                .aria_label(accessible_label.clone())
                .tab_index(0)
                .flex()
                .flex_1()
                .w_full()
                .h_auto()
                .items_start()
                .min_w_0()
                .gap_3()
                .p_2()
                .hover(|style| style.bg(cx.theme().list_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_update_group(clicked_issue_id.clone(), mobile, cx);
                }))
                .on_key_down(cx.listener(move |this, event, window, cx| {
                    if is_activation_key(event) {
                        window.prevent_default();
                        this.select_update_group(keyboard_issue_id.clone(), mobile, cx);
                    }
                }))
                .focus(|style| style.border_1().border_color(cx.theme().primary))
                .child(
                    div()
                        .id(format!("update-unread-dot-{index}"))
                        .debug_selector(move || format!("update-unread-dot-{index}"))
                        .accessibility_id(format!("update-unread-dot-{index}"))
                        .role(gpui_kit::accesskit::Role::Group)
                        .aria_label(if group.unread {
                            "Unread update marker"
                        } else {
                            "Read update marker"
                        })
                        // The marker aligns with the first metadata line rather than the card's
                        // outer top edge. Use the component spacing token so the painted native
                        // frame stays on the same midline as the metadata text.
                        .mt_2()
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
                        .w_full()
                        .gap_2()
                        .text_base()
                        .text_color(cx.theme().foreground)
                        .child(
                            h_flex()
                                .id(format!("update-metadata-{index}"))
                                .debug_selector(move || format!("update-metadata-{index}"))
                                .accessibility_id(format!("update-metadata-{index}"))
                                .role(gpui_kit::accesskit::Role::Group)
                                .aria_label("Update metadata")
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(self.issue_key_label(group.issue_key.clone(), cx))
                                .child(self.issue_type_label_with_icon(
                                    issue_type,
                                    group.issue_key.clone(),
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
                                .id(format!("update-title-{index}"))
                                .debug_selector(move || format!("update-title-{index}"))
                                .min_w_0()
                                .w_full()
                                .line_clamp(2)
                                .whitespace_normal()
                                .text_sm()
                                .when(group.unread, |this| this.font_semibold())
                                .when(!group.unread, |this| this.font_normal())
                                .child(group.issue_summary.clone()),
                        )
                        .child(
                            div()
                                .id(format!("update-timestamp-{index}"))
                                .debug_selector(move || format!("update-timestamp-{index}"))
                                .min_w_0()
                                .w_full()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(group.latest_occurred_at.clone()),
                        )
                        .child(v_flex().gap_1().children(
                            rows.iter().take(visible_row_count).enumerate().map(
                                |(row_index, row)| {
                                    self.update_row_element(
                                        index, row_index, row, group, mobile, cx,
                                    )
                                },
                            ),
                        )),
                )
                .into_any_element();
        v_flex()
            .id(("update-card", index))
            .accessibility_id(format!("update-card-{index}"))
            .role(gpui_kit::accesskit::Role::Group)
            .aria_label(format!("{accessible_label} update card"))
            .debug_selector(move || format!("update-card-{index}"))
            .w_full()
            .min_w_0()
            .gap_1()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(
                self.selected_update_issue.as_ref() == Some(&group.issue_id),
                |this| {
                    this.border_l_2()
                        .border_color(cx.theme().primary)
                        .bg(cx.theme().primary.opacity(0.08))
                },
            )
            .child(open_area)
            .child(
                h_flex()
                    .id(("update-actions", index))
                    .debug_selector(move || format!("update-actions-{index}"))
                    .w_full()
                    .min_w_0()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .px_2()
                    .pb_1()
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
        group: &UpdateGroupViewModel,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (change, occurred_at) = match row {
            CompactedUpdateRow::Event(event) => (event.change.clone(), event.occurred_at.clone()),
            CompactedUpdateRow::GenericSummary { count, occurred_at } => {
                (generic_summary_label(*count), occurred_at.clone())
            }
        };
        let show_timestamp =
            should_show_row_timestamp(group.events.len(), &group.latest_occurred_at, &occurred_at);
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
            .when(show_timestamp, |this| {
                this.child(
                    div()
                        .min_w_0()
                        .when(!mobile, |this| this.flex_shrink_0())
                        .when(mobile, |this| this.w_full().truncate())
                        .text_color(cx.theme().muted_foreground)
                        .child(occurred_at),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::super::UpdateFilter;
    use super::should_show_row_timestamp;
    use crate::dashboard::{Dashboard, Section};
    use gpui_kit::VisualTestContext;

    #[test]
    fn row_timestamp_only_omits_singleton_duplicate() {
        let cases = [
            (1, "Sep 23, 10:00", "Sep 23, 10:00", false),
            (1, "Sep 23, 10:00", "Sep 22, 09:00", true),
            (2, "Sep 23, 10:00", "Sep 23, 10:00", true),
            (2, "Sep 23, 10:00", "Sep 22, 09:00", true),
        ];

        for (event_count, latest, row, expected) in cases {
            assert_eq!(
                should_show_row_timestamp(event_count, latest, row),
                expected,
                "event_count={event_count}, latest={latest}, row={row}"
            );
        }
    }

    #[gpui_kit::test]
    fn mobile_update_selection_opens_reading_pane_and_back_returns_to_list(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let window = cx.open_window(
            gpui_kit::size(gpui_kit::px(390.), gpui_kit::px(800.)),
            |_, _| {
                let mut dashboard = Dashboard::from_sample_data();
                dashboard.section = Section::Updates;
                dashboard.selected_update_issue = None;
                dashboard
            },
        );
        let dashboard = window.root(cx).expect("dashboard root");
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let list = visual
            .debug_bounds("update-list")
            .expect("mobile updates list should be laid out");
        let row = visual
            .debug_bounds("update-card-0")
            .expect("mobile ticket row should be laid out");
        let title = visual
            .debug_bounds("update-title-0")
            .expect("mobile ticket title should be laid out");
        let timestamp = visual
            .debug_bounds("update-timestamp-0")
            .expect("mobile ticket timestamp should be laid out");
        let actions = visual
            .debug_bounds("update-actions-0")
            .expect("mobile ticket actions should be laid out");
        assert!(list.size.width > gpui_kit::px(0.) && list.size.height > gpui_kit::px(0.));
        assert!(row.size.width > gpui_kit::px(0.) && row.size.height > gpui_kit::px(0.));
        assert!(title.size.width >= gpui_kit::px(200.));
        assert!(timestamp.origin.y >= title.origin.y + title.size.height);
        assert!(actions.origin.y >= timestamp.origin.y + timestamp.size.height);
        assert!(row.origin.x >= gpui_kit::px(0.));
        assert!(row.origin.y >= gpui_kit::px(0.));
        assert!(row.origin.x + row.size.width <= gpui_kit::px(390.) + gpui_kit::px(1.));
        assert!(row.origin.y + row.size.height <= gpui_kit::px(800.) + gpui_kit::px(1.));
        assert!(visual.debug_bounds("update-reading-pane").is_none());
        let issue_id = dashboard.read_with(&visual, |dashboard, _| {
            dashboard
                .update_groups
                .first()
                .expect("sample update group")
                .issue_id
                .clone()
        });
        let open = visual
            .debug_bounds("update-open-0")
            .expect("first ticket row should expose its activation target");
        visual.simulate_click(
            gpui_kit::point(
                open.origin.x + open.size.width / 2.,
                open.origin.y + open.size.height / 2.,
            ),
            Default::default(),
        );
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let (section, selected_update_issue, mobile_detail_open) =
            dashboard.read_with(&visual, |dashboard, _| {
                (
                    dashboard.section,
                    dashboard.selected_update_issue.clone(),
                    dashboard.mobile_update_detail_open,
                )
            });
        assert_eq!(section, Section::Updates);
        assert_eq!(selected_update_issue, Some(issue_id));
        assert!(mobile_detail_open);
        let workspace = visual
            .debug_bounds("updates-workspace")
            .expect("mobile updates workspace should be laid out");
        let pane = visual
            .debug_bounds("update-reading-pane")
            .expect("mobile reading pane should be laid out");
        assert!(pane.size.width > gpui_kit::px(0.) && pane.size.height > gpui_kit::px(0.));
        assert!(pane.origin.x >= workspace.origin.x);
        assert!(pane.origin.y >= workspace.origin.y);
        assert!(
            pane.origin.x + pane.size.width
                <= workspace.origin.x + workspace.size.width + gpui_kit::px(1.)
        );
        assert!(
            pane.origin.y + pane.size.height
                <= workspace.origin.y + workspace.size.height + gpui_kit::px(1.)
        );
        assert!(visual.debug_bounds("update-list").is_none());
        let back = visual
            .debug_bounds("updates-reading-back")
            .expect("mobile reading pane should provide a back action");
        let full_details = visual
            .debug_bounds("update-open-full-details")
            .expect("mobile reading pane should provide full issue details navigation");
        for (name, bounds) in [("Back", back), ("full details", full_details)] {
            assert!(
                bounds.size.height >= gpui_kit::px(44.),
                "mobile {name} target should be at least 44px high: {bounds:?}"
            );
            assert!(
                bounds.origin.x >= pane.origin.x
                    && bounds.origin.y >= pane.origin.y
                    && bounds.origin.x + bounds.size.width
                        <= pane.origin.x + pane.size.width + gpui_kit::px(1.)
                    && bounds.origin.y + bounds.size.height
                        <= pane.origin.y + pane.size.height + gpui_kit::px(1.),
                "mobile {name} target should remain inside the reading pane: {bounds:?}, pane={pane:?}"
            );
        }
        visual.simulate_click(
            gpui_kit::point(
                back.origin.x + back.size.width / 2.,
                back.origin.y + back.size.height / 2.,
            ),
            Default::default(),
        );
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        assert!(visual.debug_bounds("update-list").is_some());
        assert!(visual.debug_bounds("update-reading-pane").is_none());
    }

    #[gpui_kit::test]
    fn desktop_updates_list_rows_and_reading_pane_stay_inside_workspace(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        for viewport_width in [1095., 1280.] {
            let window = cx.open_window(
                gpui_kit::size(gpui_kit::px(viewport_width), gpui_kit::px(900.)),
                |_, _| {
                    let mut dashboard = Dashboard::from_sample_data();
                    dashboard.section = Section::Updates;
                    dashboard
                },
            );
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            visual.update(|window, cx| window.draw(cx).clear(cx));

            let workspace = visual
                .debug_bounds("updates-workspace")
                .expect("desktop updates workspace should be laid out");
            let list = visual
                .debug_bounds("update-list")
                .expect("desktop updates list should be laid out");
            let row = visual
                .debug_bounds("update-card-0")
                .expect("desktop ticket row should be laid out");
            let metadata = visual
                .debug_bounds("update-metadata-0")
                .expect("desktop ticket metadata should be laid out");
            let title = visual
                .debug_bounds("update-title-0")
                .expect("desktop ticket title should be laid out");
            let timestamp = visual
                .debug_bounds("update-timestamp-0")
                .expect("desktop ticket timestamp should be laid out");
            let actions = visual
                .debug_bounds("update-actions-0")
                .expect("desktop ticket actions should be laid out");
            let pane = visual
                .debug_bounds("update-reading-pane")
                .expect("desktop reading pane should be laid out");

            for (name, bounds) in [("list", list), ("row", row), ("pane", pane)] {
                assert!(
                    bounds.size.width > gpui_kit::px(0.) && bounds.size.height > gpui_kit::px(0.),
                    "desktop {name} should have positive bounds at {viewport_width}px: {bounds:?}"
                );
                assert!(
                    bounds.origin.x >= workspace.origin.x,
                    "desktop {name} escapes workspace left: {bounds:?}"
                );
                assert!(
                    bounds.origin.y >= workspace.origin.y,
                    "desktop {name} escapes workspace top: {bounds:?}"
                );
                assert!(
                    bounds.origin.x + bounds.size.width
                        <= workspace.origin.x + workspace.size.width + gpui_kit::px(1.),
                    "desktop {name} escapes workspace right: {bounds:?}, workspace={workspace:?}"
                );
                assert!(
                    bounds.origin.y + bounds.size.height
                        <= workspace.origin.y + workspace.size.height + gpui_kit::px(1.),
                    "desktop {name} escapes workspace bottom: {bounds:?}, workspace={workspace:?}"
                );
            }
            assert!(metadata.size.width >= gpui_kit::px(160.));
            assert!(title.origin.y >= metadata.origin.y + metadata.size.height);
            assert!(
                title.size.width >= gpui_kit::px(200.),
                "ticket title is too narrow at {viewport_width}px: {title:?}"
            );
            assert!(timestamp.origin.y >= title.origin.y + title.size.height);
            assert!(actions.origin.y >= timestamp.origin.y + timestamp.size.height);
            assert!(
                title.origin.x >= row.origin.x
                    && title.origin.x + title.size.width
                        <= row.origin.x + row.size.width + gpui_kit::px(1.)
            );
            assert!(row.origin.x >= gpui_kit::px(0.) && row.origin.y >= gpui_kit::px(0.));
            assert!(
                row.origin.x + row.size.width <= gpui_kit::px(viewport_width) + gpui_kit::px(1.)
            );
            assert!(row.origin.y + row.size.height <= gpui_kit::px(900.) + gpui_kit::px(1.));
        }
    }

    #[gpui_kit::test]
    fn marking_selected_update_read_clears_hidden_unread_selection_but_all_keeps_group(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let window = cx.open_window(
            gpui_kit::size(gpui_kit::px(390.), gpui_kit::px(800.)),
            |_, _| {
                let mut dashboard = Dashboard::from_sample_data();
                dashboard.section = Section::Updates;
                dashboard
            },
        );
        let dashboard = window.root(cx).expect("dashboard root");
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();

        let issue_id = dashboard.read_with(&visual, |dashboard, _| {
            dashboard
                .update_groups
                .iter()
                .find(|group| group.unread)
                .expect("sample fixture should include an unread ticket")
                .issue_id
                .clone()
        });
        visual.update(|_, cx| {
            dashboard.update(cx, |dashboard, cx| {
                dashboard.select_update_group(issue_id.clone(), true, cx);
                dashboard.set_update_filter(UpdateFilter::Unread, cx);
                dashboard.mark_group_read(issue_id.clone(), cx);
            });
        });

        let (selected, mobile_detail_open, unread_groups, group_is_read) =
            dashboard.read_with(&visual, |dashboard, _| {
                let group = dashboard
                    .update_groups
                    .iter()
                    .find(|group| group.issue_id == issue_id)
                    .expect("read group remains in the local update data");
                (
                    dashboard.selected_update_issue.clone(),
                    dashboard.mobile_update_detail_open,
                    super::super::filtered_update_group_indices(
                        &dashboard.update_groups,
                        dashboard.update_filter,
                    )
                    .into_iter()
                    .map(|index| dashboard.update_groups[index].issue_id.clone())
                    .collect::<Vec<_>>(),
                    !group.unread,
                )
            });
        assert_eq!(selected, None);
        assert!(!mobile_detail_open);
        assert!(!unread_groups.contains(&issue_id));
        assert!(group_is_read);

        visual.update(|_, cx| {
            dashboard.update(cx, |dashboard, cx| {
                dashboard.set_update_filter(UpdateFilter::All, cx);
            });
        });
        let (all_groups, selected, mobile_detail_open) =
            dashboard.read_with(&visual, |dashboard, _| {
                (
                    super::super::filtered_update_group_indices(
                        &dashboard.update_groups,
                        dashboard.update_filter,
                    )
                    .into_iter()
                    .map(|index| dashboard.update_groups[index].issue_id.clone())
                    .collect::<Vec<_>>(),
                    dashboard.selected_update_issue.clone(),
                    dashboard.mobile_update_detail_open,
                )
            });
        assert!(all_groups.contains(&issue_id));
        assert_eq!(selected, None);
        assert!(!mobile_detail_open);
    }
}
