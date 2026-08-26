use super::*;

impl Dashboard {
    pub(super) fn render_settings(
        &self,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let input = self.settings_input.clone();
        let team_input = self.team_input.clone();
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
                            .child(div().text_xl().font_semibold().child("Jira settings"))
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
                                        .h(px(if layout.is_mobile() { 128. } else { 160. }))
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
                                    .child(format!("{chars} characters · {bytes} bytes · maximum {MAX_JQL_SCOPE_LENGTH} bytes")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("This is a scope expression. Jira Desk appends assigned-or-watched account membership, incremental updated overlap, and ORDER BY updated DESC. Do not include ORDER BY."),
                            )
                            .when(!live, |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().warning)
                                        .child("Settings become available after a live Jira workspace is connected."),
                                )
                            })
                            .when_some(self.settings_warning.clone(), |this, warning| {
                                this.child(div().text_sm().text_color(cx.theme().warning).child(warning))
                            })
                            .when_some(validation.map(|_| format!("Scope is invalid: it must be non-empty, within {MAX_JQL_SCOPE_LENGTH} bytes, and contain no ORDER BY")), |this, message| {
                                this.child(div().text_sm().text_color(cx.theme().danger).child(message))
                            })
                            .when_some(self.settings_feedback.clone(), |this, feedback| {
                                this.child(div().text_sm().text_color(cx.theme().muted_foreground).child(feedback))
                            })
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(div().text_base().font_semibold().child("Desktop notifications"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Send a local test through the same Freedesktop service configuration used by Jira updates. This never calls Jira or changes the local update feed."),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("App name: Jira Desk · Icon: dev.jiradesk.JiraDesk · Desktop-entry: dev.jiradesk.JiraDesk · Summary: {TEST_NOTIFICATION_SUMMARY} · Body: {TEST_NOTIFICATION_BODY}")),
                                    )
                                    .child(
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
                                    .when_some(
                                        match &self.desktop_notification_test_state {
                                            DesktopNotificationTestState::Completed(report) => {
                                                Some(report.clone())
                                            }
                                            _ => None,
                                        },
                                        |this, report| {
                                            let result = match report.outcome {
                                                DesktopNotificationTestOutcome::Accepted {
                                                    notification_id,
                                                } => format!(
                                                    "Accepted by desktop service · notification ID {notification_id}"
                                                ),
                                                DesktopNotificationTestOutcome::Failed(error) => {
                                                    format!("Failed · error category {}", desktop_notification_error_category(error))
                                                }
                                            };
                                            this.child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(div().text_sm().child(format!("Last test · {} · {result}", report.timestamp)))
                                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Accepted by the desktop service does not prove GNOME displayed a banner."))
                                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Diagnostic events are written to diagnostics.jsonl.")),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(div().text_base().font_semibold().child("Saved Jira login"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Credentials are kept in the desktop system keyring, reused automatically across Jira Desk/AppImage versions, and never written to SQLite, preferences, or logs."),
                                    )
                                    .child(
                                        Button::new("forget-saved-jira-login")
                                            .label(if saved_login_deleting {
                                                "Forgetting saved Jira login…"
                                            } else {
                                                "Forget saved Jira login"
                                            })
                                            .when(layout.is_mobile(), |this| this.w_full())
                                            .disabled(saved_login_deleting)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.begin_forget_saved_login(cx)
                                            })),
                                    )
                                    .when_some(
                                        match self.saved_login_delete_state {
                                            SavedLoginDeleteState::Completed(outcome) => {
                                                Some(outcome)
                                            }
                                            SavedLoginDeleteState::Idle
                                            | SavedLoginDeleteState::Deleting => None,
                                        },
                                        |this, outcome| {
                                            let copy = saved_login_delete_feedback(outcome);
                                            this.child(
                                                div()
                                                    .text_sm()
                                                    .text_color(match copy.severity() {
                                                        FeedbackSeverity::Error => cx.theme().danger,
                                                        FeedbackSeverity::Info => cx.theme().muted_foreground,
                                                    })
                                                    .child(copy.message()),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                h_flex()
                                    .when(layout.is_mobile(), |this| this.flex_col())
                                    .gap_2()
                                    .child(
                                        Button::new("save-settings")
                                            .primary()
                                            .label("Save and refresh")
                                            .disabled(!live || self.operation_in_progress || validation.is_some())
                                            .on_click(cx.listener(|this, _, _, cx| this.begin_save_settings(cx))),
                                    )
                                    .child(
                                        Button::new("reset-settings")
                                            .ghost()
                                            .label("Use default scope")
                                            .disabled(!live || self.operation_in_progress)
                                        .on_click(cx.listener(|this, _, window, cx| this.reset_settings_editor(window, cx))),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(div().text_base().font_semibold().child("Team tracker"))
                                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("One Jira account ID or Atlassian email per line. This shows in-progress tickets assigned to those accounts; Jira permissions still apply."))
                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Email resolution requires exactly one active Jira user because the User search domain does not retain email. Uses existing read:jira-user/read:jira-work scopes; no new scope is needed."))
                                    .when_some(team_input, |this, input| this.child(Textarea::new(&input).w_full().h(px(if layout.is_mobile() { 110. } else { 140. })).aria_label("Team tracker members").disabled(!live || self.team_task.is_some() || self.operation_in_progress)))
                                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(format!("{} configured · maximum {}", self.team_members.len(), MAX_TEAM_MEMBERS)))
                                    .when_some(self.team_feedback.clone(), |this, message| this.child(div().text_sm().text_color(cx.theme().muted_foreground).child(message)))
                                    .child(h_flex().gap_2().when(layout.is_mobile(), |this| this.flex_col()).child(Button::new("save-team").primary().label(if self.team_task.is_some() { "Saving team…" } else { "Save team" }).disabled(!live || self.team_task.is_some() || self.operation_in_progress).on_click(cx.listener(|this, _, _, cx| this.begin_save_team(cx)))).child(Button::new("refresh-team").ghost().label("Refresh team").disabled(!live || self.team_task.is_some() || self.operation_in_progress || self.team_automatic_polling_paused).on_click(cx.listener(|this, _, _, cx| this.begin_team_refresh(cx))))),
                            )
                    ),
            )
    }
}
