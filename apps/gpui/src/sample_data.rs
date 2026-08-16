//! A deterministic desktop preview until Jira credentials are configured.
//!
//! This is deliberately isolated from the view. Replacing it with application
//! services does not change the dashboard or its presentation model.

use jira_domain::{
    AccountId, ChangeValue, EventId, Issue, IssueId, IssueKey, IssueType, JiraSiteId, ParentIssue,
    Priority, Project, Status, UpdateEvent, UpdateKind, User, UserSetId,
};
use time::macros::{date, datetime};

pub fn sample_users() -> Vec<User> {
    vec![
        user("amina", "Amina Yusuf"),
        user("devon", "Devon Park"),
        user("marco", "Marco Silva"),
    ]
}

pub fn sample_issues() -> Vec<Issue> {
    vec![
        issue(IssueSpec {
            id: "10184",
            key: "DESK-184",
            issue_type: "Story",
            summary: "Surface Jira update notifications in the desktop feed",
            status: "In Progress",
            status_category: "In progress",
            priority: "High",
            assignee: "amina",
            labels: &["notifications", "phase-1"],
            updated_at: datetime!(2026-08-16 10:42 UTC),
            due_date: Some(date!(2026 - 08 - 22)),
            description: "Poll Jira incrementally and present meaningful status, assignee, priority, and due-date changes without modifying remote issues.",
            parent: Some(("DESK-150", "Read-only Jira desktop MVP")),
        }),
        issue(IssueSpec {
            id: "10179",
            key: "DESK-179",
            issue_type: "Task",
            summary: "Package the Wayland build as an AppImage",
            status: "To Do",
            status_category: "To do",
            priority: "Highest",
            assignee: "devon",
            labels: &["linux", "release"],
            updated_at: datetime!(2026-08-16 09:18 UTC),
            due_date: Some(date!(2026 - 08 - 25)),
            description: "Produce a repeatable AppImage build with the runtime libraries required by GPUI's Wayland backend.",
            parent: Some(("DESK-150", "Read-only Jira desktop MVP")),
        }),
        issue(IssueSpec {
            id: "10176",
            key: "DESK-176",
            issue_type: "Bug",
            summary: "Reconcile issues removed from a saved user set",
            status: "In Review",
            status_category: "In progress",
            priority: "Medium",
            assignee: "marco",
            labels: &["sync", "cache"],
            updated_at: datetime!(2026-08-15 18:04 UTC),
            due_date: None,
            description: "A full reconciliation should mark missing issues as removed from the current view while retaining their update history.",
            parent: Some(("DESK-150", "Read-only Jira desktop MVP")),
        }),
        issue(IssueSpec {
            id: "10171",
            key: "DESK-171",
            issue_type: "Epic",
            summary: "Read-only Jira desktop MVP",
            status: "In Progress",
            status_category: "In progress",
            priority: "High",
            assignee: "amina",
            labels: &["epic", "phase-1"],
            updated_at: datetime!(2026-08-15 14:31 UTC),
            due_date: Some(date!(2026 - 09 - 05)),
            description: "Deliver a fast Wayland-native Jira overview for an individual or a saved set of users.",
            parent: None,
        }),
        issue(IssueSpec {
            id: "10163",
            key: "DESK-163",
            issue_type: "Task",
            summary: "Model pagination cursors from enhanced search",
            status: "Done",
            status_category: "Done",
            priority: "Medium",
            assignee: "devon",
            labels: &["jira-api"],
            updated_at: datetime!(2026-08-14 16:22 UTC),
            due_date: None,
            description: "Keep Jira transport pagination details inside the adapter and expose stable application page cursors.",
            parent: Some(("DESK-150", "Read-only Jira desktop MVP")),
        }),
    ]
}

pub fn sample_updates() -> Vec<UpdateEvent> {
    vec![
        update(
            "update-1",
            "10184",
            "DESK-184",
            UpdateKind::StatusChanged {
                old: ChangeValue::Text("To Do".to_owned()),
                new: ChangeValue::Text("In Progress".to_owned()),
            },
            datetime!(2026-08-16 10:42 UTC),
        ),
        update(
            "update-2",
            "10179",
            "DESK-179",
            UpdateKind::PriorityChanged {
                old: ChangeValue::Text("High".to_owned()),
                new: ChangeValue::Text("Highest".to_owned()),
            },
            datetime!(2026-08-16 09:18 UTC),
        ),
        update(
            "update-3",
            "10176",
            "DESK-176",
            UpdateKind::CommentAdded {
                comment_id: "comment-88".to_owned(),
                author: Some(
                    AccountId::new("amina").expect("sample comment author ID must be valid"),
                ),
                excerpt: "The reconciliation test now covers pagination.".to_owned(),
            },
            datetime!(2026-08-15 18:04 UTC),
        ),
    ]
}

fn user(account_id: &str, display_name: &str) -> User {
    User::new(
        site_id(),
        AccountId::new(account_id).expect("sample account ID must be valid"),
        display_name,
        None,
        true,
    )
}

struct IssueSpec<'a> {
    id: &'a str,
    key: &'a str,
    issue_type: &'a str,
    summary: &'a str,
    status: &'a str,
    status_category: &'a str,
    priority: &'a str,
    assignee: &'a str,
    labels: &'a [&'a str],
    updated_at: jira_domain::Timestamp,
    due_date: Option<time::Date>,
    description: &'a str,
    parent: Option<(&'a str, &'a str)>,
}

fn issue(spec: IssueSpec<'_>) -> Issue {
    let mut issue = Issue::new(
        site_id(),
        IssueId::new(spec.id).expect("sample issue ID must be valid"),
        IssueKey::new(spec.key).expect("sample issue key must be valid"),
        Project {
            id: "10000".to_owned(),
            key: "DESK".to_owned(),
            name: "Developer Experience".to_owned(),
        },
        IssueType {
            id: spec.issue_type.to_lowercase(),
            name: spec.issue_type.to_owned(),
            icon_url: None,
        },
        spec.summary,
        Status {
            id: spec.status.to_lowercase().replace(' ', "-"),
            name: spec.status.to_owned(),
            category: Some(spec.status_category.to_owned()),
        },
        Priority {
            id: None,
            name: Some(spec.priority.to_owned()),
            icon_url: None,
        },
        Some(AccountId::new(spec.assignee).expect("sample assignee ID must be valid")),
        Some(AccountId::new("marco").expect("sample reporter ID must be valid")),
        spec.parent.map(|(key, summary)| ParentIssue {
            id: IssueId::new(format!("parent-{key}"))
                .expect("sample parent issue ID must be valid"),
            key: IssueKey::new(key).expect("sample parent issue key must be valid"),
            summary: Some(summary.to_owned()),
        }),
        spec.labels.iter().map(ToString::to_string).collect(),
        datetime!(2026-07-28 08:00 UTC),
        spec.updated_at,
        spec.due_date,
    );
    issue.description_text = Some(spec.description.to_owned());
    issue
}

fn update(
    event_id: &str,
    issue_id: &str,
    issue_key: &str,
    kind: UpdateKind,
    occurred_at: jira_domain::Timestamp,
) -> UpdateEvent {
    UpdateEvent::new(
        EventId::new(event_id).expect("sample event ID must be valid"),
        site_id(),
        IssueId::new(issue_id).expect("sample update issue ID must be valid"),
        IssueKey::new(issue_key).expect("sample update issue key must be valid"),
        kind,
        occurred_at,
        vec![UserSetId::new("team-platform").expect("sample user set ID must be valid")],
    )
}

fn site_id() -> JiraSiteId {
    JiraSiteId::new("sample-site").expect("sample Jira site ID must be valid")
}
