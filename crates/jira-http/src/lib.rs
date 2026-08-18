//! Jira Cloud HTTP transport for remote reads and explicitly confirmed comment, assignment, and
//! workflow-status writes. Writes are dispatched once without automatic retries.
//!
//! The adapter owns a small Tokio runtime on a worker thread. This is intentional: GPUI and a
//! future Tauri shell can poll the application ports without having to install or drive a Tokio
//! reactor of their own. Credentials are held only in memory and are never persisted here.

use std::{
    fmt,
    sync::{Arc, mpsc},
    time::Duration,
};

use jira_adapter::{
    EnhancedSearchPage, IssueMapper, JiraBulkChangelogResponse, JiraCommentPage, JiraIssue,
    JiraUser,
};
use jira_application::{
    AddCommentRequest, ApplicationError, AssignIssueRequest, AssignableUserSearchRequest,
    AttachmentBodyClass, AttachmentContent, AttachmentDownloadRequest, AttachmentImage,
    AttachmentImageRequest, AttachmentMimeClass, AttachmentReadAttempt, AttachmentReadDiagnostic,
    AttachmentTransportClass, CancellationToken, DEFAULT_ATTACHMENT_IMAGE_HEIGHT,
    DEFAULT_ATTACHMENT_IMAGE_WIDTH, DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES,
    DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES, ErrorKind, IssueChangelog, IssueChangelogRequest,
    IssueCommentsPage, IssueCommentsPageRequest, IssueDetailRequest, IssueFetchRequest,
    IssueLocator, IssuePage, IssueTransition, IssueTransitionsRequest, JiraCommentWritePort,
    JiraIssueEditPort, JiraReadPort, MAX_ASSIGNABLE_USER_SEARCH_LIMIT, PageCursor, PortFuture,
    RecentIssueCommentsRequest, TransitionIssueRequest, UserSearchRequest,
};
use jira_domain::{Issue, IssueComment, IssueId, JiraSiteId, Status, User};
use reqwest::{Client, Response, StatusCode, header};
use serde::{Serialize, de::DeserializeOwned};
use tokio::{runtime::Builder, sync::oneshot};
use url::Url;

const DEFAULT_USER_AGENT: &str = "jira-gpui/0.1 (Jira Cloud client)";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ISSUE_ID_PAGES: usize = 128;
const MAX_CHANGELOG_PAGES: usize = 8;

struct AttachmentReadOptions {
    attachment_id: String,
    cancellation: CancellationToken,
    max_bytes: usize,
    width: usize,
    height: usize,
    thumbnail: bool,
}

enum AttachmentMimeResolution {
    Declared(String),
    InferFromBody,
}

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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct JiraTransitionsResponse {
    transitions: Vec<JiraTransitionResponse>,
}

#[derive(Debug, serde::Deserialize)]
struct JiraTransitionResponse {
    id: String,
    name: String,
    to: JiraTransitionStatusResponse,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct JiraTransitionStatusResponse {
    id: String,
    name: String,
    #[serde(default)]
    status_category: Option<JiraStatusCategoryResponse>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct JiraStatusCategoryResponse {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    key: Option<String>,
}

#[derive(Debug, Serialize)]
struct JiraAssigneeRequest {
    #[serde(rename = "accountId")]
    account_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct JiraTransitionRequest {
    transition: JiraTransitionIdRequest,
}

#[derive(Debug, Serialize)]
struct JiraTransitionIdRequest {
    id: String,
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
    /// Independent hard cap for Jira image thumbnails.
    pub attachment_image_max_bytes: usize,
    /// Independent hard cap for explicit original attachment downloads.
    pub attachment_download_max_bytes: usize,
    pub user_agent: String,
}

impl Default for JiraHttpConfig {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            attachment_image_max_bytes: DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES,
            attachment_download_max_bytes: DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES,
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
        if self.attachment_image_max_bytes == 0
            || self.attachment_image_max_bytes > DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES
            || self.attachment_download_max_bytes == 0
            || self.attachment_download_max_bytes > DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES
        {
            return Err(ConfigError::InvalidAttachmentLimit);
        }
        if self.user_agent.trim().is_empty() {
            return Err(ConfigError::EmptyUserAgent);
        }
        Ok(())
    }
}

/// Jira Cloud client implementing the application read and explicit-write boundaries.
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

    fn attachment_endpoint(
        &self,
        path: &str,
        attachment_id: &str,
    ) -> Result<Url, ApplicationError> {
        let mut url = self.endpoint(path)?;
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                ApplicationError::new(ErrorKind::Internal, "invalid Jira attachment endpoint")
            })?;
            segments.push(attachment_id);
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

    async fn search_assignable_users_request(
        client: Client,
        mut url: Url,
        credentials: ApiTokenCredentials,
        request: AssignableUserSearchRequest,
        max_response_bytes: usize,
    ) -> Result<Vec<User>, ApplicationError> {
        let limit = validate_user_limit(request.limit)?;
        validate_user_query(&request.query)?;
        url.query_pairs_mut().append_pair("query", &request.query);
        append_issue_locator_query(&mut url, &request.locator)?;
        url.query_pairs_mut()
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
                            "Jira returned invalid assignable user data",
                        )
                    })
            })
            .collect()
    }

    async fn fetch_issue_transitions_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        max_response_bytes: usize,
    ) -> Result<Vec<IssueTransition>, ApplicationError> {
        let response = client
            .get(url)
            .basic_auth(credentials.email, Some(credentials.token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        let payload: JiraTransitionsResponse = read_json(response, max_response_bytes).await?;
        payload
            .transitions
            .into_iter()
            .map(map_transition)
            .collect()
    }

    async fn assign_issue_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        assignee: Option<jira_domain::AccountId>,
    ) -> Result<(), ApplicationError> {
        let body = JiraAssigneeRequest {
            account_id: assignee.map(jira_domain::AccountId::into_inner),
        };
        let response = Self::assign_issue_request_builder(&client, url, &credentials, body)
            .send()
            .await
            .map_err(write_transport_error)?;
        read_write_response(response).await
    }

    fn assign_issue_request_builder(
        client: &Client,
        url: Url,
        credentials: &ApiTokenCredentials,
        body: JiraAssigneeRequest,
    ) -> reqwest::RequestBuilder {
        client
            .put(url)
            .basic_auth(&credentials.email, Some(&credentials.token))
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
    }

    async fn transition_issue_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        transition_id: String,
    ) -> Result<(), ApplicationError> {
        let body = JiraTransitionRequest {
            transition: JiraTransitionIdRequest { id: transition_id },
        };
        let response = Self::transition_issue_request_builder(&client, url, &credentials, body)
            .send()
            .await
            .map_err(write_transport_error)?;
        read_write_response(response).await
    }

    fn transition_issue_request_builder(
        client: &Client,
        url: Url,
        credentials: &ApiTokenCredentials,
        body: JiraTransitionRequest,
    ) -> reqwest::RequestBuilder {
        client
            .post(url)
            .basic_auth(&credentials.email, Some(&credentials.token))
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
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

    async fn issue_changelog_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        request: IssueChangelogRequest,
        cancellation: CancellationToken,
        max_response_bytes: usize,
    ) -> Result<Vec<IssueChangelog>, ApplicationError> {
        let mut next_page_token = None;
        let mut changelogs = Vec::new();
        for _ in 0..MAX_CHANGELOG_PAGES {
            cancellation.check()?;
            let body = jira_adapter::bulk_changelog_request(&request, next_page_token.clone())
                .map_err(|_| ApplicationError::invalid_input("invalid Jira changelog request"))?;
            let response = client
                .post(url.clone())
                .basic_auth(&credentials.email, Some(&credentials.token))
                .header(header::ACCEPT, "application/json")
                .header(header::CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .await
                .map_err(transport_error)?;
            let payload: JiraBulkChangelogResponse =
                read_json(response, max_response_bytes).await?;
            let mapped = IssueMapper.map_changelog_page(payload).map_err(|_| {
                ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid changelog data")
            })?;
            changelogs.extend(mapped.changelogs);
            let Some(token) = mapped.next_page_token else {
                return Ok(changelogs);
            };
            if token.trim().is_empty() || next_page_token.as_deref() == Some(token.as_str()) {
                return Err(ApplicationError::new(
                    ErrorKind::Upstream,
                    "Jira changelog pagination did not advance",
                ));
            }
            next_page_token = Some(token);
        }
        Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira changelog pagination exceeded the safety limit",
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
            .append_pair("orderBy", "-created");
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

    async fn recent_issue_comments_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        request: RecentIssueCommentsRequest,
        max_response_bytes: usize,
    ) -> Result<Vec<IssueComment>, ApplicationError> {
        let url = recent_issue_comments_url(url, request.limit)?;
        let response = client
            .get(url)
            .basic_auth(credentials.email, Some(credentials.token))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        let page: JiraCommentPage = read_json(response, max_response_bytes).await?;
        IssueMapper
            .map_comment_page(page)
            .map(|page| page.comments)
            .map_err(|_| {
                ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid comment data")
            })
    }

    fn attachment_request_builder(
        client: &Client,
        url: Url,
        credentials: &ApiTokenCredentials,
    ) -> reqwest::RequestBuilder {
        client
            .get(url)
            .basic_auth(&credentials.email, Some(&credentials.token))
            .header(header::ACCEPT, "*/*")
    }

    async fn attachment_image_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        options: AttachmentReadOptions,
    ) -> Result<AttachmentContent, ApplicationError> {
        if options.max_bytes == 0 {
            return Err(ApplicationError::invalid_input(
                "attachment response limit must be positive",
            ));
        }
        let url = attachment_url_with_query(url, options.width, options.height, options.thumbnail);

        options.cancellation.check()?;
        let response = Self::attachment_request_builder(&client, url, &credentials)
            .send()
            .await
            .map_err(|error| attachment_transport_error(error, attachment_attempt(&options)))?;
        let attempt = attachment_attempt(&options);
        let status = response.status();
        if !status.is_success() {
            return Err(attachment_status_error(status, response.headers(), attempt));
        }
        let mime_type = if options.thumbnail {
            attachment_thumbnail_mime_type(response.headers(), attempt)?
        } else {
            AttachmentMimeResolution::Declared(attachment_mime_type(
                response.headers(),
                attempt,
                false,
            )?)
        };
        if response
            .content_length()
            .is_some_and(|length| length > options.max_bytes as u64)
        {
            return Err(attachment_body_error(
                attempt,
                AttachmentBodyClass::TooLarge,
            ));
        }

        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            ApplicationError::new(ErrorKind::Offline, "could not read Jira attachment")
                .with_attachment_diagnostic(AttachmentReadDiagnostic::body(
                    attempt,
                    AttachmentBodyClass::ReadFailed,
                ))
        })? {
            options.cancellation.check()?;
            if body.len().saturating_add(chunk.len()) > options.max_bytes {
                return Err(attachment_body_error(
                    attempt,
                    AttachmentBodyClass::TooLarge,
                ));
            }
            body.extend_from_slice(&chunk);
        }
        options.cancellation.check()?;
        if body.is_empty() {
            return Err(attachment_body_error(attempt, AttachmentBodyClass::Empty));
        }
        let body = finish_attachment_body(body, options.max_bytes, &options.cancellation)?;
        let mime_type = match mime_type {
            AttachmentMimeResolution::Declared(mime_type) => mime_type,
            AttachmentMimeResolution::InferFromBody => image_mime_from_signature(&body)
                .map(str::to_owned)
                .ok_or_else(|| attachment_signature_error(attempt))?,
        };
        Ok(AttachmentContent {
            attachment_id: options.attachment_id,
            mime_type,
            bytes: body,
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
    fn fetch_issue_changelog<'a>(
        &'a self,
        request: &'a IssueChangelogRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<IssueChangelog>> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if request.issue_ids.is_empty() || request.issue_ids.len() > jira_adapter::MAX_ISSUE_IDS {
            return Box::pin(std::future::ready(Err(ApplicationError::invalid_input(
                "Jira changelog issue count is invalid",
            ))));
        }
        let url = match self.endpoint("rest/api/3/changelog/bulkfetch") {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let request = request.clone();
        let cancellation_for_request = cancellation.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::issue_changelog_request(
                client,
                url,
                credentials,
                request,
                cancellation_for_request,
                max,
            )
            .await
        })
    }

    fn fetch_attachment_image<'a>(
        &'a self,
        request: &'a AttachmentImageRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentImage> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if request.attachment_id.trim().is_empty() || request.attachment_id.len() > 255 {
            return Box::pin(std::future::ready(Err(ApplicationError::invalid_input(
                "attachment ID is invalid",
            ))));
        }
        if request.max_bytes == 0
            || request.max_bytes > DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES
            || request.width == 0
            || request.width > DEFAULT_ATTACHMENT_IMAGE_WIDTH
            || request.height == 0
            || request.height > DEFAULT_ATTACHMENT_IMAGE_HEIGHT
        {
            return Box::pin(std::future::ready(Err(ApplicationError::invalid_input(
                "attachment thumbnail bounds are invalid",
            ))));
        }
        let url = match self
            .attachment_endpoint("rest/api/3/attachment/thumbnail", &request.attachment_id)
        {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let attachment_id = request.attachment_id.clone();
        let cancellation_for_request = cancellation.clone();
        let max = self
            .config
            .attachment_image_max_bytes
            .min(request.max_bytes);
        let width = request.width;
        let height = request.height;
        self.submit(cancellation, async move {
            let content = Self::attachment_image_request(
                client,
                url,
                credentials,
                AttachmentReadOptions {
                    attachment_id,
                    cancellation: cancellation_for_request,
                    max_bytes: max,
                    width,
                    height,
                    thumbnail: true,
                },
            )
            .await?;
            Ok(AttachmentImage {
                attachment_id: content.attachment_id,
                mime_type: content.mime_type,
                bytes: content.bytes,
            })
        })
    }

    fn fetch_attachment_content<'a>(
        &'a self,
        request: &'a AttachmentDownloadRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, AttachmentContent> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if request.attachment_id.trim().is_empty() || request.attachment_id.len() > 255 {
            return Box::pin(std::future::ready(Err(ApplicationError::invalid_input(
                "attachment ID is invalid",
            ))));
        }
        if request.max_bytes == 0 || request.max_bytes > DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES {
            return Box::pin(std::future::ready(Err(ApplicationError::invalid_input(
                "attachment download limit is invalid",
            ))));
        }
        let url = match self
            .attachment_endpoint("rest/api/3/attachment/content", &request.attachment_id)
        {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let attachment_id = request.attachment_id.clone();
        let cancellation_for_request = cancellation.clone();
        let max = self
            .config
            .attachment_download_max_bytes
            .min(request.max_bytes);
        self.submit(cancellation, async move {
            Self::attachment_image_request(
                client,
                url,
                credentials,
                AttachmentReadOptions {
                    attachment_id,
                    cancellation: cancellation_for_request,
                    max_bytes: max,
                    width: 0,
                    height: 0,
                    thumbnail: false,
                },
            )
            .await
        })
    }

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

    fn fetch_recent_issue_comments<'a>(
        &'a self,
        request: &'a RecentIssueCommentsRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<IssueComment>> {
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
            Self::recent_issue_comments_request(client, url, credentials, request, max).await
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

impl JiraIssueEditPort for JiraHttpClient {
    fn search_assignable_users<'a>(
        &'a self,
        request: &'a AssignableUserSearchRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<User>> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = validate_user_limit(request.limit) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = validate_user_query(&request.query) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = validate_issue_locator(&request.locator) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.endpoint("rest/api/3/user/assignable/search") {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let request = request.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::search_assignable_users_request(client, url, credentials, request, max).await
        })
    }

    fn fetch_issue_transitions<'a>(
        &'a self,
        request: &'a IssueTransitionsRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<IssueTransition>> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = validate_issue_locator(&request.locator) {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.issue_endpoint(&request.locator, Some("transitions")) {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let max = self.config.max_response_bytes;
        self.submit(cancellation, async move {
            Self::fetch_issue_transitions_request(client, url, credentials, max).await
        })
    }

    fn assign_issue<'a>(
        &'a self,
        request: &'a AssignIssueRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, ()> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = validate_issue_locator(&request.locator) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Some(account_id) = request.assignee.as_ref()
            && let Err(error) = validate_string_id(account_id.as_str(), "assignee account ID")
        {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.issue_endpoint(&request.locator, Some("assignee")) {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let assignee = request.assignee.clone();
        self.submit_write(cancellation, async move {
            Self::assign_issue_request(client, url, credentials, assignee).await
        })
    }

    fn transition_issue<'a>(
        &'a self,
        request: &'a TransitionIssueRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, ()> {
        if let Err(error) = cancellation.check() {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = self.validate_site(&request.site_id) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = validate_issue_locator(&request.locator) {
            return Box::pin(std::future::ready(Err(error)));
        }
        if let Err(error) = validate_string_id(&request.transition_id, "transition ID") {
            return Box::pin(std::future::ready(Err(error)));
        }
        let url = match self.issue_endpoint(&request.locator, Some("transitions")) {
            Ok(url) => url,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let client = self.client.clone();
        let credentials = self.credentials.clone();
        let transition_id = request.transition_id.clone();
        self.submit_write(cancellation, async move {
            Self::transition_issue_request(client, url, credentials, transition_id).await
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
    #[error("Jira attachment limits are invalid")]
    InvalidAttachmentLimit,
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

fn validate_string_id(value: &str, field: &'static str) -> Result<(), ApplicationError> {
    if value.trim().is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(ApplicationError::invalid_input(format!(
            "{field} is invalid"
        )));
    }
    Ok(())
}

fn validate_issue_locator(locator: &IssueLocator) -> Result<(), ApplicationError> {
    let value = match locator {
        IssueLocator::Id(issue_id) => issue_id.as_str(),
        IssueLocator::Key(issue_key) => issue_key.as_str(),
    };
    validate_string_id(value, "issue locator")
}

fn validate_user_limit(limit: usize) -> Result<usize, ApplicationError> {
    if limit == 0 || limit > MAX_ASSIGNABLE_USER_SEARCH_LIMIT {
        return Err(ApplicationError::invalid_input(
            "assignable user search limit is invalid",
        ));
    }
    Ok(limit)
}

fn validate_user_query(query: &str) -> Result<(), ApplicationError> {
    if query.chars().count() > 255 || query.chars().any(char::is_control) {
        return Err(ApplicationError::invalid_input(
            "assignable user search query is invalid",
        ));
    }
    Ok(())
}

fn append_issue_locator_query(
    url: &mut Url,
    locator: &IssueLocator,
) -> Result<(), ApplicationError> {
    validate_issue_locator(locator)?;
    match locator {
        IssueLocator::Id(issue_id) => {
            url.query_pairs_mut()
                .append_pair("issueId", issue_id.as_str());
        }
        IssueLocator::Key(issue_key) => {
            url.query_pairs_mut()
                .append_pair("issueKey", issue_key.as_str());
        }
    }
    Ok(())
}

fn map_transition(transition: JiraTransitionResponse) -> Result<IssueTransition, ApplicationError> {
    validate_string_id(&transition.id, "transition ID")
        .map_err(|_| invalid_transition_response())?;
    validate_string_id(&transition.name, "transition name")
        .map_err(|_| invalid_transition_response())?;
    validate_string_id(&transition.to.id, "transition status ID")
        .map_err(|_| invalid_transition_response())?;
    validate_string_id(&transition.to.name, "transition status name")
        .map_err(|_| invalid_transition_response())?;
    let category = transition.to.status_category.and_then(|category| {
        category
            .name
            .filter(|value| !value.trim().is_empty())
            .or(category.key.filter(|value| !value.trim().is_empty()))
    });
    if category
        .as_ref()
        .is_some_and(|value| value.chars().count() > 255 || value.chars().any(char::is_control))
    {
        return Err(invalid_transition_response());
    }
    Ok(IssueTransition {
        id: transition.id,
        name: transition.name,
        to: Status {
            id: transition.to.id,
            name: transition.to.name,
            category,
        },
    })
}

fn invalid_transition_response() -> ApplicationError {
    ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid transition data")
}

async fn read_write_response(response: Response) -> Result<(), ApplicationError> {
    let status = response.status();
    if status == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err(write_status_error(status, response.headers()))
    }
}

fn transport_error(error: reqwest::Error) -> ApplicationError {
    if error.is_timeout() || error.is_connect() {
        ApplicationError::new(ErrorKind::Offline, "could not connect to Jira")
    } else {
        ApplicationError::new(ErrorKind::Upstream, "Jira request failed")
    }
}

fn attachment_attempt(options: &AttachmentReadOptions) -> AttachmentReadAttempt {
    if options.thumbnail {
        AttachmentReadAttempt::Thumbnail
    } else {
        AttachmentReadAttempt::ExplicitDownload
    }
}

fn attachment_transport_error(
    error: reqwest::Error,
    attempt: AttachmentReadAttempt,
) -> ApplicationError {
    let transport_class = if error.is_timeout() {
        AttachmentTransportClass::TimedOut
    } else if error.is_connect() {
        AttachmentTransportClass::ConnectFailed
    } else {
        AttachmentTransportClass::RequestFailed
    };
    transport_error(error).with_attachment_diagnostic(AttachmentReadDiagnostic::transport(
        attempt,
        transport_class,
    ))
}

fn attachment_status_error(
    status: StatusCode,
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
) -> ApplicationError {
    status_error(status, headers)
        .with_attachment_diagnostic(AttachmentReadDiagnostic::status(attempt, status.as_u16()))
}

fn attachment_body_error(
    attempt: AttachmentReadAttempt,
    body_class: AttachmentBodyClass,
) -> ApplicationError {
    ApplicationError::new(
        ErrorKind::Upstream,
        match body_class {
            AttachmentBodyClass::Empty => "Jira returned an empty attachment",
            AttachmentBodyClass::TooLarge => "Jira attachment exceeded the size limit",
            AttachmentBodyClass::ReadFailed => "could not read Jira attachment",
        },
    )
    .with_attachment_diagnostic(AttachmentReadDiagnostic::body(attempt, body_class))
}

fn attachment_mime_type(
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
    thumbnail: bool,
) -> Result<String, ApplicationError> {
    let mime_type = parsed_attachment_mime_type(headers, attempt)?;
    if thumbnail && !is_allowed_image_mime(&mime_type) {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response was not an image",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Other,
        )));
    }
    Ok(mime_type)
}

fn attachment_thumbnail_mime_type(
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
) -> Result<AttachmentMimeResolution, ApplicationError> {
    let mime_type = parsed_attachment_mime_type(headers, attempt)?;
    if is_allowed_image_mime(&mime_type) {
        Ok(AttachmentMimeResolution::Declared(mime_type))
    } else {
        Ok(AttachmentMimeResolution::InferFromBody)
    }
}

fn parsed_attachment_mime_type(
    headers: &header::HeaderMap,
    attempt: AttachmentReadAttempt,
) -> Result<String, ApplicationError> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response had an invalid media type",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Missing,
        )));
    };
    let raw_value = value.to_str().map_err(|_| {
        ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response had an invalid media type",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Malformed,
        ))
    })?;
    let mime_type = media_type(raw_value).ok_or_else(|| {
        ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment response had an invalid media type",
        )
        .with_attachment_diagnostic(AttachmentReadDiagnostic::content_type(
            attempt,
            AttachmentMimeClass::Malformed,
        ))
    })?;
    Ok(mime_type)
}

fn attachment_signature_error(attempt: AttachmentReadAttempt) -> ApplicationError {
    ApplicationError::new(
        ErrorKind::NotFound,
        "Jira attachment response bytes did not match an image format",
    )
    .with_attachment_diagnostic(AttachmentReadDiagnostic::validation(attempt))
}

fn image_mime_from_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn media_type(value: &str) -> Option<String> {
    let media_type = value.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, subtype) = media_type.split_once('/')?;
    if kind.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !kind.bytes().all(is_media_type_token)
        || !subtype.bytes().all(is_media_type_token)
    {
        return None;
    }
    Some(media_type)
}

fn is_media_type_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn attachment_url_with_query(mut url: Url, width: usize, height: usize, thumbnail: bool) -> Url {
    if thumbnail {
        url.query_pairs_mut()
            .append_pair("redirect", "false")
            .append_pair("width", &width.to_string())
            .append_pair("height", &height.to_string())
            .append_pair("fallbackToDefault", "false");
    } else {
        url.query_pairs_mut().append_pair("redirect", "false");
    }
    url
}

fn is_allowed_image_mime(value: &str) -> bool {
    matches!(
        value,
        "application/octet-stream"
            | "image/gif"
            | "image/jpg"
            | "image/jpeg"
            | "image/png"
            | "image/webp"
    )
}

fn finish_attachment_body(
    body: Vec<u8>,
    max_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, ApplicationError> {
    cancellation.check()?;
    if body.is_empty() {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira returned an empty attachment",
        ));
    }
    if body.len() > max_bytes {
        return Err(ApplicationError::new(
            ErrorKind::Upstream,
            "Jira attachment exceeded the size limit",
        ));
    }
    Ok(body)
}

fn comment_transport_error(error: reqwest::Error) -> ApplicationError {
    write_transport_error(error)
}

fn write_dispatch_error(_error: ApplicationError) -> ApplicationError {
    write_unknown_outcome()
}

fn comment_status_error(status: StatusCode, headers: &header::HeaderMap) -> ApplicationError {
    write_status_error(status, headers)
}

fn write_transport_error(error: reqwest::Error) -> ApplicationError {
    if error.is_connect() && !error.is_timeout() {
        ApplicationError::new(ErrorKind::Offline, "could not connect to Jira")
    } else {
        write_unknown_outcome()
    }
}

fn write_unknown_outcome() -> ApplicationError {
    ApplicationError::new(ErrorKind::UnknownOutcome, "Jira write outcome is unknown")
}

fn write_status_error(status: StatusCode, headers: &header::HeaderMap) -> ApplicationError {
    match status {
        StatusCode::BAD_REQUEST
        | StatusCode::PAYLOAD_TOO_LARGE
        | StatusCode::UNPROCESSABLE_ENTITY => {
            ApplicationError::invalid_input("Jira rejected the write request")
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
        StatusCode::CONFLICT => ApplicationError::new(
            ErrorKind::Upstream,
            "Jira rejected the write due to a conflict",
        ),
        StatusCode::TOO_MANY_REQUESTS => {
            ApplicationError::rate_limited("Jira rate limit exceeded", retry_after(headers))
        }
        _ => write_unknown_outcome(),
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
        return Err(write_unknown_outcome());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| write_unknown_outcome())?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(write_unknown_outcome());
        }
        body.extend_from_slice(&chunk);
    }
    map_created_comment_body(&body)
}

fn map_created_comment_body(body: &[u8]) -> Result<IssueComment, ApplicationError> {
    let comment: jira_adapter::JiraComment =
        serde_json::from_slice(body).map_err(|_| write_unknown_outcome())?;
    IssueMapper
        .map_comment(comment)
        .map_err(|_| write_unknown_outcome())
}

fn recent_issue_comments_url(
    mut url: Url,
    requested_limit: usize,
) -> Result<Url, ApplicationError> {
    let limit = requested_limit.min(100);
    if limit == 0 {
        return Err(ApplicationError::invalid_input(
            "recent comment limit must be positive",
        ));
    }
    url.query_pairs_mut()
        .append_pair("startAt", "0")
        .append_pair("maxResults", &limit.to_string())
        .append_pair("orderBy", "-created");
    Ok(url)
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
            .append_pair("orderBy", "-created");
        assert_eq!(
            comments.path(),
            "/rest/api/3/issue/ENG%2F42%3Fprivate/comment"
        );
        assert_eq!(
            comments.query(),
            Some("startAt=20&maxResults=50&orderBy=-created")
        );
    }

    #[test]
    fn recent_comment_url_requests_newest_comments_with_a_bounded_limit() {
        let url = recent_issue_comments_url(
            Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42/comment").unwrap(),
            250,
        )
        .unwrap();
        assert_eq!(
            url.query(),
            Some("startAt=0&maxResults=100&orderBy=-created")
        );

        let error = recent_issue_comments_url(
            Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42/comment").unwrap(),
            0,
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
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
    fn assignable_user_query_uses_typed_locator_params_and_encodes_untrusted_values() {
        let mut url =
            Url::parse("https://example.atlassian.net/rest/api/3/user/assignable/search").unwrap();
        let locator = IssueLocator::Id(IssueId::new("ENG/42?private#fragment").unwrap());
        url.query_pairs_mut()
            .append_pair("query", "ada+lovelace & admin");
        append_issue_locator_query(&mut url, &locator).unwrap();
        url.query_pairs_mut().append_pair("maxResults", "25");

        assert_eq!(
            url.query(),
            Some(
                "query=ada%2Blovelace+%26+admin&issueId=ENG%2F42%3Fprivate%23fragment&maxResults=25"
            )
        );

        let mut key_url =
            Url::parse("https://example.atlassian.net/rest/api/3/user/assignable/search").unwrap();
        append_issue_locator_query(
            &mut key_url,
            &IssueLocator::Key(jira_domain::IssueKey::new("ENG-42").unwrap()),
        )
        .unwrap();
        assert_eq!(key_url.query(), Some("issueKey=ENG-42"));
    }

    #[test]
    fn issue_edit_request_builders_use_expected_methods_headers_and_json_shapes() {
        let credentials = ApiTokenCredentials::new("person@example.com", "secret-token").unwrap();
        let assign_url =
            Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42/assignee").unwrap();
        let assigned = JiraHttpClient::assign_issue_request_builder(
            &Client::new(),
            assign_url,
            &credentials,
            JiraAssigneeRequest {
                account_id: Some("557058:abc-123".to_owned()),
            },
        )
        .build()
        .unwrap();
        assert_eq!(assigned.method(), reqwest::Method::PUT);
        assert_eq!(
            assigned.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                assigned.body().and_then(reqwest::Body::as_bytes).unwrap(),
            )
            .unwrap(),
            serde_json::json!({"accountId": "557058:abc-123"})
        );

        let unassigned = JiraHttpClient::assign_issue_request_builder(
            &Client::new(),
            Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42/assignee").unwrap(),
            &credentials,
            JiraAssigneeRequest { account_id: None },
        )
        .build()
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                unassigned.body().and_then(reqwest::Body::as_bytes).unwrap(),
            )
            .unwrap(),
            serde_json::json!({"accountId": null})
        );

        let transitioned = JiraHttpClient::transition_issue_request_builder(
            &Client::new(),
            Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42/transitions")
                .unwrap(),
            &credentials,
            JiraTransitionRequest {
                transition: JiraTransitionIdRequest {
                    id: "31".to_owned(),
                },
            },
        )
        .build()
        .unwrap();
        assert_eq!(transitioned.method(), reqwest::Method::POST);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                transitioned
                    .body()
                    .and_then(reqwest::Body::as_bytes)
                    .unwrap(),
            )
            .unwrap(),
            serde_json::json!({"transition": {"id": "31"}})
        );
    }

    #[test]
    fn transition_response_maps_to_transport_neutral_status_and_rejects_invalid_values() {
        let payload: JiraTransitionsResponse = serde_json::from_value(serde_json::json!({
            "transitions": [{
                "id": "31",
                "name": "In progress",
                "to": {
                    "id": "3",
                    "name": "In Progress",
                    "statusCategory": {"key": "indeterminate"}
                }
            }]
        }))
        .unwrap();
        let transitions = payload
            .transitions
            .into_iter()
            .map(map_transition)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(transitions[0].id, "31");
        assert_eq!(transitions[0].name, "In progress");
        assert_eq!(transitions[0].to.id, "3");
        assert_eq!(transitions[0].to.name, "In Progress");
        assert_eq!(transitions[0].to.category.as_deref(), Some("indeterminate"));

        let invalid = map_transition(JiraTransitionResponse {
            id: String::new(),
            name: "In progress".to_owned(),
            to: JiraTransitionStatusResponse {
                id: "3".to_owned(),
                name: "In Progress".to_owned(),
                status_category: None,
            },
        })
        .unwrap_err();
        assert_eq!(invalid.kind(), ErrorKind::Upstream);
        assert_eq!(invalid.message(), "Jira returned invalid transition data");

        let invalid_category = map_transition(JiraTransitionResponse {
            id: "31".to_owned(),
            name: "In progress".to_owned(),
            to: JiraTransitionStatusResponse {
                id: "3".to_owned(),
                name: "In Progress".to_owned(),
                status_category: Some(JiraStatusCategoryResponse {
                    name: Some("bad\ncategory".to_owned()),
                    key: Some("indeterminate".to_owned()),
                }),
            },
        })
        .unwrap_err();
        assert_eq!(invalid_category.kind(), ErrorKind::Upstream);
    }

    #[test]
    fn write_statuses_have_definite_safe_categories_and_unexpected_statuses_are_unknown() {
        let headers = header::HeaderMap::new();
        for (status, kind) in [
            (StatusCode::BAD_REQUEST, ErrorKind::InvalidInput),
            (StatusCode::UNAUTHORIZED, ErrorKind::Authentication),
            (StatusCode::FORBIDDEN, ErrorKind::Authorization),
            (StatusCode::NOT_FOUND, ErrorKind::NotFound),
            (StatusCode::CONFLICT, ErrorKind::Upstream),
            (StatusCode::PAYLOAD_TOO_LARGE, ErrorKind::InvalidInput),
            (StatusCode::UNPROCESSABLE_ENTITY, ErrorKind::InvalidInput),
            (StatusCode::TOO_MANY_REQUESTS, ErrorKind::RateLimited),
        ] {
            assert_eq!(write_status_error(status, &headers).kind(), kind);
        }
        for status in [
            StatusCode::OK,
            StatusCode::CREATED,
            StatusCode::FOUND,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert_eq!(
                write_status_error(status, &headers).kind(),
                ErrorKind::UnknownOutcome
            );
        }
    }

    #[test]
    fn assignment_dispatches_one_http_request_without_retrying_an_unknown_result() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let responder = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::io::AsyncWriteExt::write_all(
                    &mut stream,
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            });
            let credentials = ApiTokenCredentials::new("person@example.com", "token").unwrap();
            let account_id = jira_domain::AccountId::new("557058:abc-123").unwrap();
            let error = JiraHttpClient::assign_issue_request(
                Client::new(),
                Url::parse(&format!("http://{address}/rest/api/3/issue/ENG-42/assignee"))
                    .unwrap(),
                credentials,
                Some(account_id),
            )
            .await
            .unwrap_err();
            assert!(responder.await.unwrap(), "assignment was retried");
            assert_eq!(error.kind(), ErrorKind::UnknownOutcome);
        });
    }

    #[test]
    fn cancelled_issue_edits_are_rejected_before_dispatch() {
        let site = JiraSiteId::new("configured-site").unwrap();
        let client = JiraHttpClient::new(
            site.clone(),
            "https://example.atlassian.net",
            ApiTokenCredentials::new("person@example.com", "secret-token").unwrap(),
        )
        .unwrap();
        let locator = IssueLocator::Key(jira_domain::IssueKey::new("ENG-42").unwrap());
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let assign = AssignIssueRequest {
            site_id: site.clone(),
            locator: locator.clone(),
            assignee: None,
        };
        let transition = TransitionIssueRequest {
            site_id: site.clone(),
            locator: locator.clone(),
            transition_id: "31".to_owned(),
        };
        let search = AssignableUserSearchRequest {
            site_id: site,
            locator,
            query: String::new(),
            limit: 25,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            runtime
                .block_on(JiraIssueEditPort::assign_issue(
                    &client,
                    &assign,
                    &cancellation,
                ))
                .unwrap_err()
                .kind(),
            ErrorKind::Cancelled
        );
        assert_eq!(
            runtime
                .block_on(JiraIssueEditPort::transition_issue(
                    &client,
                    &transition,
                    &cancellation,
                ))
                .unwrap_err()
                .kind(),
            ErrorKind::Cancelled
        );
        assert_eq!(
            runtime
                .block_on(JiraIssueEditPort::search_assignable_users(
                    &client,
                    &search,
                    &cancellation,
                ))
                .unwrap_err()
                .kind(),
            ErrorKind::Cancelled
        );
    }

    #[test]
    fn bulk_changelog_paginates_and_maps_documented_numeric_timestamps() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("listener");
            let address = listener.local_addr().expect("listener address");
            let responder = tokio::spawn(async move {
                for (body, token) in [
                    (r#"{"issueChangeLogs":[{"issueId":"10001","changeHistories":[{"id":"h1","created":1786876200,"items":[{"field":"Labels","fromString":"old","toString":"new"}]}]}],"nextPageToken":"page-2"}"#, true),
                    (r#"{"issueChangeLogs":[],"nextPageToken":null}"#, false),
                ] {
                    let (mut stream, _) = listener.accept().await.expect("request");
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    );
                    tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                        .await
                        .expect("response");
                    assert_eq!(token, body.contains("page-2"));
                }
            });
            let request = IssueChangelogRequest {
                site_id: JiraSiteId::new("site-a").expect("site"),
                issue_ids: vec![jira_domain::IssueId::new("10001").expect("issue")],
            };
            let logs = JiraHttpClient::issue_changelog_request(
                Client::new(),
                Url::parse(&format!("http://{address}/rest/api/3/changelog/bulkfetch"))
                    .expect("url"),
                ApiTokenCredentials::new("person@example.com", "token").expect("credentials"),
                request,
                CancellationToken::new(),
                1_048_576,
            )
            .await
            .expect("changelog response");
            responder.await.expect("responder");
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].histories[0].created.unix_timestamp(), 1786876200);
        });
    }

    #[test]
    fn cancelled_or_unbounded_bulk_changelog_reads_stop_safely() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let request = IssueChangelogRequest {
                site_id: JiraSiteId::new("site-a").expect("site"),
                issue_ids: vec![jira_domain::IssueId::new("10001").expect("issue")],
            };
            let cancellation = CancellationToken::new();
            cancellation.cancel();
            let error = JiraHttpClient::issue_changelog_request(
                Client::new(),
                Url::parse("http://127.0.0.1:1/rest/api/3/changelog/bulkfetch").expect("url"),
                ApiTokenCredentials::new("person@example.com", "token").expect("credentials"),
                request.clone(),
                cancellation,
                1_048_576,
            )
            .await
            .expect_err("cancelled read");
            assert_eq!(error.kind(), ErrorKind::Cancelled);
            assert!(MAX_CHANGELOG_PAGES > 0);

            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("listener");
            let address = listener.local_addr().expect("listener address");
            let responder = tokio::spawn(async move {
                for index in 0..MAX_CHANGELOG_PAGES {
                    let (mut stream, _) = listener.accept().await.expect("request");
                    let body = format!(
                        r#"{{"issueChangeLogs":[],"nextPageToken":"next-{index}"}}"#
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    );
                    tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                        .await
                        .expect("response");
                }
            });
            let error = JiraHttpClient::issue_changelog_request(
                Client::new(),
                Url::parse(&format!("http://{address}/rest/api/3/changelog/bulkfetch"))
                    .expect("url"),
                ApiTokenCredentials::new("person@example.com", "token").expect("credentials"),
                request,
                CancellationToken::new(),
                1_048_576,
            )
            .await
            .expect_err("pagination safety cap");
            responder.await.expect("responder");
            assert!(error.message().contains("safety limit"));
        });
    }

    #[test]
    fn attachment_thumbnail_and_content_requests_are_authenticated_and_pinned_to_jira_api() {
        let configured = JiraSiteId::new("configured-site").unwrap();
        let client = JiraHttpClient {
            site_id: configured,
            base_url: JiraBaseUrl::parse("https://example.atlassian.net").unwrap(),
            credentials: ApiTokenCredentials::new("person@example.com", "secret-token").unwrap(),
            client: Client::new(),
            runtime: Arc::new(RuntimeBridge::new().unwrap()),
            config: JiraHttpConfig::default(),
        };
        let mut thumbnail = client
            .attachment_endpoint("rest/api/3/attachment/thumbnail", "att/42?url=evil")
            .unwrap();
        thumbnail = attachment_url_with_query(thumbnail, 640, 480, true);
        let request = JiraHttpClient::attachment_request_builder(
            &client.client,
            thumbnail,
            &client.credentials,
        )
        .build()
        .unwrap();
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().path(),
            "/rest/api/3/attachment/thumbnail/att%2F42%3Furl=evil"
        );
        assert_eq!(
            request.url().query(),
            Some("redirect=false&width=640&height=480&fallbackToDefault=false")
        );
        assert_eq!(
            request.headers().get(header::AUTHORIZATION).unwrap(),
            "Basic cGVyc29uQGV4YW1wbGUuY29tOnNlY3JldC10b2tlbg=="
        );

        let content = client
            .attachment_endpoint("rest/api/3/attachment/content", "42")
            .unwrap();
        let mut content_request = JiraHttpClient::attachment_request_builder(
            &client.client,
            content,
            &client.credentials,
        )
        .build()
        .unwrap();
        content_request
            .url_mut()
            .query_pairs_mut()
            .append_pair("redirect", "false");
        assert_eq!(
            content_request.url().path(),
            "/rest/api/3/attachment/content/42"
        );
        assert_eq!(content_request.url().query(), Some("redirect=false"));
    }

    #[test]
    fn attachment_content_type_is_normalized_and_images_are_allowlisted() {
        assert_eq!(
            media_type(" IMAGE/PNG; charset=binary "),
            Some("image/png".to_owned())
        );
        assert_eq!(
            media_type("application/pdf"),
            Some("application/pdf".to_owned())
        );
        assert_eq!(media_type("missing"), None);
        assert_eq!(media_type("/"), None);
        assert_eq!(media_type("image/png/extra"), None);
        assert_eq!(media_type("image png"), None);
        assert!(is_allowed_image_mime("image/webp"));
        assert!(is_allowed_image_mime("application/octet-stream"));
        assert!(is_allowed_image_mime("image/jpg"));
        assert!(!is_allowed_image_mime("text/plain"));
        assert_eq!(
            status_error(StatusCode::FOUND, &header::HeaderMap::new()).kind(),
            ErrorKind::Upstream
        );
    }

    #[test]
    fn unknown_thumbnail_mimes_use_strict_image_signatures() {
        assert_eq!(
            image_mime_from_signature(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(
            image_mime_from_signature(b"\xff\xd8\xffrest"),
            Some("image/jpeg")
        );
        assert_eq!(image_mime_from_signature(b"GIF89arest"), Some("image/gif"));
        assert_eq!(
            image_mime_from_signature(b"RIFF\x00\x00\x00\x00WEBPrest"),
            Some("image/webp")
        );
        assert_eq!(image_mime_from_signature(b"RIFF\x00\x00\x00\x00PNG"), None);
        assert_eq!(image_mime_from_signature(b"not an image"), None);
    }

    #[test]
    fn attachment_status_diagnostic_preserves_exact_http_status() {
        for status in [StatusCode::FOUND, StatusCode::BAD_REQUEST] {
            let error = attachment_status_error(
                status,
                &header::HeaderMap::new(),
                AttachmentReadAttempt::Thumbnail,
            );
            assert_eq!(error.kind(), ErrorKind::Upstream);
            let diagnostic = error.attachment_diagnostic().expect("status diagnostic");
            assert_eq!(
                diagnostic.stage(),
                jira_application::AttachmentReadStage::Status
            );
            assert_eq!(diagnostic.attempt(), AttachmentReadAttempt::Thumbnail);
            assert_eq!(diagnostic.status_code(), Some(status.as_u16()));
        }
    }

    #[test]
    fn attachment_mime_diagnostics_use_only_safe_classes() {
        let missing = attachment_mime_type(
            &header::HeaderMap::new(),
            AttachmentReadAttempt::Thumbnail,
            true,
        )
        .expect_err("missing content type");
        assert_eq!(missing.kind(), ErrorKind::Upstream);
        assert_eq!(
            missing
                .attachment_diagnostic()
                .expect("missing MIME diagnostic")
                .mime_class(),
            Some(AttachmentMimeClass::Missing)
        );

        let mut unsupported_headers = header::HeaderMap::new();
        unsupported_headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("text/plain"),
        );
        let unsupported =
            attachment_mime_type(&unsupported_headers, AttachmentReadAttempt::Thumbnail, true)
                .expect_err("unsupported content type");
        assert_eq!(unsupported.kind(), ErrorKind::Upstream);
        assert_eq!(
            unsupported
                .attachment_diagnostic()
                .expect("unsupported MIME diagnostic")
                .mime_class(),
            Some(AttachmentMimeClass::Other)
        );
        assert!(!unsupported.message().contains("text/plain"));

        let mut malformed_headers = header::HeaderMap::new();
        malformed_headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_bytes(b"image/\xff").unwrap(),
        );
        let malformed =
            attachment_mime_type(&malformed_headers, AttachmentReadAttempt::Thumbnail, true)
                .expect_err("malformed content type");
        assert_eq!(malformed.kind(), ErrorKind::Upstream);
        assert_eq!(
            malformed
                .attachment_diagnostic()
                .expect("malformed MIME diagnostic")
                .mime_class(),
            Some(AttachmentMimeClass::Malformed)
        );
    }

    #[test]
    fn attachment_body_limits_reject_empty_and_oversized_responses_without_details() {
        let empty = finish_attachment_body(Vec::new(), 4, &CancellationToken::new()).unwrap_err();
        assert_eq!(empty.kind(), ErrorKind::Upstream);
        let oversized =
            finish_attachment_body(b"12345".to_vec(), 4, &CancellationToken::new()).unwrap_err();
        assert_eq!(oversized.kind(), ErrorKind::Upstream);
        assert!(!oversized.message().contains("12345"));
    }

    #[test]
    fn attachment_body_diagnostics_distinguish_empty_and_size_failures() {
        for (body_class, message) in [
            (
                AttachmentBodyClass::Empty,
                "Jira returned an empty attachment",
            ),
            (
                AttachmentBodyClass::TooLarge,
                "Jira attachment exceeded the size limit",
            ),
        ] {
            let error = attachment_body_error(AttachmentReadAttempt::ExplicitDownload, body_class);
            let diagnostic = error.attachment_diagnostic().expect("body diagnostic");
            assert_eq!(error.kind(), ErrorKind::Upstream);
            assert_eq!(
                diagnostic.stage(),
                jira_application::AttachmentReadStage::Body
            );
            assert_eq!(
                diagnostic.attempt(),
                AttachmentReadAttempt::ExplicitDownload
            );
            assert_eq!(diagnostic.body_class(), Some(body_class));
            assert_eq!(error.message(), message);
        }
    }

    #[test]
    fn attachment_limits_are_independent_from_json_response_limit() {
        let body = vec![0_u8; 32 * 1024 * 1024];
        assert!(
            finish_attachment_body(
                body.clone(),
                DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES,
                &CancellationToken::new()
            )
            .is_ok()
        );
        assert_eq!(
            finish_attachment_body(
                body,
                DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES,
                &CancellationToken::new()
            )
            .expect_err("32 MiB image should exceed the 8 MiB cap")
            .kind(),
            ErrorKind::Upstream
        );
        let config = JiraHttpConfig::default();
        assert_eq!(config.max_response_bytes, 16 * 1024 * 1024);
        assert_eq!(
            config.attachment_download_max_bytes,
            DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES
        );
        assert_eq!(
            config.attachment_image_max_bytes,
            DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES
        );
    }

    #[test]
    fn attachment_read_checks_cancellation_before_network_dispatch() {
        let credentials = ApiTokenCredentials::new("person@example.com", "secret-token").unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(JiraHttpClient::attachment_image_request(
                Client::new(),
                Url::parse("https://example.atlassian.net/rest/api/3/attachment/content/42")
                    .unwrap(),
                credentials,
                AttachmentReadOptions {
                    attachment_id: "42".to_owned(),
                    cancellation,
                    max_bytes: 4,
                    width: 0,
                    height: 0,
                    thumbnail: false,
                },
            ))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Cancelled);
    }

    #[test]
    fn attachment_thumbnail_accepts_a_valid_png_with_octet_stream_mime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0dIDAT\x08\xd7c\xf8\xcf\xc0\xf0\x1f\x00\x05\x00\x01\xff\x89\x99=\x1d\x00\x00\x00\x00IEND\xaeB`\x82";

        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let responder = async move {
                let (stream, _) = listener.accept().await.unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    png.len()
                );
                let mut response = response.into_bytes();
                response.extend_from_slice(png);
                let mut written = 0;
                while written < response.len() {
                    stream.writable().await.unwrap();
                    match stream.try_write(&response[written..]) {
                        Ok(count) => written += count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) => panic!("responder write failed: {error}"),
                    }
                }
            };
            let responder = tokio::spawn(responder);

            let credentials = ApiTokenCredentials::new("person@example.com", "secret-token")
                .unwrap();
            let client = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let content = JiraHttpClient::attachment_image_request(
                client,
                Url::parse(&format!(
                    "http://{address}/rest/api/3/attachment/thumbnail/42"
                ))
                .unwrap(),
                credentials,
                AttachmentReadOptions {
                    attachment_id: "42".to_owned(),
                    cancellation: CancellationToken::new(),
                    max_bytes: 1024,
                    width: 640,
                    height: 480,
                    thumbnail: true,
                },
            )
            .await
            .unwrap();
            responder.await.unwrap();

            assert_eq!(content.mime_type, "application/octet-stream");
            assert_eq!(content.bytes, png);
        });
    }

    #[test]
    fn attachment_thumbnail_accepts_a_valid_png_with_an_unknown_mime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0dIDAT\x08\xd7c\xf8\xcf\xc0\xf0\x1f\x00\x05\x00\x01\xff\x89\x99=\x1d\x00\x00\x00\x00IEND\xaeB`\x82";

        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let responder = async move {
                let (stream, _) = listener.accept().await.unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/x-atlassian-image\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    png.len()
                );
                let mut response = response.into_bytes();
                response.extend_from_slice(png);
                let mut written = 0;
                while written < response.len() {
                    stream.writable().await.unwrap();
                    match stream.try_write(&response[written..]) {
                        Ok(count) => written += count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) => panic!("responder write failed: {error}"),
                    }
                }
            };
            let responder = tokio::spawn(responder);

            let credentials = ApiTokenCredentials::new("person@example.com", "secret-token")
                .unwrap();
            let client = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let content = JiraHttpClient::attachment_image_request(
                client,
                Url::parse(&format!(
                    "http://{address}/rest/api/3/attachment/thumbnail/42"
                ))
                .unwrap(),
                credentials,
                AttachmentReadOptions {
                    attachment_id: "42".to_owned(),
                    cancellation: CancellationToken::new(),
                    max_bytes: 1024,
                    width: 640,
                    height: 480,
                    thumbnail: true,
                },
            )
            .await
            .unwrap();
            responder.await.unwrap();

            assert_eq!(content.mime_type, "image/png");
            assert_eq!(content.bytes, png);
        });
    }

    #[test]
    fn attachment_thumbnail_rejects_invalid_bytes_with_an_unknown_mime_safely() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let body = b"not an image";
        let raw_mime = "application/x-atlassian-image";

        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let responder = async move {
                let (stream, _) = listener.accept().await.unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {raw_mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let mut response = response.into_bytes();
                response.extend_from_slice(body);
                let mut written = 0;
                while written < response.len() {
                    stream.writable().await.unwrap();
                    match stream.try_write(&response[written..]) {
                        Ok(count) => written += count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) => panic!("responder write failed: {error}"),
                    }
                }
            };
            let responder = tokio::spawn(responder);

            let credentials = ApiTokenCredentials::new("person@example.com", "secret-token")
                .unwrap();
            let client = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let error = JiraHttpClient::attachment_image_request(
                client,
                Url::parse(&format!(
                    "http://{address}/rest/api/3/attachment/thumbnail/42"
                ))
                .unwrap(),
                credentials,
                AttachmentReadOptions {
                    attachment_id: "42".to_owned(),
                    cancellation: CancellationToken::new(),
                    max_bytes: 1024,
                    width: 640,
                    height: 480,
                    thumbnail: true,
                },
            )
            .await
            .expect_err("invalid image bytes must be rejected");
            responder.await.unwrap();

            assert_eq!(error.kind(), ErrorKind::NotFound);
            assert_eq!(
                error.message(),
                "Jira attachment response bytes did not match an image format"
            );
            assert!(!error.message().contains(raw_mime));
            assert!(!error.message().contains("not an image"));
            let diagnostic = error.attachment_diagnostic().expect("validation diagnostic");
            assert_eq!(diagnostic.stage(), jira_application::AttachmentReadStage::Validation);
            assert_eq!(diagnostic.attempt(), AttachmentReadAttempt::Thumbnail);
        });
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
        assert_eq!(error.message(), "Jira write outcome is unknown");
        assert!(!error.message().contains("secret-token"));
    }

    #[test]
    fn malformed_created_comment_is_an_unknown_outcome_without_leaking_body() {
        let error = map_created_comment_body(br#"{"id":"secret-token"}"#).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::UnknownOutcome);
        assert_eq!(error.message(), "Jira write outcome is unknown");
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
