use jira_application::{ApplicationError, ErrorKind, MAX_COMMENT_BYTES, MAX_COMMENT_CHARS};
use jira_domain::IssueId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommentTarget {
    pub(super) issue_id: IssueId,
    pub(super) issue_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommentSubmission {
    pub(super) issue_id: IssueId,
    pub(super) body: String,
    pub(super) generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommentValidationError {
    UnknownOutcomeNeedsRefresh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommentInvalidation {
    CancelPreDispatch,
    IgnoreDispatchedCompletion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommentCompletion {
    Ignored,
    Succeeded,
    Failed {
        message: &'static str,
        unknown_outcome: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CommentPostState {
    Idle,
    Confirming {
        issue_id: IssueId,
        issue_key: String,
        body: String,
        chars: usize,
        bytes: usize,
    },
    Posting {
        issue_id: IssueId,
        generation: u64,
    },
    Error {
        issue_id: IssueId,
        message: String,
        unknown_outcome: bool,
    },
}

pub(super) struct CommentFlow {
    state: CommentPostState,
    generation: u64,
}

impl CommentFlow {
    pub(super) fn new() -> Self {
        Self {
            state: CommentPostState::Idle,
            generation: 0,
        }
    }

    pub(super) fn is_posting(&self) -> bool {
        matches!(self.state, CommentPostState::Posting { .. })
    }

    pub(super) fn is_confirming(&self) -> bool {
        matches!(self.state, CommentPostState::Confirming { .. })
    }

    pub(super) fn composer_body<'a>(&'a self, input_body: &'a str) -> &'a str {
        match &self.state {
            CommentPostState::Confirming { body, .. } => body,
            CommentPostState::Idle
            | CommentPostState::Posting { .. }
            | CommentPostState::Error { .. } => input_body,
        }
    }

    pub(super) fn confirmation_details(&self) -> Option<(&str, &str, usize, usize)> {
        match &self.state {
            CommentPostState::Confirming {
                issue_key,
                body,
                chars,
                bytes,
                ..
            } => Some((issue_key, body, *chars, *bytes)),
            CommentPostState::Idle
            | CommentPostState::Posting { .. }
            | CommentPostState::Error { .. } => None,
        }
    }

    pub(super) fn error_details(&self) -> Option<(&str, bool)> {
        match &self.state {
            CommentPostState::Error {
                message,
                unknown_outcome,
                ..
            } => Some((message, *unknown_outcome)),
            CommentPostState::Idle
            | CommentPostState::Confirming { .. }
            | CommentPostState::Posting { .. } => None,
        }
    }

    pub(super) fn begin_confirmation(
        &mut self,
        target: CommentTarget,
        input_body: &str,
    ) -> Result<(), CommentValidationError> {
        if self.has_unknown_outcome() {
            return Err(CommentValidationError::UnknownOutcomeNeedsRefresh);
        }

        // Keep this order and copy aligned with the pre-refactor Dashboard flow.
        let body = input_body.trim().to_owned();
        if body.is_empty() {
            self.state = CommentPostState::Error {
                issue_id: target.issue_id,
                message: "Comment not posted · enter a non-empty comment".to_owned(),
                unknown_outcome: false,
            };
        } else if body.len() > MAX_COMMENT_BYTES {
            self.state = CommentPostState::Error {
                issue_id: target.issue_id,
                message: "Comment not posted · comment exceeds the byte limit".to_owned(),
                unknown_outcome: false,
            };
        } else if body.chars().count() > MAX_COMMENT_CHARS {
            self.state = CommentPostState::Error {
                issue_id: target.issue_id,
                message: "Comment not posted · comment exceeds the character limit".to_owned(),
                unknown_outcome: false,
            };
        } else {
            let chars = body.chars().count();
            let bytes = body.len();
            self.state = CommentPostState::Confirming {
                issue_id: target.issue_id,
                issue_key: target.issue_key,
                body,
                chars,
                bytes,
            };
        }
        Ok(())
    }

    pub(super) fn cancel_confirmation(&mut self) {
        self.state = CommentPostState::Idle;
    }

    pub(super) fn fail_without_dispatch(&mut self, issue_id: IssueId, message: &'static str) {
        self.state = CommentPostState::Error {
            issue_id,
            message: message.to_owned(),
            unknown_outcome: false,
        };
    }

    pub(super) fn has_confirmation_for(&self, current_target: &IssueId) -> bool {
        matches!(
            &self.state,
            CommentPostState::Confirming { issue_id, .. } if issue_id == current_target
        )
    }

    /// Atomically consumes the confirmation. A consumed confirmation cannot be
    /// obtained again, even if the caller activates post repeatedly.
    pub(super) fn consume_submission(
        &mut self,
        current_target: &IssueId,
    ) -> Option<CommentSubmission> {
        let CommentPostState::Confirming { issue_id, body, .. } = &self.state else {
            return None;
        };
        if current_target != issue_id {
            return None;
        }

        let issue_id = issue_id.clone();
        let body = body.clone();
        let generation = self.generation.wrapping_add(1);
        self.generation = generation;
        self.state = CommentPostState::Posting {
            issue_id: issue_id.clone(),
            generation,
        };
        Some(CommentSubmission {
            issue_id,
            body,
            generation,
        })
    }

    pub(super) fn invalidate_selection(&mut self) -> CommentInvalidation {
        let was_posting = self.is_posting();
        self.generation = self.generation.wrapping_add(1);
        self.state = CommentPostState::Idle;
        if was_posting {
            CommentInvalidation::IgnoreDispatchedCompletion
        } else {
            CommentInvalidation::CancelPreDispatch
        }
    }

    pub(super) fn clear_error_on_edit(&mut self) {
        if matches!(self.state, CommentPostState::Error { .. }) {
            self.state = CommentPostState::Idle;
        }
    }

    pub(super) fn can_refresh(&mut self) -> bool {
        if self.is_posting() {
            return false;
        }
        if self.has_unknown_outcome() {
            self.state = CommentPostState::Idle;
        }
        true
    }

    pub(super) fn finish_submission(
        &mut self,
        submission: &CommentSubmission,
        remote_issue_id: Option<&IssueId>,
        selected_issue: Option<&IssueId>,
        result: Result<(), &ApplicationError>,
    ) -> CommentCompletion {
        if !matches!(
            &self.state,
            CommentPostState::Posting {
                issue_id,
                generation,
            } if issue_id == &submission.issue_id && *generation == submission.generation
        ) || !comment_target_is_current(remote_issue_id, selected_issue, &submission.issue_id)
        {
            return CommentCompletion::Ignored;
        }

        match result {
            Ok(()) => {
                self.state = CommentPostState::Idle;
                CommentCompletion::Succeeded
            }
            Err(error) => {
                let (message, unknown_outcome) = comment_error_message(error);
                self.state = CommentPostState::Error {
                    issue_id: submission.issue_id.clone(),
                    message: message.to_owned(),
                    unknown_outcome,
                };
                CommentCompletion::Failed {
                    message,
                    unknown_outcome,
                }
            }
        }
    }

    fn has_unknown_outcome(&self) -> bool {
        matches!(
            self.state,
            CommentPostState::Error {
                unknown_outcome: true,
                ..
            }
        )
    }
}

pub(super) fn comment_target_is_current(
    remote_issue_id: Option<&IssueId>,
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
) -> bool {
    remote_issue_id.or(selected_issue) == Some(expected_issue)
}

pub(super) fn comment_error_message(error: &ApplicationError) -> (&'static str, bool) {
    match error.kind() {
        ErrorKind::Authentication => (
            "Comment not posted · Jira authentication was rejected",
            false,
        ),
        ErrorKind::Authorization => ("Comment not posted · Jira denied comment permission", false),
        ErrorKind::NotFound => ("Comment not posted · the Jira issue was not found", false),
        ErrorKind::RateLimited => (
            "Comment not posted · Jira rate limit reached; try later",
            false,
        ),
        ErrorKind::InvalidInput => ("Comment not posted · the comment text is invalid", false),
        ErrorKind::UnknownOutcome => (
            "Jira may have accepted this comment. Refresh comments before retrying.",
            true,
        ),
        _ => ("Comment not posted · Jira returned an error", false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jira_application::ErrorKind;

    fn target(id: &str, key: &str) -> CommentTarget {
        CommentTarget {
            issue_id: IssueId::new(id).expect("issue"),
            issue_key: key.to_owned(),
        }
    }

    fn error(kind: ErrorKind, detail: &str) -> ApplicationError {
        ApplicationError::new(kind, detail)
    }

    #[test]
    fn validation_preserves_order_messages_and_utf8_counts() {
        let issue = target("100", "IX-100");
        let mut flow = CommentFlow::new();
        flow.begin_confirmation(issue.clone(), "   ")
            .expect("handled");
        assert_eq!(
            flow.error_details(),
            Some(("Comment not posted · enter a non-empty comment", false,))
        );

        flow.begin_confirmation(issue.clone(), &"😀".repeat(MAX_COMMENT_BYTES / 4 + 1))
            .expect("handled");
        assert_eq!(
            flow.error_details(),
            Some(("Comment not posted · comment exceeds the byte limit", false,))
        );

        flow.begin_confirmation(issue.clone(), &"x".repeat(MAX_COMMENT_CHARS + 1))
            .expect("handled");
        assert_eq!(
            flow.error_details(),
            Some((
                "Comment not posted · comment exceeds the character limit",
                false,
            ))
        );

        flow.begin_confirmation(issue, "  café 😀  ")
            .expect("valid");
        assert_eq!(
            flow.confirmation_details(),
            Some(("IX-100", "café 😀", 6, 10))
        );
    }

    #[test]
    fn confirmation_is_immutable_and_consumed_once() {
        let issue = target("100", "IX-100");
        let mut flow = CommentFlow::new();
        flow.begin_confirmation(issue, "  original  ")
            .expect("valid");
        assert_eq!(flow.composer_body("edited"), "original");

        let submission = flow
            .consume_submission(&IssueId::new("100").expect("issue"))
            .expect("first submission");
        assert_eq!(submission.body, "original");
        assert!(flow.consume_submission(&submission.issue_id).is_none());
        assert!(flow.is_posting());
    }

    #[test]
    fn target_mismatch_does_not_consume_confirmation() {
        let mut flow = CommentFlow::new();
        flow.begin_confirmation(target("100", "IX-100"), "body")
            .expect("valid");
        let other = IssueId::new("200").expect("issue");
        assert!(flow.consume_submission(&other).is_none());
        assert!(flow.is_confirming());
    }

    #[test]
    fn selection_invalidation_distinguishes_pre_dispatch_and_post_dispatch() {
        let mut flow = CommentFlow::new();
        flow.begin_confirmation(target("100", "IX-100"), "body")
            .expect("valid");
        assert_eq!(
            flow.invalidate_selection(),
            CommentInvalidation::CancelPreDispatch
        );
        assert!(!flow.is_confirming());

        flow.begin_confirmation(target("100", "IX-100"), "body")
            .expect("valid");
        let submission = flow
            .consume_submission(&IssueId::new("100").expect("issue"))
            .expect("submission");
        assert_eq!(
            flow.invalidate_selection(),
            CommentInvalidation::IgnoreDispatchedCompletion
        );
        assert_eq!(
            flow.finish_submission(&submission, None, None, Ok(())),
            CommentCompletion::Ignored
        );
    }

    #[test]
    fn generation_and_target_checks_reject_stale_completions() {
        let mut flow = CommentFlow::new();
        flow.begin_confirmation(target("100", "IX-100"), "body")
            .expect("valid");
        let submission = flow
            .consume_submission(&IssueId::new("100").expect("issue"))
            .expect("submission");
        let other = IssueId::new("200").expect("issue");
        assert_eq!(
            flow.finish_submission(&submission, None, Some(&other), Ok(())),
            CommentCompletion::Ignored
        );
        assert_eq!(
            flow.finish_submission(&submission, None, Some(&submission.issue_id), Ok(())),
            CommentCompletion::Succeeded
        );
        assert_eq!(
            flow.finish_submission(&submission, None, Some(&submission.issue_id), Ok(())),
            CommentCompletion::Ignored
        );
    }

    #[test]
    fn same_target_newer_generation_ignores_old_completion_without_disturbing_new_post() {
        let mut flow = CommentFlow::new();
        let issue = IssueId::new("100").expect("issue");

        flow.begin_confirmation(target("100", "IX-100"), "older")
            .expect("valid");
        let older = flow.consume_submission(&issue).expect("older submission");
        assert_eq!(
            flow.invalidate_selection(),
            CommentInvalidation::IgnoreDispatchedCompletion
        );

        flow.begin_confirmation(target("100", "IX-100"), "newer")
            .expect("valid");
        let newer = flow.consume_submission(&issue).expect("newer submission");
        assert_ne!(older.generation, newer.generation);
        assert!(flow.is_posting());

        assert_eq!(
            flow.finish_submission(&older, None, Some(&issue), Ok(())),
            CommentCompletion::Ignored
        );
        assert!(flow.is_posting());

        assert_eq!(
            flow.finish_submission(&newer, None, Some(&issue), Ok(())),
            CommentCompletion::Succeeded
        );
        assert!(!flow.is_posting());
        assert_eq!(
            flow.finish_submission(&newer, None, Some(&issue), Ok(())),
            CommentCompletion::Ignored
        );
    }

    #[test]
    fn error_mapping_is_exact_safe_and_unknown_outcome_blocks_retry_until_refresh_without_edit() {
        let expected = [
            (
                ErrorKind::Authentication,
                (
                    "Comment not posted · Jira authentication was rejected",
                    false,
                ),
            ),
            (
                ErrorKind::Authorization,
                ("Comment not posted · Jira denied comment permission", false),
            ),
            (
                ErrorKind::NotFound,
                ("Comment not posted · the Jira issue was not found", false),
            ),
            (
                ErrorKind::RateLimited,
                (
                    "Comment not posted · Jira rate limit reached; try later",
                    false,
                ),
            ),
            (
                ErrorKind::InvalidInput,
                ("Comment not posted · the comment text is invalid", false),
            ),
            (
                ErrorKind::UnknownOutcome,
                (
                    "Jira may have accepted this comment. Refresh comments before retrying.",
                    true,
                ),
            ),
            (
                ErrorKind::Offline,
                ("Comment not posted · Jira returned an error", false),
            ),
            (
                ErrorKind::Cancelled,
                ("Comment not posted · Jira returned an error", false),
            ),
            (
                ErrorKind::Storage,
                ("Comment not posted · Jira returned an error", false),
            ),
            (
                ErrorKind::Upstream,
                ("Comment not posted · Jira returned an error", false),
            ),
            (
                ErrorKind::Notification,
                ("Comment not posted · Jira returned an error", false),
            ),
            (
                ErrorKind::Internal,
                ("Comment not posted · Jira returned an error", false),
            ),
        ];
        for (kind, expected_tuple) in expected {
            let detail = "secret detail";
            let actual = comment_error_message(&error(kind, detail));
            assert_eq!(actual, expected_tuple, "unexpected mapping for {kind:?}");
            assert!(!actual.0.contains(detail), "raw detail leaked for {kind:?}");
        }

        let mut flow = CommentFlow::new();
        flow.begin_confirmation(target("100", "IX-100"), "body")
            .expect("valid");
        let submission = flow
            .consume_submission(&IssueId::new("100").expect("issue"))
            .expect("submission");
        assert_eq!(
            flow.finish_submission(
                &submission,
                None,
                Some(&submission.issue_id),
                Err(&error(ErrorKind::UnknownOutcome, "secret")),
            ),
            CommentCompletion::Failed {
                message: "Jira may have accepted this comment. Refresh comments before retrying.",
                unknown_outcome: true,
            }
        );
        assert!(!flow.is_confirming());
        assert!(matches!(
            flow.begin_confirmation(target("100", "IX-100"), "retry"),
            Err(CommentValidationError::UnknownOutcomeNeedsRefresh)
        ));
        assert!(flow.can_refresh());
        flow.begin_confirmation(target("100", "IX-100"), "retry")
            .expect("allowed after refresh");
    }

    #[test]
    fn editing_any_error_clears_it_including_unknown_outcome() {
        let mut flow = CommentFlow::new();
        flow.begin_confirmation(target("100", "IX-100"), "body")
            .expect("valid");
        let submission = flow
            .consume_submission(&IssueId::new("100").expect("issue"))
            .expect("submission");
        flow.finish_submission(
            &submission,
            None,
            Some(&submission.issue_id),
            Err(&error(ErrorKind::UnknownOutcome, "secret")),
        );
        assert!(flow.has_unknown_outcome());
        flow.clear_error_on_edit();
        assert!(!flow.has_unknown_outcome());
        assert!(flow.error_details().is_none());
        flow.begin_confirmation(target("100", "IX-100"), "edited")
            .expect("editing clears the error today");
    }

    #[test]
    fn refresh_is_gated_while_posting_and_clears_unknown_outcome() {
        let mut flow = CommentFlow::new();
        flow.begin_confirmation(target("100", "IX-100"), "body")
            .expect("valid");
        let submission = flow
            .consume_submission(&IssueId::new("100").expect("issue"))
            .expect("submission");
        assert!(!flow.can_refresh());
        flow.finish_submission(
            &submission,
            None,
            Some(&submission.issue_id),
            Err(&error(ErrorKind::UnknownOutcome, "secret")),
        );
        assert!(flow.can_refresh());
        assert!(!flow.has_unknown_outcome());
        assert!(!flow.is_posting());
    }
}
