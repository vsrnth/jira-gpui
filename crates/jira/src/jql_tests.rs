use super::*;

#[test]
fn builds_a_deterministic_assignee_query() {
    let alpha = AccountId::parse("557058:aaa").unwrap();
    let beta = AccountId::parse("712020:bbb").unwrap();

    assert_eq!(
        assigned_issues_jql([beta, alpha.clone(), alpha]).unwrap(),
        "(issuetype in (Bug, Story, Task, Sub-task) and status not in (Canceled, rejected, Cancelled) and createdDate >= startOfYear()) AND assignee IN (\"557058:aaa\", \"712020:bbb\") ORDER BY updated DESC"
    );
}

#[test]
fn allows_project_wide_query_without_remote_assignees() {
    assert_eq!(
        assigned_issues_jql(Vec::<AccountId>::new()).unwrap(),
        "(issuetype in (Bug, Story, Task, Sub-task) and status not in (Canceled, rejected, Cancelled) and createdDate >= startOfYear()) ORDER BY updated DESC"
    );
}

#[test]
fn rejects_values_that_could_change_jql() {
    assert_eq!(
        AccountId::parse("\" OR project = ABC").unwrap_err(),
        JqlError::UnsafeAccountId
    );
    assert_eq!(
        AccountId::parse("abc\\def").unwrap_err(),
        JqlError::UnsafeAccountId
    );
    assert_eq!(
        AccountId::parse("abc\ndef").unwrap_err(),
        JqlError::UnsafeAccountId
    );
}

#[test]
fn accepts_domain_account_ids_at_the_application_boundary() {
    let account_id = jira_domain::AccountId::new("557058:abc-123").unwrap();
    assert_eq!(
        assigned_issues_for_account_ids([account_id]).unwrap(),
        "(issuetype in (Bug, Story, Task, Sub-task) and status not in (Canceled, rejected, Cancelled) and createdDate >= startOfYear()) AND assignee IN (\"557058:abc-123\") ORDER BY updated DESC"
    );
}

#[test]
fn converts_an_application_fetch_request_to_an_enhanced_search_request() {
    use jira_application::{IssueFetchRequest, PageCursor};
    use time::macros::datetime;

    let request = IssueFetchRequest {
        site_id: jira_domain::JiraSiteId::new("site").unwrap(),
        assignees: Some(vec![jira_domain::AccountId::new("557058:abc-123").unwrap()]),
        watchers: Some(vec![jira_domain::AccountId::new("712020:watcher").unwrap()]),
        jql_scope: None,
        updated_since: Some(datetime!(2026-08-15 17:20 UTC)),
        page_cursor: Some(PageCursor("opaque-token".into())),
        page_size: 100,
    };

    let enhanced = enhanced_search_request(&request).unwrap();
    assert_eq!(enhanced.next_page_token.as_deref(), Some("opaque-token"));
    assert_eq!(enhanced.max_results, Some(100));
    assert_eq!(
        enhanced.jql,
        "(issuetype in (Bug, Story, Task, Sub-task) and status not in (Canceled, rejected, Cancelled) and createdDate >= startOfYear()) AND (assignee IN (\"557058:abc-123\") OR watcher IN (\"712020:watcher\")) AND updated >= \"2026-08-15 17:20\" ORDER BY updated DESC"
    );
}

#[test]
fn builds_the_team_request_scope_as_assignee_only_in_progress_jql() {
    use jira_application::IssueFetchRequest;

    let request = IssueFetchRequest {
        site_id: jira_domain::JiraSiteId::new("site").unwrap(),
        assignees: Some(vec![
            jira_domain::AccountId::new("team-b").unwrap(),
            jira_domain::AccountId::new("team-a").unwrap(),
            jira_domain::AccountId::new("team-a").unwrap(),
        ]),
        watchers: None,
        jql_scope: Some("statusCategory = \"In Progress\"".into()),
        updated_since: None,
        page_cursor: None,
        page_size: 100,
    };

    assert_eq!(
        enhanced_search_request(&request).unwrap().jql,
        "(statusCategory = \"In Progress\") AND assignee IN (\"team-a\", \"team-b\") ORDER BY updated DESC"
    );
}

#[test]
fn incremental_project_wide_query_keeps_updated_clause_and_cursor() {
    use jira_application::{IssueFetchRequest, PageCursor};
    use time::macros::datetime;

    let request = IssueFetchRequest {
        site_id: jira_domain::JiraSiteId::new("site").unwrap(),
        assignees: None,
        watchers: None,
        jql_scope: None,
        updated_since: Some(datetime!(2026-08-15 17:20 UTC)),
        page_cursor: Some(PageCursor("opaque-token".into())),
        page_size: 100,
    };

    let enhanced = enhanced_search_request(&request).unwrap();
    assert_eq!(enhanced.next_page_token.as_deref(), Some("opaque-token"));
    assert_eq!(
        enhanced.jql,
        "(issuetype in (Bug, Story, Task, Sub-task) and status not in (Canceled, rejected, Cancelled) and createdDate >= startOfYear()) AND updated >= \"2026-08-15 17:20\" ORDER BY updated DESC"
    );

    let mut explicitly_empty = request.clone();
    explicitly_empty.assignees = Some(Vec::new());
    assert_eq!(
        enhanced_search_request(&explicitly_empty).unwrap().jql,
        enhanced.jql
    );
}

#[test]
fn builds_watcher_only_and_custom_scope_queries_deterministically() {
    let first = jira_domain::AccountId::new("watcher-b").unwrap();
    let second = jira_domain::AccountId::new("watcher-a").unwrap();
    assert_eq!(
        scoped_issues_for_account_ids(
            Some("project = APP"),
            Vec::<jira_domain::AccountId>::new(),
            [first, second.clone(), second],
        )
        .unwrap(),
        "(project = APP) AND watcher IN (\"watcher-a\", \"watcher-b\") ORDER BY updated DESC"
    );
}

#[test]
fn rejects_invalid_scope_expressions() {
    assert_eq!(
        scoped_issues_jql(Some("  "), Vec::<AccountId>::new(), Vec::<AccountId>::new())
            .unwrap_err(),
        JqlError::EmptyScope
    );
    assert_eq!(
        scoped_issues_jql(
            Some(&"x".repeat(MAX_JQL_SCOPE_LENGTH + 1)),
            Vec::<AccountId>::new(),
            Vec::<AccountId>::new(),
        )
        .unwrap_err(),
        JqlError::ScopeTooLong
    );
    assert_eq!(
        scoped_issues_jql(
            Some("project = APP ORDER\n BY x"),
            Vec::<AccountId>::new(),
            Vec::<AccountId>::new()
        )
        .unwrap_err(),
        JqlError::ScopeContainsOrderBy
    );
}

#[test]
fn builds_a_deterministic_deduplicated_issue_id_request() {
    let first = IssueId::new("1002").unwrap();
    let second = IssueId::new("1001").unwrap();
    let request =
        enhanced_search_request_for_issue_ids(&[first.clone(), second.clone(), first]).unwrap();

    assert_eq!(
        request.jql,
        "id IN (\"1001\", \"1002\") ORDER BY updated DESC"
    );
    assert_eq!(request.max_results, Some(100));
    assert_eq!(
        request.fields,
        crate::ASSIGNED_ISSUE_FIELDS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
}

fn changelog_request(issue_ids: &[IssueId]) -> jira_application::IssueChangelogRequest {
    jira_application::IssueChangelogRequest {
        site_id: jira_domain::JiraSiteId::new("site").expect("valid test site"),
        issue_ids: issue_ids.to_vec(),
    }
}

fn assert_both_bounded_builders_reject(issue_ids: &[IssueId], expected: JqlError) {
    assert_eq!(
        enhanced_search_request_for_issue_ids(issue_ids).unwrap_err(),
        expected.clone()
    );
    assert_eq!(
        bulk_changelog_request(&changelog_request(issue_ids), None).unwrap_err(),
        expected
    );
}

#[test]
fn rejects_empty_issue_ids_through_both_bounded_builders() {
    assert_both_bounded_builders_reject(&[], JqlError::NoIssueIds);
}

#[test]
fn rejects_trimmed_blank_issue_ids_through_both_bounded_builders() {
    let blank: IssueId = serde_json::from_str(r#"" \t ""#).expect("valid JSON issue ID");
    assert_both_bounded_builders_reject(&[blank], JqlError::EmptyIssueId);
}

#[test]
fn rejects_256_byte_issue_ids_through_both_bounded_builders() {
    let oversized: IssueId = serde_json::from_str(&format!("\"{}\"", "x".repeat(256))).unwrap();
    assert_both_bounded_builders_reject(&[oversized], JqlError::IssueIdTooLong);
}

#[test]
fn rejects_unsafe_issue_ids_through_both_bounded_builders() {
    for suffix in ["\n", "\"", "\\"] {
        let unsafe_id = IssueId::new(format!("100{suffix}")).unwrap();
        assert_both_bounded_builders_reject(&[unsafe_id], JqlError::UnsafeIssueId);
    }
}

#[test]
fn rejects_oversized_original_cardinality_even_when_ids_are_duplicates() {
    let ids = vec![IssueId::new("1001").unwrap(); MAX_ISSUE_IDS + 1];
    assert_both_bounded_builders_reject(
        &ids,
        JqlError::TooManyIssueIds {
            maximum: MAX_ISSUE_IDS,
            received: MAX_ISSUE_IDS + 1,
        },
    );
}

#[test]
fn normalizes_unsorted_duplicates_identically_for_both_bounded_builders() {
    let ids = vec![
        IssueId::new("1002").unwrap(),
        IssueId::new("1001").unwrap(),
        IssueId::new("1002").unwrap(),
    ];

    let enhanced = enhanced_search_request_for_issue_ids(&ids).unwrap();
    let bulk = bulk_changelog_request(&changelog_request(&ids), None).unwrap();

    assert_eq!(
        enhanced.jql,
        "id IN (\"1001\", \"1002\") ORDER BY updated DESC"
    );
    assert_eq!(bulk.issue_ids_or_keys, vec!["1001", "1002"]);
}

#[test]
fn rejects_empty_oversized_and_unsafe_issue_id_inputs() {
    assert_eq!(
        enhanced_search_request_for_issue_ids(&[]).unwrap_err(),
        JqlError::NoIssueIds
    );

    let empty: IssueId = serde_json::from_str("\"\"").unwrap();
    assert_eq!(
        enhanced_search_request_for_issue_ids(&[empty]).unwrap_err(),
        JqlError::EmptyIssueId
    );

    let oversized: IssueId = serde_json::from_str(&format!("\"{}\"", "x".repeat(256))).unwrap();
    assert_eq!(
        enhanced_search_request_for_issue_ids(&[oversized]).unwrap_err(),
        JqlError::IssueIdTooLong
    );

    let unsafe_id = IssueId::new("100\" OR project = SECRET").unwrap();
    assert_eq!(
        enhanced_search_request_for_issue_ids(&[unsafe_id]).unwrap_err(),
        JqlError::UnsafeIssueId
    );

    let ids = (0..=MAX_ISSUE_IDS)
        .map(|index| IssueId::new(index.to_string()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        enhanced_search_request_for_issue_ids(&ids).unwrap_err(),
        JqlError::TooManyIssueIds {
            maximum: MAX_ISSUE_IDS,
            received: MAX_ISSUE_IDS + 1,
        }
    );
}

#[test]
fn serializes_the_issue_id_request_using_jira_field_names() {
    let request = enhanced_search_request_for_issue_ids(&[IssueId::new("1001").unwrap()]).unwrap();
    let value = serde_json::to_value(request).unwrap();

    assert_eq!(value["jql"], "id IN (\"1001\") ORDER BY updated DESC");
    assert_eq!(value["maxResults"], 100);
    assert_eq!(value["fields"][0], "summary");
    assert!(value.get("nextPageToken").is_none());
}

#[test]
fn serializes_bounded_bulk_changelog_request_and_cursor() {
    let request = jira_application::IssueChangelogRequest {
        site_id: jira_domain::JiraSiteId::new("site").expect("valid test site"),
        issue_ids: vec![
            IssueId::new("1002").expect("valid first test issue"),
            IssueId::new("1001").expect("valid second test issue"),
        ],
    };
    let body = bulk_changelog_request(&request, Some("opaque-page".into()))
        .expect("valid bulk changelog request");
    assert_eq!(body.issue_ids_or_keys, vec!["1001", "1002"]);
    assert_eq!(body.max_results, 1_000);
    assert_eq!(body.next_page_token.as_deref(), Some("opaque-page"));
    let json = serde_json::to_value(body).expect("serializable bulk changelog request");
    assert_eq!(json["issueIdsOrKeys"], serde_json::json!(["1001", "1002"]));
    assert_eq!(json["maxResults"], 1_000);
    assert_eq!(json["nextPageToken"], "opaque-page");
}
