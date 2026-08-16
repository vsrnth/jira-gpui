use serde::{Deserialize, Serialize};
use time::Date;

use crate::{AccountId, IssueId, IssueKey, JiraSiteId, Timestamp};

/// A Jira project attached to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub key: String,
    pub name: String,
}

/// A configurable Jira issue type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueType {
    pub id: String,
    pub name: String,
    pub icon_url: Option<String>,
}

/// A workflow status as it is currently named in Jira.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
}

/// A Jira priority. Both fields are optional in the upstream API for some sites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Priority {
    pub id: Option<String>,
    pub name: Option<String>,
    pub icon_url: Option<String>,
}

/// A generic parent link. The app does not assume that a parent is an Epic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentIssue {
    pub id: IssueId,
    pub key: IssueKey,
    pub summary: Option<String>,
}

/// Whether a cached issue is still part of the latest synchronized result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IssueLifecycle {
    #[default]
    Present,
    RemovedFromView,
}

/// A read-only, normalized Jira issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub site_id: JiraSiteId,
    pub id: IssueId,
    pub key: IssueKey,
    pub project: Project,
    pub issue_type: IssueType,
    pub summary: String,
    pub status: Status,
    pub priority: Priority,
    pub assignee: Option<AccountId>,
    pub reporter: Option<AccountId>,
    pub parent: Option<ParentIssue>,
    pub labels: Vec<String>,
    /// The raw ADF document is kept at the transport/cache edge. The domain
    /// stores only an optional read-only textual representation for searching.
    pub description_text: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub due_date: Option<Date>,
    pub resolution_name: Option<String>,
    pub lifecycle: IssueLifecycle,
}

impl Issue {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        site_id: JiraSiteId,
        id: IssueId,
        key: IssueKey,
        project: Project,
        issue_type: IssueType,
        summary: impl Into<String>,
        status: Status,
        priority: Priority,
        assignee: Option<AccountId>,
        reporter: Option<AccountId>,
        parent: Option<ParentIssue>,
        labels: Vec<String>,
        created_at: Timestamp,
        updated_at: Timestamp,
        due_date: Option<Date>,
    ) -> Self {
        Self {
            site_id,
            id,
            key,
            project,
            issue_type,
            summary: summary.into(),
            status,
            priority,
            assignee,
            reporter,
            parent,
            labels,
            description_text: None,
            created_at,
            updated_at,
            due_date,
            resolution_name: None,
            lifecycle: IssueLifecycle::Present,
        }
    }

    pub fn is_assigned_to(&self, account_id: &AccountId) -> bool {
        self.assignee.as_ref() == Some(account_id)
    }
}

/// Fields whose changes are useful to surface in the local update feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueField {
    Summary,
    Status,
    Assignee,
    Priority,
    DueDate,
    Parent,
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn issue() -> Issue {
        Issue::new(
            JiraSiteId::new("site").unwrap(),
            IssueId::new("10001").unwrap(),
            IssueKey::new("APP-1").unwrap(),
            Project {
                id: "10".into(),
                key: "APP".into(),
                name: "App".into(),
            },
            IssueType {
                id: "1".into(),
                name: "Task".into(),
                icon_url: None,
            },
            "Write the application",
            Status {
                id: "1".into(),
                name: "To do".into(),
                category: None,
            },
            Priority {
                id: None,
                name: None,
                icon_url: None,
            },
            Some(AccountId::new("person").unwrap()),
            None,
            None,
            vec![],
            datetime!(2026-01-01 00:00 UTC),
            datetime!(2026-01-01 00:00 UTC),
            None,
        )
    }

    #[test]
    fn issue_assignment_is_based_on_stable_account_id() {
        assert!(issue().is_assigned_to(&AccountId::new("person").unwrap()));
        assert!(!issue().is_assigned_to(&AccountId::new("other-person").unwrap()));
    }
}
