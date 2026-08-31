//! Application-owned assets layered over the component icon catalog.
//!
//! Jira-specific icons live in this crate so their visual vocabulary can be
//! versioned with the application. Every other asset continues to come from
//! `gpui-component-assets`, which keeps component internals working normally.

use std::borrow::Cow;

use gpui::{App, AssetSource, IntoElement, RenderOnce, Result, SharedString, Window};
use gpui_component::{Icon, IconNamed};
use gpui_component_assets::Assets;

gpui_component::icon_named!(AppIconName, "assets/icons", [Debug, Copy, PartialEq, Eq]);

impl RenderOnce for AppIconName {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Icon::new(self)
    }
}

/// The composite asset source used by every native Jira Desk entry point.
pub struct AppAssets;

const APP_ICON_PATHS: [&str; 7] = [
    "icons/bug.svg",
    "icons/book-open-text.svg",
    "icons/list-checks.svg",
    "icons/refresh-cw.svg",
    "icons/chevrons-up.svg",
    "icons/equal.svg",
    "icons/chevrons-down.svg",
];

impl AppAssets {
    fn app_icon(path: &str) -> Option<Cow<'static, [u8]>> {
        match path {
            "icons/bug.svg" => Some(Cow::Borrowed(include_bytes!("../assets/icons/bug.svg"))),
            "icons/book-open-text.svg" => Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/book-open-text.svg"
            ))),
            "icons/list-checks.svg" => Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/list-checks.svg"
            ))),
            "icons/refresh-cw.svg" => Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/refresh-cw.svg"
            ))),
            "icons/chevrons-up.svg" => Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/chevrons-up.svg"
            ))),
            "icons/equal.svg" => Some(Cow::Borrowed(include_bytes!("../assets/icons/equal.svg"))),
            "icons/chevrons-down.svg" => Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/chevrons-down.svg"
            ))),
            _ => None,
        }
    }

    fn contains_path(path: &str, candidate: &str) -> bool {
        if path.is_empty() {
            return true;
        }
        let path = path.trim_end_matches('/');
        candidate == path || candidate.starts_with(&format!("{path}/"))
    }
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Self::app_icon(path)
            .map(Some)
            .map_or_else(|| Assets.load(path), Ok)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        for candidate in APP_ICON_PATHS {
            if Self::contains_path(path, candidate)
                && !paths.iter().any(|existing| existing == candidate)
            {
                paths.push(candidate.into());
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_app_icons_and_delegates_component_icons() {
        for path in APP_ICON_PATHS {
            let asset = AppAssets.load(path).expect("app icon should load");
            assert!(asset.is_some(), "missing app icon {path}");
            assert!(asset.is_some_and(|bytes| bytes.starts_with(b"<svg")));
        }

        let component_icon = AppAssets
            .load("icons/file.svg")
            .expect("component icon should load");
        assert!(component_icon.is_some());
    }

    #[test]
    fn refresh_icon_uses_the_app_owned_lucide_path() {
        assert_eq!(AppIconName::RefreshCw.path(), "icons/refresh-cw.svg");
        let asset = AppAssets
            .load("icons/refresh-cw.svg")
            .expect("refresh icon should load")
            .expect("refresh icon should be app-owned");
        assert!(
            asset
                .as_ref()
                .windows(b"M21 12a9".len())
                .any(|window| window == b"M21 12a9")
        );
    }

    #[test]
    fn lists_app_icons_alongside_component_icons() {
        let paths = AppAssets.list("icons").expect("icon list should load");
        for path in APP_ICON_PATHS {
            assert!(paths.iter().any(|listed| listed == path));
        }
    }
}
