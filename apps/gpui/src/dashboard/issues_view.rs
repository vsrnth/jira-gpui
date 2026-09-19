use std::rc::Rc;

use super::virtual_rows::{
    RowMeasureKey, cached_height, measured_row, retain_identities, revision_hash,
};
use super::*;

fn issue_row_height(layout: LayoutMode) -> f32 {
    // The row clamps summary and mobile timestamps to two lines. Keep the
    // virtual slot large enough for the wrapped mobile metadata row as well.
    if layout.is_mobile() { 172. } else { 136. }
}

fn issue_row_height_with_label(layout: LayoutMode, labelled: bool) -> f32 {
    issue_row_height(layout) + if labelled { 24. } else { 0. }
}

fn issue_row_accessible_label(
    key: &str,
    issue_type: &str,
    summary: &str,
    priority: &str,
) -> String {
    format!("Open {key} ({issue_type}): {summary} · Priority: {priority}")
}

pub(super) fn issue_count_label(visible: usize, total: usize, filtered: bool) -> String {
    if filtered {
        format!("{visible} of {total} Jira issues")
    } else {
        format!("{visible} Jira issues")
    }
}

/// Resolve a semantic issue-type tone through the active theme's contrast-aware base colors.
fn issue_type_color_for_theme(
    tone: IssueTypeTone,
    theme: &gpui_kit::component::Theme,
) -> gpui_kit::Hsla {
    match tone {
        IssueTypeTone::Red => theme.red,
        IssueTypeTone::Green => theme.green,
        IssueTypeTone::Blue => theme.blue,
        // gpui-component's magenta theme slot is purple-600 in light mode and purple-400 in
        // dark mode, keeping the Epic treatment vivid while retaining foreground contrast.
        IssueTypeTone::Purple => theme.magenta,
        IssueTypeTone::Neutral => theme.muted_foreground,
    }
}

impl Dashboard {
    pub(super) fn issue_key_label(
        &self,
        key: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .debug_selector(|| "update-key".to_owned())
            .flex_shrink_0()
            .text_xs()
            .font_semibold()
            .text_color(cx.theme().link)
            .child(key.into())
            .into_any_element()
    }

    pub(super) fn issue_type_label_with_icon(
        &self,
        label: impl Into<String>,
        id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = label.into();
        let type_semantics = issue_type_semantics(&label);
        h_flex()
            .id(format!("issue-type-{}", id.into()))
            .min_w_0()
            .gap_1()
            .text_xs()
            .text_color(self.issue_type_color(type_semantics.tone, cx))
            .role(gpui_kit::accesskit::Role::TextRun)
            .aria_label(format!("Issue type: {label}"))
            .child(
                Icon::new(type_semantics.icon)
                    .size_4()
                    .flex_shrink_0()
                    .text_color(self.issue_type_color(type_semantics.tone, cx)),
            )
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    pub(super) fn priority_badge(
        &self,
        label: String,
        accessibility_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.priority_badge_with_role(
            label,
            accessibility_id,
            gpui_kit::accesskit::Role::TextRun,
            cx,
        )
    }

    pub(super) fn priority_badge_group(
        &self,
        label: String,
        accessibility_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.priority_badge_with_role(
            label,
            accessibility_id,
            gpui_kit::accesskit::Role::Group,
            cx,
        )
    }

    fn priority_badge_with_role(
        &self,
        label: String,
        accessibility_id: impl Into<String>,
        role: gpui_kit::accesskit::Role,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (icon, tone) = priority_semantics(&label);
        let color = self.priority_color(tone, cx);
        let accessibility_id = accessibility_id.into();
        h_flex()
            .id(accessibility_id.clone())
            .accessibility_id(accessibility_id)
            .debug_selector(|| "priority-badge".to_owned())
            .role(role)
            .aria_label(format!("Priority: {label}"))
            .min_w_0()
            .items_center()
            .gap_1()
            .child(Icon::new(icon).size_4().flex_shrink_0().text_color(color))
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    fn priority_color(&self, tone: PriorityTone, cx: &mut Context<Self>) -> gpui_kit::Hsla {
        match tone {
            PriorityTone::Critical => cx.theme().danger,
            PriorityTone::Elevated => cx.theme().warning,
            PriorityTone::Neutral | PriorityTone::Unknown => cx.theme().muted_foreground,
            PriorityTone::Low | PriorityTone::Minimal => cx.theme().link,
        }
    }

    pub(super) fn issue_type_color(
        &self,
        tone: IssueTypeTone,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Hsla {
        issue_type_color_for_theme(tone, cx.theme())
    }

    pub(super) fn render_issues(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let active_query = !self.search_query.trim().is_empty();
        let active_status = self.status_filter != IssueStatusFilter::All;
        let has_active_filters = active_query || active_status;
        let lookup_enabled =
            crate::presentation::normalized_issue_key(&self.search_query).is_some();
        let remote_issue = self.remote_lookup_view();
        let has_remote_issue = remote_issue.is_some();
        let retained_issue_ids = self
            .issues
            .iter()
            .map(|issue| issue.key.clone())
            .chain(std::iter::once("remote-lookup".to_owned()).filter(|_| has_remote_issue))
            .collect();
        retain_identities(&self.issues_row_measurements, &retained_issue_ids);
        let issue_count = issue_count_label(
            self.issues.len(),
            self.domain_issues.len(),
            has_active_filters,
        );
        let issue_list = v_flex()
            .h_full()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .id("issue-list-header")
                    .debug_selector(|| "issue-list-header".to_owned())
                    .h(gpui_kit::rems(if mobile { 3.625 } else { 3.25 }))
                    .when(mobile, |this| this.px_3())
                    .when(!mobile, |this| this.px_4())
                    .justify_center()
                    .flex_shrink_0()
                    .min_w_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        h_flex()
                            .id("issue-list-summary")
                            .accessibility_id("issue-list-summary")
                            .debug_selector(|| "issue-list-summary".to_owned())
                            .flex_shrink_0()
                            .font_semibold()
                            .role(gpui_kit::accesskit::Role::Status)
                            .aria_label(issue_count.clone())
                            .child(issue_count),
                    )
                    .child(
                        div()
                            .debug_selector(|| "issue-list-context".to_owned())
                            .min_w_0()
                            .truncate()
                            .child("Assigned or watched · Updated newest first"),
                    )
                    .into_any_element(),
            )
            .when_some(self.search_input.clone(), |this, input| {
                let lookup_loading =
                    matches!(self.remote_lookup, RemoteLookupState::Loading { .. });
                if mobile {
                    this.child(
                        v_flex()
                            .gap_1()
                            .px_2()
                            .py_2()
                            .min_w_0()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                Input::new(&input)
                                    .cleanable(true)
                                    .accessibility_id("issue-search")
                                    .aria_label("Issue key or summary")
                                    .min_w_0()
                                    .w_full(),
                            )
                            .child(
                                Button::new("search-jira")
                                    .compact()
                                    .w_full()
                                    .accessibility_id("issue-search-submit")
                                    .label(if lookup_loading {
                                        "Searching Jira…"
                                    } else {
                                        "Find key"
                                    })
                                    .loading(lookup_loading)
                                    .disabled(lookup_loading || !lookup_enabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.search_jira(cx))),
                            ),
                    )
                } else {
                    this.child(
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .min_w_0()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                Input::new(&input)
                                    .cleanable(true)
                                    .accessibility_id("issue-search")
                                    .aria_label("Issue key or summary")
                                    .min_w_0()
                                    .flex_1(),
                            )
                            .child(
                                Button::new("search-jira")
                                    .compact()
                                    .accessibility_id("issue-search-submit")
                                    .label(if lookup_loading {
                                        "Searching Jira…"
                                    } else {
                                        "Find key"
                                    })
                                    .loading(lookup_loading)
                                    .disabled(lookup_loading || !lookup_enabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.search_jira(cx))),
                            ),
                    )
                }
            })
            .child(
                div()
                    .id("issue-search-help")
                    .accessibility_id("issue-search-help")
                    .debug_selector(|| "issue-search-help".to_owned())
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .px_3()
                    .pb_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Filter loaded issues as you type. Find key looks up a Jira key."),
            )
            .when(mobile, |this| {
                this.when_some(self.remote_lookup_list_status(cx), |this, status| {
                    this.child(status)
                })
            })
            .child(
                h_flex()
                    .h_11()
                    .px_3()
                    .gap_1()
                    .flex_shrink_0()
                    .min_w_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .min_w_0()
                            .flex_1()
                            .child(self.status_filter_dropdown()),
                    )
                    .when(has_active_filters, |this| {
                        this.child(
                            Button::new("clear-issue-filters")
                                .compact()
                                .ghost()
                                .flex_shrink_0()
                                .accessibility_id("issue-filters-clear")
                                .label("Clear filters")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_issue_filters(window, cx);
                                })),
                        )
                    }),
            )
            .child(
                v_flex()
                    .id("issue-list")
                    .accessibility_id("issue-list")
                    .role(gpui_kit::accesskit::Role::Group)
                    .aria_label("Jira issues")
                    .min_h_0()
                    .flex_1()
                    .vertical_scrollbar(&self.issues_scroll_handle)
                    .when(mobile, |this| this.w_full())
                    .child(if self.issues.is_empty() && !has_remote_issue {
                        div()
                            .id("issues-empty")
                            .accessibility_id("issues-empty")
                            .role(gpui_kit::accesskit::Role::Status)
                            .aria_label(if self.domain_issues.is_empty() {
                                "No Jira issues loaded yet"
                            } else {
                                "No issues match the current search and status filters"
                            })
                            .child(gpui_kit::component::empty::Empty::new().header(
                                gpui_kit::component::empty::EmptyHeader::new()
                                    .title(gpui_kit::component::empty::EmptyTitle::new().child(
                                        if self.domain_issues.is_empty() {
                                            "No Jira issues loaded yet"
                                        } else {
                                            "No matching issues"
                                        },
                                    ))
                                    .description(
                                        gpui_kit::component::empty::EmptyDescription::new().child(
                                            if self.domain_issues.is_empty() {
                                                "Refresh to check your assigned or watched view."
                                            } else {
                                                "Try changing the search or status filters."
                                            },
                                        ),
                                    ),
                            ))
                            .into_any_element()
                    } else {
                        {
                            let remote_count = usize::from(has_remote_issue);
                            let remote_issue_for_rows = remote_issue.clone();
                            let heights = Rc::new(
                                std::iter::repeat_n(
                                    gpui_kit::size(
                                        gpui_kit::px(0.),
                                        cached_height(
                                            &self.issues_row_measurements,
                                            &RowMeasureKey {
                                                identity: "remote-lookup".to_owned(),
                                                revision: remote_issue
                                                    .as_ref()
                                                    .map(|issue| {
                                                        revision_hash((
                                                            &issue.key,
                                                            &issue.summary,
                                                            &issue.status,
                                                            &issue.issue_type,
                                                            &issue.assignee,
                                                            &issue.priority,
                                                            &issue.updated,
                                                        ))
                                                    })
                                                    .unwrap_or_default(),
                                                layout: layout as u8,
                                                expanded: false,
                                            },
                                            issue_row_height_with_label(layout, true),
                                        ),
                                    ),
                                    remote_count,
                                )
                                .chain(self.issues.iter().map(|issue| {
                                    gpui_kit::size(
                                        gpui_kit::px(0.),
                                        cached_height(
                                            &self.issues_row_measurements,
                                            &RowMeasureKey {
                                                identity: issue.key.clone(),
                                                revision: revision_hash((
                                                    &issue.key,
                                                    &issue.summary,
                                                    &issue.status,
                                                    &issue.issue_type,
                                                    &issue.assignee,
                                                    &issue.priority,
                                                    &issue.updated,
                                                )),
                                                layout: layout as u8,
                                                expanded: false,
                                            },
                                            issue_row_height(layout),
                                        ),
                                    )
                                }))
                                .collect::<Vec<_>>(),
                            );
                            gpui_kit::component::v_virtual_list(
                                cx.entity(),
                                "issue-list-virtual",
                                heights,
                                move |this, range, _, cx| {
                                    range
                                        .map(|index| {
                                            if let (0, Some(issue)) =
                                                (index, remote_issue_for_rows.as_ref())
                                            {
                                                return measured_row(
                                                    RowMeasureKey {
                                                        identity: "remote-lookup".to_owned(),
                                                        revision: revision_hash((
                                                            &issue.key,
                                                            &issue.summary,
                                                            &issue.status,
                                                            &issue.issue_type,
                                                            &issue.assignee,
                                                            &issue.priority,
                                                            &issue.updated,
                                                        )),
                                                        layout: layout as u8,
                                                        expanded: false,
                                                    },
                                                    this.issues_row_measurements.clone(),
                                                    cx.entity().downgrade(),
                                                    this.issue_row_with_label(
                                                        issue,
                                                        "Jira lookup result",
                                                        layout,
                                                        cx,
                                                    ),
                                                );
                                            }
                                            let issue = &this.issues[index - remote_count];
                                            measured_row(
                                                RowMeasureKey {
                                                    identity: issue.key.clone(),
                                                    revision: revision_hash((
                                                        &issue.key,
                                                        &issue.summary,
                                                        &issue.status,
                                                        &issue.issue_type,
                                                        &issue.assignee,
                                                        &issue.priority,
                                                        &issue.updated,
                                                    )),
                                                    layout: layout as u8,
                                                    expanded: false,
                                                },
                                                this.issues_row_measurements.clone(),
                                                cx.entity().downgrade(),
                                                this.issue_row(issue, layout, cx),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .track_scroll(&self.issues_scroll_handle)
                            .into_any_element()
                        }
                    }),
            )
            .into_any_element();

        let panes = match issues_pane_mode(layout, self.mobile_detail_open) {
            IssuesPaneMode::ListAndDetail => {
                let (list_min, list_max) = layout.issue_list_range();
                let detail = v_flex()
                    .size_full()
                    .min_w_0()
                    .child(self.issue_detail(layout, cx));
                h_resizable(layout.resizable_id())
                    .child(
                        resizable_panel()
                            .size(px(layout.issue_list_width()))
                            .size_range(px(list_min)..px(list_max))
                            .flex_none()
                            .child(issue_list),
                    )
                    .child(
                        resizable_panel()
                            .size_range(px(layout.detail_min_width())..px(4_096.))
                            .child(detail),
                    )
                    .into_any_element()
            }
            IssuesPaneMode::ListOnly => issue_list,
            IssuesPaneMode::DetailOnly => v_flex()
                .size_full()
                .min_w_0()
                .child(
                    h_flex()
                        .h_11()
                        .px_3()
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            Button::new("mobile-detail-back")
                                .compact()
                                .ghost()
                                .label("Back to issues")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.mobile_detail_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(self.issue_detail(layout, cx))
                .into_any_element(),
        };

        h_flex().size_full().min_w_0().child(panes)
    }

    fn status_filter_dropdown(&self) -> impl IntoElement {
        let state = self
            .status_combobox
            .as_ref()
            .expect("status combobox initialized before issue rendering");
        Combobox::new(state)
            .w_full()
            .cleanable(true)
            .footer(|_, cx| {
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Select one or more statuses"),
                    )
                    .child(
                        Button::new("status-filter-done")
                            .secondary()
                            .outline()
                            .compact()
                            .label("Done")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(Cancel), cx);
                            }),
                    )
            })
            .render_trigger(|trigger, _, _| {
                let selection = IssueStatusSelection::from_values(
                    trigger.selection().iter().map(|(_, item)| *item.value()),
                );
                div()
                    .id("issue-status-filter")
                    .accessibility_id("issue-status-filter")
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(format!(
                        "Filter status: {}",
                        status_filter_trigger_label(selection)
                    ))
                    .min_w_0()
                    .w_full()
                    .truncate()
                    .child(status_filter_trigger_label(selection))
            })
    }

    pub(super) fn remote_lookup_view(&self) -> Option<IssueViewModel> {
        match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => {
                Some(IssueViewModel::from_domain(issue, &self.users))
            }
            RemoteLookupState::Idle
            | RemoteLookupState::Loading { .. }
            | RemoteLookupState::Error { .. } => None,
        }
    }

    fn remote_lookup_list_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.remote_lookup {
            RemoteLookupState::Loading { query } => {
                let query = super::detail_view::normalized_lookup_query(query);
                Some(
                    h_flex()
                        .id("remote-lookup-loading")
                        .debug_selector(|| "remote-lookup-loading".to_owned())
                        .role(gpui_kit::accesskit::Role::Status)
                        .aria_label(format!("Jira lookup in progress for {query}"))
                        .min_w_0()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(Spinner::new())
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(format!("Looking up {query}…")),
                        )
                        .into_any_element(),
                )
            }
            RemoteLookupState::Error { query, copy } => {
                let query = super::detail_view::normalized_lookup_query(query);
                Some(
                    v_flex()
                        .id("remote-lookup-error")
                        .debug_selector(|| "remote-lookup-error".to_owned())
                        .accessibility_id("remote-lookup-error")
                        .child(
                            gpui_kit::component::alert::Alert::error(
                                "remote-lookup-error-alert",
                                copy.message(),
                            )
                            .title(format!("Jira lookup failed for {query}")),
                        )
                        .into_any_element(),
                )
            }
            RemoteLookupState::Idle | RemoteLookupState::Loaded { .. } => None,
        }
    }

    pub(super) fn selected_issue_view(&self) -> Option<IssueViewModel> {
        selected_issue_view_from_sources(
            self.selected_issue.as_ref(),
            &self.issues,
            &self.domain_issues,
            &self.team_issues,
            self.selected_issue_core.as_ref(),
            &self.users,
        )
    }

    pub(super) fn comment_target_issue(&self) -> Option<&Issue> {
        match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(issue),
            RemoteLookupState::Idle
            | RemoteLookupState::Loading { .. }
            | RemoteLookupState::Error { .. } => self.selected_issue.as_ref().and_then(|id| {
                selected_issue_from_sources(
                    Some(id),
                    &self.domain_issues,
                    &self.team_issues,
                    self.selected_issue_core.as_ref(),
                )
            }),
        }
    }

    pub(super) fn issue_row(
        &self,
        issue: &IssueViewModel,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.issue_row_with_label(issue, "", layout, cx)
    }

    fn issue_row_with_label(
        &self,
        issue: &IssueViewModel,
        label: &str,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_issue.as_ref() == Some(&issue.id)
            || matches!(
                    &self.remote_lookup,
                    RemoteLookupState::Loaded { issue: remote, .. } if remote.id == issue.id
            );
        let issue_id = issue.id.clone();
        let keyboard_issue_id = issue.id.clone();
        let debug_issue_id = issue.id.clone();
        let accessibility_issue_id = format!("issue-row-{}", issue.key);
        let is_remote_result = !label.is_empty();
        let mobile = layout.is_mobile();
        let accessible_label = issue_row_accessible_label(
            &issue.key,
            &issue.issue_type,
            &issue.summary,
            &issue.priority,
        );
        div()
            .id(format!("issue-row-{}", issue.id))
            .debug_selector(move || format!("issue-row-{debug_issue_id}"))
            .accessibility_id(accessibility_issue_id)
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(accessible_label)
            .aria_selected(selected)
            .tab_index(0)
            .p_4()
            .gap_2()
            .items_start()
            .min_w_0()
            .w_full()
            .relative()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_2()
                        .bottom_2()
                        .left_0()
                        .w(px(3.))
                        .rounded_full()
                        .bg(cx.theme().list_active_border),
                )
            })
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().list_hover))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if !is_remote_result {
                    this.clear_remote_lookup();
                    this.select_issue(issue_id.clone(), cx, false);
                }
                this.mobile_detail_open = mobile;
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                if is_activation_key(event) {
                    window.prevent_default();
                    if !is_remote_result {
                        this.clear_remote_lookup();
                        this.select_issue(keyboard_issue_id.clone(), cx, false);
                    }
                    this.mobile_detail_open = mobile;
                    cx.notify();
                }
            }))
            // Keep pointer selection quiet; reserve the full-row ring for keyboard focus.
            .focus_visible(|style| style.border_1().border_color(cx.theme().ring))
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .text_base()
                    .text_color(cx.theme().foreground)
                    .child(
                        h_flex()
                            .min_w_0()
                            .justify_between()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_2()
                                    .child(self.issue_key_label(issue.key.clone(), cx))
                                    .child(self.issue_type_label_with_icon(
                                        issue.issue_type.clone(),
                                        issue.key.clone(),
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .id(format!("issue-status-{}", issue.key))
                                    .accessibility_id(format!("issue-status-{}", issue.key))
                                    .role(gpui_kit::accesskit::Role::TextRun)
                                    .aria_label(format!("Status: {}", issue.status))
                                    .child(
                                        gpui_kit::component::tag::Tag::secondary()
                                            .outline()
                                            .child(issue.status.clone()),
                                    ),
                            ),
                    )
                    .when(!label.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().link)
                                .child(label.to_owned()),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .line_clamp(2)
                            .text_sm()
                            .font_semibold()
                            .child(issue.summary.clone()),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .when(mobile, |this| this.flex_col().items_start().gap_1())
                            .when(!mobile, |this| this.justify_between())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .when(mobile, |this| this.w_full())
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .child(format!("{} ·", issue.assignee)),
                                    )
                                    .child(self.priority_badge(
                                        issue.priority.clone(),
                                        format!("priority-badge-list-{}", issue.key),
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .when(!mobile, |this| this.flex_shrink_0())
                                    .when(mobile, |this| {
                                        this.w_full().whitespace_normal().line_clamp(2)
                                    })
                                    .child(issue.updated.clone()),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{issue_row_accessible_label, issue_row_height, issue_type_color_for_theme};
    use crate::responsive::LayoutMode;
    use crate::semantic_icons::IssueTypeTone;

    #[test]
    fn issue_type_tones_resolve_to_theme_colors() {
        let theme = gpui_kit::component::Theme::default();
        assert_eq!(
            issue_type_color_for_theme(IssueTypeTone::Red, &theme),
            theme.red
        );
        assert_eq!(
            issue_type_color_for_theme(IssueTypeTone::Green, &theme),
            theme.green
        );
        assert_eq!(
            issue_type_color_for_theme(IssueTypeTone::Blue, &theme),
            theme.blue
        );
        assert_eq!(
            issue_type_color_for_theme(IssueTypeTone::Purple, &theme),
            theme.magenta
        );
        assert_eq!(
            issue_type_color_for_theme(IssueTypeTone::Neutral, &theme),
            theme.muted_foreground
        );
    }

    #[test]
    fn issue_row_accessible_label_retains_identity_and_priority() {
        assert_eq!(
            issue_row_accessible_label("DESK-179", "Task", "Improve caching", "Highest"),
            "Open DESK-179 (Task): Improve caching · Priority: Highest"
        );
    }

    #[test]
    fn virtual_issue_rows_reserve_mobile_metadata_wrap_space() {
        assert!(issue_row_height(LayoutMode::Mobile) > issue_row_height(LayoutMode::Standard));
        assert!(issue_row_height(LayoutMode::Standard) > 0.);
    }
}
