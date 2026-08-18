//! Native application shell and first-run Jira connection form.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Root, Sizable as _, StyledExt as _, Theme,
    ThemeMode, TitleBar, button::Button, button::ButtonVariants as _, h_flex, input::Input,
    input::InputState, scroll::ScrollableElement as _, v_flex,
};

use crate::Dashboard;
use crate::config::{LiveSession, StartupSelection, live_session_from_manual_configuration};
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

/// The top-level view: either the configured dashboard or the first-run form.
pub struct AppShell {
    dashboard: Option<Entity<Dashboard>>,
    diagnostics: DiagnosticsSink,
    base_url: Entity<InputState>,
    email: Entity<InputState>,
    api_token: Entity<InputState>,
    connection_error: Option<String>,
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

        let (dashboard, connection_error, connection_status) = match startup {
            StartupSelection::Live(session) => (
                Some(Self::dashboard_from_live(session, diagnostics.clone(), cx)),
                None,
                None,
            ),
            StartupSelection::Preview => (None, None, None),
            StartupSelection::ConfigurationError(error) => (None, Some(error.to_string()), None),
        };

        Self {
            dashboard,
            diagnostics,
            base_url,
            email,
            api_token,
            connection_error,
            connection_status,
            connecting: false,
            notification_width: cx.theme().notification.width,
        }
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
                .placeholder("Paste your Jira API token")
        })
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let base_url = self.base_url.read(cx).unmask_value().to_string();
        let email = self.email.read(cx).unmask_value().to_string();
        let api_token = self.api_token.read(cx).unmask_value().to_string();
        let diagnostics = self.diagnostics.clone();

        // Replace the control before dispatching the async request. This drops
        // the masked input's edit history even when connection fails; a retry
        // must enter a fresh token.
        let old_api_token = std::mem::replace(&mut self.api_token, Self::new_api_token(window, cx));
        drop(old_api_token);
        self.connection_error = None;
        self.connection_status = Some("Verifying Jira account…".to_owned());
        self.connecting = true;

        cx.spawn(async move |this, cx| {
            let result = live_session_from_manual_configuration(base_url, email, api_token).await;
            let _ = this.update(cx, |this, cx| {
                this.connecting = false;
                this.connection_status = None;
                match result {
                    Ok(session) => {
                        this.connection_error = None;
                        this.dashboard = Some(Self::dashboard_from_live(session, diagnostics, cx));
                    }
                    Err(error) => {
                        // StartupError's Display implementation is intentionally
                        // redacted; don't expose request or credential details here.
                        this.connection_error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
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
                    .child(
                        v_flex()
                            .gap_4()
                            .child(Self::labeled_input(
                                "Jira URL",
                                "Your Atlassian Cloud site URL, including https://",
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
                                    .child(div().text_sm().font_semibold().child("API token"))
                                    .child(
                                        Input::new(&self.api_token)
                                            .w_full()
                                            .mask_toggle()
                                            .aria_label("Jira API token"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Use an unscoped API token. The token is kept in memory for this session, never written to local storage or logged."),
                                    ),
                            )
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
                                    .child("Jira read access is required; write permissions are used only after explicit confirmation. Create an API token in your Atlassian account security settings."),
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
            div()
                .min_w_0()
                .min_h_0()
                .flex_1()
                .child(dashboard.clone())
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
    use super::{notification_width_for_viewport, opposite_theme_mode};
    use gpui_component::ThemeMode;

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
