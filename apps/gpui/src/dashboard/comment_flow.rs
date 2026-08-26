use jira_application::{ApplicationError, ErrorKind};

#[cfg(test)]
use jira_application::{MAX_COMMENT_BYTES, MAX_COMMENT_CHARS};
use jira_domain::IssueId;

use crate::presentation::{
    CommentOutcomeKind, OutcomeCopy, comment_outcome_copy, comment_validation_kind,
};

#[cfg(test)]
use crate::presentation::{FeedbackCertainty, RecoveryDirective};

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
    Failed { copy: OutcomeCopy },
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
        copy: OutcomeCopy,
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

    pub(super) fn error_details(&self) -> Option<OutcomeCopy> {
        match &self.state {
            CommentPostState::Error { copy, .. } => Some(*copy),
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
        if matches!(
            self.state,
            CommentPostState::Error { copy, .. } if copy.is_unknown()
        ) {
            return Err(CommentValidationError::UnknownOutcomeNeedsRefresh);
        }

        // Keep this order and copy aligned with the pre-refactor Dashboard flow.
        let body = input_body.trim().to_owned();
        if let Some(kind) = comment_validation_kind(&body) {
            self.state = CommentPostState::Error {
                issue_id: target.issue_id,
                copy: comment_outcome_copy(kind),
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

    pub(super) fn fail_without_dispatch(&mut self, issue_id: IssueId, copy: OutcomeCopy) {
        self.state = CommentPostState::Error { issue_id, copy };
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
        if matches!(
            self.state,
            CommentPostState::Error { copy, .. } if !copy.is_unknown()
        ) {
            self.state = CommentPostState::Idle;
        }
    }

    pub(super) fn can_refresh(&mut self) -> bool {
        if self.is_posting() {
            return false;
        }
        if matches!(
            self.state,
            CommentPostState::Error { copy, .. } if copy.is_unknown()
        ) {
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
                let copy = comment_error_message(error.kind());
                self.state = CommentPostState::Error {
                    issue_id: submission.issue_id.clone(),
                    copy,
                };
                CommentCompletion::Failed { copy }
            }
        }
    }
}

pub(super) fn comment_target_is_current(
    remote_issue_id: Option<&IssueId>,
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
) -> bool {
    remote_issue_id.or(selected_issue) == Some(expected_issue)
}

pub(super) fn comment_error_message(kind: ErrorKind) -> OutcomeCopy {
    comment_outcome_copy(CommentOutcomeKind::Error(kind))
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
            Some(comment_outcome_copy(CommentOutcomeKind::ValidationEmpty))
        );

        flow.begin_confirmation(issue.clone(), &"😀".repeat(MAX_COMMENT_BYTES / 4 + 1))
            .expect("handled");
        assert_eq!(
            flow.error_details(),
            Some(comment_outcome_copy(
                CommentOutcomeKind::ValidationByteLimit
            ))
        );

        flow.begin_confirmation(issue.clone(), &"x".repeat(MAX_COMMENT_CHARS + 1))
            .expect("handled");
        assert_eq!(
            flow.error_details(),
            Some(comment_outcome_copy(
                CommentOutcomeKind::ValidationCharacterLimit
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
        let kinds = [
            ErrorKind::Authentication,
            ErrorKind::Authorization,
            ErrorKind::NotFound,
            ErrorKind::RateLimited,
            ErrorKind::InvalidInput,
            ErrorKind::UnknownOutcome,
            ErrorKind::Offline,
            ErrorKind::Cancelled,
            ErrorKind::Storage,
            ErrorKind::Upstream,
            ErrorKind::Notification,
            ErrorKind::Internal,
        ];
        let expected = [
            (
                ErrorKind::Authentication,
                "Comment not posted · Jira authentication was rejected",
            ),
            (
                ErrorKind::Authorization,
                "Comment not posted · Jira denied comment permission",
            ),
            (
                ErrorKind::NotFound,
                "Comment not posted · the Jira issue was not found",
            ),
            (
                ErrorKind::RateLimited,
                "Comment not posted · Jira rate limit reached; try later",
            ),
            (
                ErrorKind::InvalidInput,
                "Comment not posted · the comment text is invalid",
            ),
            (
                ErrorKind::UnknownOutcome,
                "Jira may have accepted this comment. Refresh comments before retrying.",
            ),
            (
                ErrorKind::Offline,
                "Comment not posted · Jira returned an error",
            ),
            (
                ErrorKind::Cancelled,
                "Comment not posted · Jira returned an error",
            ),
            (
                ErrorKind::Storage,
                "Comment not posted · Jira returned an error",
            ),
            (
                ErrorKind::Upstream,
                "Comment not posted · Jira returned an error",
            ),
            (
                ErrorKind::Notification,
                "Comment not posted · Jira returned an error",
            ),
            (
                ErrorKind::Internal,
                "Comment not posted · Jira returned an error",
            ),
        ];
        for ((kind, expected_message), listed_kind) in expected.into_iter().zip(kinds) {
            assert_eq!(listed_kind, kind);
            let actual = comment_error_message(kind);
            assert_eq!(
                actual.message(),
                expected_message,
                "unexpected mapping for {kind:?}"
            );
            assert_eq!(
                actual.severity(),
                crate::presentation::FeedbackSeverity::Error
            );
            assert_eq!(
                actual.certainty(),
                if kind == ErrorKind::UnknownOutcome {
                    FeedbackCertainty::Unknown
                } else {
                    FeedbackCertainty::Definite
                }
            );
            assert_eq!(
                actual.recovery(),
                if kind == ErrorKind::UnknownOutcome {
                    RecoveryDirective::Refresh
                } else {
                    RecoveryDirective::Retry
                }
            );
            assert!(!actual.message().contains("secret detail"));
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
                copy: OutcomeCopy::new(
                    "Jira may have accepted this comment. Refresh comments before retrying.",
                    crate::presentation::FeedbackSeverity::Error,
                    FeedbackCertainty::Unknown,
                    RecoveryDirective::Refresh,
                ),
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
    fn editing_definite_error_clears_but_unknown_outcome_stays_blocked_until_refresh() {
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
        assert_eq!(
            flow.error_details().expect("error copy").recovery(),
            RecoveryDirective::Refresh
        );
        flow.clear_error_on_edit();
        assert!(flow.error_details().is_some());
        assert!(matches!(
            flow.begin_confirmation(target("100", "IX-100"), "edited"),
            Err(CommentValidationError::UnknownOutcomeNeedsRefresh)
        ));
        assert!(flow.can_refresh());
        flow.begin_confirmation(target("100", "IX-100"), "edited")
            .expect("allowed after refresh");
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
        assert!(flow.error_details().is_none());
        assert!(!flow.is_posting());
    }
}
