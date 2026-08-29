//! Development-only, fixture-backed GPUI screenshot capture.
//!
//! The lab composes the production [`AppShell`] around inert fixture dashboards. It never enters
//! the normal startup path, and its output is published only after GPUI teardown succeeds.

use std::path::PathBuf;

pub(crate) mod publication;

#[cfg(target_os = "macos")]
use std::{fs, path::Path, sync::Arc};

use anyhow::{Context as _, Result, bail};

pub mod matrix;
pub mod visual;
#[cfg(target_os = "macos")]
use gpui::{AppContext as _, Size, VisualTestAppContext, px, size};
#[cfg(target_os = "macos")]
use gpui_component::{Root, Theme, ThemeMode};
#[cfg(target_os = "macos")]
use gpui_component_assets::Assets;

#[cfg(target_os = "macos")]
use crate::{
    app_shell::{AppShell, AppearancePreference},
    dashboard::{Dashboard, SampleSection},
};

/// The supported semantic fixture scenarios.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLabScenario {
    /// The deterministic first-run connection surface.
    Onboarding,
    /// The first-run connection surface with its production dialog open.
    OnboardingDialog,
    /// The issues list and selected issue detail surface.
    Issues,
    /// The local update ledger surface.
    Updates,
    /// The team tracker surface.
    Team,
    /// The settings surface in disconnected preview mode.
    Settings,
}

impl UiLabScenario {
    /// Returns the stable command-line spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::OnboardingDialog => "onboarding-dialog",
            Self::Issues => "issues",
            Self::Updates => "updates",
            Self::Team => "team",
            Self::Settings => "settings",
        }
    }

    /// Parses a stable command-line spelling.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "onboarding" => Ok(Self::Onboarding),
            "onboarding-dialog" => Ok(Self::OnboardingDialog),
            "issues" => Ok(Self::Issues),
            "updates" => Ok(Self::Updates),
            "team" => Ok(Self::Team),
            "settings" => Ok(Self::Settings),
            _ => bail!(
                "unknown scenario {value:?}; expected one of: onboarding, onboarding-dialog, issues, updates, team, settings"
            ),
        }
    }
}

/// The explicit theme used by a capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLabTheme {
    /// The light component theme.
    Light,
    /// The dark component theme.
    Dark,
}

impl UiLabTheme {
    /// Returns the stable command-line spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Parses a stable command-line spelling.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => bail!("unknown theme {value:?}; expected light or dark"),
        }
    }

    #[cfg(target_os = "macos")]
    fn mode(self) -> ThemeMode {
        match self {
            Self::Light => ThemeMode::Light,
            Self::Dark => ThemeMode::Dark,
        }
    }
}

/// Minimum accepted logical capture width.
pub const MIN_UI_LAB_WIDTH: u32 = 320;
/// Minimum accepted logical capture height.
pub const MIN_UI_LAB_HEIGHT: u32 = 240;
/// Maximum accepted logical capture width.
pub const MAX_UI_LAB_WIDTH: u32 = 4096;
/// Maximum accepted logical capture height.
pub const MAX_UI_LAB_HEIGHT: u32 = 2160;
/// Maximum accepted logical pixel area. This bounds renderer allocations while allowing common
/// 2560x1440 and larger desktop captures.
pub const MAX_UI_LAB_AREA: u64 = 8_500_000;

/// A logical window size for a capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiLabSize {
    /// Logical width in pixels.
    pub width: u32,
    /// Logical height in pixels.
    pub height: u32,
}

impl UiLabSize {
    fn validate_dimensions(width: u32, height: u32) -> Result<()> {
        if width < MIN_UI_LAB_WIDTH || height < MIN_UI_LAB_HEIGHT {
            bail!("dimensions must be at least {MIN_UI_LAB_WIDTH}x{MIN_UI_LAB_HEIGHT}");
        }
        if width > MAX_UI_LAB_WIDTH || height > MAX_UI_LAB_HEIGHT {
            bail!("dimensions must not exceed {MAX_UI_LAB_WIDTH}x{MAX_UI_LAB_HEIGHT}");
        }
        if u64::from(width) * u64::from(height) > MAX_UI_LAB_AREA {
            bail!("area must not exceed {MAX_UI_LAB_AREA} logical pixels");
        }
        Ok(())
    }

    fn validate(self) -> Result<()> {
        Self::validate_dimensions(self.width, self.height)
    }

    /// Parses `WIDTHxHEIGHT`, rejecting malformed, zero, undersized, and unsafe allocations.
    pub fn parse(value: &str) -> Result<Self> {
        let (width, height) = value.split_once('x').ok_or_else(|| {
            anyhow::anyhow!("invalid logical size {value:?}; expected WIDTHxHEIGHT")
        })?;
        let width = width
            .parse::<u32>()
            .with_context(|| format!("invalid logical width {width:?} in size {value:?}"))?;
        let height = height
            .parse::<u32>()
            .with_context(|| format!("invalid logical height {height:?} in size {value:?}"))?;
        Self::validate_dimensions(width, height)
            .map_err(|error| anyhow::anyhow!("invalid logical size {value:?}; {error}"))?;
        Ok(Self { width, height })
    }

    #[cfg(target_os = "macos")]
    fn gpui_size(self) -> Size<gpui::Pixels> {
        size(px(self.width as f32), px(self.height as f32))
    }
}

/// One deterministic screenshot request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiLabCapture {
    /// Fixture scenario to render.
    pub scenario: UiLabScenario,
    /// PNG output path.
    pub output: PathBuf,
    /// Logical window size.
    pub size: UiLabSize,
    /// Component theme.
    pub theme: UiLabTheme,
}

/// Capture metadata returned after a PNG is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiLabCaptureReport {
    /// Output image width in physical pixels.
    pub width: u32,
    /// Output image height in physical pixels.
    pub height: u32,
}

/// Captures one scenario directly from GPUI's offscreen renderer.
#[cfg(target_os = "macos")]
pub fn capture(request: &UiLabCapture) -> Result<UiLabCaptureReport> {
    request
        .size
        .validate()
        .context("validate capture request")?;

    let (image, report) = {
        let mut cx = VisualTestAppContext::with_asset_source(
            gpui_platform::current_platform(false),
            Arc::new(Assets),
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            Theme::change(request.theme.mode(), None, cx);
        });

        let window = cx.open_offscreen_window(request.size.gpui_size(), |window, cx| {
            window.set_window_title("Jira Desk UI lab");
            let fixture_preference = match request.theme {
                UiLabTheme::Light => AppearancePreference::Light,
                UiLabTheme::Dark => AppearancePreference::Dark,
            };
            let mut fixture_dashboard = |section| {
                let mut dashboard = Dashboard::from_sample_data_for_section(section);
                dashboard.initialize_appearance_preference(fixture_preference);
                cx.new(|_| dashboard)
            };
            let dashboard = match request.scenario {
                UiLabScenario::Onboarding | UiLabScenario::OnboardingDialog => None,
                UiLabScenario::Issues => Some(fixture_dashboard(SampleSection::Issues)),
                UiLabScenario::Updates => Some(fixture_dashboard(SampleSection::Updates)),
                UiLabScenario::Team => Some(fixture_dashboard(SampleSection::Team)),
                UiLabScenario::Settings => Some(fixture_dashboard(SampleSection::Settings)),
            };
            let shell =
                cx.new(|cx| AppShell::new_for_ui_lab(dashboard, request.theme.mode(), window, cx));
            let root = cx.new(|cx| Root::new(shell.clone(), window, cx));
            if request.scenario == UiLabScenario::OnboardingDialog {
                // The dialog layer is owned by the production Root. Defer until this root has
                // been installed as the window root, then update the AppShell entity to invoke
                // its production dialog-opening path. This keeps the lab free of coordinate or
                // OS-level automation and exercises the same dialog users see in the app.
                window.defer(cx, move |window, cx| {
                    shell.update(cx, |shell, cx| {
                        shell.open_connection_dialog_for_ui_lab(window, cx);
                    });
                });
            }
            root
        })?;

        // Fixture views do not start async work. Running the deterministic executor once still
        // flushes GPUI's initial layout and component tasks before capture.
        cx.run_until_parked();
        let image = cx
            .capture_screenshot(window.into())
            .context("capture GPUI offscreen frame")?;
        let report = UiLabCaptureReport {
            width: image.width(),
            height: image.height(),
        };
        if report.width == 0 || report.height == 0 {
            bail!("GPUI returned an empty screenshot");
        }

        // Replace the root and drain GPUI before leaving this scope; dropping the context here
        // completes leak detection before the captured image is published.
        cx.update_window(window.into(), |_, window, app| {
            window.replace_root(app, |_, _| gpui::Empty);
        })?;
        cx.run_until_parked();
        (image, report)
    };

    publish_png(&request.output, |file| {
        image
            .write_to(file, image::ImageFormat::Png)
            .map_err(anyhow::Error::from)
    })?;
    Ok(report)
}

#[cfg(target_os = "macos")]
fn publish_png(output: &Path, save: impl FnOnce(&mut fs::File) -> Result<()>) -> Result<()> {
    publication::publish_file(output, "capture", save)
}

/// Returns a clear platform error instead of making the normal Linux binary a second capture
/// implementation.
#[cfg(not(target_os = "macos"))]
pub fn capture(request: &UiLabCapture) -> Result<UiLabCaptureReport> {
    request
        .size
        .validate()
        .context("validate capture request")?;
    bail!("jira-ui-capture requires macOS; the ui-lab is not a production runtime mode")
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_UI_LAB_AREA, MAX_UI_LAB_HEIGHT, MAX_UI_LAB_WIDTH, MIN_UI_LAB_HEIGHT, MIN_UI_LAB_WIDTH,
        UiLabCapture, UiLabScenario, UiLabSize, UiLabTheme,
    };

    #[cfg(target_os = "macos")]
    use super::{publication::create_temporary_file, publish_png};
    #[cfg(target_os = "macos")]
    use std::{fs, io::Write, path::Path};

    #[test]
    fn scenario_parser_accepts_only_named_semantic_scenarios() {
        for (value, expected) in [
            ("onboarding", UiLabScenario::Onboarding),
            ("onboarding-dialog", UiLabScenario::OnboardingDialog),
            ("issues", UiLabScenario::Issues),
            ("updates", UiLabScenario::Updates),
            ("team", UiLabScenario::Team),
            ("settings", UiLabScenario::Settings),
        ] {
            assert_eq!(UiLabScenario::parse(value).unwrap(), expected);
            assert_eq!(expected.as_str(), value);
        }
        assert!(UiLabScenario::parse("serialized-state").is_err());
    }

    #[test]
    fn onboarding_dialog_is_single_capture_only() {
        assert!(
            super::matrix::built_in_matrix()
                .iter()
                .all(|case| case.scenario != UiLabScenario::OnboardingDialog)
        );
    }

    #[test]
    fn size_parser_accepts_matrix_and_common_large_captures() {
        assert_eq!(UiLabSize::parse("1370x900").unwrap().width, 1370);
        assert!(UiLabSize::parse("2560x1440").is_ok());
        assert!(UiLabSize::parse("3840x2160").is_ok());
        assert!(UiLabSize::parse(&format!("{MIN_UI_LAB_WIDTH}x{MIN_UI_LAB_HEIGHT}")).is_ok());
    }

    #[test]
    fn size_parser_rejects_malformed_and_unsafe_boundaries() {
        for value in [
            "",
            "1280",
            "1280X900",
            "0x900",
            "319x240",
            "320x239",
            "1280x",
            "x900",
            "1280x900x1",
            "4294967296x900",
        ] {
            assert!(UiLabSize::parse(value).is_err(), "accepted {value:?}");
        }
        assert!(UiLabSize::parse(&format!("{}x900", MAX_UI_LAB_WIDTH + 1)).is_err());
        assert!(UiLabSize::parse(&format!("1370x{}", MAX_UI_LAB_HEIGHT + 1)).is_err());
        assert!(UiLabSize::parse("4096x2160").is_err());
        assert!(u64::from(1370_u32) * 900 < MAX_UI_LAB_AREA);
    }

    #[test]
    fn capture_rejects_invalid_public_sizes_before_gpui() {
        for size in [
            UiLabSize {
                width: MIN_UI_LAB_WIDTH - 1,
                height: MIN_UI_LAB_HEIGHT,
            },
            UiLabSize {
                width: MIN_UI_LAB_WIDTH,
                height: MIN_UI_LAB_HEIGHT - 1,
            },
            UiLabSize {
                width: MAX_UI_LAB_WIDTH + 1,
                height: MIN_UI_LAB_HEIGHT,
            },
            UiLabSize {
                width: MIN_UI_LAB_WIDTH,
                height: MAX_UI_LAB_HEIGHT + 1,
            },
            UiLabSize {
                width: MAX_UI_LAB_WIDTH,
                height: MAX_UI_LAB_HEIGHT,
            },
        ] {
            let request = UiLabCapture {
                scenario: UiLabScenario::Onboarding,
                output: std::path::PathBuf::new(),
                size,
                theme: UiLabTheme::Light,
            };
            let error = super::capture(&request).unwrap_err();
            assert!(
                error.to_string().contains("validate capture request"),
                "capture accepted invalid public size {size:?}: {error:#}"
            );
        }
    }

    #[test]
    fn theme_parser_is_explicit() {
        assert_eq!(UiLabTheme::parse("light").unwrap(), UiLabTheme::Light);
        assert_eq!(UiLabTheme::parse("dark").unwrap(), UiLabTheme::Dark);
        assert!(UiLabTheme::parse("system").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn temporary_capture_files_are_unique_and_reserved() {
        let root = std::env::temp_dir().join(format!("jira-ui-lab-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("capture.png");
        let (first, first_file) = create_temporary_file(Path::new(&output), "capture").unwrap();
        let (second, second_file) = create_temporary_file(Path::new(&output), "capture").unwrap();
        assert_ne!(first, second);
        drop(first_file);
        drop(second_file);
        fs::remove_file(first).unwrap();
        fs::remove_file(second).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn png_publish_is_atomic_and_cleans_failed_temporary_files() {
        let root =
            std::env::temp_dir().join(format!("jira-ui-lab-publish-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("capture.png");

        publish_png(Path::new(&output), |temporary| {
            temporary
                .write_all(b"published PNG bytes")
                .map_err(anyhow::Error::from)
        })
        .unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"published PNG bytes");

        assert!(
            publish_png(Path::new(&output), |_temporary| {
                Err(anyhow::anyhow!("simulated encoder failure"))
            })
            .is_err()
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
