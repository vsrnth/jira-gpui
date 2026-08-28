use super::*;
use crate::responsive::{
    mobile_nav_item_width, sidebar_is_rail_for_viewport, sidebar_width_for_viewport,
};
use gpui_component::{Sizable as _, tooltip::Tooltip};

struct MobileNavItem {
    id: &'static str,
    label: &'static str,
    accessible_label: String,
    selected: bool,
    section: Section,
    width: f32,
}

impl Dashboard {
    fn render_sidebar(
        &self,
        layout: LayoutMode,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rail = sidebar_is_rail_for_viewport(layout, self.sidebar_collapsed, viewport_width);
        v_flex()
            .id("dashboard-sidebar")
            .debug_selector(|| "dashboard-sidebar".to_owned())
            .h_full()
            .w(px(sidebar_width_for_viewport(
                layout,
                self.sidebar_collapsed,
                viewport_width,
            )))
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
                    .child(if !rail {
                        h_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .size_8()
                                    .flex_shrink_0()
                                    .items_center()
                                    .justify_center()
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().sidebar_primary)
                                    .text_color(cx.theme().sidebar_primary_foreground)
                                    .font_bold()
                                    .child("JD"),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .flex_1()
                                    .gap_0p5()
                                    .child(
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
                                    ),
                            )
                            .child(self.render_sidebar_toggle(layout, false, cx))
                            .into_any_element()
                    } else if self.sidebar_collapsed && layout.supports_manual_sidebar_collapse() {
                        self.render_sidebar_toggle(layout, true, cx)
                            .into_any_element()
                    } else {
                        div()
                            .flex()
                            .size_8()
                            .items_center()
                            .justify_center()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().sidebar_primary)
                            .text_color(cx.theme().sidebar_primary_foreground)
                            .font_bold()
                            .child("JD")
                            .into_any_element()
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_3()
                    .when(rail, |this| this.p_2().items_center())
                    .gap_1()
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
                                .id("sidebar-sync-status")
                                .debug_selector(|| "sidebar-sync-status".to_owned())
                                .w_full()
                                .min_w_0()
                                .aria_label(self.sync_message.clone())
                                .max_h(px(72.))
                                .overflow_y_scrollbar()
                                .whitespace_normal()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.sync_message.clone()),
                        )
                        .when(self.refresh_visible(), |this| {
                            this.child(self.render_refresh_action(
                                "sidebar-refresh",
                                false,
                                true,
                                cx,
                            ))
                        })
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
            .when(rail && self.refresh_visible(), |this| {
                this.child(
                    v_flex()
                        .p_2()
                        .items_center()
                        .border_t_1()
                        .border_color(cx.theme().sidebar_border)
                        .child(self.render_refresh_action("sidebar-refresh", true, false, cx)),
                )
            })
    }

    fn render_sidebar_toggle(
        &self,
        layout: LayoutMode,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (icon, label) = if collapsed {
            (IconName::PanelLeftOpen, "Expand sidebar")
        } else {
            (IconName::PanelLeftClose, "Collapse sidebar")
        };

        div()
            .id("sidebar-toggle")
            .role(gpui::accesskit::Role::Button)
            .aria_label(label)
            .tab_index(0)
            .size(px(32.))
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .rounded(cx.theme().radius)
            .text_color(cx.theme().sidebar_foreground)
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .focus(|style| style.border_1().border_color(cx.theme().primary))
            .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_sidebar(layout, cx);
            }))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                if is_activation_key(event) {
                    window.prevent_default();
                    this.toggle_sidebar(layout, cx);
                }
            }))
            .child(Icon::new(icon).size(px(16.)))
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
            .aria_label(tooltip.clone())
            .aria_selected(selected)
            .tab_index(0)
            .w_full()
            .h(px(36.))
            .px_3()
            .rounded(cx.theme().radius)
            .accessibility_id(label)
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().sidebar_accent))
            })
            .when(rail, |this| {
                this.w(px(48.))
                    .overflow_hidden()
                    .px_1()
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            })
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
                                .ml_auto()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(count.to_string()),
                        )
                    }),
            )
    }

    fn activate_section(&mut self, section: Section, cx: &mut Context<Self>) -> bool {
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
                return false;
            }
            self.clear_selection_for_team_scope(cx);
        }
        self.section = section;
        cx.notify();
        true
    }

    fn refresh_visible(&self) -> bool {
        refresh_visible_for_section(self.section)
    }

    fn render_refresh_action(
        &self,
        id: &'static str,
        icon_only: bool,
        sidebar_action: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = if self.operation_in_progress {
            "Refreshing…"
        } else {
            "Refresh"
        };
        let tooltip = format!(
            "{} Jira · {}",
            if self.operation_in_progress {
                "Refreshing"
            } else {
                "Refresh"
            },
            self.sync_message
        );

        if icon_only {
            div()
                .id(id)
                .accessibility_id(id)
                .role(gpui::accesskit::Role::Button)
                .aria_label(tooltip.clone())
                .tab_index(0)
                .size(px(32.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .text_color(cx.theme().sidebar_foreground)
                .cursor_pointer()
                .hover(|style| {
                    style
                        .bg(cx.theme().sidebar_accent)
                        .text_color(cx.theme().sidebar_accent_foreground)
                })
                .focus(|style| style.border_1().border_color(cx.theme().primary))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.begin_refresh(window, cx);
                }))
                .on_key_down(cx.listener(|this, event, window, cx| {
                    if is_activation_key(event) {
                        window.prevent_default();
                        this.begin_refresh(window, cx);
                    }
                }))
                .child(if self.operation_in_progress {
                    Spinner::new().xsmall().into_any_element()
                } else {
                    Icon::new(IconName::Redo2).size(px(16.)).into_any_element()
                })
                .into_any_element()
        } else {
            Button::new(id)
                .compact()
                .when(sidebar_action, |this| {
                    this.ghost().w_full().justify_start().icon(IconName::Redo2)
                })
                .when(!sidebar_action, |this| this.primary().flex_shrink_0())
                .label(label)
                .loading(self.operation_in_progress)
                .tooltip(tooltip)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.begin_refresh(window, cx);
                }))
                .into_any_element()
        }
    }

    fn render_mobile_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("mobile-sync-status")
            .debug_selector(|| "mobile-sync-status".to_owned())
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .id("mobile-sync-status-text")
                            .aria_label(self.sync_message.clone())
                            .flex_1()
                            .min_w_0()
                            .max_h(px(56.))
                            .overflow_y_scrollbar()
                            .whitespace_normal()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.sync_message.clone()),
                    )
                    .when(self.refresh_visible(), |this| {
                        this.child(self.render_refresh_action("mobile-refresh", false, false, cx))
                    }),
            )
    }

    fn render_mobile_nav(&self, viewport_width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let issues_active = self.section == Section::Issues;
        let updates_active = self.section == Section::Updates;
        let team_active = self.section == Section::Team;
        let settings_active = self.section == Section::Settings;
        let nav_item_width = mobile_nav_item_width(viewport_width);

        h_flex()
            .id("mobile-navigation")
            .debug_selector(|| "mobile-navigation".to_owned())
            .w_full()
            .h(px(48.))
            .flex_shrink_0()
            .px_1()
            .gap_1()
            .items_center()
            .overflow_x_hidden()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.mobile_nav_item(
                MobileNavItem {
                    id: "mobile-issues",
                    label: "Issues",
                    accessible_label: format!("Issues · {} issues", self.issues.len()),
                    selected: issues_active,
                    section: Section::Issues,
                    width: nav_item_width,
                },
                cx,
            ))
            .child(self.mobile_nav_item(
                MobileNavItem {
                    id: "mobile-updates",
                    label: "Updates",
                    accessible_label: format!("Updates · {} unread", self.unread_count()),
                    selected: updates_active,
                    section: Section::Updates,
                    width: nav_item_width,
                },
                cx,
            ))
            .child(self.mobile_nav_item(
                MobileNavItem {
                    id: "mobile-team",
                    label: "Team",
                    accessible_label: format!(
                        "Team · {} in-progress tickets",
                        team_issue_counts(&self.team_issues).1
                    ),
                    selected: team_active,
                    section: Section::Team,
                    width: nav_item_width,
                },
                cx,
            ))
            .child(self.mobile_nav_item(
                MobileNavItem {
                    id: "mobile-settings",
                    label: "Settings",
                    accessible_label: "Settings".to_owned(),
                    selected: settings_active,
                    section: Section::Settings,
                    width: nav_item_width,
                },
                cx,
            ))
    }

    fn mobile_nav_item(&self, item: MobileNavItem, cx: &mut Context<Self>) -> impl IntoElement {
        let MobileNavItem {
            id,
            label,
            accessible_label,
            selected,
            section,
            width,
        } = item;
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .accessibility_id(id)
            .role(gpui::accesskit::Role::Button)
            .aria_label(accessible_label.clone())
            .aria_selected(selected)
            .tab_index(0)
            .flex_1()
            .min_w_0()
            .w(px(width))
            .h(px(36.))
            .px_1()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when(!selected, |this| {
                this.text_color(cx.theme().foreground)
                    .hover(|style| style.bg(cx.theme().accent))
            })
            .tooltip(move |window, cx| Tooltip::new(accessible_label.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.activate_section(section, cx) {
                    this.mobile_detail_open = false;
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(move |this, event, window, cx| {
                if is_activation_key(event) {
                    window.prevent_default();
                    if this.activate_section(section, cx) {
                        this.mobile_detail_open = false;
                        cx.notify();
                    }
                }
            }))
            .focus(|style| style.border_1().border_color(cx.theme().primary))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .child(div().min_w_0().truncate().child(label)),
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
        let viewport_width = f32::from(window.viewport_size().width);
        let layout = layout_for_width(viewport_width);
        let table_mode = team_table_mode_for_width(f32::from(window.viewport_size().width));
        if !layout.is_mobile() {
            self.detail_sidebar_width = px(clamped_team_detail_width(
                f32::from(self.detail_sidebar_width),
                f32::from(window.viewport_size().width),
                layout,
                table_mode,
                self.sidebar_collapsed,
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
            .id("dashboard-main")
            .debug_selector(|| "dashboard-main".to_owned())
            .h_full()
            .min_w_0()
            .flex_1()
            .child(div().min_w_0().min_h_0().flex_1().child(content));

        if layout.is_mobile() {
            v_flex()
                .size_full()
                .min_w_0()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_mobile_nav(viewport_width, cx))
                .child(self.render_mobile_status(cx))
                .child(main)
        } else {
            h_flex()
                .size_full()
                .min_w_0()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_sidebar(layout, viewport_width, cx))
                .child(main)
        }
    }
}
