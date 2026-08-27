//! Command-line entry point for the development-only GPUI capture lab.

#[cfg(feature = "ui-lab")]
use std::path::PathBuf;

#[cfg(feature = "ui-lab")]
use anyhow::{Context as _, Result, bail};
#[cfg(feature = "ui-lab")]
use jira_gpui::ui_lab::{UiLabCapture, UiLabScenario, UiLabSize, UiLabTheme, capture};

#[cfg(feature = "ui-lab")]
const DEFAULT_SIZE: UiLabSize = UiLabSize {
    width: 1280,
    height: 900,
};

#[cfg(feature = "ui-lab")]
fn usage() -> &'static str {
    "Usage: cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- [OPTIONS]\n\nOptions:\n  --scenario NAME       onboarding | issues | updates | team | settings\n  --output PNG          Destination PNG path\n  --size WIDTHxHEIGHT   Logical window size (default: 1280x900)\n  --theme light|dark    Theme (default: light)\n  --list                List scenarios and themes\n  -h, --help            Show this help\n\nThis lab is fixture-backed and macOS-only. It never loads credentials, keychain\nstate, Jira, persistence, notifications, or write ports."
}

#[cfg(feature = "ui-lab")]
fn parse_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    let value = args
        .next()
        .with_context(|| format!("{flag} requires a value; use --help for usage"))?;
    if value.starts_with('-') {
        bail!("{flag} requires a value; use --help for usage");
    }
    Ok(value)
}

#[cfg(feature = "ui-lab")]
fn parse_args<I, S>(args: I) -> Result<Option<UiLabCapture>>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let mut scenario = None;
    let mut output = None;
    let mut size = DEFAULT_SIZE;
    let mut theme = UiLabTheme::Light;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--list" => {
                println!(
                    "Scenarios: onboarding, issues, updates, team, settings\nThemes: light, dark"
                );
                return Ok(None);
            }
            "--scenario" => {
                scenario = Some(UiLabScenario::parse(&parse_value(
                    &mut args,
                    "--scenario",
                )?)?);
            }
            "--output" => {
                let value = parse_value(&mut args, "--output")?;
                if value.trim().is_empty() {
                    bail!("--output cannot be empty");
                }
                output = Some(PathBuf::from(value));
            }
            "--size" => size = UiLabSize::parse(&parse_value(&mut args, "--size")?)?,
            "--theme" => theme = UiLabTheme::parse(&parse_value(&mut args, "--theme")?)?,
            _ if arg.starts_with('-') => bail!("unknown option {arg:?}; use --help for usage"),
            _ => bail!("unexpected argument {arg:?}; use --help for usage"),
        }
    }

    let scenario = scenario
        .ok_or_else(|| anyhow::anyhow!("--scenario is required; use --list to see choices"))?;
    let output =
        output.ok_or_else(|| anyhow::anyhow!("--output is required; use --help for usage"))?;
    Ok(Some(UiLabCapture {
        scenario,
        output,
        size,
        theme,
    }))
}

#[cfg(feature = "ui-lab")]
fn format_capture_report(
    request: &UiLabCapture,
    report: jira_gpui::ui_lab::UiLabCaptureReport,
) -> String {
    format!(
        "Captured {} / {}x{} logical / {} theme to {} ({}x{} physical pixels)",
        request.scenario.as_str(),
        request.size.width,
        request.size.height,
        request.theme.as_str(),
        request.output.display(),
        report.width,
        report.height,
    )
}

#[cfg(feature = "ui-lab")]
fn run() -> Result<()> {
    let Some(request) = parse_args(std::env::args().skip(1))? else {
        println!("{}", usage());
        return Ok(());
    };
    let report = capture(&request)?;
    println!("{}", format_capture_report(&request, report));
    Ok(())
}

#[cfg(feature = "ui-lab")]
fn main() {
    if let Err(error) = run() {
        eprintln!("jira-ui-capture: {error:#}");
        std::process::exit(2);
    }
}

#[cfg(not(feature = "ui-lab"))]
fn main() {
    eprintln!("jira-ui-capture is development-only; rebuild with --features ui-lab");
    std::process::exit(2);
}

#[cfg(all(test, feature = "ui-lab"))]
mod tests {
    use super::{DEFAULT_SIZE, format_capture_report, parse_args};
    use jira_gpui::ui_lab::{UiLabCaptureReport, UiLabScenario, UiLabTheme};

    #[test]
    fn parser_builds_an_explicit_capture_request() {
        let request = parse_args([
            "--scenario",
            "settings",
            "--output",
            "target/ui-lab/settings.png",
            "--size",
            "1440x900",
            "--theme",
            "dark",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(request.scenario, UiLabScenario::Settings);
        assert_eq!(request.theme, UiLabTheme::Dark);
        assert_eq!(request.size.width, 1440);
    }

    #[test]
    fn parser_uses_stable_defaults() {
        let request = parse_args(["--scenario", "issues", "--output", "issues.png"])
            .unwrap()
            .unwrap();
        assert_eq!(request.size, DEFAULT_SIZE);
        assert_eq!(request.theme, UiLabTheme::Light);
    }

    #[test]
    fn parser_table_covers_required_and_invalid_inputs() {
        let cases = [
            (vec![], "scenario is required"),
            (vec!["--scenario", "issues"], "output is required"),
            (
                vec!["--scenario", "issues", "--output"],
                "--output requires",
            ),
            (
                vec!["--scenario", "issues", "--output", ""],
                "output cannot be empty",
            ),
            (
                vec!["--scenario", "issues", "--output", "x.png", "--size"],
                "--size requires",
            ),
            (
                vec![
                    "--scenario",
                    "issues",
                    "--output",
                    "x.png",
                    "--size",
                    "0x900",
                ],
                "invalid logical size",
            ),
            (
                vec![
                    "--scenario",
                    "issues",
                    "--output",
                    "x.png",
                    "--size",
                    "4096x2160",
                ],
                "area",
            ),
            (
                vec![
                    "--scenario",
                    "issues",
                    "--output",
                    "x.png",
                    "--theme",
                    "system",
                ],
                "unknown theme",
            ),
            (
                vec!["--scenario", "nope", "--output", "x.png"],
                "unknown scenario",
            ),
            (
                vec!["--scenario", "issues", "--output", "x.png", "--wat"],
                "unknown option",
            ),
            (
                vec!["--scenario", "issues", "--output", "x.png", "extra"],
                "unexpected argument",
            ),
        ];
        for (args, message) in cases {
            let error = parse_args(args).expect_err(message);
            assert!(
                error.to_string().contains(message),
                "{error:#} lacks {message:?}"
            );
        }
    }

    #[test]
    fn report_states_selected_theme_and_logical_size_truthfully() {
        let request = parse_args([
            "--scenario",
            "issues",
            "--output",
            "issues.png",
            "--size",
            "1280x900",
            "--theme",
            "dark",
        ])
        .unwrap()
        .unwrap();
        let report = format_capture_report(
            &request,
            UiLabCaptureReport {
                width: 2560,
                height: 1800,
            },
        );
        assert_eq!(
            report,
            "Captured issues / 1280x900 logical / dark theme to issues.png (2560x1800 physical pixels)"
        );
    }
}
