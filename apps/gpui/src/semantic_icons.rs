//! Pure semantic mappings for Jira issue types and priorities.
//!
//! The pinned GPUI component asset set does not contain Jira-specific artwork, so these
//! mappings deliberately use a small set of generic, embedded icons. Text labels remain the
//! source of truth; callers can use the returned icons as secondary visual cues.

use gpui_component::IconName;

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

/// Maps a Jira issue type label to an embedded, generic icon.
pub fn issue_type_icon(label: &str) -> IconName {
    match normalize(label).as_str() {
        "story" => IconName::BookOpen,
        "initiative" => IconName::LayoutDashboard,
        "task" | "standard task" => IconName::Check,
        "sub-task" | "subtask" | "sub task" => IconName::PanelBottom,
        "bug" | "defect" => IconName::TriangleAlert,
        "epic" => IconName::Folder,
        "spike" => IconName::SquareTerminal,
        "improvement" | "new feature" | "feature" => IconName::Plus,
        "incident" | "problem" => IconName::TriangleAlert,
        "change" => IconName::Settings,
        "service request" | "service-request" => IconName::Inbox,
        _ => IconName::File,
    }
}

/// Maps a Jira priority label to an embedded icon and a theme-independent semantic tone.
pub fn priority_semantics(label: &str) -> (IconName, PriorityTone) {
    match normalize(label).as_str() {
        "highest" => (IconName::ArrowUp, PriorityTone::Critical),
        "high" => (IconName::ArrowUp, PriorityTone::Elevated),
        "medium" => (IconName::Minus, PriorityTone::Neutral),
        "low" => (IconName::ArrowDown, PriorityTone::Low),
        "lowest" => (IconName::ArrowDown, PriorityTone::Minimal),
        _ => (IconName::Minus, PriorityTone::Unknown),
    }
}

fn normalize(label: &str) -> String {
    label.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_component::IconNamed;

    fn path(icon: IconName) -> String {
        icon.path().to_string()
    }

    #[test]
    fn maps_every_supported_issue_type_case_insensitively() {
        assert_eq!(path(issue_type_icon(" Story ")), "icons/book-open.svg");
        assert_eq!(
            path(issue_type_icon("INITIATIVE")),
            "icons/layout-dashboard.svg"
        );
        assert_eq!(path(issue_type_icon("Task")), "icons/check.svg");
        assert_eq!(path(issue_type_icon("sub-TASK")), "icons/panel-bottom.svg");
        assert_eq!(path(issue_type_icon("Bug")), "icons/triangle-alert.svg");
        assert_eq!(path(issue_type_icon("epic")), "icons/folder.svg");
    }

    #[test]
    fn maps_common_jira_and_jsm_aliases() {
        for label in ["task", "standard task"] {
            assert_eq!(path(issue_type_icon(label)), "icons/check.svg");
        }
        for label in ["subtask", "sub task", "sub-task"] {
            assert_eq!(path(issue_type_icon(label)), "icons/panel-bottom.svg");
        }
        for label in ["bug", "defect", "incident", "problem"] {
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
        assert_eq!(path(icon), "icons/arrow-up.svg");
        assert_eq!(tone, PriorityTone::Critical);

        let (icon, tone) = priority_semantics("HIGH");
        assert_eq!(path(icon), "icons/arrow-up.svg");
        assert_eq!(tone, PriorityTone::Elevated);

        let (icon, tone) = priority_semantics("Medium");
        assert_eq!(path(icon), "icons/minus.svg");
        assert_eq!(tone, PriorityTone::Neutral);

        let (icon, tone) = priority_semantics("low");
        assert_eq!(path(icon), "icons/arrow-down.svg");
        assert_eq!(tone, PriorityTone::Low);

        let (icon, tone) = priority_semantics("LOWEST");
        assert_eq!(path(icon), "icons/arrow-down.svg");
        assert_eq!(tone, PriorityTone::Minimal);
    }

    #[test]
    fn unknown_priorities_use_a_neutral_fallback() {
        let (icon, tone) = priority_semantics("Not configured");
        assert_eq!(path(icon), "icons/minus.svg");
        assert_eq!(tone, PriorityTone::Unknown);
    }
}
