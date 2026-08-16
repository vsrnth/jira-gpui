//! Startup configuration for the native shell.
//!
//! The environment-backed API-token flow is intentionally an internal
//! development bootstrap. The returned session owns the constructed client
//! and local cache; the token is consumed while building the client and is
//! never kept as a separate application setting.

use std::{env, fmt, sync::Arc};

use jira_application::{CancellationToken, ErrorKind, JiraReadPort};
use jira_domain::{AccountId, JiraSiteId};
use jira_http::{ApiTokenCredentials, ConfigError, JiraBaseUrl, JiraHttpClient};
use jira_storage::SqliteStore;

use crate::local_data;

const ENV_BASE_URL: &str = "JIRA_BASE_URL";
const ENV_SITE_ID: &str = "JIRA_SITE_ID";
const ENV_EMAIL: &str = "JIRA_EMAIL";
const ENV_API_TOKEN: &str = "JIRA_API_TOKEN";

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
    pub(crate) authenticated_account: Option<AccountId>,
    pub(crate) jira: Arc<JiraHttpClient>,
    pub(crate) cache: Arc<SqliteStore>,
}

/// Safe, stable configuration errors suitable for display in the UI/logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupError {
    Incomplete,
    InvalidSiteId,
    InvalidBaseUrl,
    MissingEmail,
    MissingApiToken,
    InvalidCredentials,
    AuthenticationRejected,
    AuthorizationDenied,
    CurrentUserUnavailable,
    ClientUnavailable,
    StorageUnavailable,
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Incomplete => {
                "Jira configuration is incomplete; set the Jira URL, site ID, email, and API token"
            }
            Self::InvalidSiteId => "Jira configuration has an invalid site ID",
            Self::InvalidBaseUrl => "Jira configuration has an invalid HTTPS Atlassian URL",
            Self::MissingEmail => "Enter the Atlassian email associated with this Jira token",
            Self::MissingApiToken => "Enter an Atlassian API token",
            Self::InvalidCredentials => "The Jira API token contains invalid whitespace",
            Self::AuthenticationRejected => {
                "Jira rejected the credentials (HTTP 401); check the email and API token"
            }
            Self::AuthorizationDenied => {
                "Jira denied access (HTTP 403); check this account's Jira permissions"
            }
            Self::CurrentUserUnavailable => {
                "Jira account could not be verified; check the site URL and credentials"
            }
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
}

impl EnvironmentValues {
    fn from_process() -> Self {
        Self {
            base_url: env::var(ENV_BASE_URL).ok(),
            site_id: env::var(ENV_SITE_ID).ok(),
            email: env::var(ENV_EMAIL).ok(),
            api_token: env::var(ENV_API_TOKEN).ok(),
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
pub async fn live_session_from_manual_configuration(
    base_url: String,
    email: String,
    api_token: String,
) -> Result<LiveSession, StartupError> {
    live_session_from_manual_configuration_with_store(base_url, email, api_token, || {
        local_data::open_store().map_err(|_| StartupError::StorageUnavailable)
    })
    .await
}

async fn live_session_from_manual_configuration_with_store<F>(
    base_url: String,
    email: String,
    api_token: String,
    store_factory: F,
) -> Result<LiveSession, StartupError>
where
    F: FnOnce() -> Result<Arc<SqliteStore>, StartupError>,
{
    let (base_url, email, api_token) = normalize_manual_inputs(&base_url, &email, &api_token);
    let parsed_url = JiraBaseUrl::parse(&base_url).map_err(|_| StartupError::InvalidBaseUrl)?;
    let host = parsed_url
        .as_url()
        .host_str()
        .ok_or(StartupError::InvalidBaseUrl)?;
    let site_id = JiraSiteId::new(host.to_owned()).map_err(|_| StartupError::InvalidBaseUrl)?;

    let credentials = credentials_from_values(email, api_token)?;
    let site_label = base_url.clone();
    let jira = JiraHttpClient::new(site_id.clone(), base_url, credentials)
        .map(Arc::new)
        .map_err(|error| match error {
            ConfigError::InvalidBaseUrl => StartupError::InvalidBaseUrl,
            _ => StartupError::ClientUnavailable,
        })?;
    let assignee = resolve_initial_assignee(jira.as_ref(), &site_id).await?;
    let cache = store_factory()?;

    Ok(LiveSession {
        site_id,
        site_label,
        authenticated_account: Some(assignee),
        jira,
        cache,
    })
}

/// Resolves the one initial assignee used by interactive onboarding.
///
/// Keeping this operation at the application-port seam makes onboarding
/// testable without an HTTP server and ensures the UI never asks users to
/// discover or paste an Atlassian account ID.
async fn resolve_initial_assignee(
    jira: &dyn JiraReadPort,
    site_id: &JiraSiteId,
) -> Result<AccountId, StartupError> {
    let cancellation = CancellationToken::new();
    let user = jira
        .fetch_current_user(site_id, &cancellation)
        .await
        .map_err(|error| match error.kind() {
            ErrorKind::Authentication => StartupError::AuthenticationRejected,
            ErrorKind::Authorization => StartupError::AuthorizationDenied,
            _ => StartupError::CurrentUserUnavailable,
        })?;
    if user.site_id != *site_id {
        return Err(StartupError::CurrentUserUnavailable);
    }
    Ok(user.account_id)
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
    store_factory: F,
) -> Result<LiveSession, StartupError>
where
    F: FnOnce() -> Result<Arc<SqliteStore>, StartupError>,
{
    let (base_url, email, api_token) = normalize_manual_inputs(&base_url, &email, &api_token);

    // Consume the credential strings directly into the client. No startup
    // state retains the token after this function returns.
    let credentials = credentials_from_values(email, api_token)?;
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
        // The synchronous environment bootstrap does not call `/myself`.
        authenticated_account: None,
        jira,
        cache,
    })
}

fn normalize_manual_inputs(
    base_url: &str,
    email: &str,
    api_token: &str,
) -> (String, String, String) {
    (
        base_url.trim().to_owned(),
        email.trim().to_owned(),
        api_token.trim().to_owned(),
    )
}

fn credentials_from_values(
    email: String,
    api_token: String,
) -> Result<ApiTokenCredentials, StartupError> {
    if email.is_empty() {
        return Err(StartupError::MissingEmail);
    }
    if api_token.is_empty() {
        return Err(StartupError::MissingApiToken);
    }
    if api_token.chars().any(char::is_whitespace) {
        return Err(StartupError::InvalidCredentials);
    }
    ApiTokenCredentials::new(email, api_token).map_err(|_| StartupError::InvalidCredentials)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_lite::future::block_on;
    use jira_application::{
        ApplicationError, CancellationToken, IssueCachePort, IssueFetchRequest, IssuePage,
        PortFuture, UserSearchRequest,
    };
    use jira_domain::{Issue, IssueId, User};
    use jira_storage::SqliteStore;

    use super::*;

    fn complete() -> EnvironmentValues {
        EnvironmentValues {
            base_url: Some("https://example.atlassian.net".to_owned()),
            site_id: Some("cloud-site".to_owned()),
            email: Some("developer@example.com".to_owned()),
            api_token: Some("token-that-must-not-escape".to_owned()),
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

    #[test]
    fn environment_configuration_has_no_authenticated_account() {
        let StartupSelection::Live(session) = in_memory_startup(complete()) else {
            panic!("environment setup should not require authenticated account IDs");
        };
        assert!(session.authenticated_account.is_none());
    }

    fn in_memory_startup(values: EnvironmentValues) -> StartupSelection {
        startup_from_values_with_store(values, || {
            SqliteStore::in_memory()
                .map(Arc::new)
                .map_err(|_| StartupError::StorageUnavailable)
        })
    }

    #[derive(Clone)]
    struct FakeCurrentUser {
        result: Result<User, ApplicationError>,
    }

    impl JiraReadPort for FakeCurrentUser {
        fn fetch_current_user<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, User> {
            Box::pin(std::future::ready(self.result.clone()))
        }

        fn search_users<'a>(
            &'a self,
            _request: &'a UserSearchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<User>> {
            Box::pin(std::future::ready(Ok(Vec::new())))
        }

        fn fetch_issue_page<'a>(
            &'a self,
            _request: &'a IssueFetchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            Box::pin(std::future::ready(Err(ApplicationError::new(
                ErrorKind::Internal,
                "not used by onboarding",
            ))))
        }

        fn fetch_issues_by_id<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _issue_ids: &'a [IssueId],
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<Issue>> {
            Box::pin(std::future::ready(Ok(Vec::new())))
        }
    }

    #[test]
    fn manual_onboarding_uses_authenticated_account() {
        let site_id = JiraSiteId::new("example.atlassian.net").expect("site");
        let user = User::new(
            site_id.clone(),
            AccountId::new("712020:authenticated").expect("account"),
            "Ada Lovelace",
            None,
            true,
        );
        let jira = FakeCurrentUser { result: Ok(user) };

        let assignee = block_on(resolve_initial_assignee(&jira, &site_id))
            .expect("current user should resolve");
        assert_eq!(assignee.as_str(), "712020:authenticated");
    }

    #[test]
    fn manual_onboarding_reports_safe_identity_errors() {
        let site_id = JiraSiteId::new("example.atlassian.net").expect("site");
        let jira = FakeCurrentUser {
            result: Err(ApplicationError::new(
                ErrorKind::Authentication,
                "request contained token-that-must-not-escape",
            )),
        };

        let error = block_on(resolve_initial_assignee(&jira, &site_id)).expect_err("auth error");
        assert_eq!(error, StartupError::AuthenticationRejected);
        assert!(!error.to_string().contains("token-that-must-not-escape"));
    }

    #[test]
    fn manual_onboarding_maps_authentication_and_authorization_separately() {
        let site_id = JiraSiteId::new("example.atlassian.net").expect("site");
        for (kind, expected) in [
            (
                ErrorKind::Authentication,
                StartupError::AuthenticationRejected,
            ),
            (ErrorKind::Authorization, StartupError::AuthorizationDenied),
        ] {
            let jira = FakeCurrentUser {
                result: Err(ApplicationError::new(
                    kind,
                    "remote details must not escape",
                )),
            };
            let error =
                block_on(resolve_initial_assignee(&jira, &site_id)).expect_err("remote auth error");
            assert_eq!(error, expected);
            assert!(!error.to_string().contains("remote details"));
            let message = error.to_string();
            if kind == ErrorKind::Authentication {
                assert_eq!(
                    message,
                    "Jira rejected the credentials (HTTP 401); check the email and API token"
                );
            } else {
                assert_eq!(
                    message,
                    "Jira denied access (HTTP 403); check this account's Jira permissions"
                );
            }
        }
    }

    #[test]
    fn manual_input_snapshots_trim_surrounding_whitespace() {
        let (base_url, email, api_token) = normalize_manual_inputs(
            "  https://example.atlassian.net/  ",
            "  developer@example.com\n",
            "\tapi-token\t",
        );

        assert_eq!(base_url, "https://example.atlassian.net/");
        assert_eq!(email, "developer@example.com");
        assert_eq!(api_token, "api-token");
    }

    #[test]
    fn manual_configuration_rejects_invalid_url_and_credentials() {
        assert!(matches!(
            block_on(live_session_from_manual_configuration_with_store(
                "http://example.atlassian.net".to_owned(),
                "developer@example.com".to_owned(),
                "token".to_owned(),
                || Err(StartupError::StorageUnavailable),
            )),
            Err(StartupError::InvalidBaseUrl)
        ));
        assert!(matches!(
            block_on(live_session_from_manual_configuration_with_store(
                "https://example.atlassian.net".to_owned(),
                "developer@example.com".to_owned(),
                "".to_owned(),
                || Err(StartupError::StorageUnavailable),
            )),
            Err(StartupError::MissingApiToken)
        ));
        assert!(matches!(
            block_on(live_session_from_manual_configuration_with_store(
                "https://example.atlassian.net".to_owned(),
                "".to_owned(),
                "token".to_owned(),
                || Err(StartupError::StorageUnavailable),
            )),
            Err(StartupError::MissingEmail)
        ));
    }

    #[test]
    fn manual_configuration_does_not_open_store_for_invalid_values() {
        let called = Arc::new(AtomicBool::new(false));
        let store_called = called.clone();
        let result = block_on(live_session_from_manual_configuration_with_store(
            "https://example.atlassian.net".to_owned(),
            "developer@example.com".to_owned(),
            "".to_owned(),
            move || {
                store_called.store(true, Ordering::SeqCst);
                Err(StartupError::StorageUnavailable)
            },
        ));

        assert!(matches!(result, Err(StartupError::MissingApiToken)));
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
