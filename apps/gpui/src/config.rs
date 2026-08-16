//! Startup configuration for the native shell.
//!
//! The environment-backed API-token flow is intentionally an internal
//! development bootstrap. The returned session owns only the constructed
//! client; the token is consumed while building that client and is never kept
//! as a separate application setting.

use std::{env, fmt, sync::Arc};

use jira_application::{IssuePullService, JiraReadPort};
use jira_domain::{AccountId, JiraSiteId};
use jira_http::{ApiTokenCredentials, ConfigError, JiraHttpClient};

const ENV_BASE_URL: &str = "JIRA_BASE_URL";
const ENV_SITE_ID: &str = "JIRA_SITE_ID";
const ENV_EMAIL: &str = "JIRA_EMAIL";
const ENV_API_TOKEN: &str = "JIRA_API_TOKEN";
const ENV_ASSIGNEES: &str = "JIRA_ASSIGNEE_ACCOUNT_IDS";

/// The startup mode selected by the shell without exposing credentials.
pub enum StartupSelection {
    Preview,
    Live(LiveSession),
    ConfigurationError(StartupError),
}

/// A configured live Jira session. Secrets are held by the HTTP client only.
pub struct LiveSession {
    pub(crate) site_id: JiraSiteId,
    pub(crate) site_label: String,
    pub(crate) assignees: Vec<AccountId>,
    pub(crate) jira: Arc<JiraHttpClient>,
}

/// Safe, stable configuration errors suitable for display in the UI/logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupError {
    Incomplete,
    InvalidSiteId,
    InvalidBaseUrl,
    InvalidCredentials,
    InvalidAssignees,
    DuplicateAssignees,
    ClientUnavailable,
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Incomplete => "Jira configuration is incomplete; set all five Jira variables",
            Self::InvalidSiteId => "Jira configuration has an invalid site ID",
            Self::InvalidBaseUrl => "Jira configuration has an invalid HTTPS Atlassian URL",
            Self::InvalidCredentials => "Jira configuration has invalid credentials",
            Self::InvalidAssignees => "Jira configuration has invalid assignee account IDs",
            Self::DuplicateAssignees => "Jira configuration repeats an assignee account ID",
            Self::ClientUnavailable => "the Jira client could not be initialized",
        };
        formatter.write_str(message)
    }
}

impl LiveSession {
    pub(crate) fn pull_service(&self) -> Arc<IssuePullService> {
        let jira: Arc<dyn JiraReadPort> = self.jira.clone();
        Arc::new(IssuePullService::new(jira, Default::default()))
    }
}

#[derive(Default)]
struct EnvironmentValues {
    base_url: Option<String>,
    site_id: Option<String>,
    email: Option<String>,
    api_token: Option<String>,
    assignees: Option<String>,
}

impl EnvironmentValues {
    fn from_process() -> Self {
        Self {
            base_url: env::var(ENV_BASE_URL).ok(),
            site_id: env::var(ENV_SITE_ID).ok(),
            email: env::var(ENV_EMAIL).ok(),
            api_token: env::var(ENV_API_TOKEN).ok(),
            assignees: env::var(ENV_ASSIGNEES).ok(),
        }
    }
}

pub fn startup_from_environment() -> StartupSelection {
    startup_from_values(EnvironmentValues::from_process())
}

fn startup_from_values(values: EnvironmentValues) -> StartupSelection {
    let configured = [
        values.base_url.is_some(),
        values.site_id.is_some(),
        values.email.is_some(),
        values.api_token.is_some(),
        values.assignees.is_some(),
    ];
    let present = configured.iter().filter(|value| **value).count();
    if present == 0 {
        return StartupSelection::Preview;
    }
    if present != configured.len() {
        return StartupSelection::ConfigurationError(StartupError::Incomplete);
    }

    let site_id = match JiraSiteId::new(values.site_id.expect("presence checked")) {
        Ok(site_id) => site_id,
        Err(_) => return StartupSelection::ConfigurationError(StartupError::InvalidSiteId),
    };
    let assignees = match parse_assignees(&values.assignees.expect("presence checked")) {
        Ok(assignees) => assignees,
        Err(error) => return StartupSelection::ConfigurationError(error),
    };

    // Consume the credential strings directly into the client. No startup
    // state retains the token after this function returns.
    let credentials = match ApiTokenCredentials::new(
        values.email.expect("presence checked"),
        values.api_token.expect("presence checked"),
    ) {
        Ok(credentials) => credentials,
        Err(_) => return StartupSelection::ConfigurationError(StartupError::InvalidCredentials),
    };
    let base_url = values.base_url.expect("presence checked");
    let site_label = base_url.clone();
    let jira = match JiraHttpClient::new(site_id.clone(), base_url, credentials) {
        Ok(jira) => Arc::new(jira),
        Err(ConfigError::InvalidBaseUrl) => {
            return StartupSelection::ConfigurationError(StartupError::InvalidBaseUrl);
        }
        Err(_) => return StartupSelection::ConfigurationError(StartupError::ClientUnavailable),
    };

    StartupSelection::Live(LiveSession {
        site_id,
        site_label,
        assignees,
        jira,
    })
}

fn parse_assignees(value: &str) -> Result<Vec<AccountId>, StartupError> {
    if value.trim().is_empty() {
        return Err(StartupError::InvalidAssignees);
    }
    let mut assignees = Vec::new();
    for raw in value.split(',') {
        let account_id =
            AccountId::new(raw.trim().to_owned()).map_err(|_| StartupError::InvalidAssignees)?;
        if assignees.contains(&account_id) {
            return Err(StartupError::DuplicateAssignees);
        }
        assignees.push(account_id);
    }
    Ok(assignees)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> EnvironmentValues {
        EnvironmentValues {
            base_url: Some("https://example.atlassian.net".to_owned()),
            site_id: Some("cloud-site".to_owned()),
            email: Some("developer@example.com".to_owned()),
            api_token: Some("token-that-must-not-escape".to_owned()),
            assignees: Some("account-a, account-b".to_owned()),
        }
    }

    #[test]
    fn absent_configuration_selects_preview() {
        assert!(matches!(
            startup_from_values(EnvironmentValues::default()),
            StartupSelection::Preview
        ));
    }

    #[test]
    fn complete_configuration_builds_live_session() {
        let selection = startup_from_values(complete());
        assert!(matches!(selection, StartupSelection::Live(_)));
    }

    #[test]
    fn partial_configuration_is_rejected_without_values() {
        let mut values = complete();
        values.api_token = None;
        let selection = startup_from_values(values);
        assert!(matches!(
            selection,
            StartupSelection::ConfigurationError(StartupError::Incomplete)
        ));
    }

    #[test]
    fn empty_and_duplicate_assignees_are_rejected() {
        let mut empty = complete();
        empty.assignees = Some("account-a, ,account-b".to_owned());
        assert!(matches!(
            startup_from_values(empty),
            StartupSelection::ConfigurationError(StartupError::InvalidAssignees)
        ));

        let mut duplicate = complete();
        duplicate.assignees = Some("account-a,account-a".to_owned());
        assert!(matches!(
            startup_from_values(duplicate),
            StartupSelection::ConfigurationError(StartupError::DuplicateAssignees)
        ));
    }

    #[test]
    fn startup_errors_are_redacted() {
        let mut values = complete();
        values.site_id = None;
        let selection = startup_from_values(values);
        let StartupSelection::ConfigurationError(error) = selection else {
            panic!("expected configuration error");
        };
        let message = error.to_string();
        assert!(!message.contains("token-that-must-not-escape"));
        assert!(!message.contains("developer@example.com"));
    }
}
