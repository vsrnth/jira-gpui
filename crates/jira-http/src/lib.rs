//! Read-only Jira Cloud HTTP transport.
//!
//! The adapter owns a small Tokio runtime on a worker thread. This is intentional: GPUI and a
//! future Tauri shell can poll the application ports without having to install or drive a Tokio
//! reactor of their own. Credentials are held only in memory and are never persisted here.

use std::{
    fmt,
    sync::{Arc, mpsc},
    time::Duration,
};

use jira_adapter::{EnhancedSearchPage, IssueMapper, JiraUser};
use jira_application::{
    ApplicationError, CancellationToken, ErrorKind, IssueFetchRequest, IssuePage, JiraReadPort,
    PageCursor, PortFuture, UserSearchRequest,
};
use jira_domain::{Issue, IssueId, JiraSiteId, User};
use reqwest::{Client, Response, StatusCode, header};
use serde::de::DeserializeOwned;
use tokio::{runtime::Builder, sync::oneshot};
use url::Url;

const DEFAULT_USER_AGENT: &str = "jira-gpui/0.1 (read-only client)";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ISSUE_ID_PAGES: usize = 128;

/// Credentials for Jira Cloud basic authentication (email + API token).
///
/// The token is deliberately not exposed through `Debug`, `Display`, or an error value.
#[derive(Clone, Eq, PartialEq)]
pub struct ApiTokenCredentials {
    email: String,
    token: String,
}

impl ApiTokenCredentials {
    pub fn new(email: impl Into<String>, token: impl Into<String>) -> Result<Self, ConfigError> {
        let email = email.into();
        let token = token.into();
        if email.trim().is_empty() {
            return Err(ConfigError::EmptyCredential("email"));
        }
        if token.is_empty() {
            return Err(ConfigError::EmptyCredential("API token"));
        }
        Ok(Self { email, token })
    }
}

impl fmt::Debug for ApiTokenCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiTokenCredentials")
            .field("email", &self.email)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// A URL that passed the transport's security checks.
#[derive(Clone, Eq, PartialEq)]
pub struct JiraBaseUrl(Url);

impl JiraBaseUrl {
    /// Parses a public Jira Cloud URL. Self-hosted URLs can be enabled by an explicit future
    /// configuration surface; the default constructor intentionally restricts this to Atlassian.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ConfigError> {
        let url = Url::parse(value.as_ref()).map_err(|_| ConfigError::InvalidBaseUrl)?;
        validate_base_url(&url, true)
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for JiraBaseUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("JiraBaseUrl")
            .field(&self.0.as_str())
            .finish()
    }
}

/// Runtime and response limits. All values are bounded by the caller and never read from a
/// remote response or credential.
#[derive(Clone, Debug)]
pub struct JiraHttpConfig {
    pub request_timeout: Duration,
    pub connect_timeout: Duration,
    pub max_response_bytes: usize,
    pub user_agent: String,
}

impl Default for JiraHttpConfig {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
        }
    }
}

impl JiraHttpConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.request_timeout.is_zero() || self.connect_timeout.is_zero() {
            return Err(ConfigError::InvalidTimeout);
        }
        if self.max_response_bytes == 0 {
            return Err(ConfigError::InvalidResponseLimit);
        }
        if self.user_agent.trim().is_empty() {
            return Err(ConfigError::EmptyUserAgent);
        }
        Ok(())
    }
}

/// Read-only Jira Cloud client implementing the application boundary.
pub struct JiraHttpClient {
    site_id: JiraSiteId,
    base_url: JiraBaseUrl,
    credentials: ApiTokenCredentials,
    client: Client,
    runtime: Arc<RuntimeBridge>,
    config: JiraHttpConfig,
}

impl fmt::Debug for JiraHttpClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JiraHttpClient")
            .field("base_url", &self.base_url)
            .field("credentials", &self.credentials)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl JiraHttpClient {
    pub fn new(
        site_id: JiraSiteId,
        base_url: impl AsRef<str>,
        credentials: ApiTokenCredentials,
    ) -> Result<Self, ConfigError> {
        Self::with_config(site_id, base_url, credentials, JiraHttpConfig::default())
    }

    pub fn with_config(
        site_id: JiraSiteId,
        base_url: impl AsRef<str>,
        credentials: ApiTokenCredentials,
        config: JiraHttpConfig,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        let base_url = JiraBaseUrl::parse(base_url)?;
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .user_agent(config.user_agent.clone())
            .build()
            .map_err(|_| ConfigError::HttpClientBuild)?;
        Ok(Self {
            site_id,
            base_url,
            credentials,
            client,
            runtime: Arc::new(RuntimeBridge::new().map_err(|_| ConfigError::RuntimeBuild)?),
            config,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, ApplicationError> {
        self.base_url
            .as_url()
            .join(path)
            .map_err(|_| ApplicationError::new(ErrorKind::Internal, "invalid Jira endpoint"))
    }

    fn validate_site(&self, site_id: &JiraSiteId) -> Result<(), ApplicationError> {
        if site_id != &self.site_id {
            return Err(ApplicationError::invalid_input(
                "request site does not match the configured Jira site",
            ));
        }
        Ok(())
    }

    fn submit<T, F>(&self, cancellation: &CancellationToken, operation: F) -> PortFuture<'static, T>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, ApplicationError>> + Send + 'static,
    {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        let runtime = Arc::clone(&self.runtime);
        let cancellation = cancellation.clone();
        Box::pin(async move {
            cancellation.check()?;
            let result = runtime.dispatch(operation).await?;
            cancellation.check()?;
            result
        })
    }

    async fn search_users_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        request: UserSearchRequest,
        max_response_bytes: usize,
    ) -> Result<Vec<User>, ApplicationError> {
        let limit = request.limit.min(1_000);
        if limit == 0 {
            return Err(ApplicationError::invalid_input(
                "user search limit must be positive",
            ));
        }
        let mut url = url;
        url.query_pairs_mut()
            .append_pair("query", &request.query)
            .append_pair("maxResults", &limit.to_string());
        let response = client
            .get(url)
            .basic_auth(credentials.email, Some(credentials.token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        let users: Vec<JiraUser> = read_json(response, max_response_bytes).await?;
        users
            .into_iter()
            .map(|user| {
                IssueMapper
                    .map_user(request.site_id.clone(), user)
                    .map_err(|_| {
                        ApplicationError::new(
                            ErrorKind::Upstream,
                            "Jira returned invalid user data",
                        )
                    })
            })
            .collect()
    }

    async fn search_issue_page_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        request: IssueFetchRequest,
        max_response_bytes: usize,
    ) -> Result<IssuePage, ApplicationError> {
        let body = jira_adapter::enhanced_search_request(&request)
            .map_err(|_| ApplicationError::invalid_input("invalid Jira issue search request"))?;
        let response = client
            .post(url)
            .basic_auth(credentials.email, Some(credentials.token))
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        let page: EnhancedSearchPage = read_json(response, max_response_bytes).await?;
        let mapped = IssueMapper
            .map_domain_page(request.site_id, page)
            .map_err(|_| {
                ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid issue data")
            })?;
        Ok(IssuePage {
            issues: mapped.issues,
            next_cursor: mapped.next_page_token.map(PageCursor),
            server_time: None,
        })
    }

    async fn search_issues_by_id_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        site_id: JiraSiteId,
        issue_ids: Vec<IssueId>,
        cancellation: CancellationToken,
        max_response_bytes: usize,
    ) -> Result<Vec<Issue>, ApplicationError> {
        // The Jira adapter owns JQL construction and validation. The helper is kept at that
        // boundary so this transport never interpolates persisted issue IDs itself.
        let base_body = jira_adapter::enhanced_search_request_for_issue_ids(&issue_ids)
            .map_err(|_| ApplicationError::invalid_input("invalid Jira issue IDs"))?;
        let mut next_page_token = None;
        let mut issues = Vec::new();
        for _ in 0..MAX_ISSUE_ID_PAGES {
            cancellation.check()?;
            let mut body = base_body.clone();
            body.next_page_token = next_page_token.clone();
            let response = client
                .post(url.clone())
                .basic_auth(&credentials.email, Some(&credentials.token))
                .header(header::ACCEPT, "application/json")
                .header(header::CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .await
                .map_err(transport_error)?;
            let page: EnhancedSearchPage = read_json(response, max_response_bytes).await?;
            let mapped = IssueMapper
                .map_domain_page(site_id.clone(), page)
                .map_err(|_| {
                    ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid issue data")
                })?;
            issues.extend(mapped.issues);
            if mapped.is_last {
                return Ok(issues);
            }
            let Some(token) = mapped.next_page_token else {
                return Ok(issues);
            };
            next_page_token = Some(token);
        }
        Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira issue pagination exceeded the safety limit",
        ))
    }
}

impl JiraReadPort for JiraHttpClient {
    fn search_users<'a>(
        &'a self,
        request: &'a UserSearchRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<User>> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.endpoint("rest/api/3/user/search") {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let request = request.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::search_users_request(client, url, credentials, request, max).await
        })
    }

    fn fetch_issue_page<'a>(
        &'a self,
        request: &'a IssueFetchRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssuePage> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.endpoint("rest/api/3/search/jql") {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let request = request.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::search_issue_page_request(client, url, credentials, request, max).await
        })
    }

    fn fetch_issues_by_id<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_ids: &'a [IssueId],
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<Issue>> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if issue_ids.is_empty() {
            return Box::pin(std::future::ready(Ok(Vec::new())));
        }
        let url = match self.endpoint("rest/api/3/search/jql") {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let site_id = site_id.clone();
        let issue_ids = issue_ids.to_vec();
        let cancellation_for_request = cancellation.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::search_issues_by_id_request(
                client,
                url,
                credentials,
                site_id,
                issue_ids,
                cancellation_for_request,
                max,
            )
            .await
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfigError {
    #[error("Jira base URL must be a valid HTTPS Atlassian Cloud URL")]
    InvalidBaseUrl,
    #[error("Jira {0} cannot be empty")]
    EmptyCredential(&'static str),
    #[error("Jira HTTP timeouts must be positive")]
    InvalidTimeout,
    #[error("Jira response limit must be positive")]
    InvalidResponseLimit,
    #[error("Jira user agent cannot be empty")]
    EmptyUserAgent,
    #[error("could not initialize the Jira HTTP client")]
    HttpClientBuild,
    #[error("could not initialize the Jira runtime")]
    RuntimeBuild,
}

fn validate_base_url(url: &Url, require_atlassian: bool) -> Result<JiraBaseUrl, ConfigError> {
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::InvalidBaseUrl);
    }
    if url.path() != "" && url.path() != "/" {
        return Err(ConfigError::InvalidBaseUrl);
    }
    if require_atlassian {
        let Some(host) = url.host_str() else {
            return Err(ConfigError::InvalidBaseUrl);
        };
        if !host.ends_with(".atlassian.net") || host == "atlassian.net" {
            return Err(ConfigError::InvalidBaseUrl);
        }
    }
    Ok(JiraBaseUrl(url.clone()))
}

fn transport_error(error: reqwest::Error) -> ApplicationError {
    if error.is_timeout() || error.is_connect() {
        ApplicationError::new(ErrorKind::Offline, "could not connect to Jira")
    } else {
        ApplicationError::new(ErrorKind::Upstream, "Jira request failed")
    }
}

async fn read_json<T: DeserializeOwned>(
    response: Response,
    max_bytes: usize,
) -> Result<T, ApplicationError> {
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(status, response.headers()));
    }
    let mut response = response;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira response exceeded the size limit",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApplicationError::new(ErrorKind::Offline, "could not read Jira response"))?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(ApplicationError::new(
                ErrorKind::Upstream,
                "Jira response exceeded the size limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| ApplicationError::new(ErrorKind::Upstream, "Jira returned malformed JSON"))
}

fn status_error(status: StatusCode, headers: &header::HeaderMap) -> ApplicationError {
    match status {
        StatusCode::UNAUTHORIZED => {
            ApplicationError::new(ErrorKind::Authentication, "Jira authentication failed")
        }
        StatusCode::FORBIDDEN => {
            ApplicationError::new(ErrorKind::Authorization, "Jira authorization was denied")
        }
        StatusCode::NOT_FOUND => {
            ApplicationError::new(ErrorKind::NotFound, "Jira resource was not found")
        }
        StatusCode::TOO_MANY_REQUESTS => {
            ApplicationError::rate_limited("Jira rate limit exceeded", retry_after(headers))
        }
        _ => ApplicationError::new(
            ErrorKind::Upstream,
            "Jira returned an unsuccessful response",
        ),
    }
}

fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

type RuntimeJob = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send + 'static>;

struct RuntimeBridge {
    sender: mpsc::Sender<RuntimeJob>,
}

impl RuntimeBridge {
    fn new() -> Result<Self, ()> {
        let (sender, receiver) = mpsc::channel::<RuntimeJob>();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("jira-http-runtime".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => {
                        let _ = startup_sender.send(Ok(()));
                        runtime
                    }
                    Err(_) => {
                        let _ = startup_sender.send(Err(()));
                        return;
                    }
                };
                while let Ok(job) = receiver.recv() {
                    job(&runtime);
                }
            })
            .map_err(|_| ())?;
        startup_receiver.recv().map_err(|_| ())??;
        Ok(Self { sender })
    }

    async fn dispatch<T, F>(
        &self,
        operation: F,
    ) -> Result<Result<T, ApplicationError>, ApplicationError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, ApplicationError>> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Box::new(move |runtime| {
                let result = runtime.block_on(operation);
                let _ = sender.send(result);
            }))
            .map_err(|_| {
                ApplicationError::new(ErrorKind::Internal, "Jira runtime is unavailable")
            })?;
        receiver
            .await
            .map_err(|_| ApplicationError::new(ErrorKind::Internal, "Jira runtime stopped"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_https_atlassian_cloud_urls_without_embedded_data() {
        assert!(JiraBaseUrl::parse("https://example.atlassian.net").is_ok());
        assert!(JiraBaseUrl::parse("https://example.atlassian.net/").is_ok());
        for invalid in [
            "http://example.atlassian.net",
            "https://example.atlassian.net/?token=secret",
            "https://user@example.atlassian.net",
            "https://example.atlassian.net#fragment",
            "https://example.atlassian.net/tenant",
            "https://example.example.com",
        ] {
            assert!(JiraBaseUrl::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn credentials_debug_redacts_token() {
        let credentials =
            ApiTokenCredentials::new("person@example.com", "super-secret-token").unwrap();
        let debug = format!("{credentials:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-token"));
    }

    #[test]
    fn status_mapping_preserves_retry_after_without_response_body() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::RETRY_AFTER, header::HeaderValue::from_static("42"));
        let error = status_error(StatusCode::TOO_MANY_REQUESTS, &headers);
        assert_eq!(error.kind(), ErrorKind::RateLimited);
        assert_eq!(error.retry_after(), Some(Duration::from_secs(42)));

        let error = status_error(StatusCode::UNAUTHORIZED, &header::HeaderMap::new());
        assert_eq!(error.kind(), ErrorKind::Authentication);
        assert!(!error.message().contains("secret"));
    }

    #[test]
    fn status_mapping_uses_safe_stable_messages() {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let error = status_error(status, &header::HeaderMap::new());
            assert!(!error.message().contains("{"));
            assert!(!error.message().contains("token"));
        }
    }

    #[test]
    fn site_validation_rejects_cross_site_requests_before_dispatch() {
        let configured = JiraSiteId::new("configured-site").unwrap();
        let other = JiraSiteId::new("other-site").unwrap();
        let base = JiraHttpClient {
            site_id: configured,
            base_url: JiraBaseUrl::parse("https://example.atlassian.net").unwrap(),
            credentials: ApiTokenCredentials::new("person@example.com", "secret").unwrap(),
            client: Client::new(),
            runtime: Arc::new(RuntimeBridge::new().unwrap()),
            config: JiraHttpConfig::default(),
        };
        let error = base.validate_site(&other).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert!(!error.message().contains("other-site"));
    }

    #[test]
    fn issue_id_pagination_has_a_finite_safety_bound() {
        assert!(MAX_ISSUE_ID_PAGES >= 2);
        assert!(MAX_ISSUE_ID_PAGES * 100 >= 1_000);
    }
}
