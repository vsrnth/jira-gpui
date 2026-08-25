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
#[path = "issue_edit_tests.rs"]
mod tests;
