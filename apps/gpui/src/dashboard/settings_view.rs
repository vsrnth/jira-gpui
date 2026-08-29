use super::*;
use crate::app_shell::AppearancePreference;
use gpui_component::button::{Toggle, ToggleVariants};
use gpui_component::group_box::GroupBoxVariant;
use gpui_component::setting::{SettingGroup, SettingItem, SettingPage, Settings};

const APPEARANCE_HELP_COPY: &str = "Follow the system appearance or choose a fixed theme.";
const APPEARANCE_PREFERENCES: [AppearancePreference; 3] = [
    AppearancePreference::System,
    AppearancePreference::Light,
    AppearancePreference::Dark,
];
const SETTINGS_GROUP_LABELS: [&str; 5] = [
    "Appearance",
    "Issue scope",
    "Team tracker",
    "Desktop notifications",
    "Saved Jira login",
];
const SCOPE_HELP_COPY: &str = "This is a scope expression. Jira Desk appends assigned-or-watched account membership, incremental updated overlap, and ORDER BY updated DESC. Do not include ORDER BY.";
const LIVE_WORKSPACE_COPY: &str =
    "Settings become available after a live Jira workspace is connected.";
const LINUX_NOTIFICATION_HELP_COPY: &str = "Send a local test through the Freedesktop notification service used by Jira Desk. This never calls Jira or changes the local update feed.";
const LINUX_NOTIFICATION_DISPLAY_COPY: &str = "Accepted by the desktop service means the request was received; your desktop may still suppress or group the banner.";
const DIAGNOSTIC_EVENTS_COPY: &str = "Jira Desk attempts to write diagnostic events to diagnostics.jsonl; individual writes may fail.";
const LINUX_KEYRING_COPY: &str = "Saved credentials are stored in the Linux desktop keyring and reused automatically across Jira Desk/AppImage versions. Secrets are never written to SQLite, preferences, or logs.";
const MACOS_NOTIFICATION_HELP_COPY: &str = "Desktop notification testing is not available on macOS yet. Check Local updates for synced activity.";
const MACOS_KEYRING_COPY: &str = "Saved credentials are stored in the macOS Keychain and reused automatically across Jira Desk versions. Secrets are never written to SQLite, preferences, or logs.";
const NOTIFICATION_TEST_RESULT_ID: &str = "notification-test-result";
const NOTIFICATION_TEST_RESULT_ROLE: gpui::accesskit::Role = gpui::accesskit::Role::Status;
const SAVED_LOGIN_DELETE_RESULT_ID: &str = "saved-login-delete-result";
const SAVED_LOGIN_DELETE_RESULT_ROLE: gpui::accesskit::Role = gpui::accesskit::Role::Status;

fn saved_login_delete_feedback_for_state(state: SavedLoginDeleteState) -> Option<OutcomeCopy> {
    match state {
        SavedLoginDeleteState::Completed(outcome) => Some(saved_login_delete_feedback(outcome)),
        SavedLoginDeleteState::Idle | SavedLoginDeleteState::Deleting => None,
    }
}

// Each supported production target constructs one variant; tests exercise both policies.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsPlatform {
    Linux,
    Macos,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlatformSettingsCopy {
    notification_help: &'static str,
    notification_display: Option<&'static str>,
    notification_diagnostics: Option<&'static str>,
    notification_test_available: bool,
    keyring: &'static str,
}

fn settings_platform_copy(platform: SettingsPlatform) -> PlatformSettingsCopy {
    match platform {
        SettingsPlatform::Linux => PlatformSettingsCopy {
            notification_help: LINUX_NOTIFICATION_HELP_COPY,
            notification_display: Some(LINUX_NOTIFICATION_DISPLAY_COPY),
            notification_diagnostics: Some(DIAGNOSTIC_EVENTS_COPY),
            notification_test_available: true,
            keyring: LINUX_KEYRING_COPY,
        },
        SettingsPlatform::Macos => PlatformSettingsCopy {
            notification_help: MACOS_NOTIFICATION_HELP_COPY,
            notification_display: None,
            notification_diagnostics: None,
            notification_test_available: false,
            keyring: MACOS_KEYRING_COPY,
        },
    }
}

#[cfg(target_os = "linux")]
fn current_settings_platform() -> SettingsPlatform {
    SettingsPlatform::Linux
}

#[cfg(target_os = "macos")]
fn current_settings_platform() -> SettingsPlatform {
    SettingsPlatform::Macos
}

fn appearance_toggle_checks(selected: AppearancePreference) -> [bool; 3] {
    APPEARANCE_PREFERENCES.map(|preference| preference == selected)
}

impl Dashboard {
    fn render_appearance_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.appearance_preference;
        let dashboard = cx.entity().downgrade();
        let checks = appearance_toggle_checks(selected);
        let preference_toggles =
            APPEARANCE_PREFERENCES
                .into_iter()
                .enumerate()
                .map(|(index, preference)| {
                    let label = preference.label();
                    let id = format!("appearance-{}", label.to_lowercase());
                    let dashboard = dashboard.clone();
                    div()
                        .id(id.clone())
                        .debug_selector(move || id.clone())
                        .child(
                            Toggle::new(format!("appearance-toggle-{}", label.to_lowercase()))
                                .checked(checks[index])
                                .outline()
                                .label(label)
                                .tooltip(format!("Use {label} appearance"))
                                .on_click(move |_, window, cx| {
                                    if let Some(dashboard) = dashboard.upgrade() {
                                        dashboard.update(cx, |this, cx| {
                                            this.select_appearance_preference(
                                                preference, window, cx,
                                            );
                                        });
                                    }
                                }),
                        )
                });

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(APPEARANCE_HELP_COPY),
            )
            .child(
                h_flex()
                    .id("appearance-preferences")
                    .w_full()
                    .gap_1()
                    .children(preference_toggles),
            )
    }

    fn render_issue_scope_setting(
        &self,
        _layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.settings_input.clone();
        let text = self.settings_scope_text.clone();
        let chars = text.chars().count();
        let bytes = text.len();
        let validation = normalize_issue_jql_scope(Some(text)).err();
        let live = self.workspace.is_some();

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Choose the Jira scope used for your assigned or watched account view."),
            )
            .when_some(input, |this, input| {
                this.child(
                    Textarea::new(&input)
                        .w_full()
                        .h(px(120.))
                        .aria_label("JQL scope")
                        .disabled(!live || self.operation_in_progress),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(if validation.is_some() {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(format!(
                        "{chars} characters · {bytes} bytes · maximum {MAX_JQL_SCOPE_LENGTH} bytes"
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SCOPE_HELP_COPY),
            )
            .when(!live, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(LIVE_WORKSPACE_COPY),
                )
            })
            .when_some(self.settings_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .when_some(
                validation.map(|_| {
                    format!(
                        "Scope is invalid: it must be non-empty, within {MAX_JQL_SCOPE_LENGTH} bytes, and contain no ORDER BY"
                    )
                }),
                |this, message| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(message),
                    )
                },
            )
            .when_some(self.settings_feedback.clone(), |this, feedback| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(feedback),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-settings")
                            .primary()
                            .label("Save and refresh")
                            .disabled(
                                !live || self.operation_in_progress || validation.is_some(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.begin_save_settings(cx)
                            })),
                    )
                    .child(
                        Button::new("reset-settings")
                            .ghost()
                            .label("Use default scope")
                            .disabled(!live || self.operation_in_progress)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_settings_editor(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_team_tracker_setting(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.team_input.clone();
        let live = self.workspace.is_some();
        let task_running = self.team_task.is_some();

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("One Jira account ID or Atlassian email per line. This shows in-progress tickets assigned to those accounts; Jira permissions still apply."),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Email resolution requires exactly one active Jira user because the User search domain does not retain email. Uses existing read:jira-user/read:jira-work scopes; no new scope is needed."),
            )
            .when_some(input, |this, input| {
                this.child(
                    Textarea::new(&input)
                        .w_full()
                        .h(px(if layout.is_mobile() { 110. } else { 120. }))
                        .aria_label("Team tracker members")
                        .disabled(!live || task_running || self.operation_in_progress),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} configured · maximum {}",
                        self.team_members.len(),
                        MAX_TEAM_MEMBERS
                    )),
            )
            .when(!matches!(self.team_feedback, TeamFeedback::Idle), |this| {
                let is_error = self.team_feedback.is_error();
                let message = self
                    .team_feedback
                    .display_message()
                    .expect("non-idle team feedback has a message");
                let error_label = self.team_feedback.error_accessible_label();
                let accessibility_label = error_label.unwrap_or_else(|| message.clone());
                this.child(
                    v_flex()
                        .id("team-settings-feedback")
                        .text_sm()
                        .text_color(if is_error {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .role(if is_error {
                            gpui::accesskit::Role::Alert
                        } else {
                            gpui::accesskit::Role::Status
                        })
                        .aria_label(accessibility_label)
                        .child(message),
                )
            })
            .child(
                h_flex()
                    .when(layout.is_mobile(), |this| this.flex_col())
                    .gap_2()
                    .child(
                        Button::new("save-team")
                            .primary()
                            .label(if task_running {
                                "Saving team…"
                            } else {
                                "Save team"
                            })
                            .disabled(!live || task_running || self.operation_in_progress)
                            .on_click(cx.listener(|this, _, _, cx| this.begin_save_team(cx))),
                    )
                    .child(
                        Button::new("refresh-team")
                            .ghost()
                            .label("Refresh team")
                            .disabled(
                                !live
                                    || task_running
                                    || self.operation_in_progress
                                    || self.team_automatic_polling_paused,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.begin_team_refresh(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_notification_setting(
        &self,
        _layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let platform_copy = settings_platform_copy(current_settings_platform());
        let live = self.workspace.is_some();
        let test_running = matches!(
            self.desktop_notification_test_state,
            DesktopNotificationTestState::Sending
        );

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(platform_copy.notification_help),
            )
            .when(platform_copy.notification_test_available, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "App name: Jira Desk · Icon: dev.jiradesk.JiraDesk · Desktop entry: dev.jiradesk.JiraDesk · Summary: {TEST_NOTIFICATION_SUMMARY} · Body: {TEST_NOTIFICATION_BODY}"
                        )),
                )
            })
            .when(platform_copy.notification_test_available, |this| {
                this.child(
                    Button::new("test-desktop-notification")
                        .label(if test_running {
                            "Sending test notification…"
                        } else {
                            "Send test notification"
                        })
                        .disabled(!live || test_running)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.begin_test_desktop_notification(cx)
                        })),
                )
            })
            .when_some(
                if platform_copy.notification_test_available {
                    match &self.desktop_notification_test_state {
                        DesktopNotificationTestState::Completed(report) => Some(report.clone()),
                        _ => None,
                    }
                } else {
                    None
                },
                |this, report| {
                    let result = match report.outcome {
                        DesktopNotificationTestOutcome::Accepted { notification_id } => {
                            format!(
                                "Accepted by desktop service · notification ID {notification_id}"
                            )
                        }
                        DesktopNotificationTestOutcome::Failed(error) => format!(
                            "Failed · error category {}",
                            desktop_notification_error_category(error)
                        ),
                    };
                    this.child(
                        v_flex()
                            .id(NOTIFICATION_TEST_RESULT_ID)
                            .gap_1()
                            .role(NOTIFICATION_TEST_RESULT_ROLE)
                            .child(div().text_sm().child(format!(
                                "Last test · {} · {result}",
                                report.timestamp
                            )))
                            .when_some(platform_copy.notification_display, |this, copy| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(copy),
                                )
                            })
                            .when_some(platform_copy.notification_diagnostics, |this, copy| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(copy),
                                )
                            }),
                    )
                },
            )
            .into_any_element()
    }

    fn render_saved_login_setting(&self, layout: LayoutMode, cx: &mut Context<Self>) -> AnyElement {
        let platform_copy = settings_platform_copy(current_settings_platform());
        let deleting = matches!(
            self.saved_login_delete_state,
            SavedLoginDeleteState::Deleting
        );

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(platform_copy.keyring),
            )
            .child(
                Button::new("forget-saved-jira-login")
                    .label(if deleting {
                        "Forgetting saved Jira login…"
                    } else {
                        "Forget saved Jira login"
                    })
                    .when(layout.is_mobile(), |this| this.w_full())
                    .disabled(deleting)
                    .on_click(cx.listener(|this, _, _, cx| this.begin_forget_saved_login(cx))),
            )
            .when_some(
                saved_login_delete_feedback_for_state(self.saved_login_delete_state),
                |this, copy| {
                    this.child(
                        v_flex()
                            .id(SAVED_LOGIN_DELETE_RESULT_ID)
                            .role(SAVED_LOGIN_DELETE_RESULT_ROLE)
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(match copy.severity() {
                                        FeedbackSeverity::Error => cx.theme().danger,
                                        FeedbackSeverity::Info => cx.theme().muted_foreground,
                                    })
                                    .child(copy.message()),
                            ),
                    )
                },
            )
            .into_any_element()
    }

    pub(super) fn render_settings(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dashboard = cx.entity().downgrade();
        let appearance_dashboard = dashboard.clone();
        let issue_dashboard = dashboard.clone();
        let team_dashboard = dashboard.clone();
        let notification_dashboard = dashboard.clone();
        let login_dashboard = dashboard;
        let appearance = SettingItem::render(move |_, _, cx| {
            appearance_dashboard
                .update(cx, |this, cx| {
                    this.render_appearance_settings(cx).into_any_element()
                })
                .unwrap_or_else(|_| div().into_any_element())
        })
        .keywords(["appearance", "theme"]);
        let issue_scope = SettingItem::render(move |_, _, cx| {
            issue_dashboard
                .update(cx, |this, cx| this.render_issue_scope_setting(layout, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .keywords(["jira", "scope", "jql"]);
        let team = SettingItem::render(move |_, _, cx| {
            team_dashboard
                .update(cx, |this, cx| this.render_team_tracker_setting(layout, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .keywords(["team", "tracker", "members"]);
        let notifications = SettingItem::render(move |_, _, cx| {
            notification_dashboard
                .update(cx, |this, cx| this.render_notification_setting(layout, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .keywords(["desktop", "notifications"]);
        let saved_login = SettingItem::render(move |_, _, cx| {
            login_dashboard
                .update(cx, |this, cx| this.render_saved_login_setting(layout, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .keywords(["login", "credentials", "keychain", "keyring"]);

        let settings = Settings::new("jira-desk-settings")
            .with_group_variant(GroupBoxVariant::Outline)
            .sidebar_width(px(200.))
            .pages(vec![
                SettingPage::new("Settings")
                    .icon(IconName::Settings2)
                    .default_open(true)
                    .resettable(false)
                    .groups(vec![
                        SettingGroup::new()
                            .title(SETTINGS_GROUP_LABELS[0])
                            .item(appearance),
                        SettingGroup::new()
                            .title(SETTINGS_GROUP_LABELS[1])
                            .item(issue_scope),
                        SettingGroup::new()
                            .title(SETTINGS_GROUP_LABELS[2])
                            .item(team),
                        SettingGroup::new()
                            .title(SETTINGS_GROUP_LABELS[3])
                            .item(notifications),
                        SettingGroup::new()
                            .title(SETTINGS_GROUP_LABELS[4])
                            .item(saved_login),
                    ]),
            ]);

        div()
            .id("settings-root")
            .debug_selector(|| "settings-root".to_owned())
            .size_full()
            .min_w_0()
            .child(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APPEARANCE_HELP_COPY, APPEARANCE_PREFERENCES, DIAGNOSTIC_EVENTS_COPY, LINUX_KEYRING_COPY,
        LINUX_NOTIFICATION_DISPLAY_COPY, LINUX_NOTIFICATION_HELP_COPY, LIVE_WORKSPACE_COPY,
        MACOS_KEYRING_COPY, MACOS_NOTIFICATION_HELP_COPY, NOTIFICATION_TEST_RESULT_ID,
        NOTIFICATION_TEST_RESULT_ROLE, SAVED_LOGIN_DELETE_RESULT_ID,
        SAVED_LOGIN_DELETE_RESULT_ROLE, SCOPE_HELP_COPY, SETTINGS_GROUP_LABELS,
        SavedLoginDeleteOutcome, SavedLoginDeleteState, SettingsPlatform, appearance_toggle_checks,
        saved_login_delete_feedback_for_state, settings_platform_copy,
    };
    use crate::dashboard::Dashboard;

    #[test]
    fn appearance_is_available_in_preview_settings() {
        let dashboard = Dashboard::from_sample_data();
        assert!(dashboard.workspace.is_none());
        assert_eq!(
            APPEARANCE_HELP_COPY,
            "Follow the system appearance or choose a fixed theme."
        );
    }

    #[test]
    fn settings_page_uses_meaningful_native_groups() {
        assert_eq!(
            SETTINGS_GROUP_LABELS,
            [
                "Appearance",
                "Issue scope",
                "Team tracker",
                "Desktop notifications",
                "Saved Jira login",
            ]
        );
    }

    #[test]
    fn appearance_toggle_checks_select_exactly_one_control() {
        for (selected_index, selected) in APPEARANCE_PREFERENCES.into_iter().enumerate() {
            let checks = appearance_toggle_checks(selected);
            assert_eq!(checks.iter().filter(|checked| **checked).count(), 1);
            assert!(checks[selected_index]);
        }
    }

    #[test]
    fn protected_settings_copy_is_exact() {
        assert_eq!(
            SCOPE_HELP_COPY,
            "This is a scope expression. Jira Desk appends assigned-or-watched account membership, incremental updated overlap, and ORDER BY updated DESC. Do not include ORDER BY."
        );
        assert_eq!(
            LIVE_WORKSPACE_COPY,
            "Settings become available after a live Jira workspace is connected."
        );
        assert_eq!(
            LINUX_NOTIFICATION_HELP_COPY,
            "Send a local test through the Freedesktop notification service used by Jira Desk. This never calls Jira or changes the local update feed."
        );
        assert_eq!(
            LINUX_NOTIFICATION_DISPLAY_COPY,
            "Accepted by the desktop service means the request was received; your desktop may still suppress or group the banner."
        );
        assert_eq!(
            DIAGNOSTIC_EVENTS_COPY,
            "Jira Desk attempts to write diagnostic events to diagnostics.jsonl; individual writes may fail."
        );
        assert_eq!(
            LINUX_KEYRING_COPY,
            "Saved credentials are stored in the Linux desktop keyring and reused automatically across Jira Desk/AppImage versions. Secrets are never written to SQLite, preferences, or logs."
        );
        assert_eq!(
            MACOS_NOTIFICATION_HELP_COPY,
            "Desktop notification testing is not available on macOS yet. Check Local updates for synced activity."
        );
        assert_eq!(
            MACOS_KEYRING_COPY,
            "Saved credentials are stored in the macOS Keychain and reused automatically across Jira Desk versions. Secrets are never written to SQLite, preferences, or logs."
        );
    }

    #[test]
    fn saved_login_feedback_announces_only_completion_in_stable_status_region() {
        assert_eq!(SAVED_LOGIN_DELETE_RESULT_ID, "saved-login-delete-result");
        assert_eq!(
            SAVED_LOGIN_DELETE_RESULT_ROLE,
            gpui::accesskit::Role::Status
        );
        assert!(saved_login_delete_feedback_for_state(SavedLoginDeleteState::Idle).is_none());
        assert!(saved_login_delete_feedback_for_state(SavedLoginDeleteState::Deleting).is_none());
        assert_eq!(
            saved_login_delete_feedback_for_state(SavedLoginDeleteState::Completed(
                SavedLoginDeleteOutcome::Deleted,
            ))
            .map(|copy| copy.message()),
            Some("Saved Jira login forgotten. This session remains connected.")
        );
    }

    #[test]
    fn platform_policy_keeps_linux_diagnostic_details_and_action() {
        let copy = settings_platform_copy(SettingsPlatform::Linux);

        assert_eq!(copy.notification_help, LINUX_NOTIFICATION_HELP_COPY);
        assert_eq!(
            copy.notification_display,
            Some(LINUX_NOTIFICATION_DISPLAY_COPY)
        );
        assert_eq!(copy.notification_diagnostics, Some(DIAGNOSTIC_EVENTS_COPY));
        assert!(copy.notification_test_available);
        assert!(copy.keyring.contains("Linux desktop keyring"));
        assert!(copy.keyring.contains("AppImage"));
        assert!(!copy.keyring.contains("token"));
    }

    #[test]
    fn platform_policy_keeps_macos_copy_honest_and_free_of_linux_details() {
        let copy = settings_platform_copy(SettingsPlatform::Macos);

        assert_eq!(copy.notification_help, MACOS_NOTIFICATION_HELP_COPY);
        assert_eq!(copy.notification_display, None);
        assert_eq!(copy.notification_diagnostics, None);
        assert!(!copy.notification_test_available);
        assert!(copy.notification_help.contains("Local updates"));
        assert!(copy.keyring.contains("macOS Keychain"));
        assert!(!copy.notification_help.contains("Freedesktop"));
        assert!(!copy.notification_help.contains("GNOME"));
        assert!(!copy.notification_help.contains("desktop-entry"));
        assert!(!copy.keyring.contains("desktop-entry"));
        assert!(!copy.keyring.contains("diagnostic"));
        assert!(!copy.keyring.contains("token"));
    }

    #[test]
    fn notification_completion_uses_a_stable_status_region() {
        assert_eq!(NOTIFICATION_TEST_RESULT_ID, "notification-test-result");
        assert_eq!(NOTIFICATION_TEST_RESULT_ROLE, gpui::accesskit::Role::Status);
    }
}
