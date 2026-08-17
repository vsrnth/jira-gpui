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

use jira_adapter::{EnhancedSearchPage, IssueMapper, JiraCommentPage, JiraIssue, JiraUser};
use jira_application::{
    AddCommentRequest, ApplicationError, CancellationToken, ErrorKind, IssueCommentsPage,
    IssueCommentsPageRequest, IssueDetailRequest, IssueFetchRequest, IssueLocator, IssuePage,
    JiraCommentWritePort, JiraReadPort, PageCursor, PortFuture, UserSearchRequest,
};
use jira_domain::{Issue, IssueComment, IssueId, JiraSiteId, User};
use reqwest::{Client, Response, StatusCode, header};
use serde::{Serialize, de::DeserializeOwned};
use tokio::{runtime::Builder, sync::oneshot};
use url::Url;

const DEFAULT_USER_AGENT: &str = "jira-gpui/0.1 (read-only client)";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ISSUE_ID_PAGES: usize = 128;

#[derive(Debug, Serialize)]
struct JiraCommentCreateRequest {
    body: JiraAdfDocument,
}

#[derive(Debug, Serialize)]
struct JiraAdfDocument {
    #[serde(rename = "type")]
    kind: &'static str,
    version: u8,
    content: Vec<JiraAdfBlock>,
}

#[derive(Debug, Serialize)]
struct JiraAdfBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    content: Vec<JiraAdfText>,
}

#[derive(Debug, Serialize)]
struct JiraAdfText {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

fn jira_comment_create_body(text: &str) -> JiraCommentCreateRequest {
    JiraCommentCreateRequest {
        body: JiraAdfDocument {
            kind: "doc",
            version: 1,
            content: vec![JiraAdfBlock {
                kind: "paragraph",
                content: vec![JiraAdfText {
                    kind: "text",
                    text: text.to_owned(),
                }],
            }],
        },
    }
}

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

    fn issue_endpoint(
        &self,
        locator: &IssueLocator,
        suffix: Option<&str>,
    ) -> Result<Url, ApplicationError> {
        let issue_id_or_key = match locator {
            IssueLocator::Id(issue_id) => issue_id.as_str(),
            IssueLocator::Key(issue_key) => issue_key.as_str(),
        };
        let mut url = self.endpoint("rest/api/3/issue")?;
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                ApplicationError::new(ErrorKind::Internal, "invalid Jira issue endpoint")
            })?;
            segments.push(issue_id_or_key);
            if let Some(suffix) = suffix {
                segments.push(suffix);
            }
        }
        Ok(url)
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

    /// Dispatches a write after the preflight cancellation check. Once queued, the operation is
    /// awaited to completion so cancellation cannot turn a committed-or-unknown Jira write into
    /// a retryable cancellation result.
    fn submit_write<T, F>(
        &self,
        cancellation: &CancellationToken,
        operation: F,
    ) -> PortFuture<'static, T>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, ApplicationError>> + Send + 'static,
    {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        let runtime = Arc::clone(&self.runtime);
        Box::pin(async move {
            runtime
                .dispatch(operation)
                .await
                .map_err(write_dispatch_error)?
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

    async fn current_user_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        site_id: JiraSiteId,
        max_response_bytes: usize,
    ) -> Result<User, ApplicationError> {
        let response = Self::current_user_request_builder(&client, url, &credentials)
            .send()
            .await
            .map_err(transport_error)?;
        let user: JiraUser = read_json(response, max_response_bytes).await?;
        Self::map_current_user(site_id, user)
    }

    fn current_user_request_builder(
        client: &Client,
        url: Url,
        credentials: &ApiTokenCredentials,
    ) -> reqwest::RequestBuilder {
        client
            .get(url)
            .basic_auth(&credentials.email, Some(&credentials.token))
            .header(header::ACCEPT, "application/json")
    }

    fn map_current_user(site_id: JiraSiteId, user: JiraUser) -> Result<User, ApplicationError> {
        IssueMapper.map_user(site_id, user).map_err(|_| {
            ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid user data")
        })
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

    async fn issue_detail_request(
        client: Client,
        mut url: Url,
        credentials: ApiTokenCredentials,
        site_id: jira_domain::JiraSiteId,
        max_response_bytes: usize,
    ) -> Result<jira_domain::IssueDetailCore, ApplicationError> {
        url.query_pairs_mut()
            .append_pair("fields", &jira_adapter::issue_detail_fields_query());
        let response = client
            .get(url)
            .basic_auth(credentials.email, Some(credentials.token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        let issue: JiraIssue = read_json(response, max_response_bytes).await?;
        IssueMapper
            .map_domain_issue_detail(site_id, issue)
            .map_err(|_| {
                ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid issue detail")
            })
    }

    async fn issue_comments_page_request(
        client: Client,
        mut url: Url,
        credentials: ApiTokenCredentials,
        request: IssueCommentsPageRequest,
        max_response_bytes: usize,
    ) -> Result<IssueCommentsPage, ApplicationError> {
        let limit = request.page_size.min(100);
        if limit == 0 {
            return Err(ApplicationError::invalid_input(
                "comment page size must be positive",
            ));
        }
        url.query_pairs_mut()
            .append_pair("startAt", &request.start_at.to_string())
            .append_pair("maxResults", &limit.to_string())
            .append_pair("orderBy", "created");
        let response = client
            .get(url)
            .basic_auth(credentials.email, Some(credentials.token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        let page: JiraCommentPage = read_json(response, max_response_bytes).await?;
        IssueMapper.map_comment_page(page).map_err(|_| {
            ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid comment data")
        })
    }

    fn create_comment_request_builder(
        client: &Client,
        url: Url,
        credentials: &ApiTokenCredentials,
        body: JiraCommentCreateRequest,
    ) -> reqwest::RequestBuilder {
        client
            .post(url)
            .basic_auth(&credentials.email, Some(&credentials.token))
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
    }

    async fn create_comment_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        body: String,
        max_response_bytes: usize,
    ) -> Result<IssueComment, ApplicationError> {
        let response = Self::create_comment_request_builder(
            &client,
            url,
            &credentials,
            jira_comment_create_body(&body),
        )
        .send()
        .await
        .map_err(comment_transport_error)?;

        read_created_comment(response, max_response_bytes).await
    }
}

impl JiraReadPort for JiraHttpClient {
    fn fetch_issue_detail<'a>(
        &'a self,
        request: &'a IssueDetailRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, jira_domain::IssueDetailCore> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.issue_endpoint(&request.locator, None) {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let site_id = request.site_id.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::issue_detail_request(client, url, credentials, site_id, max).await
        })
    }

    fn fetch_issue_comments_page<'a>(
        &'a self,
        request: &'a IssueCommentsPageRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssueCommentsPage> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let locator = IssueLocator::Id(request.issue_id.clone());
        let url = match self.issue_endpoint(&locator, Some("comment")) {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let request = request.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::issue_comments_page_request(client, url, credentials, request, max).await
        })
    }

    fn fetch_current_user<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, User> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.endpoint("rest/api/3/myself") {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let site_id = site_id.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::current_user_request(client, url, credentials, site_id, max).await
        })
    }

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

impl JiraCommentWritePort for JiraHttpClient {
    fn create_comment<'a>(
        &'a self,
        request: &'a AddCommentRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssueComment> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let body = request.body.trim().to_owned();
        if body.is_empty() {
            return Box::pin(std::future::ready(Err(ApplicationError::invalid_input(
                "comment body must not be empty",
            ))));
        }
        let url = match self.issue_endpoint(&request.locator, Some("comment")) {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let max = self.config.max_response_bytes;
        self.submit_write(cancellation, async move {
            Self::create_comment_request(client, url, credentials, body, max).await
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

fn comment_transport_error(error: reqwest::Error) -> ApplicationError {
    if error.is_connect() {
        ApplicationError::new(ErrorKind::Offline, "could not connect to Jira")
    } else {
        ApplicationError::new(ErrorKind::UnknownOutcome, "Jira comment outcome is unknown")
    }
}

fn write_dispatch_error(_error: ApplicationError) -> ApplicationError {
    ApplicationError::new(ErrorKind::UnknownOutcome, "Jira comment outcome is unknown")
}

fn comment_status_error(status: StatusCode, headers: &header::HeaderMap) -> ApplicationError {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => {
            ApplicationError::invalid_input("Jira rejected the comment request")
        }
        StatusCode::UNAUTHORIZED => {
            ApplicationError::new(ErrorKind::Authentication, "Jira authentication failed")
        }
        StatusCode::FORBIDDEN => {
            ApplicationError::new(ErrorKind::Authorization, "Jira authorization was denied")
        }
        StatusCode::NOT_FOUND => {
            ApplicationError::new(ErrorKind::NotFound, "Jira issue was not found")
        }
        StatusCode::TOO_MANY_REQUESTS => {
            ApplicationError::rate_limited("Jira rate limit exceeded", retry_after(headers))
        }
        _ => ApplicationError::new(ErrorKind::UnknownOutcome, "Jira comment outcome is unknown"),
    }
}

async fn read_created_comment(
    response: Response,
    max_bytes: usize,
) -> Result<IssueComment, ApplicationError> {
    let status = response.status();
    if status != StatusCode::CREATED {
        return Err(comment_status_error(status, response.headers()));
    }
    let mut response = response;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(ApplicationError::new(
            ErrorKind::UnknownOutcome,
            "Jira comment outcome is unknown",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ApplicationError::new(ErrorKind::UnknownOutcome, "Jira comment outcome is unknown")
    })? {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(ApplicationError::new(
                ErrorKind::UnknownOutcome,
                "Jira comment outcome is unknown",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    map_created_comment_body(&body)
}

fn map_created_comment_body(body: &[u8]) -> Result<IssueComment, ApplicationError> {
    let comment: jira_adapter::JiraComment = serde_json::from_slice(body).map_err(|_| {
        ApplicationError::new(ErrorKind::UnknownOutcome, "Jira comment outcome is unknown")
    })?;
    IssueMapper.map_comment(comment).map_err(|_| {
        ApplicationError::new(ErrorKind::UnknownOutcome, "Jira comment outcome is unknown")
    })
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

    #[test]
    fn current_user_request_targets_myself_and_maps_authenticated_identity() {
        let site_id = JiraSiteId::new("example-site").expect("site");
        let credentials =
            ApiTokenCredentials::new("person@example.com", "token").expect("credentials");
        let client = Client::new();
        let request = JiraHttpClient::current_user_request_builder(
            &client,
            Url::parse("https://example.atlassian.net/rest/api/3/myself").expect("test URL"),
            &credentials,
        )
        .build()
        .expect("test request");
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(request.url().path(), "/rest/api/3/myself");
        assert_eq!(
            request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Basic cGVyc29uQGV4YW1wbGUuY29tOnRva2Vu")
        );

        let remote_user: JiraUser = serde_json::from_str(
            r#"{
                "accountId": "557058:abc-123",
                "displayName": "Ada Lovelace",
                "active": true,
                "avatarUrls": {"48x48": "https://avatar.example.test/ada.png"}
            }"#,
        )
        .expect("current user JSON");
        let user = JiraHttpClient::map_current_user(site_id.clone(), remote_user)
            .expect("current user mapping");
        assert_eq!(user.site_id, site_id);
        assert_eq!(user.account_id.as_str(), "557058:abc-123");
        assert_eq!(user.display_name, "Ada Lovelace");
        assert_eq!(
            user.avatar_url.as_deref(),
            Some("https://avatar.example.test/ada.png")
        );
        assert!(user.active);
    }

    #[test]
    fn issue_detail_and_comment_urls_encode_ids_as_path_segments_and_use_expected_queries() {
        let configured = JiraSiteId::new("configured-site").unwrap();
        let client = JiraHttpClient {
            site_id: configured,
            base_url: JiraBaseUrl::parse("https://example.atlassian.net").unwrap(),
            credentials: ApiTokenCredentials::new("person@example.com", "secret").unwrap(),
            client: Client::new(),
            runtime: Arc::new(RuntimeBridge::new().unwrap()),
            config: JiraHttpConfig::default(),
        };
        let issue_id = IssueId::new("ENG/42?private").unwrap();
        let mut detail = client
            .issue_endpoint(&IssueLocator::Id(issue_id.clone()), None)
            .unwrap();
        detail
            .query_pairs_mut()
            .append_pair("fields", &jira_adapter::issue_detail_fields_query());
        assert_eq!(detail.path(), "/rest/api/3/issue/ENG%2F42%3Fprivate");
        assert_eq!(
            detail
                .query_pairs()
                .find(|(name, _)| name == "fields")
                .map(|(_, value)| value.into_owned()),
            Some(jira_adapter::issue_detail_fields_query())
        );

        let mut comments = client
            .issue_endpoint(&IssueLocator::Id(issue_id), Some("comment"))
            .unwrap();
        comments
            .query_pairs_mut()
            .append_pair("startAt", "20")
            .append_pair("maxResults", "50")
            .append_pair("orderBy", "created");
        assert_eq!(
            comments.path(),
            "/rest/api/3/issue/ENG%2F42%3Fprivate/comment"
        );
        assert_eq!(
            comments.query(),
            Some("startAt=20&maxResults=50&orderBy=created")
        );
    }

    #[test]
    fn issue_detail_url_accepts_a_typed_issue_key_as_one_encoded_path_segment() {
        let configured = JiraSiteId::new("configured-site").unwrap();
        let client = JiraHttpClient {
            site_id: configured,
            base_url: JiraBaseUrl::parse("https://example.atlassian.net").unwrap(),
            credentials: ApiTokenCredentials::new("person@example.com", "secret").unwrap(),
            client: Client::new(),
            runtime: Arc::new(RuntimeBridge::new().unwrap()),
            config: JiraHttpConfig::default(),
        };
        let issue_key = jira_domain::IssueKey::new("ENG-42").unwrap();
        let url = client
            .issue_endpoint(&IssueLocator::Key(issue_key), None)
            .unwrap();

        assert_eq!(url.path(), "/rest/api/3/issue/ENG-42");
        assert_eq!(url.query(), None);
    }

    #[test]
    fn detail_request_builder_uses_basic_auth_without_putting_credentials_in_the_url() {
        let credentials = ApiTokenCredentials::new("person@example.com", "secret-token").unwrap();
        let request = Client::new()
            .get(Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42").unwrap())
            .basic_auth(&credentials.email, Some(&credentials.token))
            .build()
            .unwrap();
        assert_eq!(request.url().query(), None);
        assert_eq!(request.url().username(), "");
        assert!(!request.url().as_str().contains("secret-token"));
        assert!(request.headers().contains_key(header::AUTHORIZATION));
    }

    #[test]
    fn create_comment_builder_posts_plain_text_as_safe_adf_without_extra_fields() {
        let credentials = ApiTokenCredentials::new("person@example.com", "secret-token").unwrap();
        let request = JiraHttpClient::create_comment_request_builder(
            &Client::new(),
            Url::parse("https://example.atlassian.net/rest/api/3/issue/IX-123/comment").unwrap(),
            &credentials,
            jira_comment_create_body("<b>hello & goodbye</b>\nsecond"),
        )
        .build()
        .unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().path(), "/rest/api/3/issue/IX-123/comment");
        assert_eq!(request.url().query(), None);
        assert_eq!(
            request.headers().get(header::ACCEPT).unwrap(),
            "application/json"
        );
        assert_eq!(
            request.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Basic cGVyc29uQGV4YW1wbGUuY29tOnNlY3JldC10b2tlbg==")
        );
        let body = request.body().and_then(reqwest::Body::as_bytes).unwrap();
        let json: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "body": {
                    "type": "doc",
                    "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{
                            "type": "text",
                            "text": "<b>hello & goodbye</b>\nsecond"
                        }]
                    }]
                }
            })
        );
        assert!(json.get("visibility").is_none());
        assert!(json.get("properties").is_none());
        assert!(!String::from_utf8_lossy(body).contains("secret-token"));
    }

    #[test]
    fn comment_body_is_trimmed_before_adf_serialization() {
        let body = "  hello\nworld  ".trim();
        let json = serde_json::to_value(jira_comment_create_body(body)).unwrap();

        assert_eq!(
            json["body"]["content"][0]["content"][0]["text"],
            "hello\nworld"
        );
    }

    #[test]
    fn comment_status_mapping_preserves_safe_categories_and_retry_after() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::RETRY_AFTER, header::HeaderValue::from_static("7"));
        for (status, kind) in [
            (StatusCode::BAD_REQUEST, ErrorKind::InvalidInput),
            (StatusCode::PAYLOAD_TOO_LARGE, ErrorKind::InvalidInput),
            (StatusCode::UNAUTHORIZED, ErrorKind::Authentication),
            (StatusCode::FORBIDDEN, ErrorKind::Authorization),
            (StatusCode::NOT_FOUND, ErrorKind::NotFound),
        ] {
            assert_eq!(comment_status_error(status, &headers).kind(), kind);
        }
        let rate_limited = comment_status_error(StatusCode::TOO_MANY_REQUESTS, &headers);
        assert_eq!(rate_limited.kind(), ErrorKind::RateLimited);
        assert_eq!(rate_limited.retry_after(), Some(Duration::from_secs(7)));
        assert_eq!(
            comment_status_error(StatusCode::INTERNAL_SERVER_ERROR, &headers).kind(),
            ErrorKind::UnknownOutcome
        );
        assert_eq!(
            comment_status_error(StatusCode::OK, &headers).kind(),
            ErrorKind::UnknownOutcome
        );
    }

    #[test]
    fn write_dispatch_failures_are_unknown_outcomes_without_leaking_dispatch_details() {
        let error = write_dispatch_error(ApplicationError::new(
            ErrorKind::Internal,
            "runtime response channel closed with secret-token",
        ));

        assert_eq!(error.kind(), ErrorKind::UnknownOutcome);
        assert_eq!(error.message(), "Jira comment outcome is unknown");
        assert!(!error.message().contains("secret-token"));
    }

    #[test]
    fn malformed_created_comment_is_an_unknown_outcome_without_leaking_body() {
        let error = map_created_comment_body(br#"{"id":"secret-token"}"#).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::UnknownOutcome);
        assert_eq!(error.message(), "Jira comment outcome is unknown");
        assert!(!error.message().contains("secret-token"));
    }

    #[test]
    fn cancelled_comment_creation_is_rejected_before_dispatch() {
        let site = JiraSiteId::new("configured-site").unwrap();
        let client = JiraHttpClient::new(
            site.clone(),
            "https://example.atlassian.net",
            ApiTokenCredentials::new("person@example.com", "secret-token").unwrap(),
        )
        .unwrap();
        let request = AddCommentRequest {
            site_id: site,
            locator: IssueLocator::Key(jira_domain::IssueKey::new("IX-123").unwrap()),
            body: "hello".to_owned(),
        };
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let error = runtime
            .block_on(client.create_comment(&request, &cancellation))
            .unwrap_err();

        assert_eq!(error.kind(), ErrorKind::Cancelled);
    }
}
