//! Pure layout policy for the GPUI adapter.
//!
//! Keeping breakpoint decisions independent from rendering makes resize
//! behavior easy to test and keeps GPUI out of the application's domain code.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LayoutMode {
    Mobile,
    Compact,
    Standard,
    Wide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IssuesPaneMode {
    ListOnly,
    DetailOnly,
    ListAndDetail,
}

/// Widths at which the shell changes its navigation treatment.
///
/// Keep these values together so a breakpoint is not duplicated in rendering
/// code.  They are logical pixels, matching GPUI's window viewport units.
#[cfg(test)]
const MOBILE_NAV_ITEM_MIN_WIDTH: f32 = 68.0;
const COMPACT_BREAKPOINT: f32 = 720.0;
const STANDARD_BREAKPOINT: f32 = 960.0;
const WIDE_BREAKPOINT: f32 = 1_200.0;

const SIDEBAR_RAIL_WIDTH: f32 = 64.0;
const STANDARD_SIDEBAR_WIDTH: f32 = 200.0;
const FULL_SIDEBAR_MIN_WIDTH: f32 = STANDARD_SIDEBAR_WIDTH;
// 200px is wide enough for the full native labels and avoids a second
// workspace contraction at the Wide breakpoint.
const WIDE_SIDEBAR_WIDTH: f32 = STANDARD_SIDEBAR_WIDTH;

/// Number of logical pixels available to each item in the four-item mobile
/// navigation bar. The result intentionally includes the bar's fixed padding
/// and gaps, making the narrowest supported geometry directly testable.
pub(crate) fn mobile_nav_item_width(viewport_width: f32) -> f32 {
    let available = (viewport_width - 8.0 - 3.0 * 4.0).max(0.0);
    available / 4.0
}

/// Selects which issue panes the dashboard should attach for the current mode.
///
pub(crate) fn issues_pane_mode(layout: LayoutMode, mobile_detail_open: bool) -> IssuesPaneMode {
    if layout.is_mobile() {
        if mobile_detail_open {
            IssuesPaneMode::DetailOnly
        } else {
            IssuesPaneMode::ListOnly
        }
    } else {
        IssuesPaneMode::ListAndDetail
    }
}

pub(crate) fn layout_for_width(width: f32) -> LayoutMode {
    if width >= WIDE_BREAKPOINT {
        LayoutMode::Wide
    } else if width >= STANDARD_BREAKPOINT {
        LayoutMode::Standard
    } else if width >= COMPACT_BREAKPOINT {
        LayoutMode::Compact
    } else {
        LayoutMode::Mobile
    }
}

impl LayoutMode {
    pub(crate) fn is_mobile(self) -> bool {
        matches!(self, Self::Mobile)
    }

    pub(crate) fn is_rail(self) -> bool {
        matches!(self, Self::Compact)
    }

    pub(crate) fn supports_manual_sidebar_collapse(self) -> bool {
        matches!(self, Self::Standard | Self::Wide)
    }

    pub(crate) fn sidebar_width(self) -> f32 {
        match self {
            Self::Wide => WIDE_SIDEBAR_WIDTH,
            Self::Standard => STANDARD_SIDEBAR_WIDTH,
            Self::Compact | Self::Mobile => SIDEBAR_RAIL_WIDTH,
        }
    }

    pub(crate) fn issue_list_width(self) -> f32 {
        match self {
            // Keep the initial list allocation stable as the shell moves from
            // rail to full sidebar. A larger list here would make the detail
            // workspace shrink at the 960/1,200px mode boundaries.
            Self::Wide | Self::Standard | Self::Compact => 350.0,
            Self::Mobile => 0.0,
        }
    }

    pub(crate) fn issue_list_range(self) -> (f32, f32) {
        match self {
            Self::Compact => (280.0, 420.0),
            Self::Standard => (320.0, 520.0),
            Self::Wide => (320.0, 640.0),
            Self::Mobile => (0.0, 0.0),
        }
    }

    pub(crate) fn detail_min_width(self) -> f32 {
        match self {
            Self::Compact => 280.0,
            Self::Standard => 320.0,
            Self::Wide => 360.0,
            Self::Mobile => 0.0,
        }
    }

    pub(crate) fn resizable_id(self) -> &'static str {
        match self {
            Self::Compact => "issues-panes-compact",
            Self::Standard => "issues-panes-standard",
            Self::Wide => "issues-panes-wide",
            Self::Mobile => "issues-panes-mobile-unused",
        }
    }

    pub(crate) fn detail_padding(self) -> f32 {
        match self {
            Self::Wide => 24.0,
            Self::Standard => 20.0,
            Self::Compact => 16.0,
            Self::Mobile => 16.0,
        }
    }

    pub(crate) fn list_padding(self) -> f32 {
        match self {
            Self::Wide | Self::Standard => 20.0,
            Self::Compact => 16.0,
            Self::Mobile => 12.0,
        }
    }

    pub(crate) fn onboarding_padding(self) -> f32 {
        if self.is_mobile() { 16.0 } else { 32.0 }
    }
}

/// Returns whether the dashboard should render its desktop sidebar as an icon rail.
/// Compact is always a rail; only Standard and Wide honor the manual preference.
pub(crate) fn effective_sidebar_is_rail(layout: LayoutMode, manually_collapsed: bool) -> bool {
    layout.is_rail() || (layout.supports_manual_sidebar_collapse() && manually_collapsed)
}

/// Returns the sidebar space reserved by the dashboard for this layout and preference.
pub(crate) fn effective_sidebar_width(layout: LayoutMode, manually_collapsed: bool) -> f32 {
    if layout.is_mobile() {
        0.0
    } else if effective_sidebar_is_rail(layout, manually_collapsed) {
        LayoutMode::Compact.sidebar_width()
    } else {
        layout.sidebar_width()
    }
}

/// Returns the shell's sidebar allocation for a viewport.
///
/// Expanding the sidebar exactly at a breakpoint causes a one-pixel resize to
/// remove over a hundred pixels from the workspace. The policy therefore
/// interpolates the sidebar from the rail width over the extra space needed by
/// the wider shell. The workspace is continuous and non-decreasing at every
/// breakpoint, while the sidebar still reaches its intended native width.
pub(crate) fn sidebar_width_for_viewport(
    layout: LayoutMode,
    manually_collapsed: bool,
    viewport_width: f32,
) -> f32 {
    if layout.is_mobile() || manually_collapsed {
        return effective_sidebar_width(layout, manually_collapsed);
    }

    match layout {
        LayoutMode::Mobile => 0.0,
        LayoutMode::Compact => SIDEBAR_RAIL_WIDTH,
        LayoutMode::Standard => {
            let expansion = STANDARD_SIDEBAR_WIDTH - SIDEBAR_RAIL_WIDTH;
            SIDEBAR_RAIL_WIDTH + (viewport_width - STANDARD_BREAKPOINT).clamp(0.0, expansion)
        }
        LayoutMode::Wide => WIDE_SIDEBAR_WIDTH,
    }
}

/// Whether the shell should use its icon rail treatment at this width.
///
/// Standard and Wide expand progressively, so their content must remain in
/// the compact rail until the corresponding full sidebar width is available.
pub(crate) fn sidebar_is_rail_for_viewport(
    layout: LayoutMode,
    manually_collapsed: bool,
    viewport_width: f32,
) -> bool {
    effective_sidebar_is_rail(layout, manually_collapsed)
        || sidebar_width_for_viewport(layout, manually_collapsed, viewport_width)
            < match layout {
                LayoutMode::Mobile | LayoutMode::Compact => layout.sidebar_width(),
                LayoutMode::Standard | LayoutMode::Wide => FULL_SIDEBAR_MIN_WIDTH,
            }
}

/// Workspace width after the shell sidebar, before a section's own padding.
#[cfg(test)]
pub(crate) fn workspace_width_for_viewport(
    layout: LayoutMode,
    manually_collapsed: bool,
    viewport_width: f32,
) -> f32 {
    (viewport_width - sidebar_width_for_viewport(layout, manually_collapsed, viewport_width))
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use super::{
        IssuesPaneMode, LayoutMode, MOBILE_NAV_ITEM_MIN_WIDTH, effective_sidebar_is_rail,
        effective_sidebar_width, issues_pane_mode, layout_for_width, mobile_nav_item_width,
        sidebar_is_rail_for_viewport, sidebar_width_for_viewport, workspace_width_for_viewport,
    };

    #[test]
    fn issue_panes_keep_desktop_detail_visible() {
        for layout in [LayoutMode::Compact, LayoutMode::Standard, LayoutMode::Wide] {
            assert_eq!(
                issues_pane_mode(layout, false),
                IssuesPaneMode::ListAndDetail
            );
            assert_eq!(
                issues_pane_mode(layout, true),
                IssuesPaneMode::ListAndDetail
            );
        }
    }

    #[test]
    fn mobile_issue_panes_switch_between_list_and_detail() {
        assert_eq!(
            issues_pane_mode(LayoutMode::Mobile, false),
            IssuesPaneMode::ListOnly
        );
        assert_eq!(
            issues_pane_mode(LayoutMode::Mobile, true),
            IssuesPaneMode::DetailOnly
        );
    }

    #[test]
    fn desktop_resizable_defaults_are_within_bounded_ranges() {
        let layouts = [LayoutMode::Compact, LayoutMode::Standard, LayoutMode::Wide];
        for layout in layouts {
            let (min, max) = layout.issue_list_range();
            assert!(min <= layout.issue_list_width());
            assert!(layout.issue_list_width() <= max);
            assert!(layout.detail_min_width() <= max);
            assert!(!layout.resizable_id().is_empty());
        }
    }

    #[test]
    fn breakpoints_are_inclusive_at_the_wider_mode() {
        assert_eq!(layout_for_width(719.0), LayoutMode::Mobile);
        assert_eq!(layout_for_width(720.0), LayoutMode::Compact);
        assert_eq!(layout_for_width(959.0), LayoutMode::Compact);
        assert_eq!(layout_for_width(960.0), LayoutMode::Standard);
        assert_eq!(layout_for_width(1_199.0), LayoutMode::Standard);
        assert_eq!(layout_for_width(1_200.0), LayoutMode::Wide);
    }

    #[test]
    fn policy_progresses_monotonically_as_width_grows() {
        let widths = [0.0, 719.0, 720.0, 959.0, 960.0, 1_199.0, 1_200.0, 2_000.0];
        let modes = widths.map(layout_for_width);
        assert!(modes.windows(2).all(|pair| pair[0] as u8 <= pair[1] as u8));
    }

    #[test]
    fn widths_never_grow_when_the_viewport_gets_narrower() {
        let widths = [
            LayoutMode::Mobile,
            LayoutMode::Compact,
            LayoutMode::Standard,
            LayoutMode::Wide,
        ];
        for pair in widths.windows(2) {
            assert!(pair[0].sidebar_width() <= pair[1].sidebar_width());
            assert!(pair[0].issue_list_width() <= pair[1].issue_list_width());
        }
    }

    #[test]
    fn effective_sidebar_policy_respects_automatic_and_manual_rails() {
        assert!(!effective_sidebar_is_rail(LayoutMode::Mobile, false));
        assert!(!effective_sidebar_is_rail(LayoutMode::Mobile, true));
        assert!(effective_sidebar_is_rail(LayoutMode::Compact, false));
        assert!(effective_sidebar_is_rail(LayoutMode::Compact, true));
        assert!(!effective_sidebar_is_rail(LayoutMode::Standard, false));
        assert!(effective_sidebar_is_rail(LayoutMode::Standard, true));
        assert!(!effective_sidebar_is_rail(LayoutMode::Wide, false));
        assert!(effective_sidebar_is_rail(LayoutMode::Wide, true));
    }

    #[test]
    fn effective_sidebar_width_releases_desktop_space_without_mobile_sidebar() {
        assert_eq!(effective_sidebar_width(LayoutMode::Mobile, false), 0.0);
        assert_eq!(effective_sidebar_width(LayoutMode::Mobile, true), 0.0);
        assert_eq!(effective_sidebar_width(LayoutMode::Compact, false), 64.0);
        assert_eq!(effective_sidebar_width(LayoutMode::Compact, true), 64.0);
        assert_eq!(effective_sidebar_width(LayoutMode::Standard, false), 200.0);
        assert_eq!(effective_sidebar_width(LayoutMode::Standard, true), 64.0);
        assert_eq!(effective_sidebar_width(LayoutMode::Wide, false), 200.0);
        assert_eq!(effective_sidebar_width(LayoutMode::Wide, true), 64.0);
    }

    #[test]
    fn mobile_navigation_fits_the_supported_minimum_without_scrolling() {
        assert_eq!(mobile_nav_item_width(320.0), 75.0);
        assert!(mobile_nav_item_width(320.0) >= MOBILE_NAV_ITEM_MIN_WIDTH);
    }

    #[test]
    fn sidebar_expansion_is_continuous_at_mode_boundaries() {
        for (before, at, layout) in [
            (959.0, 960.0, LayoutMode::Standard),
            (1_199.0, 1_200.0, LayoutMode::Wide),
        ] {
            assert!(
                workspace_width_for_viewport(layout_for_width(before), false, before,)
                    <= workspace_width_for_viewport(layout, false, at)
            );
            assert_eq!(
                sidebar_width_for_viewport(layout, false, at),
                sidebar_width_for_viewport(layout, false, before)
            );
        }
    }

    #[test]
    fn sidebar_treatment_matches_progressive_width() {
        assert!(sidebar_is_rail_for_viewport(
            LayoutMode::Standard,
            false,
            960.0
        ));
        assert!(sidebar_is_rail_for_viewport(
            LayoutMode::Standard,
            false,
            1_095.0
        ));
        assert!(!sidebar_is_rail_for_viewport(
            LayoutMode::Standard,
            false,
            1_096.0
        ));
        assert!(!sidebar_is_rail_for_viewport(
            LayoutMode::Wide,
            false,
            1_200.0
        ));
    }

    #[test]
    fn workspace_width_never_decreases_as_viewport_grows() {
        // Mobile intentionally changes from a top navigation bar to a desktop
        // shell at 720px; the invariant applies within the desktop policy.
        let mut previous = workspace_width_for_viewport(layout_for_width(720.0), false, 720.0);
        for width in (721..=2_000).map(|width| width as f32) {
            let layout = layout_for_width(width);
            let current = workspace_width_for_viewport(layout, false, width);
            assert!(
                current + 0.001 >= previous,
                "width={width}, current={current}, previous={previous}"
            );
            previous = current;
        }
    }
}
