use super::*;
use crate::attachment_response::{
    attachment_body_error, attachment_mime_type, attachment_status_error,
    attachment_url_with_query, finish_attachment_body, image_mime_from_signature,
    is_allowed_image_mime, media_type,
};
use crate::read_response::status_error;
use crate::write_response::{
    comment_status_error, map_created_comment_body, write_dispatch_error, write_status_error,
};
use jira_application::{AttachmentBodyClass, AttachmentMimeClass, AttachmentReadAttempt};
use reqwest::StatusCode;

fn test_gateway_url() -> Url {
    gateway_base_url(&JiraCloudId::parse("cloud-id").unwrap()).unwrap()
}

#[test]
fn accepts_only_https_atlassian_cloud_urls_without_embedded_data() {
    assert!(JiraBaseUrl::parse("https://example.atlassian.net").is_ok());
    assert!(JiraBaseUrl::parse("https://example.atlassian.net/").is_ok());
    for invalid in [
        "http://example.atlassian.net",
        "https://example.atlassian.net:8443",
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
fn cloud_id_is_bounded_and_path_safe() {
    let cloud_id = JiraCloudId::parse("b1b2c3d4-1234-5678-9abc-def012345678").unwrap();
    assert_eq!(cloud_id.as_str(), "b1b2c3d4-1234-5678-9abc-def012345678");
    assert!(JiraCloudId::parse("stable_id-01").is_ok());
    for invalid in [
        "",
        "../escape",
        "cloud/id",
        "cloud id",
        "cloud?query",
        "-leading",
    ] {
        assert!(JiraCloudId::parse(invalid).is_err(), "{invalid}");
    }
    assert!(JiraCloudId::parse("a".repeat(JiraCloudId::MAX_LENGTH + 1)).is_err());
}

#[test]
fn gateway_base_is_canonical_and_endpoint_paths_stay_under_tenant_prefix() {
    let cloud_id = JiraCloudId::parse("cloud-id").unwrap();
    let base = gateway_base_url(&cloud_id).unwrap();
    assert_eq!(base.as_str(), "https://api.atlassian.com/ex/jira/cloud-id/");

    let site_id = JiraSiteId::new("site").unwrap();
    let client = JiraHttpClient::new(
        site_id,
        cloud_id,
        ApiTokenCredentials::new("person@example.com", "token").unwrap(),
    )
    .unwrap();
    let endpoint = client
        .issue_endpoint(
            &IssueLocator::Key(jira_domain::IssueKey::new("ENG-42").unwrap()),
            None,
        )
        .unwrap();
    assert_eq!(endpoint.scheme(), "https");
    assert_eq!(endpoint.host_str(), Some("api.atlassian.com"));
    assert_eq!(endpoint.port(), None);
    assert_eq!(endpoint.path(), "/ex/jira/cloud-id/rest/api/3/issue/ENG-42");
}

#[test]
fn tenant_info_requires_cloud_id_and_never_attaches_authorization() {
    let payload: TenantInfoResponse =
        serde_json::from_str(r#"{"cloudId":"cloud-id","tenantId":"tenant"}"#).unwrap();
    assert_eq!(
        JiraCloudId::parse(payload.cloud_id).unwrap().as_str(),
        "cloud-id"
    );
    assert!(serde_json::from_str::<TenantInfoResponse>(r#"{}"#).is_err());
    assert!(serde_json::from_str::<TenantInfoResponse>(r#"{"cloudId":null}"#).is_err());

    let request = tenant_info_request_builder(
        &Client::new(),
        Url::parse("https://example.atlassian.net/_edge/tenant_info").unwrap(),
    )
    .build()
    .unwrap();
    assert_eq!(request.method(), reqwest::Method::GET);
    assert!(request.headers().get(header::AUTHORIZATION).is_none());
}

#[test]
fn credentials_debug_redacts_token() {
    let credentials = ApiTokenCredentials::new("person@example.com", "super-secret-token").unwrap();
    let debug = format!("{credentials:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("person@example.com"));
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
        base_url: test_gateway_url(),
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
fn issue_id_token_progress_preserves_terminal_repeat_blank_and_safety_behavior() {
    assert!(MAX_ISSUE_ID_PAGES >= 2);
    assert!(MAX_ISSUE_ID_PAGES * 100 >= 1_000);

    let mut terminal = TokenPageProgress::issue_ids();
    assert_eq!(
        terminal.advance(Some("ignored".to_owned()), true),
        Ok(TokenPageProgression::Complete)
    );
    let mut missing = TokenPageProgress::issue_ids();
    assert_eq!(
        missing.advance(None, false),
        Ok(TokenPageProgression::Complete)
    );

    let mut repeated = TokenPageProgress::issue_ids();
    for _ in 0..(MAX_ISSUE_ID_PAGES - 1) {
        assert_eq!(
            repeated.advance(Some("same-token".to_owned()), false),
            Ok(TokenPageProgression::Continue)
        );
    }
    let error = repeated
        .advance(Some("same-token".to_owned()), false)
        .expect_err("issue-ID pagination safety cap");
    assert_eq!(error.kind(), ErrorKind::Upstream);
    assert_eq!(
        error.message(),
        "Jira issue pagination exceeded the safety limit"
    );

    let mut blank = TokenPageProgress::issue_ids();
    assert_eq!(
        blank.advance(Some("   ".to_owned()), false),
        Ok(TokenPageProgression::Continue)
    );
}

#[test]
fn changelog_token_progress_preserves_terminal_repeat_blank_and_safety_behavior() {
    assert!(MAX_CHANGELOG_PAGES >= 2);

    let mut missing = TokenPageProgress::changelog();
    assert_eq!(
        missing.advance(None, false),
        Ok(TokenPageProgression::Complete)
    );

    let mut blank = TokenPageProgress::changelog();
    let error = blank
        .advance(Some(" \t".to_owned()), false)
        .expect_err("blank changelog token");
    assert_eq!(error.kind(), ErrorKind::Upstream);
    assert_eq!(error.message(), "Jira changelog pagination did not advance");

    let mut repeated = TokenPageProgress::changelog();
    assert_eq!(
        repeated.advance(Some("page-1".to_owned()), false),
        Ok(TokenPageProgression::Continue)
    );
    let error = repeated
        .advance(Some("page-1".to_owned()), false)
        .expect_err("repeated changelog token");
    assert_eq!(error.kind(), ErrorKind::Upstream);
    assert_eq!(error.message(), "Jira changelog pagination did not advance");

    let mut safety = TokenPageProgress::changelog();
    for index in 0..(MAX_CHANGELOG_PAGES - 1) {
        assert_eq!(
            safety.advance(Some(format!("page-{index}")), false),
            Ok(TokenPageProgression::Continue)
        );
    }
    let error = safety
        .advance(Some("last-page".to_owned()), false)
        .expect_err("changelog pagination safety cap");
    assert_eq!(error.kind(), ErrorKind::Upstream);
    assert_eq!(
        error.message(),
        "Jira changelog pagination exceeded the safety limit"
    );
}

#[test]
fn current_user_request_targets_myself_and_maps_authenticated_identity() {
    let site_id = JiraSiteId::new("example-site").expect("site");
    let credentials = ApiTokenCredentials::new("person@example.com", "token").expect("credentials");
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
        base_url: test_gateway_url(),
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
    assert_eq!(
        detail.path(),
        "/ex/jira/cloud-id/rest/api/3/issue/ENG%2F42%3Fprivate"
    );
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
        "/ex/jira/cloud-id/rest/api/3/issue/ENG%2F42%3Fprivate/comment"
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
        base_url: test_gateway_url(),
        credentials: ApiTokenCredentials::new("person@example.com", "secret").unwrap(),
        client: Client::new(),
        runtime: Arc::new(RuntimeBridge::new().unwrap()),
        config: JiraHttpConfig::default(),
    };
    let issue_key = jira_domain::IssueKey::new("ENG-42").unwrap();
    let url = client
        .issue_endpoint(&IssueLocator::Key(issue_key), None)
        .unwrap();

    assert_eq!(url.path(), "/ex/jira/cloud-id/rest/api/3/issue/ENG-42");
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
        Some("query=ada%2Blovelace+%26+admin&issueId=ENG%2F42%3Fprivate%23fragment&maxResults=25")
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
        jira_adapter::assignee_request_body(Some("557058:abc-123")),
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
        jira_adapter::assignee_request_body(Some("557058:abc-123"))
    );

    let unassigned = JiraHttpClient::assign_issue_request_builder(
        &Client::new(),
        Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42/assignee").unwrap(),
        &credentials,
        jira_adapter::assignee_request_body(None),
    )
    .build()
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            unassigned.body().and_then(reqwest::Body::as_bytes).unwrap(),
        )
        .unwrap(),
        jira_adapter::assignee_request_body(None)
    );

    let transitioned = JiraHttpClient::transition_issue_request_builder(
        &Client::new(),
        Url::parse("https://example.atlassian.net/rest/api/3/issue/ENG-42/transitions").unwrap(),
        &credentials,
        jira_adapter::transition_request_body("31"),
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
        jira_adapter::transition_request_body("31")
    );
}

#[test]
fn transition_response_codec_errors_are_classified_at_the_http_boundary() {
    let malformed = map_transition_response(br#"{"transitions": [}"#).unwrap_err();
    assert_eq!(malformed.kind(), ErrorKind::Upstream);
    assert_eq!(malformed.message(), "Jira returned malformed JSON");

    let invalid = map_transition_response(
        br#"{"transitions":[{"id":"","name":"In progress","to":{"id":"3","name":"In Progress"}}]}"#,
    )
    .unwrap_err();
    assert_eq!(invalid.kind(), ErrorKind::Upstream);
    assert_eq!(invalid.message(), "Jira returned invalid transition data");
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
        JiraCloudId::parse("cloud-id").unwrap(),
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
        base_url: test_gateway_url(),
        credentials: ApiTokenCredentials::new("person@example.com", "secret-token").unwrap(),
        client: Client::new(),
        runtime: Arc::new(RuntimeBridge::new().unwrap()),
        config: JiraHttpConfig::default(),
    };
    let mut thumbnail = client
        .attachment_endpoint("rest/api/3/attachment/thumbnail", "att/42?url=evil")
        .unwrap();
    thumbnail = attachment_url_with_query(thumbnail, 640, 480, true);
    let request = attachment_response::attachment_request_builder(
        &client.client,
        thumbnail,
        &client.credentials,
    )
    .build()
    .unwrap();
    assert_eq!(request.method(), reqwest::Method::GET);
    assert_eq!(
        request.url().path(),
        "/ex/jira/cloud-id/rest/api/3/attachment/thumbnail/att%2F42%3Furl=evil"
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
    let mut content_request = attachment_response::attachment_request_builder(
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
        "/ex/jira/cloud-id/rest/api/3/attachment/content/42"
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
            Url::parse("https://example.atlassian.net/rest/api/3/attachment/content/42").unwrap(),
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
        jira_adapter::comment_create_request_body("<b>hello & goodbye</b>\nsecond"),
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
        jira_adapter::comment_create_request_body("<b>hello & goodbye</b>\nsecond")
    );
    assert!(json.get("visibility").is_none());
    assert!(json.get("properties").is_none());
    assert!(!String::from_utf8_lossy(body).contains("secret-token"));
}

#[test]
fn comment_body_is_trimmed_before_adf_serialization() {
    let body = "  hello\nworld  ".trim();
    let json = jira_adapter::comment_create_request_body(body);

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
        JiraCloudId::parse("cloud-id").unwrap(),
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
