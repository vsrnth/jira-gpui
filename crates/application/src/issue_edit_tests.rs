use std::sync::{Arc, Mutex};

use jira_domain::{AccountId, IssueId, IssueKey, JiraSiteId, Status};
use time::macros::datetime;

use super::*;
use crate::{CachedAssignableUsers, CachedIssueTransitions, IssueLocator, test_support::block_on};

#[derive(Clone)]
struct FakeEditor {
    calls: Arc<Mutex<Calls>>,
    users: Vec<User>,
    transitions: Vec<IssueTransition>,
    result: Result<(), ApplicationError>,
}

#[derive(Default)]
struct Calls {
    searches: usize,
    transition_reads: usize,
    assignments: usize,
    transitions: usize,
}

#[derive(Clone)]
struct FixedClock(Arc<Mutex<jira_domain::Timestamp>>);

impl Clock for FixedClock {
    fn now(&self) -> jira_domain::Timestamp {
        *self.0.lock().expect("clock lock")
    }
}

#[derive(Clone, Default)]
struct FakeCache {
    users: Arc<Mutex<Option<CachedAssignableUsers>>>,
    transitions: Arc<Mutex<Option<CachedIssueTransitions>>>,
    fail_invalidation: bool,
}

impl IssueEditCachePort for FakeCache {
    fn cached_assignable_users<'a>(
        &'a self,
        _site_id: &'a JiraSiteId,
        _locator: &'a IssueLocator,
    ) -> crate::PortFuture<'a, Option<CachedAssignableUsers>> {
        let value = self.users.lock().expect("users lock").clone();
        Box::pin(async move { Ok(value) })
    }

    fn replace_assignable_users<'a>(
        &'a self,
        _site_id: &'a JiraSiteId,
        _locator: &'a IssueLocator,
        users: Vec<User>,
        fetched_at: jira_domain::Timestamp,
    ) -> crate::PortFuture<'a, ()> {
        *self.users.lock().expect("users lock") = Some(CachedAssignableUsers { users, fetched_at });
        Box::pin(async { Ok(()) })
    }

    fn cached_issue_transitions<'a>(
        &'a self,
        _site_id: &'a JiraSiteId,
        _locator: &'a IssueLocator,
    ) -> crate::PortFuture<'a, Option<CachedIssueTransitions>> {
        let value = self.transitions.lock().expect("transitions lock").clone();
        Box::pin(async move { Ok(value) })
    }

    fn replace_issue_transitions<'a>(
        &'a self,
        _site_id: &'a JiraSiteId,
        _locator: &'a IssueLocator,
        transitions: Vec<IssueTransition>,
        fetched_at: jira_domain::Timestamp,
    ) -> crate::PortFuture<'a, ()> {
        *self.transitions.lock().expect("transitions lock") = Some(CachedIssueTransitions {
            transitions,
            fetched_at,
        });
        Box::pin(async { Ok(()) })
    }

    fn invalidate_issue_transitions<'a>(
        &'a self,
        _site_id: &'a JiraSiteId,
        _locator: &'a IssueLocator,
    ) -> crate::PortFuture<'a, ()> {
        if self.fail_invalidation {
            Box::pin(async {
                Err(ApplicationError::new(
                    crate::ErrorKind::Storage,
                    "cache unavailable",
                ))
            })
        } else {
            *self.transitions.lock().expect("transitions lock") = None;
            Box::pin(async { Ok(()) })
        }
    }
}

impl FakeEditor {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Calls::default())),
            users: Vec::new(),
            transitions: vec![IssueTransition {
                id: "31".into(),
                name: "In progress".into(),
                to: Status {
                    id: "3".into(),
                    name: "In Progress".into(),
                    category: None,
                },
            }],
            result: Ok(()),
        }
    }

    fn count(&self) -> Calls {
        let calls = self.calls.lock().expect("calls lock");
        Calls {
            searches: calls.searches,
            transition_reads: calls.transition_reads,
            assignments: calls.assignments,
            transitions: calls.transitions,
        }
    }
}

impl JiraIssueEditPort for FakeEditor {
    fn search_assignable_users<'a>(
        &'a self,
        _request: &'a AssignableUserSearchRequest,
        _cancellation: &'a CancellationToken,
    ) -> crate::PortFuture<'a, Vec<User>> {
        self.calls.lock().expect("calls lock").searches += 1;
        let users = self.users.clone();
        Box::pin(async move { Ok(users) })
    }

    fn fetch_issue_transitions<'a>(
        &'a self,
        _request: &'a IssueTransitionsRequest,
        _cancellation: &'a CancellationToken,
    ) -> crate::PortFuture<'a, Vec<IssueTransition>> {
        self.calls.lock().expect("calls lock").transition_reads += 1;
        let transitions = self.transitions.clone();
        Box::pin(async move { Ok(transitions) })
    }

    fn assign_issue<'a>(
        &'a self,
        _request: &'a AssignIssueRequest,
        _cancellation: &'a CancellationToken,
    ) -> crate::PortFuture<'a, ()> {
        self.calls.lock().expect("calls lock").assignments += 1;
        let result = self.result.clone();
        Box::pin(async move { result })
    }

    fn transition_issue<'a>(
        &'a self,
        _request: &'a TransitionIssueRequest,
        _cancellation: &'a CancellationToken,
    ) -> crate::PortFuture<'a, ()> {
        self.calls.lock().expect("calls lock").transitions += 1;
        let result = self.result.clone();
        Box::pin(async move { result })
    }
}

fn site() -> JiraSiteId {
    JiraSiteId::new("site").expect("site")
}

fn locator() -> crate::IssueLocator {
    crate::IssueLocator::Key(IssueKey::new("APP-1").expect("key"))
}

fn search(query: &str, limit: usize) -> AssignableUserSearchRequest {
    AssignableUserSearchRequest {
        site_id: site(),
        locator: locator(),
        query: query.into(),
        limit,
    }
}

fn assignment() -> AssignIssueRequest {
    AssignIssueRequest {
        site_id: site(),
        locator: locator(),
        assignee: Some(AccountId::new("account").expect("account")),
    }
}

#[test]
fn validates_search_limit_but_allows_empty_initial_query() {
    let editor = FakeEditor::new();
    let service = IssueEditService::new(Arc::new(editor.clone()));
    assert!(
        block_on(service.search_assignable_users(search("", 1), &CancellationToken::new())).is_ok()
    );
    for limit in [0, MAX_ASSIGNABLE_USER_SEARCH_LIMIT + 1] {
        let error = block_on(
            service.search_assignable_users(search("x", limit), &CancellationToken::new()),
        )
        .expect_err("invalid limit");
        assert_eq!(error.kind(), crate::ErrorKind::InvalidInput);
    }
    assert_eq!(editor.count().searches, 1);
}

#[test]
fn cached_empty_user_query_is_filtered_locally_case_insensitively() {
    let mut editor = FakeEditor::new();
    editor.users = vec![
        User::new(
            site(),
            AccountId::new("alice-id").expect("account"),
            "Alice Example",
            None,
            true,
        ),
        User::new(
            site(),
            AccountId::new("bob-id").expect("account"),
            "Bob Example",
            None,
            true,
        ),
    ];
    let editor = editor;
    let calls = editor.clone();
    let cache = FakeCache::default();
    let service = IssueEditService::new_with_cache(
        Arc::new(editor),
        Arc::new(cache),
        Arc::new(FixedClock(Arc::new(Mutex::new(datetime!(
            2026-01-01 00:00 UTC
        ))))),
    );
    assert_eq!(
        block_on(service.search_assignable_users(search("", 100), &CancellationToken::new()))
            .expect("initial users")
            .len(),
        2
    );
    let filtered =
        block_on(service.search_assignable_users(search("ALICE", 100), &CancellationToken::new()))
            .expect("filtered users");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].display_name, "Alice Example");
    assert_eq!(calls.count().searches, 1);
}

#[test]
fn successful_transition_does_not_retry_when_cache_invalidation_fails() {
    let editor = FakeEditor::new();
    let cache = FakeCache {
        transitions: Arc::new(Mutex::new(Some(CachedIssueTransitions {
            transitions: editor.transitions.clone(),
            fetched_at: datetime!(2026-01-01 00:00 UTC),
        }))),
        fail_invalidation: true,
        ..FakeCache::default()
    };
    let service = IssueEditService::new_with_cache(
        Arc::new(editor.clone()),
        Arc::new(cache),
        Arc::new(FixedClock(Arc::new(Mutex::new(datetime!(
            2026-01-01 00:01 UTC
        ))))),
    );
    block_on(service.transition(
        TransitionIssueRequest {
            site_id: site(),
            locator: locator(),
            transition_id: "31".into(),
        },
        &CancellationToken::new(),
    ))
    .expect("successful write remains successful");
    let transitions = block_on(service.available_transitions(
        IssueTransitionsRequest {
            site_id: site(),
            locator: locator(),
        },
        &CancellationToken::new(),
    ))
    .expect("refresh after invalidation failure");
    assert_eq!(transitions, editor.transitions);
    assert_eq!(editor.count().transition_reads, 1);
}

#[test]
fn rejects_control_characters_in_search_query_before_dispatch() {
    let editor = FakeEditor::new();
    let service = IssueEditService::new(Arc::new(editor.clone()));
    let error =
        block_on(service.search_assignable_users(search("alice\n", 10), &CancellationToken::new()))
            .expect_err("control character in query");
    assert_eq!(error.kind(), crate::ErrorKind::InvalidInput);
    assert_eq!(editor.count().searches, 0);
}

#[test]
fn validates_transition_read_locator_before_dispatch() {
    let editor = FakeEditor::new();
    let service = IssueEditService::new(Arc::new(editor.clone()));
    let error = block_on(service.available_transitions(
        IssueTransitionsRequest {
            site_id: JiraSiteId::new("site\n").expect("site"),
            locator: locator(),
        },
        &CancellationToken::new(),
    ))
    .expect_err("invalid site");
    assert_eq!(error.kind(), crate::ErrorKind::InvalidInput);

    let error = block_on(service.available_transitions(
        IssueTransitionsRequest {
            site_id: site(),
            locator: crate::IssueLocator::Id(IssueId::new("100\n").expect("issue id")),
        },
        &CancellationToken::new(),
    ))
    .expect_err("invalid locator");
    assert_eq!(error.kind(), crate::ErrorKind::InvalidInput);
    assert_eq!(editor.count().transition_reads, 0);
}

#[test]
fn rejects_invalid_transition_metadata() {
    let mut editor = FakeEditor::new();
    editor.transitions[0].name = "  ".into();
    let service = IssueEditService::new(Arc::new(editor.clone()));
    let error = block_on(service.available_transitions(
        IssueTransitionsRequest {
            site_id: site(),
            locator: locator(),
        },
        &CancellationToken::new(),
    ))
    .expect_err("invalid transition");
    assert_eq!(error.kind(), crate::ErrorKind::Upstream);
    assert_eq!(editor.count().transition_reads, 1);
}

#[test]
fn rejects_control_characters_in_transition_metadata() {
    for field in ["id", "name", "to_id", "to_name"] {
        let mut editor = FakeEditor::new();
        match field {
            "id" => editor.transitions[0].id = "31\n".into(),
            "name" => editor.transitions[0].name = "In\tprogress".into(),
            "to_id" => editor.transitions[0].to.id = "3\r".into(),
            "to_name" => editor.transitions[0].to.name = "In\u{7}Progress".into(),
            _ => unreachable!(),
        }
        let service = IssueEditService::new(Arc::new(editor));
        let error = block_on(service.available_transitions(
            IssueTransitionsRequest {
                site_id: site(),
                locator: locator(),
            },
            &CancellationToken::new(),
        ))
        .expect_err("control character in transition metadata");
        assert_eq!(error.kind(), crate::ErrorKind::Upstream);
    }
}

#[test]
fn rejects_oversized_destination_status_metadata() {
    let mut editor = FakeEditor::new();
    editor.transitions[0].to.category = Some("x".repeat(256));
    let service = IssueEditService::new(Arc::new(editor));
    let error = block_on(service.available_transitions(
        IssueTransitionsRequest {
            site_id: site(),
            locator: locator(),
        },
        &CancellationToken::new(),
    ))
    .expect_err("oversized destination status");
    assert_eq!(error.kind(), crate::ErrorKind::Upstream);
}

#[test]
fn rejects_control_or_blank_destination_status_category() {
    for category in ["  ".to_owned(), "In\nProgress".to_owned()] {
        let mut editor = FakeEditor::new();
        editor.transitions[0].to.category = Some(category);
        let service = IssueEditService::new(Arc::new(editor));
        let error = block_on(service.available_transitions(
            IssueTransitionsRequest {
                site_id: site(),
                locator: locator(),
            },
            &CancellationToken::new(),
        ))
        .expect_err("invalid destination category");
        assert_eq!(error.kind(), crate::ErrorKind::Upstream);
    }
}

#[test]
fn rejects_oversized_transition_lists_before_returning_to_ui() {
    let mut editor = FakeEditor::new();
    editor.transitions = (0..=MAX_ISSUE_TRANSITIONS)
        .map(|index| IssueTransition {
            id: index.to_string(),
            name: format!("Transition {index}"),
            to: Status {
                id: "3".into(),
                name: "In Progress".into(),
                category: None,
            },
        })
        .collect();
    let service = IssueEditService::new(Arc::new(editor));
    let error = block_on(service.available_transitions(
        IssueTransitionsRequest {
            site_id: site(),
            locator: locator(),
        },
        &CancellationToken::new(),
    ))
    .expect_err("oversized transition list");
    assert_eq!(error.kind(), crate::ErrorKind::Upstream);
}

#[test]
fn cancellation_prevents_writes() {
    let editor = FakeEditor::new();
    let service = IssueEditService::new(Arc::new(editor.clone()));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        block_on(service.assign(assignment(), &cancellation))
            .expect_err("cancelled")
            .kind(),
        crate::ErrorKind::Cancelled
    );
    assert_eq!(editor.count().assignments, 0);
}

#[test]
fn rejects_control_characters_in_transition_id_before_dispatch() {
    let editor = FakeEditor::new();
    let service = IssueEditService::new(Arc::new(editor.clone()));
    let error = block_on(service.transition(
        TransitionIssueRequest {
            site_id: site(),
            locator: locator(),
            transition_id: "31\n".into(),
        },
        &CancellationToken::new(),
    ))
    .expect_err("control character in transition id");
    assert_eq!(error.kind(), crate::ErrorKind::InvalidInput);
    assert_eq!(editor.count().transitions, 0);
}

#[test]
fn each_confirmed_write_dispatches_once_and_preserves_unknown_outcome() {
    let mut editor = FakeEditor::new();
    editor.result = Err(ApplicationError::new(
        crate::ErrorKind::UnknownOutcome,
        "check Jira",
    ));
    let service = IssueEditService::new(Arc::new(editor.clone()));
    let expected = editor.result.clone().expect_err("error");
    let assignment_error = block_on(service.assign(assignment(), &CancellationToken::new()))
        .expect_err("unknown assignment outcome");
    assert_eq!(assignment_error, expected);
    let transition_error = block_on(service.transition(
        TransitionIssueRequest {
            site_id: site(),
            locator: locator(),
            transition_id: "31".into(),
        },
        &CancellationToken::new(),
    ))
    .expect_err("unknown transition outcome");
    assert_eq!(transition_error, expected);
    let calls = editor.count();
    assert_eq!(calls.assignments, 1);
    assert_eq!(calls.transitions, 1);
}
