//! Startup configuration for the native shell.
//!
//! The environment-backed API-token flow is intentionally an internal
//! development bootstrap. The returned session owns the constructed client
//! and local cache; the token is consumed while building the client and is
//! never kept as a separate application setting.

use std::{env, fmt, sync::Arc};

use jira_domain::{AccountId, JiraSiteId};
use jira_http::{ApiTokenCredentials, ConfigError, JiraBaseUrl, JiraHttpClient};
use jira_storage::SqliteStore;

use crate::local_data;

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
    pub(crate) cache: Arc<SqliteStore>,
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
    StorageUnavailable,
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
            Self::StorageUnavailable => "local Jira Desk storage is unavailable",
        };
        formatter.write_str(message)
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

/// Builds a live session from values entered in the native configuration UI.
///
/// The site ID used by the local cache is derived from the validated Atlassian
/// hostname. This keeps the UI independent of Jira's internal cloud ID while
/// preserving a stable cache partition for each Jira site.
pub fn live_session_from_manual_configuration(
    base_url: String,
    email: String,
    api_token: String,
    assignees: String,
) -> Result<LiveSession, StartupError> {
    live_session_from_manual_configuration_with_store(base_url, email, api_token, assignees, || {
        local_data::open_store().map_err(|_| StartupError::StorageUnavailable)
    })
}

fn live_session_from_manual_configuration_with_store<F>(
    base_url: String,
    email: String,
    api_token: String,
    assignees: String,
    store_factory: F,
) -> Result<LiveSession, StartupError>
where
    F: FnOnce() -> Result<Arc<SqliteStore>, StartupError>,
{
    let parsed_url = JiraBaseUrl::parse(&base_url).map_err(|_| StartupError::InvalidBaseUrl)?;
    let host = parsed_url
        .as_url()
        .host_str()
        .ok_or(StartupError::InvalidBaseUrl)?;
    let site_id = JiraSiteId::new(host.to_owned()).map_err(|_| StartupError::InvalidBaseUrl)?;

    build_live_session(
        site_id,
        base_url,
        email,
        api_token,
        assignees,
        store_factory,
    )
}

fn startup_from_values(values: EnvironmentValues) -> StartupSelection {
    startup_from_values_with_store(values, || {
        local_data::open_store().map_err(|_| StartupError::StorageUnavailable)
    })
}

fn startup_from_values_with_store<F>(
    values: EnvironmentValues,
    store_factory: F,
) -> StartupSelection
where
    F: FnOnce() -> Result<Arc<SqliteStore>, StartupError>,
{
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
    match build_live_session(
        site_id,
        values.base_url.expect("presence checked"),
        values.email.expect("presence checked"),
        values.api_token.expect("presence checked"),
        values.assignees.expect("presence checked"),
        store_factory,
    ) {
        Ok(session) => StartupSelection::Live(session),
        Err(error) => StartupSelection::ConfigurationError(error),
    }
}

fn build_live_session<F>(
    site_id: JiraSiteId,
    base_url: String,
    email: String,
    api_token: String,
    assignees: String,
    store_factory: F,
) -> Result<LiveSession, StartupError>
where
    F: FnOnce() -> Result<Arc<SqliteStore>, StartupError>,
{
    let assignees = parse_assignees(&assignees)?;

    // Consume the credential strings directly into the client. No startup
    // state retains the token after this function returns.
    let credentials =
        ApiTokenCredentials::new(email, api_token).map_err(|_| StartupError::InvalidCredentials)?;
    let site_label = base_url.clone();
    let jira = match JiraHttpClient::new(site_id.clone(), base_url, credentials) {
        Ok(jira) => Arc::new(jira),
        Err(ConfigError::InvalidBaseUrl) => return Err(StartupError::InvalidBaseUrl),
        Err(_) => return Err(StartupError::ClientUnavailable),
    };
    let cache = store_factory()?;

    Ok(LiveSession {
        site_id,
        site_label,
        assignees,
        jira,
        cache,
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_lite::future::block_on;
    use jira_application::IssueCachePort;
    use jira_storage::SqliteStore;

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
        let selection = startup_from_values_with_store(complete(), || {
            SqliteStore::in_memory()
                .map(Arc::new)
                .map_err(|_| StartupError::StorageUnavailable)
        });
        assert!(matches!(selection, StartupSelection::Live(_)));
    }

    fn in_memory_startup(values: EnvironmentValues) -> StartupSelection {
        startup_from_values_with_store(values, || {
            SqliteStore::in_memory()
                .map(Arc::new)
                .map_err(|_| StartupError::StorageUnavailable)
        })
    }

    fn in_memory_manual(
        base_url: &str,
        email: &str,
        api_token: &str,
        assignees: &str,
    ) -> Result<LiveSession, StartupError> {
        live_session_from_manual_configuration_with_store(
            base_url.to_owned(),
            email.to_owned(),
            api_token.to_owned(),
            assignees.to_owned(),
            || {
                SqliteStore::in_memory()
                    .map(Arc::new)
                    .map_err(|_| StartupError::StorageUnavailable)
            },
        )
    }

    #[test]
    fn manual_configuration_builds_live_session() {
        let session = in_memory_manual(
            "https://example.atlassian.net",
            "developer@example.com",
            "token-that-must-not-escape",
            "account-a, account-b",
        )
        .expect("manual configuration should build");

        assert_eq!(session.site_id.as_str(), "example.atlassian.net");
        assert_eq!(session.site_label, "https://example.atlassian.net");
        assert_eq!(session.assignees.len(), 2);
    }

    #[test]
    fn manual_configuration_derives_site_id_from_url_host() {
        let session = in_memory_manual(
            "https://my-company.atlassian.net/",
            "developer@example.com",
            "token",
            "account-a",
        )
        .expect("manual configuration should build");

        assert_eq!(session.site_id.as_str(), "my-company.atlassian.net");
    }

    #[test]
    fn manual_configuration_rejects_invalid_url_credentials_and_assignees() {
        assert!(matches!(
            in_memory_manual(
                "http://example.atlassian.net",
                "developer@example.com",
                "token",
                "account-a",
            ),
            Err(StartupError::InvalidBaseUrl)
        ));
        assert!(matches!(
            in_memory_manual(
                "https://example.atlassian.net",
                "developer@example.com",
                "",
                "account-a",
            ),
            Err(StartupError::InvalidCredentials)
        ));
        assert!(matches!(
            in_memory_manual(
                "https://example.atlassian.net",
                "developer@example.com",
                "token",
                "account-a,",
            ),
            Err(StartupError::InvalidAssignees)
        ));
    }

    #[test]
    fn manual_configuration_does_not_open_store_for_invalid_values() {
        let called = Arc::new(AtomicBool::new(false));
        let store_called = called.clone();
        let result = live_session_from_manual_configuration_with_store(
            "https://example.atlassian.net".to_owned(),
            "developer@example.com".to_owned(),
            "".to_owned(),
            "account-a".to_owned(),
            move || {
                store_called.store(true, Ordering::SeqCst);
                Err(StartupError::StorageUnavailable)
            },
        );

        assert!(matches!(result, Err(StartupError::InvalidCredentials)));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn partial_configuration_is_rejected_without_values() {
        let mut values = complete();
        values.api_token = None;
        let selection = in_memory_startup(values);
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
            in_memory_startup(empty),
            StartupSelection::ConfigurationError(StartupError::InvalidAssignees)
        ));

        let mut duplicate = complete();
        duplicate.assignees = Some("account-a,account-a".to_owned());
        assert!(matches!(
            in_memory_startup(duplicate),
            StartupSelection::ConfigurationError(StartupError::DuplicateAssignees)
        ));
    }

    #[test]
    fn startup_errors_are_redacted() {
        let mut values = complete();
        values.site_id = None;
        let selection = in_memory_startup(values);
        let StartupSelection::ConfigurationError(error) = selection else {
            panic!("expected configuration error");
        };
        let message = error.to_string();
        assert!(!message.contains("token-that-must-not-escape"));
        assert!(!message.contains("developer@example.com"));
    }

    #[test]
    fn storage_factory_is_not_called_for_preview_or_invalid_configuration() {
        let called = Arc::new(AtomicBool::new(false));
        let preview_called = called.clone();
        assert!(matches!(
            startup_from_values_with_store(EnvironmentValues::default(), move || {
                preview_called.store(true, Ordering::SeqCst);
                Err(StartupError::StorageUnavailable)
            }),
            StartupSelection::Preview
        ));
        assert!(!called.load(Ordering::SeqCst));

        let called = Arc::new(AtomicBool::new(false));
        let invalid_called = called.clone();
        let mut values = complete();
        values.site_id = None;
        assert!(matches!(
            startup_from_values_with_store(values, move || {
                invalid_called.store(true, Ordering::SeqCst);
                Err(StartupError::StorageUnavailable)
            }),
            StartupSelection::ConfigurationError(StartupError::Incomplete)
        ));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn configured_live_session_contains_usable_cache() {
        let StartupSelection::Live(session) = in_memory_startup(complete()) else {
            panic!("expected live startup");
        };
        let site_id = JiraSiteId::new("cloud-site").expect("valid site");
        let user_set_id = jira_domain::UserSetId::new("missing").expect("valid set");
        assert!(
            block_on(session.cache.sync_state(&site_id, &user_set_id))
                .expect("cache query")
                .is_none()
        );
    }

    #[test]
    fn storage_unavailable_is_safe_and_redacted() {
        let selection =
            startup_from_values_with_store(complete(), || Err(StartupError::StorageUnavailable));
        let StartupSelection::ConfigurationError(error) = selection else {
            panic!("expected storage configuration error");
        };
        assert_eq!(error, StartupError::StorageUnavailable);
        let message = error.to_string();
        assert_eq!(message, "local Jira Desk storage is unavailable");
        assert!(!message.contains("token-that-must-not-escape"));
        assert!(!message.contains("developer@example.com"));
        assert!(!message.contains("/secret/jira-desk.sqlite3"));
    }
}
