use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Request body for Jira Cloud's `POST /rest/api/3/search/jql` endpoint.
///
/// It intentionally exposes only read-only search parameters. Jira's API accepts a few more
/// optional knobs, which can be added when a product use case requires them.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancedSearchRequest {
    pub jql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u16>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fields: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub expand: Vec<String>,
}

impl EnhancedSearchRequest {
    pub fn assigned_issues(jql: String, next_page_token: Option<String>) -> Self {
        Self {
            jql,
            next_page_token,
            max_results: Some(100),
            fields: crate::ASSIGNED_ISSUE_FIELDS
                .iter()
                .map(ToString::to_string)
                .collect(),
            expand: Vec::new(),
        }
    }
}

/// A page returned by Jira Cloud's enhanced issue-search endpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnhancedSearchPage {
    #[serde(default)]
    pub issues: Vec<JiraIssue>,
    #[serde(default)]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub is_last: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct JiraIssue {
    pub id: String,
    pub key: String,
    pub fields: JiraIssueFields,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssueFields {
    pub summary: String,
    #[serde(default)]
    pub issuetype: Option<JiraNamedEntity>,
    #[serde(default)]
    pub project: Option<JiraProject>,
    #[serde(default)]
    pub status: Option<JiraNamedEntity>,
    #[serde(default)]
    pub priority: Option<JiraNamedEntity>,
    #[serde(default)]
    pub assignee: Option<JiraUser>,
    #[serde(default)]
    pub reporter: Option<JiraUser>,
    #[serde(default)]
    pub parent: Option<JiraParentIssue>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub duedate: Option<String>,
    /// Preserved as ADF/JSON for a later read-only issue-detail mapper.
    #[serde(default)]
    pub description: Option<Value>,
    #[serde(default)]
    pub attachment: Vec<JiraAttachment>,
    #[serde(default)]
    pub resolution: Option<JiraNamedEntity>,
}

/// Metadata returned in an issue's `attachment` field. Content URLs are intentionally not
/// represented: this client never follows or downloads attachment bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraAttachment {
    #[serde(deserialize_with = "deserialize_string_or_number")]
    pub id: String,
    pub filename: String,
    pub size: u64,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraCommentPage {
    #[serde(default)]
    pub start_at: usize,
    #[serde(default)]
    pub max_results: usize,
    #[serde(default)]
    pub total: Option<usize>,
    #[serde(default, alias = "values")]
    pub comments: Vec<JiraComment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraComment {
    #[serde(deserialize_with = "deserialize_string_or_number")]
    pub id: String,
    #[serde(default)]
    pub author: Option<JiraUser>,
    #[serde(default)]
    pub body: Option<Value>,
    pub created: String,
    #[serde(default)]
    pub updated: Option<String>,
    /// Jira visibility is deliberately not carried into the domain until a policy is defined.
    #[serde(default)]
    pub visibility: Option<Value>,
}

fn deserialize_string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(u64),
    }

    match StringOrNumber::deserialize(deserializer)? {
        StringOrNumber::String(value) => Ok(value),
        StringOrNumber::Number(value) => Ok(value.to_string()),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraParentIssue {
    pub id: String,
    pub key: String,
    #[serde(default)]
    pub fields: Option<JiraParentFields>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraParentFields {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub issuetype: Option<JiraNamedEntity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraNamedEntity {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub status_category: Option<JiraStatusCategory>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct JiraStatusCategory {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JiraUser {
    pub account_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub avatar_urls: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::JiraUser;

    #[test]
    fn deserializes_the_documented_top_level_jira_user_array() {
        let users: Vec<JiraUser> = serde_json::from_str(
            r#"[
                {
                    "accountId": "557058:abc-123",
                    "displayName": "Ada Lovelace",
                    "active": true,
                    "avatarUrls": {
                        "48x48": "https://example.test/avatar.png"
                    }
                }
            ]"#,
        )
        .unwrap();

        assert_eq!(users.len(), 1);
        assert_eq!(users[0].account_id, "557058:abc-123");
        assert_eq!(users[0].display_name, "Ada Lovelace");
        assert!(users[0].active);
        assert_eq!(
            users[0].avatar_urls.get("48x48").map(String::as_str),
            Some("https://example.test/avatar.png")
        );
    }
}
