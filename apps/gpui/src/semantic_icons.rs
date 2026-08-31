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

/// Maps a Jira issue type label to an embedded icon.
pub fn issue_type_icon(label: &str) -> IssueTypeIcon {
    match normalize(label).as_str() {
        "story" => IssueTypeIcon::App(AppIconName::BookOpenText),
        "task" | "standard task" => IssueTypeIcon::App(AppIconName::ListChecks),
        "bug" | "defect" => IssueTypeIcon::App(AppIconName::Bug),
        "initiative" => IssueTypeIcon::Component(IconName::LayoutDashboard),
        "sub-task" | "subtask" | "sub task" => IssueTypeIcon::Component(IconName::PanelBottom),
        "epic" => IssueTypeIcon::Component(IconName::Folder),
        "spike" => IssueTypeIcon::Component(IconName::SquareTerminal),
        "improvement" | "new feature" | "feature" => IssueTypeIcon::Component(IconName::Plus),
        "incident" | "problem" => IssueTypeIcon::Component(IconName::TriangleAlert),
        "change" => IssueTypeIcon::Component(IconName::Settings),
        "service request" | "service-request" => IssueTypeIcon::Component(IconName::Inbox),
        _ => IssueTypeIcon::Component(IconName::File),
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
        assert_eq!(path(issue_type_icon(" Story ")), "icons/book-open-text.svg");
        assert_eq!(
            path(issue_type_icon("INITIATIVE")),
            "icons/layout-dashboard.svg"
        );
        assert_eq!(path(issue_type_icon("Task")), "icons/list-checks.svg");
        assert_eq!(path(issue_type_icon("sub-TASK")), "icons/panel-bottom.svg");
        assert_eq!(path(issue_type_icon("Bug")), "icons/bug.svg");
        assert_eq!(path(issue_type_icon("epic")), "icons/folder.svg");
    }

    #[test]
    fn maps_common_jira_and_jsm_aliases() {
        for label in ["task", "standard task"] {
            assert_eq!(path(issue_type_icon(label)), "icons/list-checks.svg");
        }
        for label in ["subtask", "sub task", "sub-task"] {
            assert_eq!(path(issue_type_icon(label)), "icons/panel-bottom.svg");
        }
        for label in ["bug", "defect"] {
            assert_eq!(path(issue_type_icon(label)), "icons/bug.svg");
        }
        for label in ["incident", "problem"] {
            assert_eq!(path(issue_type_icon(label)), "icons/triangle-alert.svg");
        }
        assert_eq!(path(issue_type_icon("spike")), "icons/square-terminal.svg");
        for label in ["improvement", "new feature", "feature"] {
            assert_eq!(path(issue_type_icon(label)), "icons/plus.svg");
        }
        assert_eq!(path(issue_type_icon("change")), "icons/settings.svg");
        for label in ["service request", "service-request"] {
            assert_eq!(path(issue_type_icon(label)), "icons/inbox.svg");
        }
    }

    #[test]
    fn unknown_issue_types_use_a_neutral_file_icon() {
        assert_eq!(path(issue_type_icon("custom type")), "icons/file.svg");
        assert_eq!(path(issue_type_icon("")), "icons/file.svg");
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
