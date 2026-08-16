//! Native application shell and first-run Jira connection form.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, StyledExt as _, button::Button, button::ButtonVariants as _, h_flex,
    input::Input, input::InputState, scroll::ScrollableElement as _, v_flex,
};

use crate::Dashboard;
use crate::config::{LiveSession, StartupSelection, live_session_from_manual_configuration};

/// The top-level view: either the configured dashboard or the first-run form.
pub struct AppShell {
    dashboard: Option<Entity<Dashboard>>,
    base_url: Entity<InputState>,
    email: Entity<InputState>,
    api_token: Entity<InputState>,
    assignees: Entity<InputState>,
    connection_error: Option<String>,
}

impl AppShell {
    pub fn new(startup: StartupSelection, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let base_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://your-team.atlassian.net"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        let api_token = Self::new_api_token(window, cx);
        let assignees =
            cx.new(|cx| InputState::new(window, cx).placeholder("account-id-1, account-id-2"));

        let (dashboard, connection_error) = match startup {
            StartupSelection::Live(session) => (Some(Self::dashboard_from_live(session, cx)), None),
            StartupSelection::Preview => (None, None),
            StartupSelection::ConfigurationError(error) => (None, Some(error.to_string())),
        };

        Self {
            dashboard,
            base_url,
            email,
            api_token,
            assignees,
            connection_error,
        }
    }

    fn dashboard_from_live(session: LiveSession, cx: &mut Context<Self>) -> Entity<Dashboard> {
        cx.new(|cx| Dashboard::from_live(session, cx))
    }

    fn new_api_token(window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste your Jira API token")
        })
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let base_url = self.base_url.read(cx).unmask_value().to_string();
        let email = self.email.read(cx).unmask_value().to_string();
        let api_token = self.api_token.read(cx).unmask_value().to_string();
        let assignees = self.assignees.read(cx).unmask_value().to_string();

        match live_session_from_manual_configuration(base_url, email, api_token, assignees) {
            Ok(session) => {
                // Replace the control so the secret and its edit history are
                // discarded before handing the session to the dashboard. The
                // HTTP client owns the credential for this session.
                let old_api_token =
                    std::mem::replace(&mut self.api_token, Self::new_api_token(window, cx));
                drop(old_api_token);
                self.connection_error = None;
                self.dashboard = Some(Self::dashboard_from_live(session, cx));
            }
            Err(error) => {
                // StartupError's Display implementation is intentionally
                // redacted; don't expose request or credential details here.
                self.connection_error = Some(error.to_string());
            }
        }
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

    fn render_connection_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.connection_error.as_ref().map(|message| {
            div()
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
            .items_center()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(560.))
                    .gap_5()
                    .p_8()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
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
                                            .gap_0p5()
                                            .child(div().text_xl().font_semibold().child("Connect Jira"))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Configure a read-only Jira Cloud workspace"),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Jira Desk pulls issues, epics, and updates for the account IDs you provide. It never writes to Jira."),
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
                                            .child("Internal API-token mode · use a token created without scopes. Scoped tokens require a future cloud-ID gateway flow. Token kept in memory for this session; not written to local storage"),
                                    ),
                            )
                            .child(Self::labeled_input(
                                "Assignee account IDs",
                                "Comma-separated Atlassian account IDs to follow",
                                &self.assignees,
                                cx.theme().muted_foreground,
                            )),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                Button::new("connect-jira")
                                    .label("Connect read-only")
                                    .primary()
                                    .w_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.connect(window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Read-only access is required. Create an API token in your Atlassian account security settings."),
                            ),
                    ),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(dashboard) = &self.dashboard {
            return div()
                .size_full()
                .child(dashboard.clone())
                .into_any_element();
        }

        self.render_connection_form(cx).into_any_element()
    }
}
