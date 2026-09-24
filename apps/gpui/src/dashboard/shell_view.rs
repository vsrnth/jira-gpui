use super::*;
use std::rc::Rc;

use crate::app_assets::AppIconName;
use crate::responsive::{effective_sidebar_is_rail, mobile_nav_item_width};
use gpui_kit::MouseButton;
use gpui_kit::component::{
    Collapsible as _, Sizable as _,
    menu::{DropdownMenu as _, PopupMenuItem},
    sidebar::{
        Sidebar, SidebarCollapsible, SidebarFooter, SidebarHeader, SidebarItem, SidebarMenuItem,
        SidebarToggleButton,
    },
    tooltip::Tooltip,
};

type SidebarActivation = Rc<dyn Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App)>;

#[derive(Clone)]
struct AccessibleSidebarMenu {
    items: Vec<AccessibleSidebarMenuItem>,
    collapsed: bool,
}

impl AccessibleSidebarMenu {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            collapsed: false,
        }
    }

    fn child(mut self, child: AccessibleSidebarMenuItem) -> Self {
        self.items.push(child);
        self
    }
}

impl gpui_kit::component::Collapsible for AccessibleSidebarMenu {
    fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
}

impl SidebarItem for AccessibleSidebarMenu {
    fn render(
        self,
        id: impl Into<gpui_kit::ElementId>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> impl IntoElement {
        let id = id.into();
        v_flex()
            .gap_2()
            .children(self.items.into_iter().enumerate().map(|(index, item)| {
                item.collapsed(self.collapsed)
                    .render(format!("{id}-{index}"), window, cx)
                    .into_any_element()
            }))
    }
}

#[derive(Clone)]
struct AccessibleSidebarMenuItem {
    item: SidebarMenuItem,
    accessibility_id: &'static str,
    accessible_label: String,
    selected: bool,
    activation: SidebarActivation,
}

impl AccessibleSidebarMenuItem {
    fn new(
        item: SidebarMenuItem,
        accessibility_id: &'static str,
        accessible_label: impl Into<String>,
        selected: bool,
        activation: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    ) -> Self {
        Self {
            item,
            accessibility_id,
            accessible_label: accessible_label.into(),
            selected,
            activation: Rc::new(activation),
        }
    }
}

impl gpui_kit::component::Collapsible for AccessibleSidebarMenuItem {
    fn is_collapsed(&self) -> bool {
        self.item.is_collapsed()
    }

    fn collapsed(mut self, collapsed: bool) -> Self {
        self.item = self.item.collapsed(collapsed);
        self
    }
}

impl SidebarItem for AccessibleSidebarMenuItem {
    fn render(
        self,
        id: impl Into<gpui_kit::ElementId>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> impl IntoElement {
        let item = self.item.render(id, window, cx).into_any_element();
        let accessibility_id = self.accessibility_id;
        let accessible_label = self.accessible_label;
        let selected = self.selected;
        let activation = self.activation;
        div()
            .id(accessibility_id)
            .debug_selector(move || accessibility_id.to_owned())
            .accessibility_id(accessibility_id)
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(accessible_label)
            .aria_selected(selected)
            .tab_index(0)
            .on_key_down({
                let activation = activation.clone();
                move |event, window, cx| {
                    if is_activation_key(event) {
                        window.prevent_default();
                        activation(&gpui_kit::ClickEvent::default(), window, cx);
                    }
                }
            })
            .on_click(move |event, window, cx| activation(event, window, cx))
            .child(item)
    }
}

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

pub(super) fn should_render_sidebar_sync_message(message: &str) -> bool {
    message != "Preview data · Jira connection not configured"
}

fn should_render_mobile_sync_status(has_workspace: bool, message: &str) -> bool {
    has_workspace || should_render_sidebar_sync_message(message)
}

impl Dashboard {
    fn render_sidebar(
        &self,
        layout: LayoutMode,
        _viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let collapsed = effective_sidebar_is_rail(layout, self.sidebar_collapsed);
        let menu = AccessibleSidebarMenu::new()
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
            .child(self.sidebar_menu_item(
                "Settings",
                0,
                self.section == Section::Settings,
                Section::Settings,
                cx,
            ));

        let workspace = h_flex()
            .id("sidebar-workspace-header")
            .debug_selector(|| "sidebar-workspace-header".to_owned())
            .accessibility_id("sidebar-workspace-header")
            .role(gpui_kit::accesskit::Role::Group)
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
                                .accessibility_id("sidebar-workspace-site")
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
                            .role(gpui_kit::accesskit::Role::Button)
                            .aria_label(if collapsed {
                                "Expand sidebar"
                            } else {
                                "Collapse sidebar"
                            })
                            .w_full()
                            .justify_center()
                            .child(
                                h_flex()
                                    .id("sidebar-toggle-button")
                                    .debug_selector(|| "sidebar-toggle-button".to_owned())
                                    .justify_center()
                                    .child(
                                        SidebarToggleButton::new().collapsed(collapsed).on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle_sidebar(layout, cx);
                                            }),
                                        ),
                                    ),
                            ),
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
                                .role(gpui_kit::accesskit::Role::Button)
                                .aria_label("Collapse sidebar")
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
        let profile = self.render_profile_trigger(profile_label, collapsed, cx);

        let footer = v_flex()
            .id("sidebar-footer-content")
            .w_full()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().sidebar_border)
            .when(
                !collapsed && should_render_sidebar_sync_message(&self.sync_message),
                |this| {
                    this.max_h(gpui_kit::rems(11.)).min_h_0().child(
                        div()
                            .id("sidebar-sync-status")
                            .debug_selector(|| "sidebar-sync-status".to_owned())
                            .accessibility_id("sidebar-sync-status")
                            .role(gpui_kit::accesskit::Role::Status)
                            .w_full()
                            .min_w_0()
                            .aria_label(self.sync_message.clone())
                            .h(gpui_kit::rems(3.5))
                            .max_h(gpui_kit::rems(4.5))
                            .min_h_0()
                            .flex_shrink_1()
                            .overflow_y_scrollbar()
                            .whitespace_normal()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.sync_message.clone()),
                    )
                },
            )
            .child(
                SidebarFooter::new().child(
                    h_flex()
                        .id("sidebar-profile-actions")
                        .debug_selector(|| "sidebar-profile-actions".to_owned())
                        .accessibility_id("sidebar-profile-actions")
                        .role(gpui_kit::accesskit::Role::Group)
                        .aria_label("Account and refresh actions")
                        .w_full()
                        .min_w_0()
                        .items_center()
                        .when(collapsed, |this| {
                            this.flex_col()
                                .h(gpui_kit::rems(5.))
                                .gap_1()
                                .justify_center()
                        })
                        .when(!collapsed, |this| this.gap_1())
                        .child(profile)
                        .when(self.refresh_visible(), |this| {
                            this.child(self.render_refresh_action(
                                "sidebar-refresh",
                                collapsed,
                                true,
                                cx,
                            ))
                        }),
                ),
            );

        div()
            .id("dashboard-sidebar-shell")
            .debug_selector(|| "dashboard-sidebar".to_owned())
            .accessibility_id("dashboard-sidebar")
            .role(gpui_kit::accesskit::Role::Group)
            .aria_label("Jira Desk sidebar")
            .h_full()
            .w(gpui_kit::rems(if collapsed { 3. } else { 15. }))
            .flex_shrink_0()
            .overflow_hidden()
            .child(
                Sidebar::new("dashboard-sidebar-component")
                    .collapsible(SidebarCollapsible::Icon)
                    .collapsed(collapsed)
                    .w(gpui_kit::rems(15.))
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

    fn render_profile_trigger(
        &self,
        profile_label: String,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dashboard = cx.entity().downgrade();
        Button::new("sidebar-profile")
            .debug_selector(|| "sidebar-profile".to_owned())
            .accessibility_id("sidebar-profile")
            .ghost()
            .compact()
            .when(collapsed, |this| {
                this.icon(IconName::CircleUser).xsmall().size_4()
            })
            .when(!collapsed, |this| {
                this.icon(IconName::CircleUser)
                    .label(profile_label.clone())
                    .dropdown_caret(true)
                    .flex_1()
                    .flex_shrink_1()
                    .min_w_0()
                    .max_w(gpui_kit::rems(10.5))
                    .justify_start()
            })
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let settings = [
                    ("Appearance", SettingsCategory::Appearance),
                    ("Issue scope", SettingsCategory::IssueScope),
                    ("Team tracker", SettingsCategory::TeamTracker),
                    (
                        "Desktop notifications",
                        SettingsCategory::DesktopNotifications,
                    ),
                    ("Saved Jira login", SettingsCategory::SavedJiraLogin),
                ];
                settings.into_iter().fold(
                    menu.label("Settings").separator(),
                    |menu, (label, category)| {
                        let dashboard = dashboard.clone();
                        menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let _ = dashboard.update(cx, |dashboard, cx| {
                                dashboard.settings_category = category;
                                dashboard.activate_section(Section::Settings, cx);
                            });
                        }))
                    },
                )
            })
            .into_any_element()
    }

    fn sidebar_menu_item(
        &self,
        label: &'static str,
        count: usize,
        selected: bool,
        section: Section,
        cx: &mut Context<Self>,
    ) -> AccessibleSidebarMenuItem {
        let icon = match section {
            Section::Issues => IconName::LayoutDashboard,
            Section::Updates => IconName::Bell,
            Section::Team => IconName::CircleUser,
            Section::Settings => IconName::Settings2,
        };
        let accessibility_id = match section {
            Section::Issues => "nav-issues",
            Section::Updates => "nav-updates",
            Section::Team => "nav-team",
            Section::Settings => "nav-settings",
        };
        let accessible_label = match section {
            Section::Issues => format!("Issues · {count} issues"),
            Section::Updates => format!("Local updates · {count} unread"),
            Section::Team => format!("Team tracker · {count} in-progress tickets"),
            Section::Settings => "Settings".to_owned(),
        };
        AccessibleSidebarMenuItem::new(
            SidebarMenuItem::new(label)
                .icon(icon)
                .active(selected)
                .when(count > 0, |this| {
                    this.suffix(move |_, _| div().text_xs().child(count.to_string()))
                }),
            accessibility_id,
            accessible_label,
            selected,
            cx.listener(move |this, _, _, cx| {
                this.activate_section(section, cx);
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
        let label = refresh_action_label(self.operation_in_progress);
        let tooltip = label.to_owned();

        if icon_only || sidebar_action {
            let dashboard_for_a11y = cx.entity().downgrade();
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .accessibility_id(id)
                .role(gpui_kit::accesskit::Role::Button)
                .aria_label(tooltip.clone())
                .tab_index(0)
                .flex_shrink_0()
                .when(icon_only, |this| this.size_4())
                .when(!icon_only, |this| this.size_6())
                .flex()
                .items_center()
                .justify_center()
                .rounded(cx.theme().radius)
                .text_color(cx.theme().sidebar_foreground)
                .when(self.operation_in_progress, |this| this.opacity(0.8))
                .hover(|style| {
                    style
                        .bg(cx.theme().sidebar_accent)
                        .text_color(cx.theme().sidebar_accent_foreground)
                })
                .active(|style| {
                    style
                        .bg(cx.theme().sidebar_accent)
                        .text_color(cx.theme().sidebar_accent_foreground)
                })
                .focus(|style| style.border_1().border_color(cx.theme().primary))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_a11y_action(gpui_kit::AccessibleAction::Click, move |_, window, cx| {
                    if let Some(dashboard) = dashboard_for_a11y.upgrade() {
                        dashboard.update(cx, |this, cx| {
                            this.begin_refresh(window, cx);
                        });
                    }
                })
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.begin_refresh(window, cx);
                    }),
                )
                .on_key_down(cx.listener(|this, event, window, cx| {
                    if is_activation_key(event) {
                        window.prevent_default();
                        this.begin_refresh(window, cx);
                    }
                }))
                .child(if self.operation_in_progress {
                    Icon::new(IconName::Loader).size_3().into_any_element()
                } else {
                    Icon::new(AppIconName::RefreshCw)
                        .size_3()
                        .into_any_element()
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
            .role(gpui_kit::accesskit::Role::Status)
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex_shrink_0()
            .max_h(gpui_kit::rems(6.5))
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
                            .max_h(gpui_kit::rems(3.5))
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
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(accessible_label.clone())
            .aria_selected(selected)
            .tab_index(0)
            .flex_1()
            .min_w_0()
            .w(px(width))
            .h_11()
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
        if matches!(
            self.issue_edit_flow.state(),
            IssueEditState::AssigneeChooser { .. }
        ) {
            self.ensure_assignee_list(window, cx);
        }
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
                .when(
                    should_render_mobile_sync_status(self.workspace.is_some(), &self.sync_message),
                    |this| this.child(self.render_mobile_status(cx)),
                )
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

#[cfg(test)]
mod tests {
    use super::{Dashboard, should_render_mobile_sync_status};
    use gpui_kit::{VisualTestContext, px};

    #[gpui_kit::test]
    fn mobile_fixture_has_large_navigation_targets_and_hides_default_preview_status(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let dashboard = Dashboard::from_sample_data();
        let window = cx.open_window(gpui_kit::size(px(390.), px(800.)), |_, _| dashboard);
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let navigation = visual
            .debug_bounds("mobile-navigation")
            .expect("mobile navigation should be laid out");
        for id in [
            "mobile-issues",
            "mobile-updates",
            "mobile-team",
            "mobile-settings",
        ] {
            let bounds = visual
                .debug_bounds(id)
                .unwrap_or_else(|| panic!("{id} should be laid out"));
            assert!(
                bounds.size.height >= px(44.),
                "{id} has a touch target shorter than 44 logical pixels: {bounds:?}"
            );
            assert!(
                bounds.origin.y >= navigation.origin.y
                    && bounds.origin.y + bounds.size.height
                        <= navigation.origin.y + navigation.size.height,
                "{id} escapes the mobile navigation bounds: item={bounds:?}, nav={navigation:?}"
            );
        }

        assert!(
            visual.debug_bounds("mobile-sync-status").is_none(),
            "the default preview status strip should be absent"
        );
    }

    #[test]
    fn mobile_status_keeps_live_and_nondefault_feedback() {
        assert!(!should_render_mobile_sync_status(
            false,
            "Preview data · Jira connection not configured"
        ));
        assert!(should_render_mobile_sync_status(
            true,
            "Preview data · Jira connection not configured"
        ));
        assert!(should_render_mobile_sync_status(
            false,
            "Opening local cache…"
        ));
    }
}
