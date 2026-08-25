//! Jira Cloud HTTP transport for remote reads and explicitly confirmed comment, assignment, and
//! workflow-status writes. Writes are dispatched once without automatic retries.
//!
//! The adapter owns a small Tokio runtime on a worker thread. This is intentional: GPUI and a
//! future Tauri shell can poll the application ports without having to install or drive a Tokio
//! reactor of their own. Credentials are held only in memory and are never persisted here.

use std::{fmt, sync::Arc, time::Duration};

use jira_adapter::{
    EnhancedSearchPage, IssueMapper, JiraBulkChangelogResponse, JiraCommentPage, JiraIssue,
    JiraUser,
};
use jira_application::{
    AddCommentRequest, ApplicationError, AssignIssueRequest, AssignableUserSearchRequest,
    AttachmentContent, AttachmentDownloadRequest, AttachmentImage, AttachmentImageRequest,
    CancellationToken, DEFAULT_ATTACHMENT_IMAGE_HEIGHT, DEFAULT_ATTACHMENT_IMAGE_WIDTH,
    DEFAULT_MAX_ATTACHMENT_DOWNLOAD_BYTES, DEFAULT_MAX_ATTACHMENT_IMAGE_BYTES, ErrorKind,
    IssueChangelog, IssueChangelogRequest, IssueCommentsPage, IssueCommentsPageRequest,
    IssueDetailRequest, IssueFetchRequest, IssueLocator, IssuePage, IssueTransition,
    IssueTransitionsRequest, JiraAttachmentReadPort, JiraCommentWritePort, JiraIssueActivityPort,
    JiraIssueDetailReadPort, JiraIssueEditPort, JiraIssueSearchPort, JiraReadPort,
    JiraSyncReadPort, JiraUserReadPort, MAX_ASSIGNABLE_USER_SEARCH_LIMIT, PageCursor, PortFuture,
    RecentIssueCommentsRequest, TransitionIssueRequest, UserSearchRequest,
};
use jira_domain::{Issue, IssueComment, IssueId, JiraSiteId, User};
use reqwest::{Client, header};
use url::Url;

mod attachment_response;
mod read_response;
mod runtime_bridge;
mod write_response;

use attachment_response::AttachmentReadOptions;
use read_response::{read_json, transport_error};
use runtime_bridge::RuntimeBridge;
use write_response::{
    comment_transport_error, read_created_comment, read_write_response, submit_write,
    write_transport_error,
};

const DEFAULT_USER_AGENT: &str = "jira-gpui/0.1 (Jira Cloud client)";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TENANT_INFO_RESPONSE_BYTES: usize = 8 * 1024;
const MAX_ISSUE_ID_PAGES: usize = 128;
const MAX_CHANGELOG_PAGES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenPageKind {
    IssueIds,
    Changelog,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenPageProgression {
    Complete,
    Continue,
}

/// Tracks pagination owned by this transport without imposing one endpoint's token policy on
/// another. Issue-ID search deliberately accepts any continuation token; bulk changelog requires
/// a non-blank token that differs from the previous token.
#[derive(Debug)]
struct TokenPageProgress {
    kind: TokenPageKind,
    next_page_token: Option<String>,
    pages_fetched: usize,
}

impl TokenPageProgress {
    fn issue_ids() -> Self {
        Self {
            kind: TokenPageKind::IssueIds,
            next_page_token: None,
            pages_fetched: 0,
        }
    }

    fn changelog() -> Self {
        Self {
            kind: TokenPageKind::Changelog,
            next_page_token: None,
            pages_fetched: 0,
        }
    }

    fn next_page_token(&self) -> Option<String> {
        self.next_page_token.clone()
    }

    fn advance(
        &mut self,
        next_page_token: Option<String>,
        is_last: bool,
    ) -> Result<TokenPageProgression, ApplicationError> {
        self.pages_fetched += 1;
        if is_last {
            return Ok(TokenPageProgression::Complete);
        }
        let Some(token) = next_page_token else {
            return Ok(TokenPageProgression::Complete);
        };
        if self.kind == TokenPageKind::Changelog
            && (token.trim().is_empty() || self.next_page_token.as_deref() == Some(token.as_str()))
        {
            return Err(ApplicationError::new(
                ErrorKind::Upstream,
                "Jira changelog pagination did not advance",
            ));
        }
        if self.pages_fetched
            >= match self.kind {
                TokenPageKind::IssueIds => MAX_ISSUE_ID_PAGES,
                TokenPageKind::Changelog => MAX_CHANGELOG_PAGES,
            }
        {
            let message = match self.kind {
                TokenPageKind::IssueIds => "Jira issue pagination exceeded the safety limit",
                TokenPageKind::Changelog => "Jira changelog pagination exceeded the safety limit",
            };
            return Err(ApplicationError::new(ErrorKind::Upstream, message));
        }
        self.next_page_token = Some(token);
        Ok(TokenPageProgression::Continue)
    }
}

/// Credentials for Jira Cloud basic authentication (email + API token).
///
/// The identity and token are deliberately not exposed through `Debug`, `Display`, or an error
/// value.
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
            .field("email", &"[REDACTED]")
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

/// The stable Atlassian Cloud tenant identifier used by the scoped-token API gateway.
///
/// Cloud IDs are intentionally kept separate from [`JiraSiteId`]. The latter is the
/// application's cache/site partition, while this value is only used to construct the
/// Atlassian API gateway path. Only a conservative, path-safe ASCII token is accepted.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct JiraCloudId(String);

impl JiraCloudId {
    const MAX_LENGTH: usize = 128;

    pub fn parse(value: impl AsRef<str>) -> Result<Self, ConfigError> {
        let value = value.as_ref();
        if value.is_empty()
            || value.len() > Self::MAX_LENGTH
            || !value.is_ascii()
            || !value.bytes().all(is_cloud_id_byte)
            || !value
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !value
                .bytes()
                .last()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(ConfigError::InvalidCloudId);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for JiraCloudId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for JiraCloudId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("JiraCloudId").field(&self.0).finish()
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
    base_url: Url,
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
        cloud_id: JiraCloudId,
        credentials: ApiTokenCredentials,
    ) -> Result<Self, ConfigError> {
        Self::with_config(site_id, cloud_id, credentials, JiraHttpConfig::default())
    }

    pub fn with_config(
        site_id: JiraSiteId,
        cloud_id: JiraCloudId,
        credentials: ApiTokenCredentials,
        config: JiraHttpConfig,
    ) -> Result<Self, ConfigError> {
        config.validate()?;
        let base_url = gateway_base_url(&cloud_id)?;
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
        let url = self
            .base_url
            .join(path)
            .map_err(|_| ApplicationError::new(ErrorKind::Internal, "invalid Jira endpoint"))?;
        if url.scheme() != "https"
            || url.host_str() != Some("api.atlassian.com")
            || url.port().is_some()
            || !url.path().starts_with(self.base_url.path())
        {
            return Err(ApplicationError::new(
                ErrorKind::Internal,
                "invalid Jira endpoint",
            ));
        }
        Ok(url)
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
        submit_write(Arc::clone(&self.runtime), cancellation, operation)
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
        let body = read_response::read_body(response, max_response_bytes).await?;
        map_transition_response(&body)
    }

    async fn assign_issue_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        assignee: Option<jira_domain::AccountId>,
    ) -> Result<(), ApplicationError> {
        let body = jira_adapter::assignee_request_body(assignee.as_ref().map(|id| id.as_str()));
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
        body: serde_json::Value,
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
        let body = jira_adapter::transition_request_body(&transition_id);
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
        body: serde_json::Value,
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
        let mut progress = TokenPageProgress::issue_ids();
        let mut issues = Vec::new();
        loop {
            cancellation.check()?;
            let mut body = base_body.clone();
            body.next_page_token = progress.next_page_token();
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
            if progress.advance(mapped.next_page_token, mapped.is_last)?
                == TokenPageProgression::Complete
            {
                return Ok(issues);
            }
        }
    }

    async fn issue_changelog_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        request: IssueChangelogRequest,
        cancellation: CancellationToken,
        max_response_bytes: usize,
    ) -> Result<Vec<IssueChangelog>, ApplicationError> {
        let mut progress = TokenPageProgress::changelog();
        let mut changelogs = Vec::new();
        loop {
            cancellation.check()?;
            let next_page_token = progress.next_page_token();
            let body = jira_adapter::bulk_changelog_request(&request, next_page_token)
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
            match progress.advance(mapped.next_page_token, false)? {
                TokenPageProgression::Complete => return Ok(changelogs),
                TokenPageProgression::Continue => {}
            }
        }
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

    async fn attachment_image_request(
        client: Client,
        url: Url,
        credentials: ApiTokenCredentials,
        options: AttachmentReadOptions,
    ) -> Result<AttachmentContent, ApplicationError> {
        attachment_response::read_attachment(client, url, credentials, options).await
    }

    fn create_comment_request_builder(
        client: &Client,
        url: Url,
        credentials: &ApiTokenCredentials,
        body: serde_json::Value,
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
            jira_adapter::comment_create_request_body(&body),
        )
        .send()
        .await
        .map_err(comment_transport_error)?;

        read_created_comment(response, max_response_bytes).await
    }
}

impl JiraIssueActivityPort for JiraHttpClient {
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
}

impl JiraAttachmentReadPort for JiraHttpClient {
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
}

impl JiraIssueDetailReadPort for JiraHttpClient {
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
}

impl JiraUserReadPort for JiraHttpClient {
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
}

impl JiraIssueSearchPort for JiraHttpClient {
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

impl JiraSyncReadPort for JiraHttpClient {}
impl JiraReadPort for JiraHttpClient {}

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
    #[error("Jira Cloud ID is invalid")]
    InvalidCloudId,
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
    #[error("could not discover the Jira Cloud ID")]
    CloudIdDiscovery,
}

fn validate_base_url(url: &Url, require_atlassian: bool) -> Result<JiraBaseUrl, ConfigError> {
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.port().is_some()
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

fn is_cloud_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn gateway_base_url(cloud_id: &JiraCloudId) -> Result<Url, ConfigError> {
    Url::parse(&format!(
        "https://api.atlassian.com/ex/jira/{}/",
        cloud_id.as_str()
    ))
    .map_err(|_| ConfigError::InvalidCloudId)
}

#[derive(Debug, serde::Deserialize)]
struct TenantInfoResponse {
    #[serde(rename = "cloudId")]
    cloud_id: String,
}

/// Discovers the stable Atlassian Cloud ID for a validated Jira site URL.
///
/// This operation is deliberately unauthenticated. The tenant-info request is made by the
/// transport-owned Tokio runtime and never receives caller credentials.
pub async fn discover_cloud_id(site_url: JiraBaseUrl) -> Result<JiraCloudId, ConfigError> {
    let runtime = RuntimeBridge::new().map_err(|_| ConfigError::RuntimeBuild)?;
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DEFAULT_REQUEST_TIMEOUT)
        .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
        .user_agent(DEFAULT_USER_AGENT)
        .build()
        .map_err(|_| ConfigError::HttpClientBuild)?;
    let url = site_url
        .as_url()
        .join("_edge/tenant_info")
        .map_err(|_| ConfigError::CloudIdDiscovery)?;
    runtime
        .dispatch(discover_cloud_id_request(client, url))
        .await
        .map_err(|_| ConfigError::CloudIdDiscovery)?
        .map_err(|_| ConfigError::CloudIdDiscovery)
}

async fn discover_cloud_id_request(
    client: Client,
    url: Url,
) -> Result<JiraCloudId, ApplicationError> {
    let response = tenant_info_request_builder(&client, url)
        .send()
        .await
        .map_err(transport_error)?;
    let payload: TenantInfoResponse = read_json(response, MAX_TENANT_INFO_RESPONSE_BYTES).await?;
    JiraCloudId::parse(payload.cloud_id).map_err(|_| {
        ApplicationError::new(ErrorKind::Upstream, "Jira returned an invalid Cloud ID")
    })
}

fn tenant_info_request_builder(client: &Client, url: Url) -> reqwest::RequestBuilder {
    client.get(url).header(header::ACCEPT, "application/json")
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

fn map_transition_response(body: &[u8]) -> Result<Vec<IssueTransition>, ApplicationError> {
    jira_adapter::decode_transitions_response(body).map_err(|error| match error {
        jira_adapter::JiraCodecError::MalformedJson => {
            ApplicationError::new(ErrorKind::Upstream, "Jira returned malformed JSON")
        }
        jira_adapter::JiraCodecError::InvalidData => invalid_transition_response(),
    })
}

fn invalid_transition_response() -> ApplicationError {
    ApplicationError::new(ErrorKind::Upstream, "Jira returned invalid transition data")
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

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
