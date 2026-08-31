//! Native application shell and first-run Jira connection form.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Rems, Render, Role, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, relative, rems,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Root, Sizable as _, StyledExt as _, Theme, ThemeMode,
    TitleBar, WindowExt as _,
    button::Button,
    button::ButtonVariants as _,
    checkbox::Checkbox,
    dialog::{
        DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
    },
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState},
    scroll::ScrollableElement as _,
    spinner::Spinner,
    v_flex,
};

use jira_http::JiraBaseUrl;

use crate::config::{
    LiveSession, StartupSelection, live_session_from_manual_configuration,
    normalize_manual_base_url,
};
use crate::credential_store::{
    CredentialStoreError, SavedCredentials, load_saved_credentials, save_credentials,
};
use crate::dashboard::{Dashboard, DashboardEvent};
use crate::diagnostics::DiagnosticsSink;
use crate::responsive::layout_for_width;

const NOTIFICATION_SIDE_MARGIN_REMS: f32 = 1.0;

fn notification_width_for_viewport(
    viewport_width: Pixels,
    preferred_width: Rems,
    rem_size: Pixels,
) -> Pixels {
    let side_margin = rems(NOTIFICATION_SIDE_MARGIN_REMS).to_pixels(rem_size);
    let available_width = (viewport_width.as_f32() - (side_margin * 2.0).as_f32()).max(0.0);
    Pixels::from(
        preferred_width
            .to_pixels(rem_size)
            .as_f32()
            .min(available_width),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AppearancePreference {
    System,
    Light,
    Dark,
}

impl AppearancePreference {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub(crate) fn manual_theme_mode(self) -> Option<ThemeMode> {
        match self {
            Self::System => None,
            Self::Light => Some(ThemeMode::Light),
            Self::Dark => Some(ThemeMode::Dark),
        }
    }
}

const REMEMBER_CREDENTIALS_DEFAULT: bool = true;
const REMEMBER_CREDENTIALS_LABEL: &str = "Remember securely in system keyring";
const CHECKING_KEYRING_STATUS: &str = "Checking system keyring…";
const VERIFYING_CREDENTIALS_STATUS: &str =
    "Verifying Jira credentials and configuring your Jira connection…";
const JIRA_SITE_LABEL: &str = "Jira site";
const SCOPED_TOKEN_LABEL: &str = "Scoped API token";
const SCOPED_TOKEN_PLACEHOLDER: &str = "Paste your scoped Jira API token";
const SCOPED_TOKEN_SCOPES: &str =
    "Required scopes: read:jira-user, read:jira-work, write:jira-work.";
const KEYRING_STORAGE_COPY: &str = "Stored only in the system keyring—not app data.";
const UNSAVED_CREDENTIALS_COPY: &str = "Not saved after this session.";
const WRITE_SAFETY_COPY: &str = "Jira writes always require explicit confirmation.";
const TOKEN_REENTRY_COPY: &str = "Re-enter your scoped API token and try again.";

const ONBOARDING_MAX_WIDTH_REMS: f32 = 31.5;
const ONBOARDING_CARD_PADDING_REMS: f32 = 1.25;
const ONBOARDING_DIALOG_ACTION_WIDTH_REMS: f32 = 7.0;

fn is_submit_event(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::PressEnter {
            secondary: false,
            shift: false,
        }
    )
}

fn should_check_saved_credentials(startup: &StartupSelection) -> bool {
    matches!(startup, StartupSelection::Preview)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConnectionReadiness {
    base_url: bool,
    email: bool,
    api_token: bool,
}

impl ConnectionReadiness {
    fn is_ready(self) -> bool {
        self.base_url && self.email && self.api_token
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OnboardingBusyPolicy {
    inputs_disabled: bool,
    remember_disabled: bool,
    submit_disabled: bool,
    cancel_disabled: bool,
    trigger_disabled: bool,
}

fn onboarding_busy_policy(connecting: bool) -> OnboardingBusyPolicy {
    OnboardingBusyPolicy {
        inputs_disabled: connecting,
        remember_disabled: connecting,
        submit_disabled: connecting,
        cancel_disabled: connecting,
        trigger_disabled: connecting,
    }
}

/// Keeps the onboarding gate local, deterministic, and independent of the network request.
fn connection_readiness(base_url: &str, email: &str, api_token: &str) -> ConnectionReadiness {
    let api_token = api_token.trim();
    ConnectionReadiness {
        base_url: JiraBaseUrl::parse(normalize_manual_base_url(base_url)).is_ok(),
        email: is_plausible_email(email),
        api_token: !api_token.is_empty()
            && api_token
                .chars()
                .all(|character| !character.is_whitespace()),
    }
}

fn should_show_validation_guidance(
    connecting: bool,
    validation_attempted: bool,
    user_started_entering: bool,
) -> bool {
    !connecting && (validation_attempted || user_started_entering)
}

/// Keeps a busy onboarding form visibly and accessibly occupied even if an async transition has
/// not supplied a more specific status yet.
fn onboarding_status(connecting: bool, status: Option<&str>) -> Option<String> {
    status
        .map(str::to_owned)
        .or_else(|| connecting.then(|| VERIFYING_CREDENTIALS_STATUS.to_owned()))
}

fn validation_guidance(readiness: ConnectionReadiness) -> Option<String> {
    let mut guidance = Vec::new();
    if !readiness.base_url {
        guidance.push("Enter a Jira site name (for example, your-team) or an https:// URL ending in .atlassian.net with no path, port, query, or fragment.");
    }
    if !readiness.email {
        guidance.push("Enter a valid Atlassian account email.");
    }
    if !readiness.api_token {
        guidance.push("Enter a non-empty scoped API token without whitespace.");
    }
    (!guidance.is_empty()).then(|| guidance.join(" "))
}

fn is_plausible_email(value: &str) -> bool {
    let value = value.trim();
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || local
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return false;
    }

    let labels: Vec<_> = domain.split('.').collect();
    !labels.iter().any(|label| {
        label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    }) && labels.len() >= 2
}

fn connection_failure_copy(error: &str) -> String {
    format!("{error} {TOKEN_REENTRY_COPY}")
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
    validation_attempted: bool,
    user_started_entering: bool,
    connecting: bool,
    notification_width: Rems,
    input_subscriptions: Vec<Subscription>,
    api_token_subscription: Option<Subscription>,
    appearance_preference: AppearancePreference,
    appearance_subscription: Option<Subscription>,
    dashboard_subscription: Option<Subscription>,
    connection_enabled: bool,
}

impl AppShell {
    pub fn new(startup: StartupSelection, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // gpui-component initializes its theme to light. Resolve the system appearance before
        // constructing inputs or capturing theme-dependent state so dark launches do not flash
        // or remain light until the first platform appearance event.
        Theme::sync_system_appearance(Some(window), cx);
        let diagnostics = DiagnosticsSink::from_environment();
        diagnostics.session_started();
        let base_url = cx.new(|cx| InputState::new(window, cx).placeholder("your-team"));
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
            validation_attempted: false,
            user_started_entering: false,
            connecting: preview,
            notification_width: rems(cx.theme().notification.width / window.rem_size()),
            input_subscriptions: Vec::new(),
            api_token_subscription: None,
            appearance_preference: AppearancePreference::System,
            appearance_subscription: None,
            dashboard_subscription: None,
            connection_enabled: true,
        };
        shell.install_submit_shortcuts(window, cx);
        shell.appearance_subscription =
            Some(cx.observe_window_appearance(window, |this, window, cx| {
                if this.appearance_preference == AppearancePreference::System {
                    Theme::sync_system_appearance(Some(window), cx);
                    cx.notify();
                }
            }));
        if let Some(dashboard) = shell.dashboard.clone() {
            shell.install_dashboard_subscription(&dashboard, window, cx);
        }
        if preview {
            shell.connection_status = Some(CHECKING_KEYRING_STATUS.to_owned());
            shell.start_saved_login_check(cx);
        }
        shell
    }

    /// Constructs the production shell for an inert, fixture-backed UI-lab capture.
    ///
    /// This deliberately skips system-appearance synchronization, diagnostics setup, saved-login
    /// checks, and all live startup work. The same connection form, title bar, appearance state,
    /// and responsive layout are still rendered; the connect action is disabled as a safety
    /// boundary.
    #[cfg(any(feature = "ui-lab", feature = "ui-automation"))]
    pub(crate) fn new_for_ui_lab(
        dashboard: Option<Entity<Dashboard>>,
        theme: ThemeMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let base_url = cx.new(|cx| InputState::new(window, cx).placeholder("your-team"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        let api_token = Self::new_api_token(window, cx);
        let appearance_preference = match theme {
            ThemeMode::Light => AppearancePreference::Light,
            ThemeMode::Dark => AppearancePreference::Dark,
        };
        let mut shell = Self {
            dashboard,
            diagnostics: DiagnosticsSink::disabled(),
            base_url,
            email,
            api_token,
            remember_credentials: REMEMBER_CREDENTIALS_DEFAULT,
            connection_error: None,
            connection_warning: None,
            connection_status: None,
            validation_attempted: false,
            user_started_entering: false,
            connecting: false,
            notification_width: rems(cx.theme().notification.width / window.rem_size()),
            input_subscriptions: Vec::new(),
            api_token_subscription: None,
            appearance_preference,
            appearance_subscription: None,
            dashboard_subscription: None,
            connection_enabled: false,
        };
        // Keep the real Input controls fully constructed. The installed production submit path
        // is additionally guarded by `connection_enabled`, so it cannot initiate a request here.
        shell.install_submit_shortcuts(window, cx);
        if let Some(dashboard) = shell.dashboard.clone() {
            shell.install_dashboard_subscription(&dashboard, window, cx);
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

    fn install_dashboard_subscription(
        &mut self,
        dashboard: &Entity<Dashboard>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        // Appearance is a separate event subscription because it also updates shell state.
        self.dashboard_subscription.take();
        self.dashboard_subscription = Some(cx.subscribe_in(
            dashboard,
            window,
            |this, _, event: &DashboardEvent, _, cx| {
                let DashboardEvent::AppearanceChanged(preference) = event;
                this.appearance_preference = *preference;
                cx.notify();
            },
        ));
    }

    fn set_dashboard(
        &mut self,
        dashboard: Entity<Dashboard>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.dashboard = Some(dashboard.clone());
        self.install_dashboard_subscription(&dashboard, window, cx);
    }

    fn install_submit_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_subscriptions
            .push(Self::submit_subscription(&self.base_url, window, cx));
        self.input_subscriptions
            .push(Self::submit_subscription(&self.email, window, cx));
        self.api_token_subscription = Some(Self::submit_subscription(&self.api_token, window, cx));
    }

    fn submit_subscription(
        input: &Entity<InputState>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.user_started_entering = true;
                this.validation_attempted = false;
                cx.notify();
            } else if is_submit_event(event) {
                this.connect(window, cx);
            }
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
                        this.connection_status = Some(VERIFYING_CREDENTIALS_STATUS.to_owned());
                        cx.notify();
                    });
                    let result =
                        live_session_from_manual_configuration(base_url, email, api_token).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.connecting = false;
                        this.connection_status = None;
                        match result {
                            Ok(session) => {
                                this.connection_error = None;
                                let dashboard = Self::dashboard_from_live(session, diagnostics, cx);
                                this.set_dashboard(dashboard, window, cx);
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
        let base_url = normalize_manual_base_url(&self.base_url.read(cx).unmask_value());
        let email = self.email.read(cx).unmask_value().to_string();
        let api_token = self.api_token.read(cx).unmask_value().to_string();
        if !connection_readiness(&base_url, &email, &api_token).is_ready() {
            self.validation_attempted = true;
            // Local validation must not replace the masked input: the user can correct the
            // fields and submit again without having to paste the token again.
            cx.notify();
            return;
        }
        if !self.connection_enabled {
            return;
        }
        // A valid dispatch owns the form feedback from here: show only the request progress
        // until it resolves, then retain any actual failure alert and re-entry copy.
        self.validation_attempted = false;
        self.user_started_entering = false;
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
        let fresh_api_token = Self::new_api_token(window, cx);
        drop(self.api_token_subscription.take());
        let old_api_token = std::mem::replace(&mut self.api_token, fresh_api_token);
        drop(old_api_token);
        self.api_token_subscription = Some(Self::submit_subscription(&self.api_token, window, cx));
        self.connection_error = None;
        self.connection_status = Some(VERIFYING_CREDENTIALS_STATUS.to_owned());
        self.connecting = true;

        cx.spawn(async move |this, cx| {
            let result = live_session_from_manual_configuration(base_url, email, api_token).await;
            match result {
                Ok(session) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.connecting = false;
                        this.connection_status = None;
                        this.connection_error = None;
                        this.connection_warning = None;
                        let dashboard = Self::dashboard_from_live(session, diagnostics, cx);
                        this.set_dashboard(dashboard, window, cx);
                        if window.has_active_dialog(cx) {
                            window.close_dialog(cx);
                        }
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
                        this.connection_error = Some(connection_failure_copy(&error.to_string()));
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
        accessibility_id: &'static str,
        content_type: Option<InputContentType>,
        help: Option<&'static str>,
        disabled: bool,
        state: &Entity<InputState>,
        muted_foreground: gpui::Hsla,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(div().text_sm().font_semibold().child(label))
            .child(
                Input::new(state)
                    .w_full()
                    .when_some(content_type, |this, content_type| {
                        this.content_type(content_type)
                    })
                    .disabled(disabled)
                    .accessibility_id(accessibility_id)
                    .aria_label(label),
            )
            .when_some(help, |this, help| {
                this.child(div().text_xs().text_color(muted_foreground).child(help))
            })
    }

    fn build_connection_dialog_content(
        content: DialogContent,
        view: &Entity<Self>,
        base_url: &Entity<InputState>,
        email: &Entity<InputState>,
        api_token: &Entity<InputState>,
        cx: &mut gpui::App,
    ) -> DialogContent {
        let shell = view.read(cx);
        let busy_policy = onboarding_busy_policy(shell.connecting);
        let connection_status =
            onboarding_status(shell.connecting, shell.connection_status.as_deref());
        let readiness = connection_readiness(
            &base_url.read(cx).unmask_value(),
            &email.read(cx).unmask_value(),
            &api_token.read(cx).unmask_value(),
        );
        let validation_guidance = should_show_validation_guidance(
            shell.connecting,
            shell.validation_attempted,
            shell.user_started_entering,
        )
        .then(|| validation_guidance(readiness))
        .flatten();
        let error = shell.connection_error.as_ref().map(|message| {
            h_flex()
                .min_w_0()
                .w_full()
                .items_start()
                .gap_2()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().danger)
                .bg(cx.theme().danger.opacity(0.08))
                .px_3()
                .py_2()
                .text_sm()
                .text_color(cx.theme().danger)
                .id("connection-error")
                .accessibility_id("onboarding-error")
                .aria_label(message.clone())
                .role(Role::Alert)
                .child(div().flex_shrink_0().font_semibold().child("!"))
                .child(div().min_w_0().child(message.clone()))
        });
        let warning = shell.connection_warning.as_ref().map(|message| {
            h_flex()
                .min_w_0()
                .w_full()
                .items_start()
                .gap_2()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().warning)
                .bg(cx.theme().warning.opacity(0.08))
                .px_3()
                .py_2()
                .text_sm()
                .text_color(cx.theme().warning)
                .id("connection-warning")
                .accessibility_id("onboarding-warning")
                .aria_label(message.clone())
                .role(Role::Status)
                .child(div().flex_shrink_0().font_semibold().child("i"))
                .child(div().min_w_0().child(message.clone()))
        });
        let remember_copy = if shell.remember_credentials {
            KEYRING_STORAGE_COPY
        } else {
            UNSAVED_CREDENTIALS_COPY
        };
        let submit_view = view.clone();
        let remember_view = view.clone();

        content
            .child(
                DialogHeader::new()
                    .p_4()
                    .pb_0()
                    .child(DialogTitle::new().child("Connect Jira"))
                    .child(
                        DialogDescription::new()
                            .child("Enter your Jira site, Atlassian email, and scoped API token."),
                    ),
            )
            .child(
                v_flex()
                    .id("connection-dialog-body")
                    .debug_selector(|| "onboarding-connect-dialog-body".to_owned())
                    .accessibility_id("onboarding-connect-dialog-body")
                    .role(Role::Group)
                    .aria_label("Jira connection details")
                    .px_4()
                    .pb_4()
                    .gap_3()
                    .when_some(error, |this, error| this.child(error))
                    .when_some(warning, |this, warning| this.child(warning))
                    .when_some(validation_guidance, |this, guidance| {
                        this.child(
                            h_flex()
                                .id("connection-validation-guidance")
                                .accessibility_id("onboarding-validation")
                                .role(Role::Status)
                                .aria_label(guidance.clone())
                                .min_w_0()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(guidance),
                        )
                    })
                    .child(Self::labeled_input(
                        JIRA_SITE_LABEL,
                        "onboarding-jira-site",
                        None,
                        Some("Use your-team or a full HTTPS Atlassian Cloud URL."),
                        busy_policy.inputs_disabled,
                        base_url,
                        cx.theme().muted_foreground,
                    ))
                    .child(Self::labeled_input(
                        "Atlassian email",
                        "onboarding-atlassian-email",
                        Some(InputContentType::EmailAddress),
                        None,
                        busy_policy.inputs_disabled,
                        email,
                        cx.theme().muted_foreground,
                    ))
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_semibold().child(SCOPED_TOKEN_LABEL))
                            .child(
                                Input::new(api_token)
                                    .w_full()
                                    .when(!busy_policy.inputs_disabled, |this| this.mask_toggle())
                                    .content_type(InputContentType::Password)
                                    .disabled(busy_policy.inputs_disabled)
                                    .accessibility_id("onboarding-api-token")
                                    .aria_label(SCOPED_TOKEN_LABEL),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SCOPED_TOKEN_SCOPES),
                            ),
                    )
                    .child(
                        Checkbox::new("remember-jira-login")
                            .checked(shell.remember_credentials)
                            .disabled(busy_policy.remember_disabled)
                            .on_click(move |checked, _, cx| {
                                remember_view.update(cx, |this, cx| {
                                    this.remember_credentials = *checked;
                                    cx.notify();
                                });
                            })
                            .aria_label(REMEMBER_CREDENTIALS_LABEL)
                            .text_sm()
                            .child(
                                div()
                                    .min_w_0()
                                    .w_full()
                                    .line_height(relative(1.2))
                                    .child(REMEMBER_CREDENTIALS_LABEL),
                            ),
                    )
                    .when_some(connection_status, |this, status| {
                        this.child(
                            h_flex()
                                .id("connection-status")
                                .accessibility_id("onboarding-status")
                                .role(Role::Status)
                                .aria_label(status.clone())
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().primary.opacity(0.5))
                                .bg(cx.theme().primary.opacity(0.08))
                                .px_3()
                                .py_2()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .when(shell.connecting, |this| {
                                    this.child(Spinner::new().small().color(cx.theme().primary))
                                })
                                .child(div().min_w_0().child(status.clone())),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{remember_copy} {WRITE_SAFETY_COPY}")),
                    ),
            )
            .child(
                DialogFooter::new()
                    .px_4()
                    .pb_4()
                    .child(div().w(rems(ONBOARDING_DIALOG_ACTION_WIDTH_REMS)).child(
                        if busy_policy.cancel_disabled {
                            Button::new("cancel-jira-connection")
                                .w_full()
                                .label("Cancel")
                                .accessibility_id("onboarding-connect-dialog-cancel")
                                .debug_selector(|| "onboarding-connect-dialog-cancel".to_owned())
                                .outline()
                                .disabled(true)
                                .into_any_element()
                        } else {
                            DialogClose::new()
                                .child(
                                    Button::new("cancel-jira-connection")
                                        .w_full()
                                        .label("Cancel")
                                        .accessibility_id("onboarding-connect-dialog-cancel")
                                        .debug_selector(|| {
                                            "onboarding-connect-dialog-cancel".to_owned()
                                        })
                                        .outline(),
                                )
                                .into_any_element()
                        },
                    ))
                    .child(
                        Button::new("connect-jira-submit")
                            .w(rems(ONBOARDING_DIALOG_ACTION_WIDTH_REMS))
                            .label(if shell.connecting {
                                "Verifying…"
                            } else {
                                "Connect"
                            })
                            .primary()
                            .accessibility_id("onboarding-connect-dialog-submit")
                            .debug_selector(|| "onboarding-connect-dialog-submit".to_owned())
                            .disabled(
                                busy_policy.submit_disabled
                                    || !shell.connection_enabled
                                    || !readiness.is_ready(),
                            )
                            .on_click(move |_, window, cx| {
                                submit_view.update(cx, |this, cx| {
                                    this.connect(window, cx);
                                });
                            }),
                    ),
            )
    }

    fn open_connection_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let base_url = self.base_url.clone();
        let email = self.email.clone();
        let api_token = self.api_token.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let base_url = base_url.clone();
            let email = email.clone();
            let api_token = api_token.clone();
            let cancel_guard_view = view.clone();
            dialog
                .on_cancel(move |_, _, cx| !cancel_guard_view.read(cx).connecting)
                .p_0()
                .content(move |content, _, cx| {
                    Self::build_connection_dialog_content(
                        content, &view, &base_url, &email, &api_token, cx,
                    )
                })
        });
    }

    #[cfg(feature = "ui-lab")]
    pub(crate) fn open_connection_dialog_for_ui_lab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_connection_dialog(window, cx);
    }

    fn render_connection_form(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let mobile = layout.is_mobile();
        let busy_policy = onboarding_busy_policy(self.connecting);

        let welcome = v_flex()
            .min_w_0()
            .gap_3()
            .items_center()
            .px(rems(ONBOARDING_CARD_PADDING_REMS))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
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
                    .child(div().text_xl().font_semibold().child("Connect Jira")),
            )
            .child(
                div()
                    .max_w(rems(29.0))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .text_center()
                    .child("Sync issues assigned to or watched by your Jira account."),
            )
            .child(
                Button::new("connect-jira")
                    .label("Connect Jira")
                    .primary()
                    .accessibility_id("onboarding-connect-trigger")
                    .debug_selector(|| "onboarding-connect-trigger".to_owned())
                    .disabled(busy_policy.trigger_disabled)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_connection_dialog(window, cx);
                    })),
            );

        v_flex()
            .id("connection-form-scroll")
            .size_full()
            .bg(cx.theme().background)
            .when(!mobile, |this| this.items_center())
            .when(!mobile, |this| this.justify_center())
            .when(mobile, |this| this.items_start())
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .min_w_0()
                    .w_full()
                    .max_w(rems(ONBOARDING_MAX_WIDTH_REMS))
                    .gap_4()
                    .p(rems(layout.onboarding_padding() / 16.0))
                    .child(welcome),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notification_width = notification_width_for_viewport(
            window.viewport_size().width,
            self.notification_width,
            window.rem_size(),
        );
        Theme::global_mut(cx).notification.width = notification_width;
        let notification_layer = Root::render_notification_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let content = if let Some(dashboard) = &self.dashboard {
            v_flex()
                .min_w_0()
                .min_h_0()
                .flex_1()
                .when_some(self.connection_warning.as_ref(), |this, warning| {
                    this.child(
                        h_flex()
                            .items_start()
                            .gap_2()
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
                            .child(div().flex_shrink_0().font_semibold().child("i"))
                            .child(div().min_w_0().child(warning.clone())),
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

        v_flex()
            .size_full()
            .min_w_0()
            .child(TitleBar::new())
            .child(content)
            .when_some(dialog_layer, |this, layer| this.child(layer))
            .when_some(notification_layer, |this, layer| this.child(layer))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppearancePreference, CHECKING_KEYRING_STATUS, ConnectionReadiness, JIRA_SITE_LABEL,
        KEYRING_STORAGE_COPY, REMEMBER_CREDENTIALS_DEFAULT, REMEMBER_CREDENTIALS_LABEL,
        SCOPED_TOKEN_LABEL, SCOPED_TOKEN_PLACEHOLDER, SCOPED_TOKEN_SCOPES, TOKEN_REENTRY_COPY,
        UNSAVED_CREDENTIALS_COPY, VERIFYING_CREDENTIALS_STATUS, WRITE_SAFETY_COPY,
        connection_failure_copy, connection_readiness, is_submit_event,
        notification_width_for_viewport, onboarding_busy_policy, onboarding_status,
        save_credentials_warning, saved_login_warning, should_check_saved_credentials,
        should_show_validation_guidance, validation_guidance,
    };
    use crate::config::{StartupError, StartupSelection};
    use crate::credential_store::CredentialStoreError;
    use gpui::{WindowAppearance, px, rems};
    use gpui_component::ThemeMode;
    use gpui_component::input::InputEvent;

    #[test]
    fn connection_readiness_requires_all_three_fields() {
        assert!(
            connection_readiness(
                "https://example.atlassian.net",
                "person@example.com",
                "token-value",
            )
            .is_ready()
        );
        assert!(
            !connection_readiness(
                "http://example.atlassian.net",
                "person@example.com",
                "token-value"
            )
            .is_ready()
        );
        assert!(
            !connection_readiness("https://example.atlassian.net", "person", "token-value")
                .is_ready()
        );
        assert!(
            !connection_readiness(
                "https://example.atlassian.net",
                "person@example.com",
                " \t "
            )
            .is_ready()
        );
        assert!(
            connection_readiness(" Example-Team ", "person@example.com", "token-value").is_ready()
        );
        assert!(
            connection_readiness(
                " Example-Team.ATLASSIAN.NET ",
                "person@example.com",
                "token-value",
            )
            .is_ready()
        );
    }

    #[test]
    fn connection_readiness_matches_jira_base_url_and_manual_token_rules() {
        assert!(
            connection_readiness(
                "https://jira.example.atlassian.net/",
                "person+jira@example.co.uk",
                "token",
            )
            .is_ready()
        );
        for invalid_url in [
            "example team",
            "-example-team",
            "example-team-",
            "example.team",
            "example.atlassian.com",
            "example.atlassian.net/path",
            "http://example-team.atlassian.net",
            "https://jira.example.com",
            "https://jira.example.atlassian.net/tenant",
            "https://jira.example.atlassian.net:8443",
            "https://jira.example.atlassian.net/?token=secret",
            "https://jira.example.atlassian.net#fragment",
        ] {
            assert!(
                !connection_readiness(invalid_url, "person@example.com", "token").is_ready(),
                "{invalid_url}"
            );
        }
        assert!(
            connection_readiness(
                "https://jira.example.atlassian.net",
                "person@example.com",
                "\t token \n",
            )
            .is_ready()
        );
        assert!(
            !connection_readiness(
                "https://jira.example.atlassian.net",
                "person@example.com",
                "tok en",
            )
            .is_ready()
        );
        assert!(
            !connection_readiness(
                "https://jira.example.atlassian.net",
                "@example.com",
                "token"
            )
            .is_ready()
        );
        assert!(
            !connection_readiness(
                "https://jira.example.atlassian.net",
                "person@example",
                "token",
            )
            .is_ready()
        );
    }

    #[test]
    fn validation_guidance_is_targeted_and_absent_when_everything_is_ready() {
        let guidance = validation_guidance(ConnectionReadiness {
            base_url: false,
            email: false,
            api_token: false,
        })
        .unwrap();
        assert!(guidance.contains(".atlassian.net"));
        assert!(guidance.contains("account email"));
        assert!(guidance.contains("without whitespace"));
        assert!(
            validation_guidance(ConnectionReadiness {
                base_url: true,
                email: true,
                api_token: true,
            })
            .is_none()
        );
    }

    #[test]
    fn validation_guidance_is_suppressed_while_connecting() {
        assert!(!should_show_validation_guidance(true, true, true));
        assert!(!should_show_validation_guidance(true, false, true));
        assert!(should_show_validation_guidance(false, true, false));
        assert!(should_show_validation_guidance(false, false, true));
        assert!(!should_show_validation_guidance(false, false, false));
    }

    #[test]
    fn onboarding_busy_state_has_stable_fallback_status() {
        assert_eq!(
            onboarding_status(true, None).as_deref(),
            Some(VERIFYING_CREDENTIALS_STATUS)
        );
        assert_eq!(
            onboarding_status(true, Some(CHECKING_KEYRING_STATUS)).as_deref(),
            Some(CHECKING_KEYRING_STATUS)
        );
        assert_eq!(onboarding_status(false, None), None);
    }

    #[test]
    fn onboarding_busy_policy_disables_every_mutable_control_and_action() {
        assert_eq!(
            onboarding_busy_policy(true),
            super::OnboardingBusyPolicy {
                inputs_disabled: true,
                remember_disabled: true,
                submit_disabled: true,
                cancel_disabled: true,
                trigger_disabled: true,
            }
        );
        assert_eq!(
            onboarding_busy_policy(false),
            super::OnboardingBusyPolicy {
                inputs_disabled: false,
                remember_disabled: false,
                submit_disabled: false,
                cancel_disabled: false,
                trigger_disabled: false,
            }
        );
    }

    #[test]
    fn failed_connection_copy_requires_token_reentry_without_exposing_a_secret() {
        let copy = connection_failure_copy("Jira rejected the credentials");
        assert!(copy.contains("Jira rejected the credentials"));
        assert!(copy.contains(TOKEN_REENTRY_COPY));
        assert!(copy.contains("Re-enter"));
        assert!(!copy.contains("token-value"));
    }

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
    fn remembered_login_copy_is_stable_and_is_the_accessibility_label() {
        assert_eq!(
            REMEMBER_CREDENTIALS_LABEL,
            "Remember securely in system keyring"
        );
    }

    #[test]
    fn onboarding_uses_scoped_token_copy_and_statuses() {
        assert_eq!(JIRA_SITE_LABEL, "Jira site");
        assert_eq!(SCOPED_TOKEN_LABEL, "Scoped API token");
        assert_eq!(SCOPED_TOKEN_PLACEHOLDER, "Paste your scoped Jira API token");
        assert!(SCOPED_TOKEN_SCOPES.contains("read:jira-user"));
        assert!(SCOPED_TOKEN_SCOPES.contains("read:jira-work"));
        assert!(SCOPED_TOKEN_SCOPES.contains("write:jira-work"));
        assert_eq!(
            KEYRING_STORAGE_COPY,
            "Stored only in the system keyring—not app data."
        );
        assert_eq!(
            WRITE_SAFETY_COPY,
            "Jira writes always require explicit confirmation."
        );
        assert_eq!(UNSAVED_CREDENTIALS_COPY, "Not saved after this session.");
        assert!(CHECKING_KEYRING_STATUS.contains("Checking system keyring"));
        assert!(VERIFYING_CREDENTIALS_STATUS.contains("Verifying Jira credentials"));
        assert!(VERIFYING_CREDENTIALS_STATUS.contains("configuring"));
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
    fn appearance_preference_labels_identify_the_current_theme() {
        assert_eq!(AppearancePreference::System.label(), "System");
        assert_eq!(AppearancePreference::Light.label(), "Light");
        assert_eq!(AppearancePreference::Dark.label(), "Dark");
    }

    #[test]
    fn system_is_the_only_preference_without_a_manual_theme_mode() {
        assert_eq!(AppearancePreference::System.manual_theme_mode(), None);
        assert_eq!(
            AppearancePreference::Light.manual_theme_mode(),
            Some(ThemeMode::Light)
        );
        assert_eq!(
            AppearancePreference::Dark.manual_theme_mode(),
            Some(ThemeMode::Dark)
        );
    }

    #[test]
    fn system_appearance_maps_vibrant_modes_to_their_base_theme() {
        assert_eq!(ThemeMode::from(WindowAppearance::Light), ThemeMode::Light);
        assert_eq!(
            ThemeMode::from(WindowAppearance::VibrantLight),
            ThemeMode::Light
        );
        assert_eq!(ThemeMode::from(WindowAppearance::Dark), ThemeMode::Dark);
        assert_eq!(
            ThemeMode::from(WindowAppearance::VibrantDark),
            ThemeMode::Dark
        );
    }

    #[test]
    fn only_plain_enter_submits_the_connection_form() {
        assert!(is_submit_event(&InputEvent::PressEnter {
            secondary: false,
            shift: false,
        }));
        assert!(!is_submit_event(&InputEvent::PressEnter {
            secondary: true,
            shift: false,
        }));
        assert!(!is_submit_event(&InputEvent::PressEnter {
            secondary: false,
            shift: true,
        }));
        assert!(!is_submit_event(&InputEvent::Change));
    }

    #[test]
    fn notification_width_preserves_margins_on_narrow_viewports() {
        assert_eq!(
            notification_width_for_viewport(px(320.0), rems(382.0 / 16.0), px(16.0)),
            px(288.0)
        );
        assert_eq!(
            notification_width_for_viewport(px(360.0), rems(382.0 / 16.0), px(16.0)),
            px(328.0)
        );
        assert_eq!(
            notification_width_for_viewport(px(390.0), rems(382.0 / 16.0), px(16.0)),
            px(358.0)
        );
    }

    #[test]
    fn notification_width_caps_at_preferred_desktop_width() {
        assert_eq!(
            notification_width_for_viewport(px(1_024.0), rems(382.0 / 16.0), px(16.0)),
            px(382.0)
        );
        assert_eq!(
            notification_width_for_viewport(px(1_024.0), rems(300.0 / 16.0), px(16.0)),
            px(300.0)
        );
    }
}
