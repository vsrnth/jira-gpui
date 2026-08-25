use std::sync::Arc;

use time::Duration;

use jira_domain::User;

use crate::{
    ApplicationError, AssignIssueRequest, AssignableUserSearchRequest, CancellationToken, Clock,
    IssueEditCachePort, IssueTransition, IssueTransitionsRequest, JiraIssueEditPort,
    TransitionIssueRequest,
};

use super::issue_edit_policy::IssueEditCachePolicy;

/// Jira's assignable-user search is intentionally bounded at the application
/// boundary. The empty query is allowed so a picker can load initial candidates.
pub const MAX_ASSIGNABLE_USER_SEARCH_LIMIT: usize = 100;
pub const MAX_ISSUE_TRANSITIONS: usize = 100;
pub const ISSUE_EDIT_CACHE_TTL: Duration = Duration::hours(24);

/// Application orchestration for explicit Jira assignment and workflow edits.
///
/// Read calls validate both their bounded inputs and the shape of the returned
/// workflow metadata. Write calls are only for requests that the presentation
/// layer has already confirmed: each is cancellation-checked immediately
/// before one port dispatch, and neither is retried.
#[derive(Clone)]
pub struct IssueEditService {
    editor: Arc<dyn JiraIssueEditPort>,
    cache: Option<Arc<dyn IssueEditCachePort>>,
    clock: Option<Arc<dyn Clock>>,
    cache_policy: IssueEditCachePolicy,
}

impl IssueEditService {
    pub fn new(editor: Arc<dyn JiraIssueEditPort>) -> Self {
        Self {
            editor,
            cache: None,
            clock: None,
            cache_policy: IssueEditCachePolicy::default(),
        }
    }

    /// Creates an issue-edit service backed by the durable edit-options cache.
    /// Keeping the cache and clock behind application ports lets GPUI wire
    /// SQLite in later without bringing persistence into this crate's logic.
    pub fn new_with_cache(
        editor: Arc<dyn JiraIssueEditPort>,
        cache: Arc<dyn IssueEditCachePort>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            editor,
            cache: Some(cache),
            clock: Some(clock),
            cache_policy: IssueEditCachePolicy::default(),
        }
    }

    /// Alias for callers that prefer the dependency-injection naming style.
    pub fn with_cache(
        editor: Arc<dyn JiraIssueEditPort>,
        cache: Arc<dyn IssueEditCachePort>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self::new_with_cache(editor, cache, clock)
    }

    pub async fn search_assignable_users(
        &self,
        request: AssignableUserSearchRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<User>, ApplicationError> {
        validate_user_search_request(&request)?;
        cancellation.check()?;
        let users = if let (Some(cache), Some(clock)) = (&self.cache, &self.clock) {
            let now = clock.now();
            let cached = cache
                .cached_assignable_users(&request.site_id, &request.locator)
                .await?;
            match cached.filter(|cached| {
                IssueEditCachePolicy::cache_is_fresh(cached.fetched_at, now, ISSUE_EDIT_CACHE_TTL)
            }) {
                Some(cached) => {
                    validate_assignable_users(&cached.users, &request.site_id)?;
                    cached.users
                }
                None => {
                    // The cache is populated only by Jira's empty, bounded
                    // candidate query. Typed searches then remain local.
                    let fetch_request = AssignableUserSearchRequest {
                        query: String::new(),
                        limit: MAX_ASSIGNABLE_USER_SEARCH_LIMIT,
                        ..request.clone()
                    };
                    let users = self
                        .editor
                        .search_assignable_users(&fetch_request, cancellation)
                        .await?;
                    cancellation.check()?;
                    validate_assignable_users(&users, &request.site_id)?;
                    cache
                        .replace_assignable_users(
                            &request.site_id,
                            &request.locator,
                            users.clone(),
                            now,
                        )
                        .await?;
                    users
                }
            }
        } else {
            self.editor
                .search_assignable_users(&request, cancellation)
                .await?
        };
        cancellation.check()?;
        if self.cache.is_some() {
            Ok(IssueEditCachePolicy::filter_assignable_users(
                users,
                &request.query,
                request.limit,
            ))
        } else {
            validate_assignable_users(&users, &request.site_id)?;
            if users.len() > request.limit {
                return Err(upstream(
                    "Jira returned more assignable users than requested",
                ));
            }
            Ok(users)
        }
    }

    pub async fn available_transitions(
        &self,
        request: IssueTransitionsRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<IssueTransition>, ApplicationError> {
        validate_locator_request(&request.site_id, &request.locator)?;
        cancellation.check()?;
        let transitions = if let (Some(cache), Some(clock)) = (&self.cache, &self.clock) {
            let now = clock.now();
            let cached = cache
                .cached_issue_transitions(&request.site_id, &request.locator)
                .await?;
            match cached.filter(|cached| {
                self.cache_policy.transitions_are_fresh(
                    &request.site_id,
                    &request.locator,
                    cached.fetched_at,
                    now,
                    ISSUE_EDIT_CACHE_TTL,
                )
            }) {
                Some(cached) => cached.transitions,
                None => {
                    let transitions = self
                        .editor
                        .fetch_issue_transitions(&request, cancellation)
                        .await?;
                    cancellation.check()?;
                    if transitions.len() > MAX_ISSUE_TRANSITIONS {
                        return Err(upstream(
                            "Jira returned more issue transitions than the configured limit",
                        ));
                    }
                    validate_transitions(&transitions)?;
                    cache
                        .replace_issue_transitions(
                            &request.site_id,
                            &request.locator,
                            transitions.clone(),
                            now,
                        )
                        .await?;
                    self.cache_policy
                        .mark_transitions_refreshed(&request.site_id, &request.locator);
                    transitions
                }
            }
        } else {
            self.editor
                .fetch_issue_transitions(&request, cancellation)
                .await?
        };
        cancellation.check()?;
        if transitions.len() > MAX_ISSUE_TRANSITIONS {
            return Err(upstream(
                "Jira returned more issue transitions than the configured limit",
            ));
        }
        validate_transitions(&transitions)?;
        Ok(transitions)
    }

    /// Dispatch one already-confirmed assignment exactly once. This method
    /// deliberately performs no retry because Jira may have accepted a write
    /// even when its response is not observed.
    pub async fn assign(
        &self,
        request: AssignIssueRequest,
        cancellation: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        validate_locator_request(&request.site_id, &request.locator)?;
        cancellation.check()?;
        self.editor.assign_issue(&request, cancellation).await
    }

    /// Dispatch one already-confirmed workflow transition exactly once. This
    /// method deliberately performs no retry because Jira may have accepted a
    /// write even when its response is not observed.
    pub async fn transition(
        &self,
        request: TransitionIssueRequest,
        cancellation: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        validate_locator_request(&request.site_id, &request.locator)?;
        validate_string_id(&request.transition_id, "transition id")?;
        cancellation.check()?;
        let result = self.editor.transition_issue(&request, cancellation).await;
        if result.is_ok()
            && let Some(cache) = &self.cache
        {
            // A definite success makes every previously displayed transition
            // stale. Do not leave old-state choices available to the picker.
            self.cache_policy
                .mark_transitions_invalidated(&request.site_id, &request.locator);
            // Invalidation is best effort after a successful remote write. A
            // local guard above prevents this service from using stale values
            // even if the adapter is temporarily unavailable.
            let _ = cache
                .invalidate_issue_transitions(&request.site_id, &request.locator)
                .await;
        }
        result
    }
}

fn validate_assignable_users(
    users: &[User],
    site_id: &jira_domain::JiraSiteId,
) -> Result<(), ApplicationError> {
    if users.len() > MAX_ASSIGNABLE_USER_SEARCH_LIMIT {
        return Err(upstream(
            "Jira returned more assignable users than the configured limit",
        ));
    }
    if users.iter().any(|user| user.site_id != *site_id) {
        return Err(upstream(
            "Jira returned an assignable user for another site",
        ));
    }
    Ok(())
}

fn validate_user_search_request(
    request: &AssignableUserSearchRequest,
) -> Result<(), ApplicationError> {
    validate_locator_request(&request.site_id, &request.locator)?;
    if !(1..=MAX_ASSIGNABLE_USER_SEARCH_LIMIT).contains(&request.limit) {
        return Err(ApplicationError::invalid_input(
            "assignable-user search limit must be between 1 and 100",
        ));
    }
    if request.query.len() > 255 || request.query.chars().any(char::is_control) {
        return Err(ApplicationError::invalid_input(
            "assignable-user search query is invalid",
        ));
    }
    Ok(())
}

fn validate_locator_request(
    site_id: &jira_domain::JiraSiteId,
    locator: &crate::IssueLocator,
) -> Result<(), ApplicationError> {
    validate_string_id(site_id.as_str(), "site id")?;
    match locator {
        crate::IssueLocator::Id(issue_id) => validate_string_id(issue_id.as_str(), "issue id"),
        crate::IssueLocator::Key(issue_key) => validate_string_id(issue_key.as_str(), "issue key"),
    }
}

fn validate_string_id(value: &str, field: &'static str) -> Result<(), ApplicationError> {
    if value.trim().is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(ApplicationError::invalid_input(format!(
            "{field} is invalid"
        )));
    }
    Ok(())
}

fn validate_transitions(transitions: &[IssueTransition]) -> Result<(), ApplicationError> {
    for transition in transitions {
        if transition.id.trim().is_empty() || transition.id.chars().any(char::is_control) {
            return Err(upstream("Jira returned a transition without an id"));
        }
        if transition.name.trim().is_empty() || transition.name.chars().any(char::is_control) {
            return Err(upstream("Jira returned a transition without a name"));
        }
        if transition.id.chars().count() > 255 || transition.name.chars().count() > 255 {
            return Err(upstream("Jira returned an oversized transition field"));
        }
        if transition.to.id.trim().is_empty()
            || transition.to.name.trim().is_empty()
            || transition.to.id.chars().any(char::is_control)
            || transition.to.name.chars().any(char::is_control)
        {
            return Err(upstream("Jira returned an invalid transition status"));
        }
        if transition.to.id.chars().count() > 255
            || transition.to.name.chars().count() > 255
            || transition.to.category.as_ref().is_some_and(|category| {
                category.trim().is_empty()
                    || category.chars().count() > 255
                    || category.chars().any(char::is_control)
            })
        {
            return Err(upstream("Jira returned an oversized transition status"));
        }
    }
    Ok(())
}

fn upstream(message: &'static str) -> ApplicationError {
    ApplicationError::new(crate::ErrorKind::Upstream, message)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use jira_domain::{AccountId, IssueId, IssueKey, JiraSiteId, Status};
    use time::macros::datetime;

    use super::*;
    use crate::{
        CachedAssignableUsers, CachedIssueTransitions, IssueLocator, test_support::block_on,
    };

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
            *self.users.lock().expect("users lock") =
                Some(CachedAssignableUsers { users, fetched_at });
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
            block_on(service.search_assignable_users(search("", 1), &CancellationToken::new()))
                .is_ok()
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
        let filtered = block_on(
            service.search_assignable_users(search("ALICE", 100), &CancellationToken::new()),
        )
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
        let error = block_on(
            service.search_assignable_users(search("alice\n", 10), &CancellationToken::new()),
        )
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
}
