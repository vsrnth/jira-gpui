use super::*;

impl Dashboard {
    pub(super) fn issue_key_with_icon(
        &self,
        key: impl Into<String>,
        issue_type: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .debug_selector(|| "update-key".to_owned())
            .flex_shrink_0()
            .gap_1()
            .child(Icon::new(issue_type_icon(issue_type)).text_color(cx.theme().link))
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().link)
                    .child(key.into()),
            )
            .into_any_element()
    }

    pub(super) fn priority_badge(&self, label: String, cx: &mut Context<Self>) -> AnyElement {
        let (icon, tone) = priority_semantics(&label);
        let color = self.priority_color(tone, cx);
        h_flex()
            .min_w_0()
            .gap_1()
            .child(Icon::new(icon).text_color(color))
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    fn priority_color(&self, tone: PriorityTone, cx: &mut Context<Self>) -> gpui::Hsla {
        match tone {
            PriorityTone::Critical => cx.theme().danger,
            PriorityTone::Elevated => cx.theme().warning,
            PriorityTone::Neutral | PriorityTone::Unknown => cx.theme().muted_foreground,
            PriorityTone::Low | PriorityTone::Minimal => cx.theme().link,
        }
    }

    pub(super) fn render_issues(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mobile = layout.is_mobile();
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
                    .h(gpui::rems(if mobile { 3.625 } else { 3.25 }))
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
                        div()
                            .debug_selector(|| "issue-list-summary".to_owned())
                            .flex_shrink_0()
                            .font_semibold()
                            .child(format!("{} Jira issues", self.issues.len())),
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
                let lookup_loading = matches!(self.remote_lookup, RemoteLookupState::Loading { .. });
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
                                        "Search Jira"
                                    })
                                    .loading(lookup_loading)
                                    .disabled(lookup_loading)
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
                                        "Search Jira"
                                    })
                                    .loading(lookup_loading)
                                    .disabled(lookup_loading)
                                    .on_click(cx.listener(|this, _, _, cx| this.search_jira(cx))),
                            ),
                    )
                }
            })
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
                    .child(self.status_filter_dropdown()),
            )
            .child(
                v_flex()
                    .id("issue-list")
                    .min_h_0()
                    .flex_1()
                    .when(mobile, |this| this.w_full())
                    .overflow_y_scrollbar()
                    .when_some(self.remote_lookup_view(), |this, issue| {
                        this.child(self.issue_row_with_label(
                            &issue,
                            "Jira lookup result",
                            layout,
                            cx,
                        ))
                    })
                    .children(
                        self.issues
                            .iter()
                            .map(|issue| self.issue_row(issue, layout, cx)),
                    )
                    .when(self.issues.is_empty() && self.remote_lookup_view().is_none(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_2()
                                .p_6()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.domain_issues.is_empty() {
                                    "No Jira issues loaded yet. Refresh to check your assigned or watched view."
                                } else {
                                    "No issues match the current search and status filters."
                                }),
                        )
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
                        .role(gpui::accesskit::Role::Status)
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
                        .role(gpui::accesskit::Role::Alert)
                        .aria_label(format!(
                            "Jira lookup failed for {query}: {}",
                            copy.message()
                        ))
                        .min_w_0()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(cx.theme().danger.opacity(0.45))
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(div().font_semibold().child("Jira lookup failed"))
                        .child(div().min_w_0().child(copy.message()))
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
            self.selected_issue_core.as_ref(),
            &self.users,
        )
        .or_else(|| {
            let selected = self.selected_issue.as_ref()?;
            self.team_issues
                .iter()
                .find(|issue| &issue.id == selected)
                .map(|issue| IssueViewModel::from_domain(issue, &self.users))
        })
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
        let accessible_label = format!("Open {}: {}", issue.key, issue.summary);
        div()
            .id(format!("issue-row-{}", issue.id))
            .debug_selector(move || format!("issue-row-{debug_issue_id}"))
            .accessibility_id(accessibility_issue_id)
            .role(gpui::accesskit::Role::Button)
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
                this.bg(cx.theme().list_active).child(
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
                                    .child(self.issue_key_with_icon(
                                        issue.key.clone(),
                                        &issue.issue_type,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(issue.issue_type.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_shrink_0()
                                    .truncate()
                                    .px_2()
                                    .py_1()
                                    .rounded_full()
                                    .bg(cx.theme().secondary)
                                    .border_1()
                                    .border_color(cx.theme().link.opacity(0.4))
                                    .text_color(cx.theme().secondary_foreground)
                                    .text_xs()
                                    .font_semibold()
                                    .child(issue.status.clone()),
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
                                    .child(self.priority_badge(issue.priority.clone(), cx)),
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
