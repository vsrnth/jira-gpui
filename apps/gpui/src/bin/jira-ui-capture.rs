//! Command-line entry point for the development-only GPUI capture lab.

#[cfg(feature = "ui-lab")]
use std::path::PathBuf;

#[cfg(feature = "ui-lab")]
use anyhow::{Context as _, Result, bail};
#[cfg(feature = "ui-lab")]
use jira_gpui::ui_lab::{UiLabCapture, UiLabScenario, UiLabSize, UiLabTheme, capture};
#[cfg(feature = "ui-lab")]
use jira_gpui::ui_lab::{matrix, visual::CompareOptions};

#[cfg(feature = "ui-lab")]
const DEFAULT_SIZE: UiLabSize = UiLabSize {
    width: 1280,
    height: 900,
};

#[cfg(feature = "ui-lab")]
const SCENARIO_LIST: &str = "onboarding, onboarding-dialog, issues, updates, team, settings";

#[cfg(feature = "ui-lab")]
fn usage() -> &'static str {
    "Usage: cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- [MODE] [OPTIONS]\n\nSingle capture (backward compatible):\n  --scenario NAME       onboarding | onboarding-dialog | issues | updates | team | settings\n  --output PNG          Destination PNG path\n  --size WIDTHxHEIGHT   Logical window size (default: 1280x900)\n  --theme light|dark    Theme (default: light)\n\nMatrix and review modes (mutually exclusive):\n  --matrix --output-dir DIR\n                        Capture the five built-in cases and matrix-manifest.json\n  --compare --actual-dir DIR --baseline-dir DIR --diff-dir DIR --report FILE\n                        Compare every known case (strict 0 threshold and 0 percent by default)\n  --accept-baselines --actual-dir DIR --baseline-dir DIR --confirm-reviewed\n                        Explicitly publish a complete candidate matrix; never automatic\n  --pixel-threshold N   Ignore channel deltas up to N (compare only, 0..255)\n  --max-diff-percent P  Allow at most P percent changed pixels (compare only, 0..100)\n\nOther:\n  --list                List scenarios and themes\n  -h, --help            Show this help\n\nThis lab is fixture-backed and macOS-only. It never loads credentials, keychain\nstate, Jira, persistence, notifications, or write ports."
}

#[cfg(feature = "ui-lab")]
fn parse_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    let value = args
        .next()
        .with_context(|| format!("{flag} requires a value; use --help for usage"))?;
    if value.starts_with('-') {
        bail!("{flag} requires a value; use --help for usage");
    }
    if value.trim().is_empty() {
        bail!("{flag} cannot be empty");
    }
    Ok(value)
}

#[cfg(feature = "ui-lab")]
#[derive(Debug)]
enum Command {
    Capture(UiLabCapture),
    Matrix {
        output_dir: PathBuf,
    },
    Compare(CompareOptions),
    AcceptBaselines {
        actual_dir: PathBuf,
        baseline_dir: PathBuf,
    },
}

#[cfg(feature = "ui-lab")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Matrix,
    Compare,
    AcceptBaselines,
}

#[cfg(feature = "ui-lab")]
fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Matrix => "matrix",
        Mode::Compare => "compare",
        Mode::AcceptBaselines => "accept-baselines",
    }
}

#[cfg(feature = "ui-lab")]
fn select_mode(mode: &mut Option<Mode>, next: Mode) -> Result<()> {
    if let Some(previous) = *mode {
        bail!(
            "--{} conflicts with the already selected --{} mode; choose one mode",
            mode_name(next),
            mode_name(previous)
        );
    }
    *mode = Some(next);
    Ok(())
}

#[cfg(feature = "ui-lab")]
fn parse_args<I, S>(args: I) -> Result<Option<Command>>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let mut mode = None;
    let mut scenario = None;
    let mut output = None;
    let mut size = DEFAULT_SIZE;
    let mut size_set = false;
    let mut theme = UiLabTheme::Light;
    let mut theme_set = false;
    let mut output_dir = None;
    let mut actual_dir = None;
    let mut baseline_dir = None;
    let mut diff_dir = None;
    let mut report = None;
    let mut pixel_threshold = 0_u8;
    let mut pixel_threshold_set = false;
    let mut max_diff_percent = 0.0_f64;
    let mut max_diff_percent_set = false;
    let mut confirm_reviewed = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--list" => {
                if mode.is_some()
                    || scenario.is_some()
                    || output.is_some()
                    || size_set
                    || theme_set
                    || output_dir.is_some()
                    || actual_dir.is_some()
                    || baseline_dir.is_some()
                    || diff_dir.is_some()
                    || report.is_some()
                    || pixel_threshold_set
                    || max_diff_percent_set
                    || confirm_reviewed
                {
                    bail!("--list cannot be combined with a mode or other options");
                }
                println!("Scenarios: {SCENARIO_LIST}\nThemes: light, dark");
                return Ok(None);
            }
            "--matrix" => select_mode(&mut mode, Mode::Matrix)?,
            "--compare" => select_mode(&mut mode, Mode::Compare)?,
            "--accept-baselines" => select_mode(&mut mode, Mode::AcceptBaselines)?,
            "--scenario" => {
                scenario = Some(UiLabScenario::parse(&parse_value(
                    &mut args,
                    "--scenario",
                )?)?)
            }
            "--output" => output = Some(PathBuf::from(parse_value(&mut args, "--output")?)),
            "--size" => {
                size_set = true;
                size = UiLabSize::parse(&parse_value(&mut args, "--size")?)?;
            }
            "--theme" => {
                theme_set = true;
                theme = UiLabTheme::parse(&parse_value(&mut args, "--theme")?)?;
            }
            "--output-dir" => {
                output_dir = Some(PathBuf::from(parse_value(&mut args, "--output-dir")?))
            }
            "--actual-dir" => {
                actual_dir = Some(PathBuf::from(parse_value(&mut args, "--actual-dir")?))
            }
            "--baseline-dir" => {
                baseline_dir = Some(PathBuf::from(parse_value(&mut args, "--baseline-dir")?))
            }
            "--diff-dir" => diff_dir = Some(PathBuf::from(parse_value(&mut args, "--diff-dir")?)),
            "--report" => report = Some(PathBuf::from(parse_value(&mut args, "--report")?)),
            "--pixel-threshold" => {
                pixel_threshold_set = true;
                pixel_threshold = parse_value(&mut args, "--pixel-threshold")?
                    .parse::<u8>()
                    .context("--pixel-threshold must be an integer from 0 to 255")?;
            }
            "--max-diff-percent" => {
                max_diff_percent_set = true;
                max_diff_percent = parse_value(&mut args, "--max-diff-percent")?
                    .parse::<f64>()
                    .context("--max-diff-percent must be a number from 0 to 100")?;
                if !max_diff_percent.is_finite() || !(0.0..=100.0).contains(&max_diff_percent) {
                    bail!("--max-diff-percent must be a finite number from 0 to 100");
                }
            }
            "--confirm-reviewed" => confirm_reviewed = true,
            _ if arg.starts_with('-') => bail!("unknown option {arg:?}; use --help for usage"),
            _ => bail!("unexpected argument {arg:?}; use --help for usage"),
        }
    }

    match mode {
        None => {
            for (flag, present) in [
                ("--output-dir", output_dir.is_some()),
                ("--actual-dir", actual_dir.is_some()),
                ("--baseline-dir", baseline_dir.is_some()),
                ("--diff-dir", diff_dir.is_some()),
                ("--report", report.is_some()),
                ("--confirm-reviewed", confirm_reviewed),
            ] {
                if present {
                    bail!("{flag} requires a matrix mode; use --help for usage");
                }
            }
            if pixel_threshold_set {
                bail!("--pixel-threshold requires --compare mode; use --help for usage");
            }
            if max_diff_percent_set {
                bail!("--max-diff-percent requires --compare mode; use --help for usage");
            }
            let scenario = scenario.ok_or_else(|| {
                anyhow::anyhow!("--scenario is required; use --list to see choices")
            })?;
            let output = output
                .ok_or_else(|| anyhow::anyhow!("--output is required; use --help for usage"))?;
            Ok(Some(Command::Capture(UiLabCapture {
                scenario,
                output,
                size,
                theme,
            })))
        }
        Some(Mode::Matrix) => {
            if scenario.is_some()
                || output.is_some()
                || size_set
                || theme_set
                || actual_dir.is_some()
                || baseline_dir.is_some()
                || diff_dir.is_some()
                || report.is_some()
                || confirm_reviewed
                || pixel_threshold_set
                || max_diff_percent_set
            {
                bail!("matrix mode accepts only --matrix and --output-dir; use --help for usage");
            }
            Ok(Some(Command::Matrix {
                output_dir: output_dir
                    .ok_or_else(|| anyhow::anyhow!("--output-dir is required with --matrix"))?,
            }))
        }
        Some(Mode::Compare) => {
            if scenario.is_some()
                || output.is_some()
                || size_set
                || theme_set
                || output_dir.is_some()
                || confirm_reviewed
            {
                bail!(
                    "capture and acceptance flags are not valid with --compare; use --help for usage"
                );
            }
            Ok(Some(Command::Compare(CompareOptions {
                actual_dir: actual_dir
                    .ok_or_else(|| anyhow::anyhow!("--actual-dir is required with --compare"))?,
                baseline_dir: baseline_dir
                    .ok_or_else(|| anyhow::anyhow!("--baseline-dir is required with --compare"))?,
                diff_dir: diff_dir
                    .ok_or_else(|| anyhow::anyhow!("--diff-dir is required with --compare"))?,
                report: report
                    .ok_or_else(|| anyhow::anyhow!("--report is required with --compare"))?,
                pixel_threshold,
                max_diff_percent,
            })))
        }
        Some(Mode::AcceptBaselines) => {
            if scenario.is_some()
                || output.is_some()
                || size_set
                || theme_set
                || output_dir.is_some()
                || diff_dir.is_some()
                || report.is_some()
                || pixel_threshold_set
                || max_diff_percent_set
            {
                bail!(
                    "acceptance mode accepts only --actual-dir, --baseline-dir, and --confirm-reviewed; use --help for usage"
                );
            }
            if !confirm_reviewed {
                bail!("--confirm-reviewed is required with --accept-baselines");
            }
            Ok(Some(Command::AcceptBaselines {
                actual_dir: actual_dir.ok_or_else(|| {
                    anyhow::anyhow!("--actual-dir is required with --accept-baselines")
                })?,
                baseline_dir: baseline_dir.ok_or_else(|| {
                    anyhow::anyhow!("--baseline-dir is required with --accept-baselines")
                })?,
            }))
        }
    }
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
    let Some(command) = parse_args(std::env::args().skip(1))? else {
        println!("{}", usage());
        return Ok(());
    };
    match command {
        Command::Capture(request) => {
            println!("{}", format_capture_report(&request, capture(&request)?))
        }
        Command::Matrix { output_dir } => {
            matrix::capture_matrix(&output_dir)?;
            println!("Captured built-in matrix to {}", output_dir.display());
        }
        Command::Compare(options) => {
            let outcome = jira_gpui::ui_lab::visual::compare_matrix(&options)?;
            println!("Wrote comparison report to {}", options.report.display());
            if outcome.has_failures {
                bail!("one or more matrix cases failed comparison");
            }
        }
        Command::AcceptBaselines {
            actual_dir,
            baseline_dir,
        } => {
            matrix::accept_baselines(&actual_dir, &baseline_dir, true)?;
            println!(
                "Published reviewed matrix baselines to {}",
                baseline_dir.display()
            );
        }
    }
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
    use super::{Command, DEFAULT_SIZE, format_capture_report, parse_args};
    use jira_gpui::ui_lab::{UiLabCaptureReport, UiLabScenario, UiLabTheme};

    #[test]
    fn parser_builds_an_explicit_capture_request() {
        let Some(Command::Capture(request)) = parse_args([
            "--scenario",
            "settings",
            "--output",
            "target/ui-lab/settings.png",
            "--size",
            "1440x900",
            "--theme",
            "dark",
        ])
        .expect("explicit capture parser input is valid") else {
            panic!("expected capture");
        };
        assert_eq!(request.scenario, UiLabScenario::Settings);
        assert_eq!(request.theme, UiLabTheme::Dark);
        assert_eq!(request.size.width, 1440);
    }

    #[test]
    fn parser_uses_stable_defaults() {
        let Some(Command::Capture(request)) =
            parse_args(["--scenario", "issues", "--output", "issues.png"]).unwrap()
        else {
            panic!("expected capture");
        };
        assert_eq!(request.size, DEFAULT_SIZE);
        assert_eq!(request.theme, UiLabTheme::Light);
    }

    #[test]
    fn parser_accepts_the_production_onboarding_dialog_scenario() {
        let Some(Command::Capture(request)) = parse_args([
            "--scenario",
            "onboarding-dialog",
            "--output",
            "onboarding-dialog.png",
        ])
        .expect("onboarding-dialog parser input is valid") else {
            panic!("expected capture");
        };
        assert_eq!(request.scenario, UiLabScenario::OnboardingDialog);
    }

    #[test]
    fn help_lists_the_dialog_scenario() {
        assert!(super::usage().contains("onboarding | onboarding-dialog | issues"));
    }

    #[test]
    fn list_includes_the_dialog_scenario() {
        assert_eq!(
            super::SCENARIO_LIST,
            "onboarding, onboarding-dialog, issues, updates, team, settings"
        );
    }

    #[test]
    fn parser_rejects_conflicts_and_mode_inapplicable_flags() {
        let cases = [
            (vec!["--matrix", "--compare"], "conflicts"),
            (
                vec!["--matrix", "--output", "x.png"],
                "only --matrix and --output-dir",
            ),
            (
                vec!["--matrix", "--output-dir", "candidate", "--size", "960x700"],
                "only --matrix and --output-dir",
            ),
            (
                vec!["--compare", "--actual-dir", "a"],
                "--baseline-dir is required",
            ),
            (
                vec![
                    "--compare",
                    "--actual-dir",
                    "a",
                    "--baseline-dir",
                    "b",
                    "--diff-dir",
                    "d",
                    "--report",
                    "r",
                    "--theme",
                    "dark",
                ],
                "capture and acceptance flags",
            ),
            (
                vec![
                    "--accept-baselines",
                    "--actual-dir",
                    "a",
                    "--baseline-dir",
                    "b",
                ],
                "--confirm-reviewed is required",
            ),
            (
                vec![
                    "--scenario",
                    "issues",
                    "--output",
                    "x.png",
                    "--diff-dir",
                    "d",
                ],
                "requires a matrix mode",
            ),
            (
                vec![
                    "--scenario",
                    "issues",
                    "--output",
                    "x.png",
                    "--pixel-threshold",
                    "0",
                ],
                "requires --compare mode",
            ),
        ];
        for (args, message) in cases {
            let error = parse_args(args).expect_err(message);
            assert!(
                error.to_string().contains(message),
                "{error:#} lacks {message:?}"
            );
        }
        assert!(matches!(
            parse_args(["--matrix", "--output-dir", "candidate"]).unwrap(),
            Some(Command::Matrix { .. })
        ));
        assert!(matches!(
            parse_args([
                "--accept-baselines",
                "--actual-dir",
                "a",
                "--baseline-dir",
                "b",
                "--confirm-reviewed"
            ])
            .unwrap(),
            Some(Command::AcceptBaselines { .. })
        ));
    }

    #[test]
    fn report_states_selected_theme_and_logical_size_truthfully() {
        let Some(Command::Capture(request)) = parse_args([
            "--scenario",
            "issues",
            "--output",
            "issues.png",
            "--size",
            "1280x900",
            "--theme",
            "dark",
        ])
        .unwrap() else {
            panic!("expected capture");
        };
        assert_eq!(
            format_capture_report(
                &request,
                UiLabCaptureReport {
                    width: 2560,
                    height: 1800
                }
            ),
            "Captured issues / 1280x900 logical / dark theme to issues.png (2560x1800 physical pixels)"
        );
    }
}
