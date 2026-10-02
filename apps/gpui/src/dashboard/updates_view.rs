use std::{collections::HashMap, rc::Rc};

use super::virtual_rows::{
    RowMeasureKey, cached_height, measured_row, retain_identities, revision_hash,
};
use super::*;
use gpui_kit::component::Selectable as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};

fn update_group_height(group: &UpdateGroupViewModel, layout: LayoutMode, expanded: bool) -> f32 {
    let rows = compact_update_rows(&group.events);
    let visible = visible_update_row_count(rows.len(), expanded);
    // Event text is line-clamped on mobile and single-line on desktop. The
    // extra allowance covers the action row and card padding without clipping.
    if !expanded {
        if layout.is_mobile() { 84. } else { 62. }
    } else if layout.is_mobile() {
        100. + visible as f32 * 40.
    } else {
        72. + visible as f32 * 22.
    }
}

pub(super) fn update_filter_is_selected(current: UpdateFilter, option: UpdateFilter) -> bool {
    current == option
}

pub(super) fn visible_inbox_groups(dashboard: &Dashboard) -> Vec<usize> {
    let query = dashboard.inbox_query.trim().to_lowercase();
    let issues_by_id = dashboard
        .domain_issues
        .iter()
        .chain(dashboard.team_issues.iter())
        .chain(dashboard.selected_issue_core.iter())
        .fold(HashMap::new(), |mut issues, issue| {
            issues.entry(issue.id.clone()).or_insert(issue);
            issues
        });
    filtered_update_group_indices(&dashboard.update_groups, dashboard.update_filter)
        .into_iter()
        .filter(|index| {
            let group = &dashboard.update_groups[*index];
            let issue = issues_by_id.get(&group.issue_id).copied();
            let matches_query = query.is_empty()
                || group.issue_key.to_lowercase().contains(&query)
                || group.issue_summary.to_lowercase().contains(&query)
                || group
                    .events
                    .iter()
                    .any(|event| event.change.to_lowercase().contains(&query));
            let matches_status = dashboard.inbox_status_filter == IssueStatusFilter::All
                || issue.is_some_and(|issue| {
                    dashboard
                        .inbox_status_filter
                        .matches(issue.status.category.as_deref().unwrap_or_default())
                });
            matches_query && matches_status
        })
        .collect()
}

fn should_show_row_timestamp(
    event_count: usize,
    latest_occurred_at: &str,
    row_occurred_at: &str,
) -> bool {
    event_count != 1 || row_occurred_at != latest_occurred_at
}

impl Dashboard {
    fn inbox_status_dropdown(&self, cx: &mut Context<Self>) -> AnyElement {
        let selection = self.inbox_status_filter;
        let dashboard = cx.entity().downgrade();
        Button::new("inbox-status-trigger")
            .compact()
            .secondary()
            .outline()
            .accessibility_id("inbox-status-trigger")
            .label(selection.label())
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                [
                    ("All statuses", IssueStatusFilter::All),
                    ("To do", IssueStatusFilter::ToDo),
                    ("In progress", IssueStatusFilter::InProgress),
                    ("Done", IssueStatusFilter::Done),
                    ("Uncategorized", IssueStatusFilter::Uncategorized),
                ]
                .into_iter()
                .fold(menu, |menu, (label, filter)| {
                    let dashboard = dashboard.clone();
                    menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        if let Some(dashboard) = dashboard.upgrade() {
                            dashboard.update(cx, |this, cx| {
                                this.inbox_status_filter = filter;
                                this.clear_hidden_update_selection();
                                this.reset_update_list_scroll();
                                cx.notify();
                            });
                        }
                    }))
                })
            })
            .into_any_element()
    }

    pub(super) fn render_updates(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let unread = self.unread_count();
        let visible_groups = visible_inbox_groups(self);
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
                h_flex()
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
                    .when(mobile, |this| this.flex_col().items_stretch())
                    .child(
                        h_flex().min_w_0().justify_between().child(
                            h_flex().min_w_0().gap_2().child(
                                div()
                                    .id("updates-heading")
                                    .accessibility_id("inbox-heading")
                                    .debug_selector(|| "updates-heading".to_owned())
                                    .role(gpui_kit::accesskit::Role::Heading)
                                    .aria_label("Activity")
                                    .text_lg()
                                    .font_semibold()
                                    .child("Activity"),
                            ),
                        ),
                    )
                    .child(
                        h_flex()
                            .id("updates-filters")
                            .debug_selector(|| "updates-filters".to_owned())
                            .min_w_0()
                            .flex_1()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("updates-filter-unread")
                                    .compact()
                                    .accessibility_id("updates-filter-unread")
                                    .selected(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::Unread,
                                    ))
                                    .toggled(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::Unread,
                                    ))
                                    .when(self.update_filter == UpdateFilter::Unread, |this| {
                                        this.bg(cx.theme().blue.opacity(0.14))
                                            .text_color(cx.theme().blue)
                                            .border_1()
                                            .border_color(cx.theme().blue)
                                    })
                                    .label(format!("Unread events · {unread}"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_update_filter(UpdateFilter::Unread, cx)
                                    })),
                            )
                            .child(
                                Button::new("updates-filter-all")
                                    .compact()
                                    .accessibility_id("updates-filter-all")
                                    .selected(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::All,
                                    ))
                                    .toggled(update_filter_is_selected(
                                        self.update_filter,
                                        UpdateFilter::All,
                                    ))
                                    .when(self.update_filter == UpdateFilter::All, |this| {
                                        this.bg(cx.theme().blue.opacity(0.14))
                                            .text_color(cx.theme().blue)
                                            .border_1()
                                            .border_color(cx.theme().blue)
                                    })
                                    .label(format!("All · {}", self.update_groups.len()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_update_filter(UpdateFilter::All, cx)
                                    })),
                            )
                            .when_some(self.inbox_search_input.clone(), |this, input| {
                                this.child(
                                    div()
                                        .id("inbox-search-wrap")
                                        .debug_selector(|| "inbox-search".to_owned())
                                        .min_w_0()
                                        .when(!mobile, |this| this.flex_1())
                                        .when(mobile, |this| this.w_full())
                                        .child(
                                            Input::new(&input)
                                                .cleanable(true)
                                                .prefix(Icon::new(IconName::Search))
                                                .accessibility_id("inbox-search")
                                                .aria_label("Search issues, keys, or activity")
                                                .min_w_0()
                                                .w_full(),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .id("inbox-status-filter")
                                    .debug_selector(|| "inbox-status-filter".to_owned())
                                    .accessibility_id("inbox-status-filter")
                                    .min_w_0()
                                    .w(gpui_kit::rems(9.))
                                    .child(
                                        div()
                                            .id("inbox-status-trigger-wrap")
                                            .debug_selector(|| "inbox-status-trigger".to_owned())
                                            .child(self.inbox_status_dropdown(cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("inbox-refresh-wrap")
                                    .debug_selector(|| "inbox-refresh".to_owned())
                                    .child(
                                        Button::new("inbox-refresh")
                                            .compact()
                                            .ghost()
                                            .icon(IconName::RefreshCw)
                                            .accessibility_label("Refresh Jira")
                                            .tooltip("Refresh Jira")
                                            .disabled(self.operation_in_progress)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.begin_refresh(window, cx)
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .id("mark-all-read-wrap")
                                    .debug_selector(|| "mark-all-read".to_owned())
                                    .child(
                                        Button::new("mark-all-read")
                                            .compact()
                                            .ghost()
                                            .disabled(unread == 0 || self.operation_in_progress)
                                            .label("Mark all read")
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.mark_all_read(cx)
                                                }),
                                            ),
                                    ),
                            ),
                    )
                    .when(mobile, |this| {
                        this.child(
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
                        )
                    }),
            )
            .child(
                h_flex()
                    .id("updates-workspace")
                    .debug_selector(|| "updates-workspace".to_owned())
                    .flex_1()
                    .w_full()
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
        let no_filter_matches = !self.inbox_query.trim().is_empty()
            || self.inbox_status_filter != IssueStatusFilter::All;
        h_flex()
                    .id("update-list")
                    .debug_selector(|| "update-list".to_owned())
                    .accessibility_id("update-list")
                    .role(gpui_kit::accesskit::Role::Group)
                    .aria_label("Local Jira activity")
                    .flex_1()
                    .when(!layout.is_mobile(), |this| this.flex_grow(1.5))
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
                                        .aria_label(if no_filter_matches {
                                            "No matching local activity. Change the search or status filter."
                                        } else if self.update_filter == UpdateFilter::Unread {
                                            "You are all caught up. New local updates will appear after refresh."
                                        } else {
                                            "No local updates yet. Refresh to check Jira activity."
                                        })
                                        .child(gpui_kit::component::empty::Empty::new().header(
                                            gpui_kit::component::empty::EmptyHeader::new()
                                                .title(gpui_kit::component::empty::EmptyTitle::new().child(
                                                    if no_filter_matches {
                                                        "No matching activity"
                                                    } else if self.update_filter == UpdateFilter::Unread {
                                                        "You are all caught up"
                                                    } else {
                                                        "No local updates yet"
                                                    },
                                                ))
                                                .description(
                                                    gpui_kit::component::empty::EmptyDescription::new()
                                                        .child(if no_filter_matches {
                                                            "Change the search or status filter to see activity."
                                                        } else if self.update_filter == UpdateFilter::Unread {
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
            visible_inbox_groups(self)
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
            })
            .cloned();
        let has_cached_issue_detail = issue.as_ref().is_some_and(issue_has_cached_detail);
        let issue_view = issue
            .as_ref()
            .map(|issue| IssueViewModel::from_domain(issue, &self.users));
        let palette = RichTextPalette {
            foreground: cx.theme().foreground,
            muted: cx.theme().muted_foreground,
            border: cx.theme().border,
            code_surface: cx.theme().muted.opacity(0.18),
            link: cx.theme().link,
            info: cx.theme().link,
            warning: cx.theme().warning,
            success: cx.theme().success,
            danger: cx.theme().danger,
        };
        let mobile = layout.is_mobile();
        let issue_id = group.issue_id.clone();
        let status_label = issue.as_ref().map(|issue| issue.status.name.clone());
        let title = issue.as_ref().map_or_else(
            || group.issue_summary.clone(),
            |issue| issue.summary.clone(),
        );
        let assignee = issue_view
            .as_ref()
            .map_or_else(|| "Unassigned".to_owned(), |view| view.assignee.clone());
        let priority = issue
            .as_ref()
            .and_then(|issue| issue.priority.name.clone())
            .unwrap_or_else(|| "No priority".to_owned());
        let description_content = issue
            .as_ref()
            .filter(|_| has_cached_issue_detail)
            .map(|issue| {
                issue
                    .rich_description
                    .as_ref()
                    .map(|document| {
                        let states = if self.selected_issue.as_ref() == Some(&group.issue_id) {
                            self.selected_image_states.clone()
                        } else {
                            RichImageRenderStates::default()
                        };
                        render_rich_text(document, palette, &states, 1, ImageSource::ResolvedAdf)
                    })
                    .unwrap_or_else(|| {
                        div()
                            .text_sm()
                            .child(
                                issue
                                    .description_text
                                    .clone()
                                    .unwrap_or_else(|| "No description supplied.".to_owned()),
                            )
                            .into_any_element()
                    })
            });
        let description_accessible_text = issue
            .as_ref()
            .and_then(|issue| {
                issue.description_text.clone().or_else(|| {
                    issue
                        .rich_description
                        .as_ref()
                        .map(jira_domain::RichTextDocument::plain_text)
                })
            })
            .unwrap_or_else(|| "No description supplied.".to_owned());
        let linked_issue_rows = issue
            .as_ref()
            .map(|issue| {
                issue
                    .linked_issues
                    .iter()
                    .enumerate()
                    .map(|(index, linked)| {
                        let key = linked.key.clone();
                        h_flex()
                            .id(("inbox-linked-issue", index))
                            .accessibility_id(format!("inbox-linked-issue-{index}"))
                            .role(gpui_kit::accesskit::Role::Group)
                            .aria_label(format!(
                                "{}: {}",
                                linked.key,
                                linked.summary.as_deref().unwrap_or("No summary supplied")
                            ))
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(linked.relationship.clone()),
                            )
                            .child(
                                Button::new(("inbox-linked-key", index))
                                    .compact()
                                    .accessibility_id(format!("inbox-linked-key-{index}"))
                                    .debug_selector(move || format!("inbox-linked-key-{index}"))
                                    .label(key.to_string())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.issue_edit_flow.is_submitting() {
                                            this.section = Section::Issues;
                                            this.mobile_detail_open = mobile;
                                        }
                                        this.open_related_issue_key(key.clone(), cx);
                                    })),
                            )
                            .child(
                                div().min_w_0().flex_1().truncate().text_sm().child(
                                    linked
                                        .summary
                                        .clone()
                                        .unwrap_or_else(|| "No summary supplied".to_owned()),
                                ),
                            )
                            .when_some(linked.status.clone(), |this, status| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded(cx.theme().radius)
                                        .bg(cx.theme().muted)
                                        .text_xs()
                                        .child(status),
                                )
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let linked_issues_view = (!linked_issue_rows.is_empty()).then(|| {
            v_flex()
                .id("update-reading-linked-issues")
                .accessibility_id("update-reading-linked-issues")
                .role(gpui_kit::accesskit::Role::Group)
                .aria_label(format!(
                    "Linked issues: {}",
                    issue
                        .as_ref()
                        .map(|issue| issue
                            .linked_issues
                            .iter()
                            .map(|linked| linked.key.to_string())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                ))
                .gap_2()
                .child(div().text_sm().font_semibold().child("Linked issues"))
                .children(linked_issue_rows)
                .into_any_element()
        });
        let body = v_flex()
            .id("update-reading-body")
            .debug_selector(|| "update-reading-body".to_owned())
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .min_w_0()
            .overflow_x_hidden()
            .gap_3()
            .p(gpui_kit::rems(layout.detail_padding() / 16.))
            .child(
                h_flex()
                    .id("inbox-reading-key")
                    .accessibility_id("inbox-reading-key")
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(div().text_base().font_semibold().child(group.issue_key.clone()))
                    .when_some(status_label, |this, status| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().blue.opacity(0.16))
                                .text_xs()
                                .child(status),
                        )
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .whitespace_normal()
                    .text_xl()
                    .font_semibold()
                    .child(title),
            )
            .child(
                    h_flex()
                        .id("inbox-reading-metadata")
                        .accessibility_id("inbox-reading-metadata")
                        .min_w_0()
                        .flex_wrap()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(assignee)
                        .child("·")
                        .child(priority)
                        .child("·")
                        .child(group.latest_occurred_at.clone()),
            )
            .when_some(description_content, |this, description| {
                    this.child(
                        v_flex()
                            .id("update-reading-description")
                            .accessibility_id("update-reading-description")
                            .role(gpui_kit::accesskit::Role::Group)
                            .aria_label(format!("Description: {description_accessible_text}"))
                            .min_w_0()
                            .gap_2()
                            .child(div().text_sm().font_semibold().child("Description"))
                            .child(
                                div()
                                    .id("inbox-description")
                                    .p_3()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .min_w_0()
                                    .whitespace_normal()
                                    .text_sm()
                                    .child(description),
                            ),
                    )
                })
            .when_some(linked_issues_view, |this, links| this.child(links))
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
            .into_any_element();
        v_flex()
            .id("update-reading-pane")
            .debug_selector(|| "update-reading-pane".to_owned())
            .accessibility_id("update-reading-pane")
            .role(gpui_kit::accesskit::Role::Group)
            .aria_label(format!("{} local updates", group.issue_key))
            .flex_1()
            .when(!mobile, |this| this.flex_grow(1.))
            .h_full()
            .min_w_0()
            .min_h_0()
            .overflow_x_hidden()
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
                        .on_click(
                            cx.listener(|this, _, _, cx| this.close_mobile_update_detail(cx)),
                        ),
                )
            })
            .child(body)
            .child(
                h_flex()
                    .id("update-reading-actions")
                    .debug_selector(|| "update-reading-actions".to_owned())
                    .flex_shrink_0()
                    .w_full()
                    .p_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("update-open-full-details")
                            .compact()
                            .debug_selector(|| "update-open-full-details".to_owned())
                            .when(mobile, |this| this.h_11().w_full())
                            .label("Open full issue details")
                            .accessibility_id("update-open-full-details")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_update_issue(issue_id.clone(), mobile, cx)
                            })),
                    ),
            )
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
        let read_state = if group.unread { "Unread" } else { "Read" };
        let accessible_label = format!(
            "{read_state} update. Open {} ({}): {}",
            group.issue_key, issue_type, group.issue_summary
        );
        let open_area = div()
            .id(("update-open", index))
            .debug_selector(move || format!("update-open-{index}"))
            .accessibility_id(format!("update-open-{index}"))
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(accessible_label.clone())
            .tab_index(0)
            .flex()
            .h_auto()
            .items_start()
            .min_w_0()
            .when(!mobile, |this| this.flex_1())
            .when(mobile, |this| this.flex_none().w_full())
            .gap_3()
            .p_2()
            .border_1()
            .border_color(cx.theme().transparent)
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
            .focus_visible(|style| style.border_1().border_color(cx.theme().ring))
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
                    .when(group.unread, |this| this.bg(cx.theme().blue))
                    .when(!group.unread, |this| this.bg(cx.theme().muted)),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .flex_1()
                    .w_full()
                    .when(mobile, |this| this.flex_col().items_stretch())
                    .when(!mobile, |this| this.items_center())
                    .gap_3()
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
                            .child(
                                div()
                                    .id(format!("update-group-count-{index}"))
                                    .accessibility_id(format!("update-group-count-{index}"))
                                    .role(gpui_kit::accesskit::Role::Status)
                                    .aria_label(format!("{} activity events", group.events.len()))
                                    .px_2()
                                    .rounded_full()
                                    .bg(cx.theme().blue.opacity(0.16))
                                    .text_xs()
                                    .child(group.events.len().to_string()),
                            ),
                    )
                    .child(
                        div()
                            .id(format!("update-title-{index}"))
                            .debug_selector(move || format!("update-title-{index}"))
                            .min_w_0()
                            .flex_1()
                            .when(!mobile, |this| this.truncate())
                            .when(mobile, |this| {
                                this.flex_none().w_full().whitespace_normal().line_clamp(2)
                            })
                            .text_base()
                            .when(group.unread, |this| this.font_semibold())
                            .when(!group.unread, |this| this.font_normal())
                            .child(group.issue_summary.clone()),
                    )
                    .when_some(
                        self.domain_issues
                            .iter()
                            .find(|issue| issue.id == group.issue_id),
                        |this, issue| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().muted)
                                    .text_xs()
                                    .child(issue.status.name.clone()),
                            )
                        },
                    )
                    .when(mobile, |this| {
                        this.child(
                            div()
                                .id(format!("update-timestamp-{index}"))
                                .debug_selector(move || format!("update-timestamp-{index}"))
                                .min_w_0()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(group.latest_occurred_at.clone()),
                        )
                    }),
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
            .relative()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(
                self.selected_update_issue.as_ref() == Some(&group.issue_id),
                |this| this.bg(cx.theme().blue.opacity(0.08)),
            )
            .child(
                h_flex()
                    .id(("update-header-row", index))
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .when(mobile, |this| this.flex_col().items_stretch())
                    .gap_1()
                    .child(open_area)
                    .child(
                        h_flex()
                            .id(("update-actions", index))
                            .debug_selector(move || format!("update-actions-{index}"))
                            .flex_shrink_0()
                            .items_center()
                            .when(mobile, |this| this.w_full().justify_end())
                            .gap_1()
                            .when(!expanded, |this| {
                                let issue_id = issue_id.clone();
                                this.child(
                                    Button::new(("update-expand", index))
                                        .accessibility_id(format!("update-group-expand-{index}"))
                                        .debug_selector(move || {
                                            format!("update-group-expand-{index}")
                                        })
                                        .compact()
                                        .ghost()
                                        .icon(IconName::ChevronDown)
                                        .accessibility_label("Show activity")
                                        .tooltip(format!(
                                            "Show {} activity events",
                                            group.events.len()
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_update_group_expanded(issue_id.clone(), cx);
                                        })),
                                )
                            })
                            .when(expanded, |this| {
                                let issue_id = issue_id.clone();
                                this.child(
                                    Button::new(("update-collapse", index))
                                        .accessibility_id(format!("update-group-expand-{index}"))
                                        .debug_selector(move || {
                                            format!("update-group-expand-{index}")
                                        })
                                        .compact()
                                        .ghost()
                                        .icon(IconName::ChevronUp)
                                        .accessibility_label("Hide activity")
                                        .tooltip("Hide activity events")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_update_group_expanded(issue_id.clone(), cx);
                                        })),
                                )
                            })
                            .when(group.unread, |this| {
                                this.child(
                                    Button::new(("update-mark-read", index))
                                        .accessibility_id(format!("update-mark-read-{index}"))
                                        .ghost()
                                        .compact()
                                        .icon(IconName::Check)
                                        .accessibility_label("Mark read")
                                        .tooltip("Mark activity read")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.mark_group_read(issue_id.clone(), cx);
                                        })),
                                )
                            }),
                    ),
            )
            .when(expanded, |this| {
                this.child(
                    v_flex()
                        .id(format!("update-rows-{index}"))
                        .gap_1()
                        .px_2()
                        .children(rows.iter().take(visible_row_count).enumerate().map(
                            |(row_index, row)| {
                                self.update_row_element(index, row_index, row, group, mobile, cx)
                            },
                        )),
                )
            })
            .when(
                self.selected_update_issue.as_ref() == Some(&group.issue_id),
                |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(px(2.))
                            .bg(cx.theme().blue),
                    )
                },
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
            .accessibility_id(format!("update-row-{group_index}-{row_index}"))
            .role(gpui_kit::accesskit::Role::Group)
            .aria_label(format!("Activity event: {change}"))
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

    #[test]
    fn inbox_search_and_status_filters_are_local_and_use_status_categories() {
        let mut dashboard = Dashboard::from_sample_data();
        dashboard.section = Section::Updates;
        dashboard.search_query = "issue-page query stays here".to_owned();
        dashboard.status_filter = crate::presentation::IssueStatusFilter::Done;
        dashboard.inbox_query = "DESK-176".to_owned();
        dashboard.inbox_status_filter = crate::presentation::IssueStatusFilter::InProgress;

        let visible = super::visible_inbox_groups(&dashboard);
        assert_eq!(visible.len(), 1);
        let issue_id = dashboard.update_groups[visible[0]].issue_id.clone();
        let issue = dashboard
            .domain_issues
            .iter()
            .find(|issue| issue.id == issue_id)
            .expect("cached fixture issue should accompany its activity group");
        assert_eq!(issue.status.name, "In Review");
        assert_eq!(issue.status.category.as_deref(), Some("In progress"));

        dashboard.inbox_query = "Status:".to_owned();
        dashboard.inbox_status_filter = crate::presentation::IssueStatusFilter::All;
        assert!(super::visible_inbox_groups(&dashboard).iter().any(|index| {
            dashboard.update_groups[*index]
                .events
                .iter()
                .any(|event| event.change.contains("Status:"))
        }));
        assert_eq!(dashboard.search_query, "issue-page query stays here");
        assert_eq!(
            dashboard.status_filter,
            crate::presentation::IssueStatusFilter::Done
        );
        assert!(matches!(
            dashboard.remote_lookup,
            super::super::RemoteLookupState::Idle
        ));

        dashboard.selected_update_issue = Some(issue_id);
        dashboard.inbox_query = "no matching ticket".to_owned();
        dashboard.clear_hidden_update_selection();
        assert!(dashboard.selected_update_issue.is_none());

        dashboard.inbox_query.clear();
        dashboard.inbox_status_filter = crate::presentation::IssueStatusFilter::All;
        let removed_id = dashboard.update_groups[0].issue_id.clone();
        dashboard
            .domain_issues
            .retain(|issue| issue.id != removed_id);
        assert!(
            super::visible_inbox_groups(&dashboard)
                .iter()
                .any(|index| dashboard.update_groups[*index].issue_id == removed_id)
        );
        dashboard.inbox_status_filter = crate::presentation::IssueStatusFilter::InProgress;
        assert!(
            !super::visible_inbox_groups(&dashboard)
                .iter()
                .any(|index| dashboard.update_groups[*index].issue_id == removed_id)
        );
    }

    #[gpui_kit::test]
    fn inbox_event_rows_expand_and_collapse_with_the_compact_group_control(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let window = cx.open_window(
            gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(900.)),
            |_, _| {
                let mut dashboard = Dashboard::from_sample_data();
                dashboard.section = Section::Updates;
                dashboard
            },
        );
        let dashboard = window.root(cx).expect("dashboard root");
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        assert!(visual.debug_bounds("update-row-0-0").is_none());
        let issue_id = dashboard.read_with(&visual, |dashboard, _| {
            dashboard.update_groups[0].issue_id.clone()
        });
        visual.update(|_, cx| {
            dashboard.update(cx, |dashboard, cx| {
                dashboard.toggle_update_group_expanded(issue_id.clone(), cx);
            });
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("update-row-0-0").is_some());

        visual.update(|_, cx| {
            dashboard.update(cx, |dashboard, cx| {
                dashboard.toggle_update_group_expanded(issue_id.clone(), cx);
            });
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("update-row-0-0").is_none());
    }

    #[gpui_kit::test]
    fn mobile_ticket_summaries_wrap_inside_the_row_at_narrow_widths(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        for width in [320., 390.] {
            let window = cx.open_window(
                gpui_kit::size(gpui_kit::px(width), gpui_kit::px(800.)),
                |_, _| {
                    let mut dashboard = Dashboard::from_sample_data();
                    dashboard.section = Section::Updates;
                    dashboard.selected_update_issue = None;
                    dashboard
                },
            );
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let row = visual
                .debug_bounds("update-card-0")
                .expect("mobile compact ticket row should be visible");
            let title = visual
                .debug_bounds("update-title-0")
                .expect("ticket summary should be visible");
            let timestamp = visual
                .debug_bounds("update-timestamp-0")
                .expect("ticket timestamp should be visible");
            let actions = visual
                .debug_bounds("update-actions-0")
                .expect("mobile ticket actions should be visible");
            assert!(
                title.size.width >= gpui_kit::px(200.),
                "ticket summary should have useful width at {width}px: {title:?}"
            );
            assert!(title.size.height > gpui_kit::px(26.));
            assert!(title.origin.x >= row.origin.x);
            assert!(
                title.origin.x + title.size.width
                    <= row.origin.x + row.size.width + gpui_kit::px(1.),
                "ticket title should wrap within the row at {width}px: title={title:?}, row={row:?}"
            );
            assert!(
                actions.origin.y >= timestamp.origin.y + timestamp.size.height,
                "mobile actions should sit below the timestamp at {width}px: actions={actions:?}, timestamp={timestamp:?}"
            );
            assert!(
                actions.origin.x + actions.size.width
                    <= row.origin.x + row.size.width + gpui_kit::px(1.),
                "mobile actions should stay inside the row at {width}px: actions={actions:?}, row={row:?}"
            );
        }
    }

    #[gpui_kit::test]
    fn inbox_linked_issue_navigates_to_cached_issue_without_remote_lookup(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let window = cx.open_window(
            gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(900.)),
            |_, _| {
                let mut dashboard = Dashboard::from_sample_data();
                dashboard.section = Section::Updates;
                dashboard
            },
        );
        let dashboard = window.root(cx).expect("dashboard root");
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let linked_key = visual
            .debug_bounds("inbox-linked-key-0")
            .expect("cached DESK-184 reading pane should expose DESK-179 relationship");
        visual.simulate_click(
            gpui_kit::point(
                linked_key.origin.x + linked_key.size.width / 2.,
                linked_key.origin.y + linked_key.size.height / 2.,
            ),
            Default::default(),
        );
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let (section, selected_key, lookup_is_idle) =
            dashboard.read_with(&visual, |dashboard, _| {
                let selected_key = dashboard.selected_issue.as_ref().and_then(|selected| {
                    dashboard
                        .domain_issues
                        .iter()
                        .find(|issue| &issue.id == selected)
                        .map(|issue| issue.key.to_string())
                });
                (
                    dashboard.section,
                    selected_key,
                    matches!(
                        dashboard.remote_lookup,
                        super::super::RemoteLookupState::Idle
                    ),
                )
            });
        assert_eq!(section, Section::Issues);
        assert_eq!(selected_key.as_deref(), Some("DESK-179"));
        assert!(
            lookup_is_idle,
            "cached linked navigation must not start a remote lookup"
        );
        assert!(visual.debug_bounds("update-list").is_none());
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
        assert!(title.size.width > gpui_kit::px(0.));
        assert!(timestamp.origin.y >= title.origin.y + title.size.height);
        assert!(actions.origin.x >= row.origin.x);
        assert!(
            actions.origin.x + actions.size.width
                <= row.origin.x + row.size.width + gpui_kit::px(1.)
        );
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
                <= workspace.origin.y + workspace.size.height + gpui_kit::px(1.),
            "mobile pane escapes workspace: pane={pane:?}, workspace={workspace:?}"
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
            assert!(metadata.size.width > gpui_kit::px(0.));
            assert!(title.origin.x >= metadata.origin.x + metadata.size.width);
            assert!(
                title.size.width >= gpui_kit::px(80.),
                "ticket title is too narrow at {viewport_width}px: {title:?}"
            );
            assert!(actions.origin.y <= row.origin.y + row.size.height);
            assert!(row.size.height <= gpui_kit::px(90.));
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
