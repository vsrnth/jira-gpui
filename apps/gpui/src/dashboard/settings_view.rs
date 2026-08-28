use super::*;
use crate::app_shell::AppearancePreference;
use gpui_component::Selectable;

const APPEARANCE_HELP_COPY: &str = "Follow the system appearance or choose a fixed theme.";
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

impl Dashboard {
    fn render_appearance_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.appearance_preference;
        let preference_button = |preference: AppearancePreference| {
            let label = preference.label();
            let selected = selected == preference;
            Button::new(format!("appearance-{}", label.to_lowercase()))
                .compact()
                .flex_1()
                .selected(selected)
                .toggled(selected)
                .when(selected, |this| this.primary())
                .label(label)
                .tooltip(format!("Use {label} appearance"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_appearance_preference(preference, window, cx);
                }))
        };

        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.10))
            .child(div().text_base().font_semibold().child("Appearance"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(APPEARANCE_HELP_COPY),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .child(preference_button(AppearancePreference::System))
                    .child(preference_button(AppearancePreference::Light))
                    .child(preference_button(AppearancePreference::Dark)),
            )
    }

    pub(super) fn render_settings(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let input = self.settings_input.clone();
        let team_input = self.team_input.clone();
        let platform_copy = settings_platform_copy(current_settings_platform());
        let text = self.settings_scope_text.clone();
        let chars = text.chars().count();
        let bytes = text.len();
        let validation = normalize_issue_jql_scope(Some(text.clone())).err();
        let live = self.workspace.is_some();
        let test_running = matches!(
            self.desktop_notification_test_state,
            DesktopNotificationTestState::Sending
        );
        let saved_login_deleting = matches!(
            self.saved_login_delete_state,
            SavedLoginDeleteState::Deleting
        );
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                h_flex()
                    .id("settings-scroll")
                    .flex_1()
                    .overflow_y_scrollbar()
                    .justify_center()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(820.))
                            .p(px(layout.list_padding()))
                            .gap_3()
                            .child(self.render_appearance_settings(cx))
                            .child(
                                v_flex()
                                    .gap_2()
                                    .p_3()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().secondary.opacity(0.10))
                                    .child(div().text_base().font_semibold().child("Issue scope"))
                                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Choose the Jira scope used for your assigned or watched account view."))
                                    .when_some(input, |this, input| this.child(Textarea::new(&input).w_full().h(px(if layout.is_mobile() { 128. } else { 120. })).aria_label("JQL scope").disabled(!live || self.operation_in_progress)))
                                    .child(div().text_xs().text_color(if validation.is_some() { cx.theme().danger } else { cx.theme().muted_foreground }).child(format!("{chars} characters · {bytes} bytes · maximum {MAX_JQL_SCOPE_LENGTH} bytes")))
                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(SCOPE_HELP_COPY))
                                    .when(!live, |this| this.child(div().text_sm().text_color(cx.theme().warning).child(LIVE_WORKSPACE_COPY)))
                                    .when_some(self.settings_warning.clone(), |this, warning| this.child(div().text_sm().text_color(cx.theme().warning).child(warning)))
                                    .when_some(validation.map(|_| format!("Scope is invalid: it must be non-empty, within {MAX_JQL_SCOPE_LENGTH} bytes, and contain no ORDER BY")), |this, message| this.child(div().text_sm().text_color(cx.theme().danger).child(message)))
                                    .when_some(self.settings_feedback.clone(), |this, feedback| this.child(div().text_sm().text_color(cx.theme().muted_foreground).child(feedback)))
                                    .child(
                                        h_flex()
                                            .when(layout.is_mobile(), |this| this.flex_col())
                                            .gap_2()
                                            .child(Button::new("save-settings").primary().label("Save and refresh").disabled(!live || self.operation_in_progress || validation.is_some()).on_click(cx.listener(|this, _, _, cx| this.begin_save_settings(cx))))
                                            .child(Button::new("reset-settings").ghost().label("Use default scope").disabled(!live || self.operation_in_progress).on_click(cx.listener(|this, _, window, cx| this.reset_settings_editor(window, cx)))),
                            )
                            .child(
                                v_flex()
                                    .gap_2()
                                    .p_3()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().secondary.opacity(0.10))
                                    .child(div().text_base().font_semibold().child("Team tracker"))
                                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("One Jira account ID or Atlassian email per line. This shows in-progress tickets assigned to those accounts; Jira permissions still apply."))
                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Email resolution requires exactly one active Jira user because the User search domain does not retain email. Uses existing read:jira-user/read:jira-work scopes; no new scope is needed."))
                                    .when_some(team_input, |this, input| this.child(Textarea::new(&input).w_full().h(px(if layout.is_mobile() { 110. } else { 120. })).aria_label("Team tracker members").disabled(!live || self.team_task.is_some() || self.operation_in_progress)))
                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(format!("{} configured · maximum {}", self.team_members.len(), MAX_TEAM_MEMBERS)))
                                    .when_some(self.team_feedback.clone(), |this, message| this.child(div().text_sm().text_color(cx.theme().muted_foreground).child(message)))
                                    .child(h_flex().gap_2().when(layout.is_mobile(), |this| this.flex_col()).child(Button::new("save-team").primary().label(if self.team_task.is_some() { "Saving team…" } else { "Save team" }).disabled(!live || self.team_task.is_some() || self.operation_in_progress).on_click(cx.listener(|this, _, _, cx| this.begin_save_team(cx)))).child(Button::new("refresh-team").ghost().label("Refresh team").disabled(!live || self.team_task.is_some() || self.operation_in_progress || self.team_automatic_polling_paused).on_click(cx.listener(|this, _, _, cx| this.begin_team_refresh(cx))))),
                            )
                            .child(
                                v_flex()
                                    .gap_2()
                                    .p_3()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().secondary.opacity(0.10))
                                    .child(div().text_base().font_semibold().child("Desktop notifications"))
                                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(platform_copy.notification_help))
                                    .when(platform_copy.notification_test_available, |this| {
                                        this.child(div().text_xs().text_color(cx.theme().muted_foreground).child(format!("App name: Jira Desk · Icon: dev.jiradesk.JiraDesk · Desktop entry: dev.jiradesk.JiraDesk · Summary: {TEST_NOTIFICATION_SUMMARY} · Body: {TEST_NOTIFICATION_BODY}")))
                                    })
                                    .when(platform_copy.notification_test_available, |this| {
                                        this.child(Button::new("test-desktop-notification").label(if test_running { "Sending test notification…" } else { "Send test notification" }).disabled(!live || test_running).on_click(cx.listener(|this, _, _, cx| this.begin_test_desktop_notification(cx))))
                                    })
                                    .when_some(if platform_copy.notification_test_available { match &self.desktop_notification_test_state { DesktopNotificationTestState::Completed(report) => Some(report.clone()), _ => None } } else { None }, |this, report| {
                                        let result = match report.outcome {
                                            DesktopNotificationTestOutcome::Accepted { notification_id } => format!("Accepted by desktop service · notification ID {notification_id}"),
                                            DesktopNotificationTestOutcome::Failed(error) => format!("Failed · error category {}", desktop_notification_error_category(error)),
                                        };
                                        this.child(v_flex().id(NOTIFICATION_TEST_RESULT_ID).gap_1().role(NOTIFICATION_TEST_RESULT_ROLE).child(div().text_sm().child(format!("Last test · {} · {result}", report.timestamp))).when_some(platform_copy.notification_display, |this, copy| this.child(div().text_xs().text_color(cx.theme().muted_foreground).child(copy))).when_some(platform_copy.notification_diagnostics, |this, copy| this.child(div().text_xs().text_color(cx.theme().muted_foreground).child(copy))))
                                    }),
                            )
                            .child(
                                v_flex()
                                    .gap_2()
                                    .p_3()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().secondary.opacity(0.10))
                                    .child(div().text_base().font_semibold().child("Saved Jira login"))
                                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(platform_copy.keyring))
                                    .child(Button::new("forget-saved-jira-login").label(if saved_login_deleting { "Forgetting saved Jira login…" } else { "Forget saved Jira login" }).when(layout.is_mobile(), |this| this.w_full()).disabled(saved_login_deleting).on_click(cx.listener(|this, _, _, cx| this.begin_forget_saved_login(cx))))
                                    .when_some(saved_login_delete_feedback_for_state(self.saved_login_delete_state), |this, copy| {
                                        this.child(v_flex().id(SAVED_LOGIN_DELETE_RESULT_ID).role(SAVED_LOGIN_DELETE_RESULT_ROLE).child(div().text_sm().text_color(match copy.severity() { FeedbackSeverity::Error => cx.theme().danger, FeedbackSeverity::Info => cx.theme().muted_foreground }).child(copy.message())))
                                    }),
                            ),
                    ),
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APPEARANCE_HELP_COPY, DIAGNOSTIC_EVENTS_COPY, LINUX_KEYRING_COPY,
        LINUX_NOTIFICATION_DISPLAY_COPY, LINUX_NOTIFICATION_HELP_COPY, LIVE_WORKSPACE_COPY,
        MACOS_KEYRING_COPY, MACOS_NOTIFICATION_HELP_COPY, NOTIFICATION_TEST_RESULT_ID,
        NOTIFICATION_TEST_RESULT_ROLE, SAVED_LOGIN_DELETE_RESULT_ID,
        SAVED_LOGIN_DELETE_RESULT_ROLE, SCOPE_HELP_COPY, SavedLoginDeleteOutcome,
        SavedLoginDeleteState, SettingsPlatform, saved_login_delete_feedback_for_state,
        settings_platform_copy,
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
