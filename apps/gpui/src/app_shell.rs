//! Native application shell and first-run Jira connection form.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Root, Sizable as _, StyledExt as _, Theme,
    ThemeMode, TitleBar, button::Button, button::ButtonVariants as _, checkbox::Checkbox, h_flex,
    input::Input, input::InputState, scroll::ScrollableElement as _, v_flex,
};

use crate::Dashboard;
use crate::config::{LiveSession, StartupSelection, live_session_from_manual_configuration};
use crate::credential_store::{
    CredentialStoreError, SavedCredentials, load_saved_credentials, save_credentials,
};
use crate::diagnostics::DiagnosticsSink;
use crate::responsive::layout_for_width;

const NOTIFICATION_SIDE_MARGIN: f32 = 16.0;

fn notification_width_for_viewport(viewport_width: f32, preferred_width: f32) -> f32 {
    let available_width = (viewport_width - NOTIFICATION_SIDE_MARGIN * 2.0).max(0.0);
    preferred_width.min(available_width)
}

fn opposite_theme_mode(mode: ThemeMode) -> ThemeMode {
    if mode.is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    }
}

const REMEMBER_CREDENTIALS_DEFAULT: bool = true;
const CHECKING_KEYRING_STATUS: &str = "Checking system keyring…";
const VERIFYING_SCOPED_TOKEN_STATUS: &str = "Resolving Jira site and verifying scoped token…";
const SCOPED_TOKEN_LABEL: &str = "Scoped API token";
const SCOPED_TOKEN_PLACEHOLDER: &str = "Paste your scoped Jira API token";
const SCOPED_TOKEN_SCOPES: &str =
    "For full functionality, select exactly: read:jira-user, read:jira-work, write:jira-work.";

fn should_check_saved_credentials(startup: &StartupSelection) -> bool {
    matches!(startup, StartupSelection::Preview)
}

fn saved_login_warning(error: CredentialStoreError) -> String {
    format!("Saved Jira login unavailable: {error}. Enter your credentials to continue.")
}

fn save_credentials_warning(error: CredentialStoreError) -> String {
    format!(
        "Could not save your Jira login securely ({error}). This session remains connected, but you will need to sign in again next time."
    )
}

/// The top-level view: either the configured dashboard or the first-run form.
pub struct AppShell {
    dashboard: Option<Entity<Dashboard>>,
    diagnostics: DiagnosticsSink,
    base_url: Entity<InputState>,
    email: Entity<InputState>,
    api_token: Entity<InputState>,
    remember_credentials: bool,
    connection_error: Option<String>,
    connection_warning: Option<String>,
    connection_status: Option<String>,
    connecting: bool,
    notification_width: Pixels,
}

impl AppShell {
    pub fn new(startup: StartupSelection, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let diagnostics = DiagnosticsSink::from_environment();
        diagnostics.session_started();
        let base_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://your-team.atlassian.net"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        let api_token = Self::new_api_token(window, cx);

        let preview = should_check_saved_credentials(&startup);
        let (dashboard, connection_error, connection_status) = match startup {
            StartupSelection::Live(session) => (
                Some(Self::dashboard_from_live(session, diagnostics.clone(), cx)),
                None,
                None,
            ),
            StartupSelection::Preview => (None, None, None),
            StartupSelection::ConfigurationError(error) => (None, Some(error.to_string()), None),
        };

        let mut shell = Self {
            dashboard,
            diagnostics,
            base_url,
            email,
            api_token,
            remember_credentials: REMEMBER_CREDENTIALS_DEFAULT,
            connection_error,
            connection_warning: None,
            connection_status,
            connecting: preview,
            notification_width: cx.theme().notification.width,
        };
        if preview {
            shell.connection_status = Some(CHECKING_KEYRING_STATUS.to_owned());
            shell.start_saved_login_check(cx);
        }
        shell
    }

    fn dashboard_from_live(
        session: LiveSession,
        diagnostics: DiagnosticsSink,
        cx: &mut Context<Self>,
    ) -> Entity<Dashboard> {
        cx.new(|cx| Dashboard::from_live(session, diagnostics, cx))
    }

    fn new_api_token(window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(SCOPED_TOKEN_PLACEHOLDER)
        })
    }

    fn start_saved_login_check(&mut self, cx: &mut Context<Self>) {
        let diagnostics = self.diagnostics.clone();
        cx.spawn(async move |this, cx| {
            match load_saved_credentials().await {
                Ok(None) => {
                    let _ = this.update(cx, |this, cx| {
                        this.connecting = false;
                        this.connection_status = None;
                        cx.notify();
                    });
                }
                Ok(Some(saved)) => {
                    let (base_url, email, api_token) = saved.into_parts();
                    let _ = this.update(cx, |this, cx| {
                        this.connection_status = Some(VERIFYING_SCOPED_TOKEN_STATUS.to_owned());
                        cx.notify();
                    });
                    let result =
                        live_session_from_manual_configuration(base_url, email, api_token).await;
                    let _ = this.update(cx, |this, cx| {
                        this.connecting = false;
                        this.connection_status = None;
                        match result {
                            Ok(session) => {
                                this.connection_error = None;
                                this.dashboard =
                                    Some(Self::dashboard_from_live(session, diagnostics, cx));
                            }
                            Err(error) => {
                                // StartupError's Display implementation is intentionally
                                // redacted; don't expose request or credential details here.
                                this.connection_error = Some(error.to_string());
                            }
                        }
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.connecting = false;
                        this.connection_status = None;
                        this.connection_warning = Some(saved_login_warning(error));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let base_url = self.base_url.read(cx).unmask_value().to_string();
        let email = self.email.read(cx).unmask_value().to_string();
        let api_token = self.api_token.read(cx).unmask_value().to_string();
        let credentials_to_save = if self.remember_credentials {
            Some(SavedCredentials::new(
                base_url.clone(),
                email.clone(),
                api_token.clone(),
            ))
        } else {
            None
        };
        let diagnostics = self.diagnostics.clone();

        // Replace the control before dispatching the async request. This drops
        // the masked input's edit history even when connection fails; a retry
        // must enter a fresh token.
        let old_api_token = std::mem::replace(&mut self.api_token, Self::new_api_token(window, cx));
        drop(old_api_token);
        self.connection_error = None;
        self.connection_status = Some(VERIFYING_SCOPED_TOKEN_STATUS.to_owned());
        self.connecting = true;

        cx.spawn(async move |this, cx| {
            let result = live_session_from_manual_configuration(base_url, email, api_token).await;
            match result {
                Ok(session) => {
                    let _ = this.update(cx, |this, cx| {
                        this.connecting = false;
                        this.connection_status = None;
                        this.connection_error = None;
                        this.connection_warning = None;
                        this.dashboard = Some(Self::dashboard_from_live(session, diagnostics, cx));
                        cx.notify();
                    });

                    let save_warning = match credentials_to_save {
                        Some(Ok(credentials)) => save_credentials(credentials)
                            .await
                            .err()
                            .map(save_credentials_warning),
                        Some(Err(error)) => Some(save_credentials_warning(error)),
                        None => None,
                    };
                    if let Some(warning) = save_warning {
                        let _ = this.update(cx, |this, cx| {
                            this.connection_warning = Some(warning);
                            cx.notify();
                        });
                    }
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.connecting = false;
                        this.connection_status = None;
                        // StartupError's Display implementation is intentionally
                        // redacted; don't expose request or credential details here.
                        this.connection_error = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn labeled_input(
        label: &'static str,
        help: &'static str,
        state: &Entity<InputState>,
        muted_foreground: gpui::Hsla,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(div().text_sm().font_semibold().child(label))
            .child(Input::new(state).w_full().aria_label(label))
            .child(div().text_xs().text_color(muted_foreground).child(help))
    }

    fn render_connection_form(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let mobile = layout.is_mobile();
        let error = self.connection_error.as_ref().map(|message| {
            div()
                .min_w_0()
                .w_full()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().danger)
                .bg(cx.theme().danger.opacity(0.08))
                .px_3()
                .py_2()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(message.clone())
        });
        let warning = self.connection_warning.as_ref().map(|message| {
            div()
                .min_w_0()
                .w_full()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().warning)
                .bg(cx.theme().warning.opacity(0.08))
                .px_3()
                .py_2()
                .text_sm()
                .text_color(cx.theme().warning)
                .child(message.clone())
        });

        v_flex()
            .id("connection-form-scroll")
            .size_full()
            .when(!mobile, |this| this.items_center())
            .when(mobile, |this| this.items_start())
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .min_w_0()
                    .w_full()
                    .max_w(px(560.))
                    .gap_5()
                    .p(px(layout.onboarding_padding()))
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap_2()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .size_10()
                                            .items_center()
                                            .justify_center()
                                            .rounded(cx.theme().radius)
                                            .bg(cx.theme().primary)
                                            .text_color(cx.theme().primary_foreground)
                                            .font_bold()
                                            .child("JD"),
                                    )
                                    .child(
                                        v_flex()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(div().text_xl().font_semibold().child("Connect Jira"))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                            .child("Configure a Jira workspace"),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Jira Desk discovers the authenticated user's assigned or watched issues from Jira; no project name is required. Jira changes are limited to explicitly confirmed comments, assignee changes, and status transitions."),
                            ),
                    )
                    .when_some(error, |this, error| this.child(error))
                    .when_some(warning, |this, warning| this.child(warning))
                    .child(
                        v_flex()
                            .gap_4()
                            .child(Self::labeled_input(
                                "Jira URL",
                                "Your Atlassian Cloud site URL, including https://. Cloud ID is discovered automatically.",
                                &self.base_url,
                                cx.theme().muted_foreground,
                            ))
                            .child(Self::labeled_input(
                                "Atlassian email",
                                "The email address associated with your Jira account",
                                &self.email,
                                cx.theme().muted_foreground,
                            ))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .child(SCOPED_TOKEN_LABEL),
                                    )
                                    .child(
                                        Input::new(&self.api_token)
                                            .w_full()
                                            .mask_toggle()
                                            .aria_label(SCOPED_TOKEN_LABEL),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Create an API token with scopes in your Atlassian account security settings."),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(SCOPED_TOKEN_SCOPES),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Jira project permissions still apply. After successful authentication, the token is stored only in the system keyring when enabled—never in SQLite, preferences, or logs."),
                                    ),
                            )
                    )
                    .child(
                        Checkbox::new("remember-jira-login")
                            .checked(self.remember_credentials)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.remember_credentials = *checked;
                                cx.notify();
                            }))
                            .label("Remember securely in system keyring")
                            .text_sm(),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .when_some(self.connection_status.as_ref(), |this, status| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(status.clone()),
                                )
                            })
                            .child(
                                Button::new("connect-jira")
                                    .label(if self.connecting {
                                        "Connecting…"
                                    } else {
                                        "Connect"
                                    })
                                    .primary()
                                    .disabled(self.connecting)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.connect(window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Jira read access is required; write permissions are used only after explicit confirmation. Jira permissions still apply to the selected token scopes."),
                            ),
                    ),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notification_width = notification_width_for_viewport(
            window.viewport_size().width.as_f32(),
            self.notification_width.as_f32(),
        );
        Theme::global_mut(cx).notification.width = px(notification_width);
        let notification_layer = Root::render_notification_layer(window, cx);
        let content = if let Some(dashboard) = &self.dashboard {
            v_flex()
                .min_w_0()
                .min_h_0()
                .flex_1()
                .when_some(self.connection_warning.as_ref(), |this, warning| {
                    this.child(
                        div()
                            .mx_4()
                            .mb_3()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().warning)
                            .bg(cx.theme().warning.opacity(0.08))
                            .px_3()
                            .py_2()
                            .text_sm()
                            .text_color(cx.theme().warning)
                            .child(warning.clone()),
                    )
                })
                .child(div().min_w_0().min_h_0().flex_1().child(dashboard.clone()))
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .min_h_0()
                .flex_1()
                .child(self.render_connection_form(window, cx))
                .into_any_element()
        };

        let current_mode = cx.theme().mode;
        let dark_mode = current_mode.is_dark();
        let (theme_icon, theme_tooltip, theme_accessibility_id) = if dark_mode {
            (
                IconName::Sun,
                "Switch to light mode",
                "switch-to-light-mode",
            )
        } else {
            (IconName::Moon, "Switch to dark mode", "switch-to-dark-mode")
        };
        let destination_mode = opposite_theme_mode(current_mode);
        let theme_toggle = Button::new("theme-toggle")
            .compact()
            .ghost()
            .xsmall()
            .icon(theme_icon)
            .tooltip(theme_tooltip)
            .accessibility_id(theme_accessibility_id)
            .on_click(move |_, window, cx| {
                Theme::change(destination_mode, Some(window), cx);
            });

        v_flex()
            .size_full()
            .min_w_0()
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .pr_2()
                        .child(div().text_sm().font_semibold().child("Jira Desk"))
                        .child(theme_toggle),
                ),
            )
            .child(content)
            .when_some(notification_layer, |this, layer| this.child(layer))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHECKING_KEYRING_STATUS, REMEMBER_CREDENTIALS_DEFAULT, SCOPED_TOKEN_LABEL,
        SCOPED_TOKEN_PLACEHOLDER, SCOPED_TOKEN_SCOPES, VERIFYING_SCOPED_TOKEN_STATUS,
        notification_width_for_viewport, opposite_theme_mode, save_credentials_warning,
        saved_login_warning, should_check_saved_credentials,
    };
    use crate::config::{StartupError, StartupSelection};
    use crate::credential_store::CredentialStoreError;
    use gpui_component::ThemeMode;

    #[test]
    fn saved_credentials_are_checked_only_for_preview() {
        assert!(should_check_saved_credentials(&StartupSelection::Preview));
        assert!(!should_check_saved_credentials(
            &StartupSelection::ConfigurationError(StartupError::Incomplete,)
        ));
    }

    #[test]
    fn remembered_login_is_enabled_by_default() {
        assert!(REMEMBER_CREDENTIALS_DEFAULT);
    }

    #[test]
    fn onboarding_uses_scoped_token_copy_and_statuses() {
        assert_eq!(SCOPED_TOKEN_LABEL, "Scoped API token");
        assert_eq!(SCOPED_TOKEN_PLACEHOLDER, "Paste your scoped Jira API token");
        assert!(SCOPED_TOKEN_SCOPES.contains("read:jira-user"));
        assert!(SCOPED_TOKEN_SCOPES.contains("read:jira-work"));
        assert!(SCOPED_TOKEN_SCOPES.contains("write:jira-work"));
        assert!(CHECKING_KEYRING_STATUS.contains("Checking system keyring"));
        assert!(VERIFYING_SCOPED_TOKEN_STATUS.contains("verifying scoped token"));
    }

    #[test]
    fn credential_store_failures_are_safe_and_non_secret() {
        let load_warning = saved_login_warning(CredentialStoreError::Unavailable);
        let save_warning = save_credentials_warning(CredentialStoreError::Malformed);
        assert!(load_warning.contains("Enter your credentials"));
        assert!(save_warning.contains("session remains connected"));
        assert!(!load_warning.contains("token"));
        assert!(!save_warning.contains("token"));
    }

    #[test]
    fn opposite_theme_mode_switches_between_light_and_dark() {
        assert_eq!(opposite_theme_mode(ThemeMode::Light), ThemeMode::Dark);
        assert_eq!(opposite_theme_mode(ThemeMode::Dark), ThemeMode::Light);
    }

    #[test]
    fn notification_width_preserves_margins_on_narrow_viewports() {
        assert_eq!(notification_width_for_viewport(320.0, 382.0), 288.0);
        assert_eq!(notification_width_for_viewport(360.0, 382.0), 328.0);
        assert_eq!(notification_width_for_viewport(390.0, 382.0), 358.0);
    }

    #[test]
    fn notification_width_caps_at_preferred_desktop_width() {
        assert_eq!(notification_width_for_viewport(1_024.0, 382.0), 382.0);
        assert_eq!(notification_width_for_viewport(1_024.0, 300.0), 300.0);
    }
}
