use jira_application::{ApplicationError, ErrorKind, IssueTransition};
use jira_domain::{AccountId, IssueId, User};

#[cfg(test)]
use crate::presentation::{FeedbackCertainty, RecoveryDirective};

use crate::presentation::{
    IssueEditPhase, OutcomeCopy, issue_edit_error_copy, issue_edit_workspace_unavailable_copy,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum IssueEditState {
    Idle,
    LoadingAssignees {
        issue_id: IssueId,
        query: String,
    },
    AssigneeChooser {
        issue_id: IssueId,
        issue_key: String,
        query: String,
        users: Vec<User>,
    },
    LoadingTransitions {
        issue_id: IssueId,
    },
    TransitionChooser {
        issue_id: IssueId,
        issue_key: String,
        transitions: Vec<IssueTransition>,
    },
    ConfirmingAssignee {
        issue_id: IssueId,
        issue_key: String,
        account_id: Option<AccountId>,
        display_name: String,
    },
    ConfirmingTransition {
        issue_id: IssueId,
        issue_key: String,
        transition_id: String,
        transition_name: String,
        target_status: String,
    },
    Submitting {
        identity: SubmissionIdentity,
        target: String,
    },
    Error {
        issue_id: IssueId,
        copy: OutcomeCopy,
        operation: IssueEditOperation,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IssueEditOperation {
    Assignee,
    Transition,
}

/// Immutable identity for one user-confirmed write attempt.
///
/// The generation is allocated only when a confirmation is consumed, so a
/// second activation cannot reuse the first attempt's identity even when the
/// issue and operation are unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SubmissionIdentity {
    issue_id: IssueId,
    operation: IssueEditOperation,
    generation: u64,
}

impl SubmissionIdentity {
    pub(super) fn issue_id(&self) -> &IssueId {
        &self.issue_id
    }

    pub(super) fn operation(&self) -> IssueEditOperation {
        self.operation
    }

    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AssigneeSubmission {
    pub(super) issue_id: IssueId,
    pub(super) account_id: Option<AccountId>,
    pub(super) display_name: String,
    identity: SubmissionIdentity,
}

impl AssigneeSubmission {
    pub(super) fn identity(&self) -> &SubmissionIdentity {
        &self.identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TransitionSubmission {
    pub(super) issue_id: IssueId,
    pub(super) transition_id: String,
    pub(super) transition_name: String,
    pub(super) target_status: String,
    identity: SubmissionIdentity,
}

impl TransitionSubmission {
    pub(super) fn identity(&self) -> &SubmissionIdentity {
        &self.identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum IssueEditSubmission {
    Assignee(AssigneeSubmission),
    Transition(TransitionSubmission),
}

impl IssueEditSubmission {
    pub(super) fn identity(&self) -> &SubmissionIdentity {
        match self {
            Self::Assignee(submission) => submission.identity(),
            Self::Transition(submission) => submission.identity(),
        }
    }

    pub(super) fn target(&self) -> String {
        match self {
            Self::Assignee(submission) => {
                format!("assignee {}", submission.display_name)
            }
            Self::Transition(submission) => {
                format!("status {}", submission.target_status)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BusyDirective {
    Retain,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IssueEditCompletion {
    Applied,
    Failed { copy: OutcomeCopy },
    Ignored { busy: BusyDirective },
}

pub(super) fn issue_edit_error_message(kind: ErrorKind, phase: IssueEditPhase) -> OutcomeCopy {
    issue_edit_error_copy(kind, phase)
}

pub(super) fn status_control_is_editable(
    has_workspace: bool,
    is_selected_issue: bool,
    is_remote_lookup: bool,
    operation_in_progress: bool,
    issue_edit_state: &IssueEditState,
) -> bool {
    has_workspace
        && is_selected_issue
        && !is_remote_lookup
        && !operation_in_progress
        && matches!(
            issue_edit_state,
            IssueEditState::Idle
                | IssueEditState::LoadingTransitions { .. }
                | IssueEditState::TransitionChooser { .. }
        )
}

pub(super) fn issue_edit_target_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    selected_issue == Some(expected_issue) && generation == expected_generation
}

#[derive(Clone, Debug)]
pub(super) struct IssueEditFlow {
    state: IssueEditState,
    status_popover_open: bool,
    generation: u64,
    reconciliation_pending: bool,
    invalidated_submissions: Vec<SubmissionIdentity>,
    last_submission: Option<SubmissionIdentity>,
}

impl IssueEditFlow {
    pub(super) fn new() -> Self {
        Self {
            state: IssueEditState::Idle,
            status_popover_open: false,
            generation: 0,
            reconciliation_pending: false,
            invalidated_submissions: Vec::new(),
            last_submission: None,
        }
    }

    pub(super) fn state(&self) -> &IssueEditState {
        &self.state
    }

    pub(super) fn status_popover_open(&self) -> bool {
        self.status_popover_open
    }

    pub(super) fn set_status_popover_open(&mut self, open: bool) {
        self.status_popover_open = open;
    }

    pub(super) fn is_submitting(&self) -> bool {
        matches!(self.state, IssueEditState::Submitting { .. })
    }

    pub(super) fn reconciliation_pending(&self) -> bool {
        self.reconciliation_pending
    }

    pub(super) fn begin_assignee_loading(&mut self, issue_id: IssueId, query: String) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.state = IssueEditState::LoadingAssignees { issue_id, query };
        self.generation
    }

    pub(super) fn begin_transition_loading(&mut self, issue_id: IssueId) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.state = IssueEditState::LoadingTransitions { issue_id };
        self.generation
    }

    pub(super) fn target_is_current(
        &self,
        selected_issue: Option<&IssueId>,
        expected_issue: &IssueId,
        expected_generation: u64,
    ) -> bool {
        issue_edit_target_is_current(
            selected_issue,
            expected_issue,
            self.generation,
            expected_generation,
        )
    }

    pub(super) fn finish_assignee_loading(
        &mut self,
        selected_issue: Option<&IssueId>,
        issue_id: IssueId,
        issue_key: String,
        query: String,
        expected_generation: u64,
        result: Result<Vec<User>, ApplicationError>,
    ) -> bool {
        if !self.target_is_current(selected_issue, &issue_id, expected_generation) {
            return false;
        }
        self.state = match result {
            Ok(users) => IssueEditState::AssigneeChooser {
                issue_id,
                issue_key,
                query,
                users,
            },
            Err(error) => {
                let copy = issue_edit_error_message(error.kind(), IssueEditPhase::Lookup);
                IssueEditState::Error {
                    issue_id,
                    copy,
                    operation: IssueEditOperation::Assignee,
                }
            }
        };
        true
    }

    pub(super) fn finish_transition_loading(
        &mut self,
        selected_issue: Option<&IssueId>,
        issue_id: IssueId,
        issue_key: String,
        expected_generation: u64,
        result: Result<Vec<IssueTransition>, ApplicationError>,
    ) -> bool {
        if !self.target_is_current(selected_issue, &issue_id, expected_generation) {
            return false;
        }
        self.state = match result {
            Ok(transitions) => IssueEditState::TransitionChooser {
                issue_id,
                issue_key,
                transitions,
            },
            Err(error) => {
                let copy = issue_edit_error_message(error.kind(), IssueEditPhase::Lookup);
                self.status_popover_open = false;
                IssueEditState::Error {
                    issue_id,
                    copy,
                    operation: IssueEditOperation::Transition,
                }
            }
        };
        true
    }

    pub(super) fn choose_assignee(&mut self, account_id: Option<AccountId>, display_name: String) {
        let IssueEditState::AssigneeChooser {
            issue_id,
            issue_key,
            ..
        } = &self.state
        else {
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        self.state = IssueEditState::ConfirmingAssignee {
            issue_id: issue_id.clone(),
            issue_key: issue_key.clone(),
            account_id,
            display_name,
        };
    }

    pub(super) fn choose_transition(&mut self, transition: IssueTransition) {
        let IssueEditState::TransitionChooser {
            issue_id,
            issue_key,
            ..
        } = &self.state
        else {
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        self.state = IssueEditState::ConfirmingTransition {
            issue_id: issue_id.clone(),
            issue_key: issue_key.clone(),
            transition_id: transition.id,
            transition_name: transition.name,
            target_status: transition.to.name,
        };
        self.status_popover_open = false;
    }

    pub(super) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let IssueEditState::Submitting { identity, .. } = &self.state {
            self.invalidated_submissions.push(identity.clone());
        }
        self.state = IssueEditState::Idle;
        self.status_popover_open = false;
    }

    /// Invalidates reads and pre-dispatch confirmation without cancelling a
    /// dispatched write. A dispatched identity is retained until its detached
    /// completion arrives, allowing the dashboard to release only that write's
    /// busy flag without touching a newer attempt.
    pub(super) fn invalidate_selection(&mut self) -> bool {
        self.generation = self.generation.wrapping_add(1);
        if let IssueEditState::Submitting { identity, .. } = &self.state {
            self.invalidated_submissions.push(identity.clone());
            self.state = IssueEditState::Idle;
            self.status_popover_open = false;
            return false;
        }
        self.state = IssueEditState::Idle;
        self.status_popover_open = false;
        true
    }

    fn next_submission_identity(
        &mut self,
        issue_id: &IssueId,
        operation: IssueEditOperation,
    ) -> SubmissionIdentity {
        self.generation = self.generation.wrapping_add(1);
        SubmissionIdentity {
            issue_id: issue_id.clone(),
            operation,
            generation: self.generation,
        }
    }

    pub(super) fn consume_assignee_submission(&mut self) -> Option<IssueEditSubmission> {
        let (issue_id, account_id, display_name) = match &self.state {
            IssueEditState::ConfirmingAssignee {
                issue_id,
                account_id,
                display_name,
                ..
            } => (issue_id.clone(), account_id.clone(), display_name.clone()),
            _ => return None,
        };
        let identity = self.next_submission_identity(&issue_id, IssueEditOperation::Assignee);
        let submission = IssueEditSubmission::Assignee(AssigneeSubmission {
            issue_id,
            account_id,
            display_name,
            identity: identity.clone(),
        });
        self.last_submission = Some(identity.clone());
        self.state = IssueEditState::Submitting {
            identity,
            target: submission.target(),
        };
        Some(submission)
    }

    pub(super) fn consume_transition_submission(&mut self) -> Option<IssueEditSubmission> {
        let (issue_id, transition_id, transition_name, target_status) = match &self.state {
            IssueEditState::ConfirmingTransition {
                issue_id,
                transition_id,
                transition_name,
                target_status,
                ..
            } => (
                issue_id.clone(),
                transition_id.clone(),
                transition_name.clone(),
                target_status.clone(),
            ),
            _ => return None,
        };
        let identity = self.next_submission_identity(&issue_id, IssueEditOperation::Transition);
        let submission = IssueEditSubmission::Transition(TransitionSubmission {
            issue_id,
            transition_id,
            transition_name,
            target_status,
            identity: identity.clone(),
        });
        self.last_submission = Some(identity.clone());
        self.state = IssueEditState::Submitting {
            identity,
            target: submission.target(),
        };
        Some(submission)
    }

    pub(super) fn unavailable(&mut self, issue_id: IssueId, operation: IssueEditOperation) {
        self.state = IssueEditState::Error {
            issue_id,
            copy: issue_edit_workspace_unavailable_copy(),
            operation,
        };
    }

    /// Applies a completion only when its immutable identity still names the
    /// current submission and the selected issue. Every other completion is
    /// inert; an invalidated detached attempt may additionally release its old
    /// dashboard busy flag when no newer attempt owns it.
    pub(super) fn finish_write(
        &mut self,
        identity: SubmissionIdentity,
        selected_issue: Option<&IssueId>,
        result: Result<(), ApplicationError>,
    ) -> IssueEditCompletion {
        let is_current = matches!(
            &self.state,
            IssueEditState::Submitting {
                identity: current,
                ..
            } if current == &identity && selected_issue == Some(identity.issue_id())
        );
        if is_current {
            return match result {
                Ok(()) => {
                    self.state = IssueEditState::Idle;
                    self.reconciliation_pending = true;
                    IssueEditCompletion::Applied
                }
                Err(error) => {
                    let copy = issue_edit_error_message(error.kind(), IssueEditPhase::Write);
                    self.state = IssueEditState::Error {
                        issue_id: identity.issue_id().clone(),
                        copy,
                        operation: identity.operation(),
                    };
                    IssueEditCompletion::Failed { copy }
                }
            };
        }

        let Some(index) = self
            .invalidated_submissions
            .iter()
            .position(|invalidated| invalidated == &identity)
        else {
            return IssueEditCompletion::Ignored {
                busy: BusyDirective::Retain,
            };
        };
        self.invalidated_submissions.swap_remove(index);
        let superseded = self
            .last_submission
            .as_ref()
            .is_some_and(|latest| latest != &identity);
        IssueEditCompletion::Ignored {
            busy: if superseded {
                BusyDirective::Retain
            } else {
                BusyDirective::Release
            },
        }
    }

    pub(super) fn refresh_failed(&mut self) {
        // An unsuccessful Jira refresh cannot reconcile an uncertain write.
    }

    pub(super) fn refresh_succeeded(&mut self) {
        self.reconciliation_pending = false;
        if matches!(self.state, IssueEditState::Error { copy, .. } if copy.is_unknown()) {
            self.state = IssueEditState::Idle;
        }
    }

    #[cfg(test)]
    pub(super) fn set_state_for_test(&mut self, state: IssueEditState) {
        self.state = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(value: &str) -> IssueId {
        IssueId::new(value).expect("issue id")
    }

    fn transition(id: &str, name: &str, target: &str) -> IssueTransition {
        IssueTransition {
            id: id.to_owned(),
            name: name.to_owned(),
            to: jira_domain::Status {
                id: id.to_owned(),
                name: target.to_owned(),
                category: None,
            },
        }
    }

    #[test]
    fn submissions_are_typed_and_consumed_once() {
        let mut flow = IssueEditFlow::new();
        flow.set_state_for_test(IssueEditState::ConfirmingAssignee {
            issue_id: issue("1"),
            issue_key: "IX-1".to_owned(),
            account_id: None,
            display_name: "Unassigned".to_owned(),
        });
        let first = flow.consume_assignee_submission().expect("submission");
        let assignee_identity = first.identity().clone();
        assert_eq!(assignee_identity.issue_id(), &issue("1"));
        assert_eq!(assignee_identity.operation(), IssueEditOperation::Assignee);
        assert_eq!(assignee_identity.generation(), 1);
        assert!(matches!(
            &first,
            IssueEditSubmission::Assignee(AssigneeSubmission {
                issue_id,
                account_id: None,
                display_name,
                ..
            }) if issue_id == &issue("1") && display_name == "Unassigned"
        ));
        assert_eq!(first.target(), "assignee Unassigned");
        assert!(matches!(
            flow.state(),
            IssueEditState::Submitting { identity, target }
                if identity == &assignee_identity && target == "assignee Unassigned"
        ));
        assert!(flow.consume_assignee_submission().is_none());

        flow.set_state_for_test(IssueEditState::ConfirmingTransition {
            issue_id: issue("1"),
            issue_key: "IX-1".to_owned(),
            transition_id: "2".to_owned(),
            transition_name: "Start".to_owned(),
            target_status: "In Progress".to_owned(),
        });
        let first = flow
            .consume_transition_submission()
            .expect("transition submission");
        let transition_identity = first.identity().clone();
        assert_eq!(transition_identity.issue_id(), &issue("1"));
        assert_eq!(
            transition_identity.operation(),
            IssueEditOperation::Transition
        );
        assert_eq!(transition_identity.generation(), 2);
        assert!(matches!(
            &first,
            IssueEditSubmission::Transition(TransitionSubmission {
                issue_id,
                transition_id,
                transition_name,
                target_status,
                ..
            }) if issue_id == &issue("1")
                && transition_id == "2"
                && transition_name == "Start"
                && target_status == "In Progress"
        ));
        assert_eq!(first.target(), "status In Progress");
        assert!(matches!(
            flow.state(),
            IssueEditState::Submitting { identity, target }
                if identity == &transition_identity && target == "status In Progress"
        ));
        assert!(flow.consume_transition_submission().is_none());
    }

    #[test]
    fn loading_generation_and_target_checks_reject_stale_results() {
        let selected = issue("1");
        let other = issue("2");
        let mut flow = IssueEditFlow::new();
        let first = flow.begin_assignee_loading(selected.clone(), "".to_owned());
        let second = flow.begin_assignee_loading(selected.clone(), "exact".to_owned());
        assert!(!flow.target_is_current(Some(&selected), &selected, first));
        assert!(!flow.target_is_current(Some(&other), &selected, second));
        assert!(flow.target_is_current(Some(&selected), &selected, second));
        assert!(!flow.finish_assignee_loading(
            Some(&selected),
            selected.clone(),
            "IX-1".to_owned(),
            "".to_owned(),
            first,
            Ok(Vec::new()),
        ));
    }

    #[test]
    fn chooser_order_and_confirmation_data_are_preserved() {
        let selected = issue("1");
        let mut flow = IssueEditFlow::new();
        flow.set_status_popover_open(true);
        let generation = flow.begin_transition_loading(selected.clone());
        assert!(flow.finish_transition_loading(
            Some(&selected),
            selected.clone(),
            "IX-1".to_owned(),
            generation,
            Ok(vec![
                transition("1", "First action", "To Do"),
                transition("2", "Second action", "Done"),
            ]),
        ));
        assert!(matches!(
            flow.state(),
            IssueEditState::TransitionChooser { transitions, .. }
                if transitions[0].name == "First action" && transitions[1].name == "Second action"
        ));
        flow.choose_transition(transition("2", "Second action", "Done"));
        assert!(matches!(
            flow.state(),
            IssueEditState::ConfirmingTransition {
                transition_id,
                target_status,
                ..
            } if transition_id == "2" && target_status == "Done"
        ));
        assert!(!flow.status_popover_open());
    }

    #[test]
    fn selection_invalidation_preserves_only_dispatched_submission() {
        let selected = issue("1");
        let mut flow = IssueEditFlow::new();
        flow.set_state_for_test(IssueEditState::ConfirmingAssignee {
            issue_id: selected.clone(),
            issue_key: "IX-1".to_owned(),
            account_id: None,
            display_name: "Unassigned".to_owned(),
        });
        assert!(flow.invalidate_selection());
        assert!(matches!(flow.state(), IssueEditState::Idle));

        flow.set_state_for_test(IssueEditState::ConfirmingTransition {
            issue_id: selected,
            issue_key: "IX-1".to_owned(),
            transition_id: "2".to_owned(),
            transition_name: "Start".to_owned(),
            target_status: "In Progress".to_owned(),
        });
        let submission = flow
            .consume_transition_submission()
            .expect("detached submission");
        assert!(!flow.invalidate_selection());
        assert!(matches!(flow.state(), IssueEditState::Idle));
        assert_eq!(
            flow.finish_write(submission.identity().clone(), Some(&issue("2")), Ok(())),
            IssueEditCompletion::Ignored {
                busy: BusyDirective::Release,
            }
        );
        assert!(matches!(flow.state(), IssueEditState::Idle));
    }

    #[test]
    fn completion_and_unknown_reconciliation_require_successful_refresh() {
        let selected = issue("1");
        let mut flow = IssueEditFlow::new();
        flow.set_state_for_test(IssueEditState::ConfirmingAssignee {
            issue_id: selected.clone(),
            issue_key: "IX-1".to_owned(),
            account_id: None,
            display_name: "Unassigned".to_owned(),
        });
        let submission = flow.consume_assignee_submission().expect("submission");
        assert_eq!(
            flow.finish_write(
                submission.identity().clone(),
                Some(&selected),
                Err(ApplicationError::new(ErrorKind::UnknownOutcome, "secret")),
            ),
            IssueEditCompletion::Failed {
                copy: issue_edit_error_copy(ErrorKind::UnknownOutcome, IssueEditPhase::Write)
            }
        );
        assert!(!matches!(flow.state(), IssueEditState::Idle));
        flow.refresh_failed();
        assert!(!matches!(flow.state(), IssueEditState::Idle));
        flow.refresh_succeeded();
        assert!(matches!(flow.state(), IssueEditState::Idle));

        flow.set_state_for_test(IssueEditState::ConfirmingTransition {
            issue_id: selected.clone(),
            issue_key: "IX-1".to_owned(),
            transition_id: "2".to_owned(),
            transition_name: "Start".to_owned(),
            target_status: "In Progress".to_owned(),
        });
        let submission = flow.consume_transition_submission().expect("submission");
        assert_eq!(
            flow.finish_write(submission.identity().clone(), Some(&selected), Ok(())),
            IssueEditCompletion::Applied
        );
        assert!(flow.reconciliation_pending());
        flow.refresh_succeeded();
        assert!(!flow.reconciliation_pending());

        flow.set_state_for_test(IssueEditState::ConfirmingAssignee {
            issue_id: selected.clone(),
            issue_key: "IX-1".to_owned(),
            account_id: None,
            display_name: "Nobody".to_owned(),
        });
        let definite = flow
            .consume_assignee_submission()
            .expect("definite submission");
        assert_eq!(
            flow.finish_write(
                definite.identity().clone(),
                Some(&selected),
                Err(ApplicationError::new(ErrorKind::Authorization, "denied")),
            ),
            IssueEditCompletion::Failed {
                copy: issue_edit_error_copy(ErrorKind::Authorization, IssueEditPhase::Write)
            }
        );
        let definite_state = flow.state().clone();
        flow.refresh_failed();
        assert_eq!(flow.state(), &definite_state);
        flow.refresh_succeeded();
        assert_eq!(flow.state(), &definite_state);
    }

    #[test]
    fn stale_duplicate_mismatched_and_superseded_completions_are_ignored() {
        let selected = issue("1");
        let mut flow = IssueEditFlow::new();
        flow.set_state_for_test(IssueEditState::ConfirmingAssignee {
            issue_id: selected.clone(),
            issue_key: "IX-1".to_owned(),
            account_id: None,
            display_name: "Alice".to_owned(),
        });
        let old = flow.consume_assignee_submission().expect("old submission");
        flow.invalidate_selection();
        flow.set_state_for_test(IssueEditState::ConfirmingTransition {
            issue_id: selected.clone(),
            issue_key: "IX-1".to_owned(),
            transition_id: "7".to_owned(),
            transition_name: "Start".to_owned(),
            target_status: "In Progress".to_owned(),
        });
        let newer = flow
            .consume_transition_submission()
            .expect("new submission");

        assert_eq!(
            flow.finish_write(
                old.identity().clone(),
                Some(&selected),
                Err(ApplicationError::new(ErrorKind::Authorization, "old")),
            ),
            IssueEditCompletion::Ignored {
                busy: BusyDirective::Retain,
            }
        );
        assert!(matches!(
            flow.state(),
            IssueEditState::Submitting { identity, .. } if identity == newer.identity()
        ));
        assert_eq!(
            flow.finish_write(old.identity().clone(), Some(&selected), Ok(()),),
            IssueEditCompletion::Ignored {
                busy: BusyDirective::Retain,
            }
        );

        let wrong_operation = SubmissionIdentity {
            issue_id: selected.clone(),
            operation: IssueEditOperation::Transition,
            generation: old.identity().generation(),
        };
        assert_eq!(
            flow.finish_write(wrong_operation, Some(&selected), Ok(())),
            IssueEditCompletion::Ignored {
                busy: BusyDirective::Retain,
            }
        );
        let wrong_issue = SubmissionIdentity {
            issue_id: issue("2"),
            operation: IssueEditOperation::Transition,
            generation: old.identity().generation(),
        };
        assert_eq!(
            flow.finish_write(wrong_issue, Some(&selected), Ok(())),
            IssueEditCompletion::Ignored {
                busy: BusyDirective::Retain,
            }
        );
        assert_eq!(
            flow.finish_write(newer.identity().clone(), Some(&issue("2")), Ok(()),),
            IssueEditCompletion::Ignored {
                busy: BusyDirective::Retain,
            }
        );
        assert!(matches!(
            flow.state(),
            IssueEditState::Submitting { identity, .. } if identity == newer.identity()
        ));
        assert_eq!(
            flow.finish_write(newer.identity().clone(), Some(&selected), Ok(())),
            IssueEditCompletion::Applied
        );
    }

    #[test]
    fn error_copy_maps_every_kind_by_lookup_or_write_context() {
        let expected = [
            (
                ErrorKind::Authentication,
                IssueEditPhase::Lookup,
                "Change not applied · Jira authentication was rejected",
            ),
            (
                ErrorKind::Authentication,
                IssueEditPhase::Write,
                "Change not applied · Jira authentication was rejected",
            ),
            (
                ErrorKind::Authorization,
                IssueEditPhase::Lookup,
                "Change not applied · Jira denied permission",
            ),
            (
                ErrorKind::Authorization,
                IssueEditPhase::Write,
                "Change not applied · Jira denied permission",
            ),
            (
                ErrorKind::NotFound,
                IssueEditPhase::Lookup,
                "Change not applied · the Jira issue was not found",
            ),
            (
                ErrorKind::NotFound,
                IssueEditPhase::Write,
                "Change not applied · the Jira issue was not found",
            ),
            (
                ErrorKind::RateLimited,
                IssueEditPhase::Lookup,
                "Change not applied · Jira rate limit reached; try later",
            ),
            (
                ErrorKind::RateLimited,
                IssueEditPhase::Write,
                "Change not applied · Jira rate limit reached; try later",
            ),
            (
                ErrorKind::Offline,
                IssueEditPhase::Lookup,
                "Change not applied · Jira is unreachable",
            ),
            (
                ErrorKind::Offline,
                IssueEditPhase::Write,
                "Change not applied · Jira is unreachable",
            ),
            (
                ErrorKind::InvalidInput,
                IssueEditPhase::Lookup,
                "Change not applied · Jira rejected the requested change",
            ),
            (
                ErrorKind::InvalidInput,
                IssueEditPhase::Write,
                "Change not applied · Jira rejected the requested change",
            ),
            (
                ErrorKind::Cancelled,
                IssueEditPhase::Lookup,
                "Change cancelled",
            ),
            (
                ErrorKind::Cancelled,
                IssueEditPhase::Write,
                "Change cancelled",
            ),
            (
                ErrorKind::Storage,
                IssueEditPhase::Lookup,
                "Jira options unavailable · request was not completed",
            ),
            (
                ErrorKind::Storage,
                IssueEditPhase::Write,
                "Change not applied · Jira returned an error",
            ),
            (
                ErrorKind::Upstream,
                IssueEditPhase::Lookup,
                "Jira options unavailable · request was not completed",
            ),
            (
                ErrorKind::Upstream,
                IssueEditPhase::Write,
                "Change not applied · Jira returned an error",
            ),
            (
                ErrorKind::Notification,
                IssueEditPhase::Lookup,
                "Jira options unavailable · request was not completed",
            ),
            (
                ErrorKind::Notification,
                IssueEditPhase::Write,
                "Change not applied · Jira returned an error",
            ),
            (
                ErrorKind::Internal,
                IssueEditPhase::Lookup,
                "Jira options unavailable · request was not completed",
            ),
            (
                ErrorKind::Internal,
                IssueEditPhase::Write,
                "Change not applied · Jira returned an error",
            ),
            (
                ErrorKind::UnknownOutcome,
                IssueEditPhase::Lookup,
                "Jira may have accepted this change. Refresh Jira before another attempt.",
            ),
            (
                ErrorKind::UnknownOutcome,
                IssueEditPhase::Write,
                "Jira may have accepted this change. Refresh Jira before another attempt.",
            ),
        ];
        for (kind, phase, expected_message) in expected {
            let copy = issue_edit_error_message(kind, phase);
            assert_eq!(copy.message(), expected_message);
            assert_eq!(
                copy.severity(),
                crate::presentation::FeedbackSeverity::Error
            );
            assert_eq!(
                copy.certainty(),
                if kind == ErrorKind::UnknownOutcome {
                    FeedbackCertainty::Unknown
                } else {
                    FeedbackCertainty::Definite
                }
            );
            assert_eq!(
                copy.recovery(),
                if kind == ErrorKind::UnknownOutcome {
                    RecoveryDirective::Refresh
                } else {
                    RecoveryDirective::Retry
                }
            );
            assert!(!copy.message().contains("redacted detail"));
        }
    }

    #[test]
    fn status_editability_matches_dashboard_policy() {
        assert!(status_control_is_editable(
            true,
            true,
            false,
            false,
            &IssueEditState::Idle
        ));
        assert!(!status_control_is_editable(
            true,
            true,
            false,
            false,
            &IssueEditState::ConfirmingAssignee {
                issue_id: issue("1"),
                issue_key: "IX-1".to_owned(),
                account_id: None,
                display_name: "Nobody".to_owned(),
            }
        ));
    }
}
