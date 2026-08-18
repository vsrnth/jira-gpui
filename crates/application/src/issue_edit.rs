use std::sync::Arc;

use jira_domain::User;

use crate::{
    ApplicationError, AssignIssueRequest, AssignableUserSearchRequest, CancellationToken,
    IssueTransition, IssueTransitionsRequest, JiraIssueEditPort, TransitionIssueRequest,
};

/// Jira's assignable-user search is intentionally bounded at the application
/// boundary. The empty query is allowed so a picker can load initial candidates.
pub const MAX_ASSIGNABLE_USER_SEARCH_LIMIT: usize = 100;
pub const MAX_ISSUE_TRANSITIONS: usize = 100;

/// Application orchestration for explicit Jira assignment and workflow edits.
///
/// Read calls validate both their bounded inputs and the shape of the returned
/// workflow metadata. Write calls are only for requests that the presentation
/// layer has already confirmed: each is cancellation-checked immediately
/// before one port dispatch, and neither is retried.
#[derive(Clone)]
pub struct IssueEditService {
    editor: Arc<dyn JiraIssueEditPort>,
}

impl IssueEditService {
    pub fn new(editor: Arc<dyn JiraIssueEditPort>) -> Self {
        Self { editor }
    }

    pub async fn search_assignable_users(
        &self,
        request: AssignableUserSearchRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<User>, ApplicationError> {
        validate_user_search_request(&request)?;
        cancellation.check()?;
        let users = self
            .editor
            .search_assignable_users(&request, cancellation)
            .await?;
        cancellation.check()?;
        if users.len() > request.limit {
            return Err(upstream(
                "Jira returned more assignable users than requested",
            ));
        }
        if users.iter().any(|user| user.site_id != request.site_id) {
            return Err(upstream(
                "Jira returned an assignable user for another site",
            ));
        }
        Ok(users)
    }

    pub async fn available_transitions(
        &self,
        request: IssueTransitionsRequest,
        cancellation: &CancellationToken,
    ) -> Result<Vec<IssueTransition>, ApplicationError> {
        validate_locator_request(&request.site_id, &request.locator)?;
        cancellation.check()?;
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
        self.editor.transition_issue(&request, cancellation).await
    }
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
    use std::{
        future::Future,
        sync::{Arc, Mutex},
        task::{Context, Poll, Wake, Waker},
    };

    use jira_domain::{AccountId, IssueId, IssueKey, JiraSiteId, Status};

    use super::*;

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

    fn block_on<F: Future>(future: F) -> F::Output {
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }
        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }
}
