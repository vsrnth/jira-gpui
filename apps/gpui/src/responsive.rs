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

pub(crate) fn layout_for_width(width: f32) -> LayoutMode {
    if width >= 1_200.0 {
        LayoutMode::Wide
    } else if width >= 960.0 {
        LayoutMode::Standard
    } else if width >= 720.0 {
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

    pub(crate) fn sidebar_width(self) -> f32 {
        match self {
            Self::Wide => 236.0,
            Self::Standard => 200.0,
            Self::Compact | Self::Mobile => 64.0,
        }
    }

    pub(crate) fn issue_list_width(self) -> f32 {
        match self {
            Self::Wide => 494.0,
            Self::Standard => 414.0,
            Self::Compact => 350.0,
            Self::Mobile => 0.0,
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

#[cfg(test)]
mod tests {
    use super::{LayoutMode, layout_for_width};

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
}
