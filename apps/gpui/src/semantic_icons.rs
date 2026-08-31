//! Pure semantic mappings for Jira issue types and priorities.
//!
//! Text labels remain the source of truth; callers can use the returned icons as secondary visual
//! cues. The few common Jira types with a dedicated application asset use the app-owned Lucide
//! paths, while the rest retain the pinned component catalog's generic icons.

use gpui::SharedString;
use gpui_component::{IconName, IconNamed};

use crate::app_assets::AppIconName;

/// An issue-type icon from either the app-owned or component asset catalog.
pub enum IssueTypeIcon {
    /// A Jira-specific icon owned by this application.
    App(AppIconName),
    /// A generic icon supplied by gpui-component.
    Component(IconName),
}

/// The semantic color assigned to a known Jira issue type.
///
/// This is intentionally a color role rather than a concrete color. Dashboard rendering resolves
/// the role through the active theme so the same identity cue remains readable in light and dark
/// themes. Unknown/custom types stay neutral and do not acquire an accidental meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssueTypeTone {
    /// Bug and defect issues use the theme's red base color.
    Red,
    /// Task issues use the theme's green base color.
    Green,
    /// Story issues use the theme's blue base color.
    Blue,
    /// Epic issues use the theme's bright purple base color (the component theme's magenta slot).
    Purple,
    /// Unknown and less common issue types use the neutral muted foreground.
    Neutral,
}

/// The complete semantic presentation for a Jira issue type.
pub struct IssueTypeSemantics {
    pub icon: IssueTypeIcon,
    pub tone: IssueTypeTone,
}

impl IconNamed for IssueTypeIcon {
    fn path(self) -> SharedString {
        match self {
            Self::App(icon) => icon.path(),
            Self::Component(icon) => icon.path(),
        }
    }
}

/// A semantic priority icon from either the app-owned or component asset
/// catalog. The component catalog supplies single chevrons; the app owns the
/// Jira-specific double chevrons and equal mark that are not available there.
pub enum PriorityIcon {
    /// A priority icon owned by this application.
    App(AppIconName),
    /// A generic icon supplied by gpui-component.
    Component(IconName),
}

impl IconNamed for PriorityIcon {
    fn path(self) -> SharedString {
        match self {
            Self::App(icon) => icon.path(),
            Self::Component(icon) => icon.path(),
        }
    }
}

/// A semantic priority level that the dashboard can resolve to theme colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PriorityTone {
    /// The most urgent priority; callers should use the danger tone.
    Critical,
    /// A high priority; callers should use the warning tone.
    Elevated,
    /// A normal priority; callers should use a neutral tone.
    Neutral,
    /// A lower priority; callers may use a quiet informational tone.
    Low,
    /// The least urgent priority; callers should use a muted tone.
    Minimal,
    /// The priority label was not recognized.
    Unknown,
}

/// Maps a Jira issue type label to its Lucide icon and semantic color tone.
pub fn issue_type_semantics(label: &str) -> IssueTypeSemantics {
    match normalize(label).as_str() {
        "story" => IssueTypeSemantics {
            icon: IssueTypeIcon::App(AppIconName::BookOpenText),
            tone: IssueTypeTone::Blue,
        },
        "task" | "standard task" => IssueTypeSemantics {
            icon: IssueTypeIcon::App(AppIconName::ListChecks),
            tone: IssueTypeTone::Green,
        },
        "bug" | "defect" => IssueTypeSemantics {
            icon: IssueTypeIcon::App(AppIconName::Bug),
            tone: IssueTypeTone::Red,
        },
        "initiative" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::LayoutDashboard),
            tone: IssueTypeTone::Neutral,
        },
        "sub-task" | "subtask" | "sub task" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::PanelBottom),
            tone: IssueTypeTone::Neutral,
        },
        "epic" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::Folder),
            tone: IssueTypeTone::Purple,
        },
        "spike" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::SquareTerminal),
            tone: IssueTypeTone::Neutral,
        },
        "improvement" | "new feature" | "feature" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::Plus),
            tone: IssueTypeTone::Neutral,
        },
        "incident" | "problem" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::TriangleAlert),
            tone: IssueTypeTone::Neutral,
        },
        "change" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::Settings),
            tone: IssueTypeTone::Neutral,
        },
        "service request" | "service-request" => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::Inbox),
            tone: IssueTypeTone::Neutral,
        },
        _ => IssueTypeSemantics {
            icon: IssueTypeIcon::Component(IconName::File),
            tone: IssueTypeTone::Neutral,
        },
    }
}

/// Maps a Jira priority label to an embedded icon and a theme-independent semantic tone.
pub fn priority_semantics(label: &str) -> (PriorityIcon, PriorityTone) {
    match normalize(label).as_str() {
        "highest" => (
            PriorityIcon::App(AppIconName::ChevronsUp),
            PriorityTone::Critical,
        ),
        "high" => (
            PriorityIcon::Component(IconName::ChevronUp),
            PriorityTone::Elevated,
        ),
        "medium" => (PriorityIcon::App(AppIconName::Equal), PriorityTone::Neutral),
        "low" => (
            PriorityIcon::Component(IconName::ChevronDown),
            PriorityTone::Low,
        ),
        "lowest" => (
            PriorityIcon::App(AppIconName::ChevronsDown),
            PriorityTone::Minimal,
        ),
        _ => (PriorityIcon::App(AppIconName::Equal), PriorityTone::Unknown),
    }
}

fn normalize(label: &str) -> String {
    label.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_component::IconNamed;

    fn path(icon: impl IconNamed) -> String {
        icon.path().to_string()
    }

    #[test]
    fn maps_every_supported_issue_type_case_insensitively() {
        assert_eq!(
            path(issue_type_semantics(" Story ").icon),
            "icons/book-open-text.svg"
        );
        assert_eq!(
            path(issue_type_semantics("INITIATIVE").icon),
            "icons/layout-dashboard.svg"
        );
        assert_eq!(
            path(issue_type_semantics("Task").icon),
            "icons/list-checks.svg"
        );
        assert_eq!(
            path(issue_type_semantics("sub-TASK").icon),
            "icons/panel-bottom.svg"
        );
        assert_eq!(path(issue_type_semantics("Bug").icon), "icons/bug.svg");
        assert_eq!(path(issue_type_semantics("epic").icon), "icons/folder.svg");
    }

    #[test]
    fn maps_common_jira_and_jsm_aliases() {
        for label in ["task", "standard task"] {
            assert_eq!(
                path(issue_type_semantics(label).icon),
                "icons/list-checks.svg"
            );
        }
        for label in ["subtask", "sub task", "sub-task"] {
            assert_eq!(
                path(issue_type_semantics(label).icon),
                "icons/panel-bottom.svg"
            );
        }
        for label in ["bug", "defect"] {
            assert_eq!(path(issue_type_semantics(label).icon), "icons/bug.svg");
        }
        for label in ["incident", "problem"] {
            assert_eq!(
                path(issue_type_semantics(label).icon),
                "icons/triangle-alert.svg"
            );
        }
        assert_eq!(
            path(issue_type_semantics("spike").icon),
            "icons/square-terminal.svg"
        );
        for label in ["improvement", "new feature", "feature"] {
            assert_eq!(path(issue_type_semantics(label).icon), "icons/plus.svg");
        }
        assert_eq!(
            path(issue_type_semantics("change").icon),
            "icons/settings.svg"
        );
        for label in ["service request", "service-request"] {
            assert_eq!(path(issue_type_semantics(label).icon), "icons/inbox.svg");
        }
    }

    #[test]
    fn unknown_issue_types_use_a_neutral_file_icon() {
        assert_eq!(
            path(issue_type_semantics("custom type").icon),
            "icons/file.svg"
        );
        assert_eq!(path(issue_type_semantics("").icon), "icons/file.svg");
    }

    #[test]
    fn known_issue_types_use_the_requested_color_tones() {
        assert_eq!(issue_type_semantics("Bug").tone, IssueTypeTone::Red);
        assert_eq!(
            issue_type_semantics("standard task").tone,
            IssueTypeTone::Green
        );
        assert_eq!(issue_type_semantics(" story ").tone, IssueTypeTone::Blue);
        assert_eq!(issue_type_semantics("EPIC").tone, IssueTypeTone::Purple);
    }

    #[test]
    fn unknown_and_uncolored_issue_types_remain_neutral() {
        for label in ["custom type", "Initiative", "sub-task", "Spike", ""] {
            assert_eq!(issue_type_semantics(label).tone, IssueTypeTone::Neutral);
        }
    }

    #[test]
    fn maps_every_supported_priority_to_icon_and_tone() {
        let (icon, tone) = priority_semantics(" Highest ");
        assert_eq!(path(icon), "icons/chevrons-up.svg");
        assert_eq!(tone, PriorityTone::Critical);

        let (icon, tone) = priority_semantics("HIGH");
        assert_eq!(path(icon), "icons/chevron-up.svg");
        assert_eq!(tone, PriorityTone::Elevated);

        let (icon, tone) = priority_semantics("Medium");
        assert_eq!(path(icon), "icons/equal.svg");
        assert_eq!(tone, PriorityTone::Neutral);

        let (icon, tone) = priority_semantics("low");
        assert_eq!(path(icon), "icons/chevron-down.svg");
        assert_eq!(tone, PriorityTone::Low);

        let (icon, tone) = priority_semantics("LOWEST");
        assert_eq!(path(icon), "icons/chevrons-down.svg");
        assert_eq!(tone, PriorityTone::Minimal);
    }

    #[test]
    fn unknown_priorities_use_a_neutral_fallback() {
        let (icon, tone) = priority_semantics("Not configured");
        assert_eq!(path(icon), "icons/equal.svg");
        assert_eq!(tone, PriorityTone::Unknown);
    }
}
