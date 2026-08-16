use crate::models::{EnhancedSearchPage, JiraIssue, JiraNamedEntity, JiraProject, JiraUser};
use jira_domain::{
    AccountId, Issue, IssueId, IssueKey, IssueType, JiraSiteId, ParentIssue, Priority, Project,
    Status, Timestamp, User,
};
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};

/// Converts Jira transport values to portable records. These records have no HTTP, UI, or
/// persistence concerns and are a deliberately narrow adapter seam for a future Tauri UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct IssueMapper;

impl IssueMapper {
    /// Maps an enhanced-search page to the framework-independent domain model.
    ///
    /// The site ID is supplied by the authenticated connection; Jira issue search responses do
    /// not repeat it. This function performs no network traffic and cannot modify Jira.
    pub fn map_domain_page(
        &self,
        site_id: JiraSiteId,
        page: EnhancedSearchPage,
    ) -> Result<DomainIssuePage, MappingError> {
        let issues = page
            .issues
            .into_iter()
            .map(|issue| self.map_domain_issue(site_id.clone(), issue))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DomainIssuePage {
            issues,
            next_page_token: page.next_page_token,
            is_last: page.is_last,
        })
    }

    pub fn map_domain_issue(
        &self,
        site_id: JiraSiteId,
        issue: JiraIssue,
    ) -> Result<Issue, MappingError> {
        let id = IssueId::new(issue.id).map_err(MappingError::InvalidDomainValue)?;
        let key = IssueKey::new(issue.key).map_err(MappingError::InvalidDomainValue)?;
        non_empty("issue summary", &issue.fields.summary)?;
        let summary = issue.fields.summary;
        let project = required("project", issue.fields.project)?;
        let issue_type = required("issue type", issue.fields.issuetype)?;
        let status = required("status", issue.fields.status)?;
        let created_at = parse_timestamp(required("created timestamp", issue.fields.created)?)?;
        let updated_at = parse_timestamp(required("updated timestamp", issue.fields.updated)?)?;
        let due_date = issue
            .fields
            .duedate
            .map(|value| parse_date(&value))
            .transpose()?;

        let parent = issue
            .fields
            .parent
            .map(|parent| {
                Ok(ParentIssue {
                    id: IssueId::new(parent.id).map_err(MappingError::InvalidDomainValue)?,
                    key: IssueKey::new(parent.key).map_err(MappingError::InvalidDomainValue)?,
                    summary: parent.fields.and_then(|fields| fields.summary),
                })
            })
            .transpose()?;

        let assignee = issue.fields.assignee.map(domain_account_id).transpose()?;
        let reporter = issue.fields.reporter.map(domain_account_id).transpose()?;
        let priority = issue
            .fields
            .priority
            .map(|priority| Priority {
                id: priority.id,
                name: Some(priority.name),
                icon_url: priority.icon_url,
            })
            .unwrap_or(Priority {
                id: None,
                name: None,
                icon_url: None,
            });

        let mut domain_issue = Issue::new(
            site_id,
            id,
            key,
            Project {
                id: project.id,
                key: project.key,
                name: project.name,
            },
            IssueType {
                id: issue_type.id.unwrap_or_default(),
                name: issue_type.name,
                icon_url: issue_type.icon_url,
            },
            summary,
            Status {
                id: status.id.unwrap_or_default(),
                name: status.name,
                category: status.status_category.and_then(|category| category.name),
            },
            priority,
            assignee,
            reporter,
            parent,
            issue.fields.labels,
            created_at,
            updated_at,
            due_date,
        );
        domain_issue.resolution_name = issue.fields.resolution.map(|resolution| resolution.name);
        Ok(domain_issue)
    }

    pub fn map_user(&self, site_id: JiraSiteId, user: JiraUser) -> Result<User, MappingError> {
        let account_id = domain_account_id(user.clone())?;
        Ok(User::new(
            site_id,
            account_id,
            user.display_name,
            avatar_url(&user.avatar_urls),
            user.active,
        ))
    }

    pub fn map_page(&self, page: EnhancedSearchPage) -> Result<RemoteIssuePage, MappingError> {
        let issues = page
            .issues
            .into_iter()
            .map(|issue| self.map_issue(issue))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RemoteIssuePage {
            issues,
            next_page_token: page.next_page_token,
            is_last: page.is_last,
        })
    }

    pub fn map_issue(&self, issue: JiraIssue) -> Result<RemoteIssue, MappingError> {
        non_empty("issue id", &issue.id)?;
        non_empty("issue key", &issue.key)?;
        non_empty("issue summary", &issue.fields.summary)?;

        let parent = issue.fields.parent.map(|parent| RemoteIssueReference {
            id: parent.id,
            key: parent.key,
            summary: parent.fields.and_then(|fields| fields.summary),
        });

        Ok(RemoteIssue {
            id: issue.id,
            key: issue.key,
            summary: issue.fields.summary,
            issue_type: issue.fields.issuetype.map(map_named_entity),
            project: issue.fields.project.map(map_project),
            status: issue.fields.status.map(map_named_entity),
            priority: issue.fields.priority.map(map_named_entity),
            assignee: issue.fields.assignee.map(map_user),
            parent,
            labels: issue.fields.labels,
            created: issue.fields.created,
            updated: issue.fields.updated,
            due_date: issue.fields.duedate,
            resolution: issue.fields.resolution.map(map_named_entity),
        })
    }
}

fn map_named_entity(entity: JiraNamedEntity) -> RemoteNamedEntity {
    RemoteNamedEntity {
        id: entity.id,
        name: entity.name,
    }
}

fn map_project(project: JiraProject) -> RemoteProject {
    RemoteProject {
        id: project.id,
        key: project.key,
        name: project.name,
    }
}

fn map_user(user: JiraUser) -> RemoteUser {
    RemoteUser {
        account_id: user.account_id,
        display_name: user.display_name,
        active: user.active,
    }
}

fn domain_account_id(user: JiraUser) -> Result<AccountId, MappingError> {
    AccountId::new(user.account_id).map_err(MappingError::InvalidDomainValue)
}

fn avatar_url(urls: &std::collections::BTreeMap<String, String>) -> Option<String> {
    ["48x48", "32x32", "24x24", "16x16"]
        .iter()
        .find_map(|size| urls.get(*size).cloned())
        .or_else(|| urls.values().next().cloned())
}

fn required<T>(field: &'static str, value: Option<T>) -> Result<T, MappingError> {
    value.ok_or(MappingError::MissingRequiredField(field))
}

fn parse_timestamp(value: String) -> Result<Timestamp, MappingError> {
    OffsetDateTime::parse(&value, &Rfc3339)
        .or_else(|_| OffsetDateTime::parse(&value, JIRA_OFFSET_TIMESTAMP_FORMAT))
        .map_err(|_| MappingError::InvalidTimestamp(value))
}

const JIRA_OFFSET_TIMESTAMP_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond][offset_hour sign:mandatory][offset_minute]"
);

const JIRA_DATE_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");

fn parse_date(value: &str) -> Result<Date, MappingError> {
    Date::parse(value, JIRA_DATE_FORMAT).map_err(|_| MappingError::InvalidDate(value.to_owned()))
}

fn non_empty(field: &'static str, value: &str) -> Result<(), MappingError> {
    if value.trim().is_empty() {
        Err(MappingError::MissingRequiredField(field))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteIssuePage {
    pub issues: Vec<RemoteIssue>,
    pub next_page_token: Option<String>,
    pub is_last: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainIssuePage {
    pub issues: Vec<Issue>,
    pub next_page_token: Option<String>,
    pub is_last: bool,
}

/// Jira's read-only issue representation, normalized just enough to isolate Jira JSON field
/// spelling from the rest of the program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteIssue {
    pub id: String,
    pub key: String,
    pub summary: String,
    pub issue_type: Option<RemoteNamedEntity>,
    pub project: Option<RemoteProject>,
    pub status: Option<RemoteNamedEntity>,
    pub priority: Option<RemoteNamedEntity>,
    pub assignee: Option<RemoteUser>,
    pub parent: Option<RemoteIssueReference>,
    pub labels: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub due_date: Option<String>,
    pub resolution: Option<RemoteNamedEntity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteIssueReference {
    pub id: String,
    pub key: String,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteProject {
    pub id: String,
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteNamedEntity {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteUser {
    pub account_id: String,
    pub display_name: String,
    pub active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MappingError {
    #[error("Jira response is missing required {0}")]
    MissingRequiredField(&'static str),
    #[error("Jira response contains an invalid domain value: {0}")]
    InvalidDomainValue(#[source] jira_domain::DomainError),
    #[error("Jira response has an invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("Jira response has an invalid due date: {0}")]
    InvalidDate(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_an_enhanced_search_page_without_leaking_json_field_names() {
        let page: EnhancedSearchPage =
            serde_json::from_str(include_str!("../tests/fixtures/enhanced-search-page.json"))
                .unwrap();
        let mapped = IssueMapper.map_page(page).unwrap();

        assert!(mapped.is_last);
        assert_eq!(mapped.next_page_token, None);
        assert_eq!(mapped.issues.len(), 1);

        let issue = &mapped.issues[0];
        assert_eq!(issue.key, "ENG-42");
        assert_eq!(issue.summary, "Ship the Wayland dashboard");
        assert_eq!(
            issue.assignee.as_ref().unwrap().account_id,
            "557058:abc-123"
        );
        assert_eq!(issue.parent.as_ref().unwrap().key, "ENG-1");
        assert_eq!(issue.project.as_ref().unwrap().name, "Engineering");
    }

    #[test]
    fn maps_the_same_fixture_into_the_domain_model() {
        let page: EnhancedSearchPage =
            serde_json::from_str(include_str!("../tests/fixtures/enhanced-search-page.json"))
                .unwrap();
        let site_id = JiraSiteId::new("site-123").unwrap();
        let mapped = IssueMapper.map_domain_page(site_id, page).unwrap();
        let issue = &mapped.issues[0];

        assert_eq!(issue.key.as_str(), "ENG-42");
        assert_eq!(issue.status.name, "In Progress");
        assert_eq!(issue.due_date.unwrap().to_string(), "2026-08-30");
        assert_eq!(issue.parent.as_ref().unwrap().key.as_str(), "ENG-1");
    }

    #[test]
    fn rejects_a_blank_issue_key() {
        let issue: JiraIssue = serde_json::from_str(
            r#"{"id":"10001","key":" ","fields":{"summary":"A real summary"}}"#,
        )
        .unwrap();

        assert_eq!(
            IssueMapper.map_issue(issue),
            Err(MappingError::MissingRequiredField("issue key"))
        );
    }
}
