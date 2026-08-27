use super::*;

impl Dashboard {
    fn render_sidebar(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let rail = layout.is_rail();
        v_flex()
            .h_full()
            .w(px(layout.sidebar_width()))
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                h_flex()
                    .h(px(60.))
                    .px_3()
                    .gap_2()
                    .when(rail, |this| this.justify_center().px_2().gap_0())
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div()
                            .flex()
                            .size_9()
                            .items_center()
                            .justify_center()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().sidebar_primary)
                            .text_color(cx.theme().sidebar_primary_foreground)
                            .font_bold()
                            .child("JD"),
                    )
                    .child(v_flex().min_w_0().gap_0p5().when(!rail, |this| {
                        this.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_semibold()
                                .child(self.workspace_name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.workspace_members.clone()),
                        )
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_3()
                    .gap_1()
                    .when(!rail, |this| {
                        this.child(
                            div()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("Workspace"),
                        )
                    })
                    .child(self.nav_item(
                        "Issues",
                        Some(self.issues.len()),
                        self.section == Section::Issues,
                        Section::Issues,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Local updates",
                        Some(self.unread_count()),
                        self.section == Section::Updates,
                        Section::Updates,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Team tracker",
                        Some(team_issue_counts(&self.team_issues).1),
                        self.section == Section::Team,
                        Section::Team,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Settings",
                        None,
                        self.section == Section::Settings,
                        Section::Settings,
                        rail,
                        cx,
                    )),
            )
            .when(!rail, |this| {
                this.child(
                    v_flex()
                        .p_4()
                        .gap_1()
                        .border_t_1()
                        .border_color(cx.theme().sidebar_border)
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(self.site_label.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.mode_label.clone()),
                        ),
                )
            })
    }

    fn nav_item(
        &self,
        label: &'static str,
        count: Option<usize>,
        selected: bool,
        section: Section,
        rail: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon = match section {
            Section::Issues => IconName::LayoutDashboard,
            Section::Updates => IconName::Bell,
            Section::Team => IconName::CircleUser,
            Section::Settings => IconName::Settings2,
        };
        let count = count.unwrap_or_default();
        let icon_color = if selected {
            cx.theme().sidebar_accent_foreground
        } else {
            cx.theme().sidebar_foreground
        };
        let tooltip = if count > 0 {
            format!("{label} · {count}")
        } else {
            label.to_owned()
        };
        div()
            .id(label)
            .role(gpui::accesskit::Role::Button)
            .aria_label(tooltip)
            .aria_selected(selected)
            .tab_index(0)
            .w_full()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .accessibility_id(label)
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(cx.theme().sidebar_accent)
                    .border_l_2()
                    .border_color(cx.theme().primary)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().sidebar_accent))
            })
            .when(rail, |this| this.w(px(48.)).overflow_hidden().px_1())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.activate_section(section, cx);
            }))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                if is_activation_key(event) {
                    window.prevent_default();
                    this.activate_section(section, cx);
                }
            }))
            .focus(|style| style.border_1().border_color(cx.theme().primary))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .overflow_x_hidden()
                    .justify_center()
                    .gap_2()
                    .child(Icon::new(icon).text_color(icon_color))
                    .when(!rail, |this| {
                        this.child(div().flex_1().min_w_0().truncate().child(label))
                    })
                    .when(!rail && count > 0, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .min_w(px(26.))
                                .ml_auto()
                                .px_2()
                                .py_0p5()
                                .rounded_full()
                                .bg(cx.theme().muted)
                                .text_center()
                                .text_xs()
                                .child(count.to_string()),
                        )
                    }),
            )
    }

    fn activate_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if section == Section::Team
            && self
                .selected_issue
                .as_ref()
                .is_some_and(|selected| !self.team_issues.iter().any(|issue| &issue.id == selected))
        {
            if self.issue_edit_flow.is_submitting() {
                self.sync_message =
                    "Finish the confirmed Jira change before changing views".to_owned();
                cx.notify();
                return;
            }
            self.clear_selection_for_team_scope(cx);
        }
        self.section = section;
        cx.notify();
    }

    fn render_header(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        v_flex()
            .id("header")
            .debug_selector(|| "header".to_owned())
            .h(px(if mobile { 88. } else { 76. }))
            .px(px(if mobile { 12. } else { 20. }))
            .py(px(if mobile { 9. } else { 10. }))
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_lg()
                            .font_semibold()
                            .child(match self.section {
                                Section::Issues => "Jira issues",
                                Section::Updates => "Local updates",
                                Section::Team => "Team tracker",
                                Section::Settings => "Settings",
                            }),
                    )
                    .when(self.section != Section::Settings, |this| {
                        this.child(
                            Button::new("refresh")
                                .compact()
                                .primary()
                                .flex_shrink_0()
                                .label(if self.operation_in_progress {
                                    "Refreshing…"
                                } else {
                                    "Refresh"
                                })
                                .loading(self.operation_in_progress)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.begin_refresh(window, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(if mobile { 420. } else { 720. }))
                    .max_h(px(if mobile { 44. } else { 36. }))
                    .overflow_y_scrollbar()
                    .whitespace_normal()
                    .text_xs()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary.opacity(0.45))
                    .text_color(cx.theme().muted_foreground)
                    .child(self.sync_message.clone()),
            )
    }

    fn render_mobile_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(48.))
            .flex_shrink_0()
            .px_2()
            .gap_1()
            .items_center()
            .overflow_x_scrollbar()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("mobile-issues")
                    .compact()
                    .flex_1()
                    .min_w(px(76.))
                    .when(self.section == Section::Issues, |this| this.primary())
                    .label(format!("Issues · {}", self.issues.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Issues;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("mobile-updates")
                    .compact()
                    .flex_1()
                    .min_w(px(76.))
                    .when(self.section == Section::Updates, |this| this.primary())
                    .label(format!("Updates · {}", self.unread_count()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Updates;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("mobile-team")
                    .compact()
                    .flex_1()
                    .min_w(px(76.))
                    .when(self.section == Section::Team, |this| this.primary())
                    .label(format!("Team · {}", team_issue_counts(&self.team_issues).1))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.selected_issue.as_ref().is_some_and(|selected| {
                            !this.team_issues.iter().any(|issue| &issue.id == selected)
                        }) {
                            if this.issue_edit_flow.is_submitting() {
                                this.sync_message =
                                    "Finish the confirmed Jira change before changing views"
                                        .to_owned();
                                cx.notify();
                                return;
                            }
                            this.clear_selection_for_team_scope(cx);
                        }
                        this.section = Section::Team;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("mobile-settings")
                    .compact()
                    .flex_1()
                    .min_w(px(76.))
                    .when(self.section == Section::Settings, |this| this.primary())
                    .label("Settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Settings;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_status_combobox(window, cx);
        self.ensure_search_input(window, cx);
        self.ensure_comment_input(window, cx);
        self.ensure_settings_input(window, cx);
        self.ensure_team_input(window, cx);
        self.ensure_team_table(window, cx);
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let table_mode = team_table_mode_for_width(f32::from(window.viewport_size().width));
        if !layout.is_mobile() {
            self.detail_sidebar_width = px(clamped_team_detail_width(
                f32::from(self.detail_sidebar_width),
                f32::from(window.viewport_size().width),
                layout,
                table_mode,
            ));
        }
        let content = match self.section {
            Section::Issues => self.render_issues(layout, cx).into_any_element(),
            Section::Updates => self.render_updates(layout, cx).into_any_element(),
            Section::Team => self
                .render_team(
                    layout,
                    team_table_mode_for_width(f32::from(window.viewport_size().width)),
                    cx,
                )
                .into_any_element(),
            Section::Settings => self.render_settings(layout, cx).into_any_element(),
        };

        let main = v_flex()
            .h_full()
            .min_w_0()
            .flex_1()
            .child(self.render_header(layout, cx))
            .child(div().min_w_0().min_h_0().flex_1().child(content));

        if layout.is_mobile() {
            v_flex()
                .size_full()
                .min_w_0()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_mobile_nav(cx))
                .child(main)
        } else {
            h_flex()
                .size_full()
                .min_w_0()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_sidebar(layout, cx))
                .child(main)
        }
    }
}
