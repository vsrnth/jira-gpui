use super::*;
use crate::responsive::{effective_sidebar_is_rail, mobile_nav_item_width};
use gpui_component::{
    Sizable as _,
    sidebar::{
        Sidebar, SidebarCollapsible, SidebarFooter, SidebarHeader, SidebarMenu, SidebarMenuItem,
        SidebarToggleButton,
    },
    tooltip::Tooltip,
};

struct MobileNavItem {
    id: &'static str,
    label: &'static str,
    accessible_label: String,
    selected: bool,
    section: Section,
    width: f32,
}

pub(super) fn refresh_action_label(operation_in_progress: bool) -> &'static str {
    if operation_in_progress {
        "Refreshing Jira…"
    } else {
        "Refresh Jira"
    }
}

impl Dashboard {
    fn render_sidebar(
        &self,
        layout: LayoutMode,
        _viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let collapsed = effective_sidebar_is_rail(layout, self.sidebar_collapsed);
        let menu = SidebarMenu::new()
            .child(self.sidebar_menu_item(
                "Issues",
                self.issues.len(),
                self.section == Section::Issues,
                Section::Issues,
                cx,
            ))
            .child(self.sidebar_menu_item(
                "Local updates",
                self.unread_count(),
                self.section == Section::Updates,
                Section::Updates,
                cx,
            ))
            .child(self.sidebar_menu_item(
                "Team tracker",
                team_issue_counts(&self.team_issues).1,
                self.section == Section::Team,
                Section::Team,
                cx,
            ))
            .child(self.settings_sidebar_menu_item(cx));

        let workspace = h_flex()
            .id("sidebar-workspace-header")
            .debug_selector(|| "sidebar-workspace-header".to_owned())
            .accessibility_id("sidebar-workspace-header")
            .aria_label(format!("{} · {}", self.site_label, self.mode_label))
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("sidebar-workspace-icon")
                    .debug_selector(|| "sidebar-workspace-icon".to_owned())
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().sidebar_primary)
                    .text_color(cx.theme().sidebar_primary_foreground)
                    .when(collapsed, |this| {
                        this.size_4()
                            .bg(cx.theme().transparent)
                            .text_color(cx.theme().foreground)
                    })
                    .when(!collapsed, |this| this.size_8())
                    .child(Icon::new(IconName::GalleryVerticalEnd)),
            )
            .when(!collapsed, |this| {
                this.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("sidebar-workspace-site")
                                .debug_selector(|| "sidebar-workspace-site".to_owned())
                                .text_sm()
                                .font_semibold()
                                .truncate()
                                .child(self.site_label.clone()),
                        )
                        .child(
                            div()
                                .id("sidebar-workspace-mode")
                                .debug_selector(|| "sidebar-workspace-mode".to_owned())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(self.mode_label.clone()),
                        ),
                )
            });

        let header = if collapsed {
            v_flex()
                .id("sidebar-navigation")
                .debug_selector(|| "sidebar-navigation".to_owned())
                .w_full()
                .min_w_0()
                .gap_1()
                .child(SidebarHeader::new().child(workspace))
                .when(layout.supports_manual_sidebar_collapse(), |this| {
                    this.child(
                        h_flex()
                            .id("sidebar-toggle")
                            .debug_selector(|| "sidebar-toggle".to_owned())
                            .accessibility_id("sidebar-toggle")
                            .w_full()
                            .justify_end()
                            .child(SidebarToggleButton::new().collapsed(collapsed).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_sidebar(layout, cx);
                                }),
                            )),
                    )
                })
                .into_any_element()
        } else {
            v_flex()
                .id("sidebar-navigation")
                .debug_selector(|| "sidebar-navigation".to_owned())
                .w_full()
                .min_w_0()
                .child(
                    SidebarHeader::new().child(
                        workspace.child(
                            h_flex()
                                .id("sidebar-toggle")
                                .debug_selector(|| "sidebar-toggle".to_owned())
                                .accessibility_id("sidebar-toggle")
                                .flex_shrink_0()
                                .child(SidebarToggleButton::new().collapsed(collapsed).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.toggle_sidebar(layout, cx);
                                    }),
                                )),
                        ),
                    ),
                )
                .into_any_element()
        };

        let profile_label = self.sidebar_profile_label();
        let profile = SidebarFooter::new().child(
            h_flex()
                .id("sidebar-profile")
                .debug_selector(|| "sidebar-profile".to_owned())
                .accessibility_id("sidebar-profile")
                .aria_label(profile_label.clone())
                .min_w_0()
                .items_center()
                .gap_2()
                .child(Icon::new(IconName::CircleUser).size_4())
                .when(!collapsed, |this| {
                    this.child(
                        div()
                            .id("sidebar-profile-label")
                            .debug_selector(|| "sidebar-profile-label".to_owned())
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(profile_label.clone()),
                    )
                }),
        );

        let footer = v_flex()
            .id("sidebar-footer-content")
            .w_full()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .when(!collapsed, |this| {
                this.max_h(gpui::rems(11.)).min_h_0().child(
                    div()
                        .id("sidebar-sync-status")
                        .debug_selector(|| "sidebar-sync-status".to_owned())
                        .role(gpui::accesskit::Role::Status)
                        .w_full()
                        .min_w_0()
                        .aria_label(self.sync_message.clone())
                        .h(gpui::rems(3.5))
                        .max_h(gpui::rems(4.5))
                        .min_h_0()
                        .flex_shrink_1()
                        .overflow_y_scrollbar()
                        .whitespace_normal()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.sync_message.clone()),
                )
            })
            .when(self.refresh_visible(), |this| {
                this.child(self.render_refresh_action("sidebar-refresh", collapsed, !collapsed, cx))
            })
            .child(profile);

        div()
            .id("dashboard-sidebar-shell")
            .debug_selector(|| "dashboard-sidebar".to_owned())
            .h_full()
            .w(gpui::rems(if collapsed { 3. } else { 12.5 }))
            .flex_shrink_0()
            .overflow_hidden()
            .child(
                Sidebar::new("dashboard-sidebar-component")
                    .collapsible(SidebarCollapsible::Icon)
                    .collapsed(collapsed)
                    .w(gpui::rems(12.5))
                    .header(header)
                    .child(menu)
                    .footer(footer),
            )
    }

    fn sidebar_profile_label(&self) -> String {
        self.authenticated_account
            .as_ref()
            .and_then(|account| self.users.iter().find(|user| &user.account_id == account))
            .map(|user| user.display_name.clone())
            .unwrap_or_else(|| {
                if self.workspace.is_some() {
                    "Jira account".to_owned()
                } else {
                    "Preview data".to_owned()
                }
            })
    }

    fn sidebar_menu_item(
        &self,
        label: &'static str,
        count: usize,
        selected: bool,
        section: Section,
        cx: &mut Context<Self>,
    ) -> SidebarMenuItem {
        let icon = match section {
            Section::Issues => IconName::LayoutDashboard,
            Section::Updates => IconName::Bell,
            Section::Team => IconName::CircleUser,
            Section::Settings => IconName::Settings2,
        };
        SidebarMenuItem::new(label)
            .icon(icon)
            .active(selected)
            .when(count > 0, |this| {
                this.suffix(move |_, _| div().text_xs().child(count.to_string()))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.activate_section(section, cx);
            }))
    }

    fn settings_sidebar_menu_item(&self, cx: &mut Context<Self>) -> SidebarMenuItem {
        SidebarMenuItem::new("Settings")
            .icon(IconName::Settings2)
            .active(self.section == Section::Settings)
            .click_to_open(true)
            .default_open(self.section == Section::Settings)
            .children([
                self.settings_category_menu_item("Appearance", SettingsCategory::Appearance, cx),
                self.settings_category_menu_item("Issue scope", SettingsCategory::IssueScope, cx),
                self.settings_category_menu_item("Team tracker", SettingsCategory::TeamTracker, cx),
                self.settings_category_menu_item(
                    "Desktop notifications",
                    SettingsCategory::DesktopNotifications,
                    cx,
                ),
                self.settings_category_menu_item(
                    "Saved Jira login",
                    SettingsCategory::SavedJiraLogin,
                    cx,
                ),
            ])
            .on_click(cx.listener(|this, _, _, cx| {
                this.activate_section(Section::Settings, cx);
            }))
    }

    fn settings_category_menu_item(
        &self,
        label: &'static str,
        category: SettingsCategory,
        cx: &mut Context<Self>,
    ) -> SidebarMenuItem {
        SidebarMenuItem::new(label)
            .active(self.section == Section::Settings && self.settings_category == category)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.settings_category = category;
                this.activate_section(Section::Settings, cx);
            }))
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
        let label = refresh_action_label(self.operation_in_progress);
        let tooltip = label.to_owned();

        if icon_only {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .accessibility_id(id)
                .role(gpui::accesskit::Role::Button)
                .aria_label(tooltip.clone())
                .tab_index(0)
                .size_8()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .text_color(cx.theme().sidebar_foreground)
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
                    Icon::new(IconName::Redo2).size_4().into_any_element()
                })
                .into_any_element()
        } else {
            Button::new(id)
                .debug_selector(move || id.to_owned())
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
            .role(gpui::accesskit::Role::Status)
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex_shrink_0()
            .max_h(gpui::rems(6.5))
            .overflow_hidden()
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
                            .debug_selector(|| "mobile-sync-status-text".to_owned())
                            .aria_label(self.sync_message.clone())
                            .flex_1()
                            .min_w_0()
                            .max_h(gpui::rems(3.5))
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
            .h_12()
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
            .h_9()
            .px_1()
            .rounded(cx.theme().radius)
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
        if !layout.is_mobile() {
            self.ensure_team_panes_state(cx);
        }
        let content = match self.section {
            Section::Issues => self.render_issues(layout, cx).into_any_element(),
            Section::Updates => self.render_updates(layout, cx).into_any_element(),
            Section::Team => self
                .render_team(
                    layout,
                    team_table_mode_for_width(viewport_width),
                    viewport_width,
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
            .min_h_0()
            .flex_1()
            .child(
                div()
                    .min_w_0()
                    .min_h_0()
                    .flex_1()
                    .overflow_hidden()
                    .child(content),
            );

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
