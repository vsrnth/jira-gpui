//! Development-only native host for local macOS accessibility automation.
//!
//! This host intentionally composes the production [`AppShell`] and [`Root`] around deterministic
//! fixture data. It does not call the normal startup path, inspect the environment, open the
//! keychain, create a store, contact Jira, start polling, or dispatch notifications and writes.

use anyhow::{Result, bail};

/// A deterministic production-view scenario exposed to the local automation driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAutomationScenario {
    /// The first-run connection form, with the dialog initially closed.
    Onboarding,
    /// The local Jira issue list and detail view.
    Issues,
    /// Rich issue content with a preloaded image fixture.
    RichContent,
    /// The local change ledger.
    Updates,
    /// The local team tracker.
    Team,
    /// The local settings view.
    Settings,
}

impl UiAutomationScenario {
    /// Returns the stable command-line spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::Issues => "issues",
            Self::RichContent => "rich-content",
            Self::Updates => "updates",
            Self::Team => "team",
            Self::Settings => "settings",
        }
    }

    /// Parses one of the explicitly supported scenario names.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "onboarding" => Ok(Self::Onboarding),
            "issues" => Ok(Self::Issues),
            "rich-content" => Ok(Self::RichContent),
            "updates" => Ok(Self::Updates),
            "team" => Ok(Self::Team),
            "settings" => Ok(Self::Settings),
            _ => bail!(
                "unknown scenario {value:?}; expected one of: onboarding, issues, rich-content, updates, team, settings"
            ),
        }
    }
}

/// Parsed host command. Help and list are intentionally commands rather than implicit fallbacks,
/// so malformed invocations never launch a window by accident.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Help,
    List,
    Launch(UiAutomationScenario),
}

/// Text printed by `--help`.
pub const HELP: &str = "Usage: cargo run -p jira-gpui --features ui-automation --bin jira-ui-automation-host -- --scenario NAME\n\nOptions:\n  --scenario NAME  onboarding | issues | rich-content | updates | team | settings\n  --list            List supported scenarios\n  -h, --help        Show this help\n\nThe host opens one visible, fixture-backed Jira Desk window for local macOS accessibility automation.\nIt does not load environment startup, keychain, persistence, Jira, network, polling, notifications, or write services.";

const SCENARIOS: &str = "onboarding, issues, rich-content, updates, team, settings";

fn next_value(args: &mut impl Iterator<Item = String>) -> Result<String> {
    let value = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("--scenario requires a value; use --help for usage"))?;
    if value.starts_with('-') || value.trim().is_empty() {
        bail!("--scenario requires a non-empty scenario name; use --help for usage");
    }
    Ok(value)
}

/// Parses the deliberately small, strict host CLI.
pub fn parse_args<I, S>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let Some(first) = args.next() else {
        bail!("--scenario is required; use --list to see choices")
    };

    let command = match first.as_str() {
        "-h" | "--help" => Command::Help,
        "--list" => Command::List,
        "--scenario" => Command::Launch(UiAutomationScenario::parse(&next_value(&mut args)?)?),
        _ if first.starts_with('-') => bail!("unknown option {first:?}; use --help for usage"),
        _ => bail!("unexpected argument {first:?}; use --help for usage"),
    };

    if let Some(extra) = args.next() {
        bail!("unexpected argument {extra:?}; use --help for usage");
    }
    Ok(command)
}

/// Parses and runs one local automation-host command.
pub fn run<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    match parse_args(args)? {
        Command::Help => {
            println!("{HELP}");
            Ok(())
        }
        Command::List => {
            println!("Scenarios: {SCENARIOS}");
            Ok(())
        }
        Command::Launch(scenario) => launch(scenario),
    }
}

#[cfg(target_os = "macos")]
fn launch(scenario: UiAutomationScenario) -> Result<()> {
    use crate::{
        AppAssets, AppShell,
        app_shell::AppearancePreference,
        dashboard::{Dashboard, SampleSection},
    };
    use gpui::{
        App, AppContext as _, Bounds, Pixels, Size, WindowBounds, WindowDecorations, WindowOptions,
        px, size,
    };
    use gpui_component::{Root, Theme, ThemeMode, TitleBar};

    const WINDOW_SIZE: Size<Pixels> = size(px(1240.), px(900.));
    const APP_ID: &str = "dev.jiradesk.JiraDesk.UIAutomation";
    const WINDOW_TITLE: &str = "Jira Desk UI Automation";

    gpui_platform::application()
        .with_assets(AppAssets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            // Do not synchronize with the host system appearance: the fixture must be stable.
            Theme::change(ThemeMode::Light, None, cx);

            let display = cx.primary_display();
            let bounds = display
                .map(|display| {
                    let visible = display.visible_bounds();
                    let available = visible.size;
                    let width = WINDOW_SIZE.width.as_f32().min(available.width.as_f32());
                    let height = WINDOW_SIZE.height.as_f32().min(available.height.as_f32());
                    WindowBounds::Windowed(Bounds::centered_at(
                        visible.center(),
                        size(px(width.max(1.)), px(height.max(1.))),
                    ))
                })
                .unwrap_or_else(|| WindowBounds::centered(WINDOW_SIZE, cx));
            let window_options = WindowOptions {
                window_bounds: Some(bounds),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some(APP_ID.to_owned()),
                ..TitleBar::window_options()
            };

            cx.spawn(async move |cx| {
                cx.open_window(window_options, |window, cx| {
                    window.activate_window();
                    window.set_window_title(WINDOW_TITLE);

                    let fixture_dashboard = |mut dashboard: Dashboard| {
                        dashboard.initialize_appearance_preference(AppearancePreference::Light);
                        cx.new(|_| dashboard)
                    };
                    let dashboard = match scenario {
                        UiAutomationScenario::Onboarding => None,
                        UiAutomationScenario::Issues => Some(
                            Dashboard::from_sample_data_for_section(SampleSection::Issues),
                        ),
                        UiAutomationScenario::RichContent => {
                            Some(Dashboard::from_ui_automation_rich_content())
                        }
                        UiAutomationScenario::Updates => Some(
                            Dashboard::from_sample_data_for_section(SampleSection::Updates),
                        ),
                        UiAutomationScenario::Team => {
                            Some(Dashboard::from_sample_data_for_section(SampleSection::Team))
                        }
                        UiAutomationScenario::Settings => Some(
                            Dashboard::from_sample_data_for_section(SampleSection::Settings),
                        ),
                    }
                    .map(fixture_dashboard);
                    let shell = cx.new(|cx| {
                        AppShell::new_for_ui_lab(dashboard, ThemeMode::Light, window, cx)
                    });
                    cx.new(|cx| Root::new(shell, window, cx))
                })
                .map(|_| ())
                .map_err(|error| anyhow::anyhow!("failed to open automation host window: {error}"))
            })
            .detach();
        });
    // `application().run` returns only after the native event loop exits.
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn launch(_: UiAutomationScenario) -> Result<()> {
    bail!(
        "jira-ui-automation-host requires macOS; local accessibility automation is unavailable on this platform"
    )
}

#[cfg(test)]
mod tests {
    use super::{Command, UiAutomationScenario, parse_args};

    #[test]
    fn parser_accepts_only_the_supported_scenario_form() {
        for (name, scenario) in [
            ("onboarding", UiAutomationScenario::Onboarding),
            ("issues", UiAutomationScenario::Issues),
            ("rich-content", UiAutomationScenario::RichContent),
            ("updates", UiAutomationScenario::Updates),
            ("team", UiAutomationScenario::Team),
            ("settings", UiAutomationScenario::Settings),
        ] {
            assert_eq!(
                parse_args(["--scenario", name]).unwrap(),
                Command::Launch(scenario)
            );
            assert_eq!(scenario.as_str(), name);
        }
    }

    #[test]
    fn parser_rejects_unsafe_or_ambiguous_invocations() {
        for args in [
            vec![],
            vec!["issues"],
            vec!["--scenario"],
            vec!["--scenario", "issues", "--scenario", "team"],
            vec!["--scenario=issues"],
            vec!["--scenario", "issues", "--help"],
            vec!["--unknown"],
        ] {
            assert!(parse_args(args).is_err());
        }
    }

    #[test]
    fn parser_keeps_help_and_list_non_launching() {
        assert_eq!(parse_args(["--help"]).unwrap(), Command::Help);
        assert_eq!(parse_args(["--list"]).unwrap(), Command::List);
        assert!(parse_args(["--list", "--scenario", "issues"]).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn launch_reports_the_platform_boundary_without_starting_gpui() {
        let error = super::launch(UiAutomationScenario::Issues).unwrap_err();
        assert!(error.to_string().contains("requires macOS"));
    }
}
