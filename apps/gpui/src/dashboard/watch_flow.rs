use jira_application::{ApplicationError, ErrorKind};
use jira_domain::IssueId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum WatchState {
    Idle,
    Loading {
        issue_id: IssueId,
        generation: u64,
    },
    Ready {
        issue_id: IssueId,
        issue_key: String,
        watching: bool,
    },
    Confirming {
        issue_id: IssueId,
        issue_key: String,
        target_watching: bool,
    },
    Submitting {
        identity: WatchSubmissionIdentity,
        target_watching: bool,
    },
    Error {
        issue_id: IssueId,
        message: String,
        unknown: bool,
    },
}

/// Identity for one consumed user confirmation. Its generation prevents a late
/// completion from an earlier attempt on the same issue changing newer state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WatchSubmissionIdentity {
    issue_id: IssueId,
    issue_key: String,
    target_watching: bool,
    generation: u64,
}

impl WatchSubmissionIdentity {
    pub(super) fn issue_id(&self) -> &IssueId {
        &self.issue_id
    }

    pub(super) fn issue_key(&self) -> &str {
        &self.issue_key
    }

    pub(super) fn target_watching(&self) -> bool {
        self.target_watching
    }

    #[cfg(test)]
    fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WatchSubmission {
    pub(super) target_watching: bool,
    identity: WatchSubmissionIdentity,
}

impl WatchSubmission {
    pub(super) fn identity(&self) -> &WatchSubmissionIdentity {
        &self.identity
    }

    pub(super) fn issue_id(&self) -> &IssueId {
        self.identity.issue_id()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BusyDirective {
    Retain,
    Release,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum WatchCompletion {
    Applied,
    Failed { message: String, unknown: bool },
    Ignored { busy: BusyDirective },
}

#[derive(Clone, Debug)]
pub(super) struct WatchFlow {
    state: WatchState,
    generation: u64,
    reconciliation_pending: bool,
    invalidated_submissions: Vec<WatchSubmissionIdentity>,
    last_submission: Option<WatchSubmissionIdentity>,
}

impl WatchFlow {
    pub(super) fn new() -> Self {
        Self {
            state: WatchState::Idle,
            generation: 0,
            reconciliation_pending: false,
            invalidated_submissions: Vec::new(),
            last_submission: None,
        }
    }

    pub(super) fn state(&self) -> &WatchState {
        &self.state
    }

    pub(super) fn is_submitting(&self) -> bool {
        matches!(self.state, WatchState::Submitting { .. })
    }

    #[cfg(test)]
    pub(super) fn reconciliation_pending(&self) -> bool {
        self.reconciliation_pending
    }

    /// Begin a watcher-state read for the selected issue. Its generation must
    /// accompany the request so stale reads can be discarded on completion.
    /// Reads cannot overlap a dispatched write.
    pub(super) fn begin_read(&mut self, issue_id: IssueId) -> Option<u64> {
        if self.is_submitting() {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.state = WatchState::Loading {
            issue_id,
            generation,
        };
        Some(generation)
    }

    /// Apply a read only when it still owns the loading state and the same
    /// issue remains selected. A successful read reconciles an uncertain write.
    pub(super) fn finish_read(
        &mut self,
        selected_issue: Option<&IssueId>,
        issue_id: IssueId,
        issue_key: String,
        expected_generation: u64,
        result: Result<bool, ApplicationError>,
    ) -> bool {
        if !matches!(
            &self.state,
            WatchState::Loading {
                issue_id: loading_issue,
                generation,
            } if loading_issue == &issue_id
                && *generation == expected_generation
                && selected_issue == Some(&issue_id)
        ) {
            return false;
        }

        self.state = match result {
            Ok(watching) => {
                self.reconciliation_pending = false;
                WatchState::Ready {
                    issue_id,
                    issue_key,
                    watching,
                }
            }
            Err(error) => WatchState::Error {
                issue_id,
                message: read_error_message(error.kind()).to_owned(),
                unknown: self.reconciliation_pending,
            },
        };
        true
    }

    /// Move known watcher state into confirmation. This only changes local
    /// state; it does not dispatch the Jira write.
    pub(super) fn begin_confirmation(&mut self) -> bool {
        if self.reconciliation_pending {
            return false;
        }
        let WatchState::Ready {
            issue_id,
            issue_key,
            watching,
        } = &self.state
        else {
            return false;
        };
        let issue_id = issue_id.clone();
        let issue_key = issue_key.clone();
        let target_watching = !*watching;
        self.generation = self.generation.wrapping_add(1);
        self.state = WatchState::Confirming {
            issue_id,
            issue_key,
            target_watching,
        };
        true
    }

    pub(super) fn cancel_confirmation(&mut self) {
        match &self.state {
            WatchState::Confirming {
                issue_id,
                issue_key,
                target_watching,
            } => {
                let issue_id = issue_id.clone();
                let issue_key = issue_key.clone();
                let watching = !*target_watching;
                self.generation = self.generation.wrapping_add(1);
                self.state = WatchState::Ready {
                    issue_id,
                    issue_key,
                    watching,
                };
            }
            WatchState::Error { .. } => {
                self.generation = self.generation.wrapping_add(1);
                self.state = WatchState::Idle;
            }
            _ => {}
        }
    }

    /// Atomically consume the confirmation. A second activation cannot obtain
    /// the same write attempt.
    pub(super) fn consume_submission(&mut self) -> Option<WatchSubmission> {
        let WatchState::Confirming {
            issue_id,
            issue_key,
            target_watching,
        } = &self.state
        else {
            return None;
        };
        let issue_id = issue_id.clone();
        let issue_key = issue_key.clone();
        let target_watching = *target_watching;
        self.generation = self.generation.wrapping_add(1);
        let identity = WatchSubmissionIdentity {
            issue_id,
            issue_key,
            target_watching,
            generation: self.generation,
        };
        self.last_submission = Some(identity.clone());
        self.state = WatchState::Submitting {
            identity: identity.clone(),
            target_watching,
        };
        Some(WatchSubmission {
            target_watching,
            identity,
        })
    }

    /// Invalidate the selected issue. A pending read or confirmation can be
    /// cancelled; a dispatched write remains in flight and its completion is
    /// retained only to release its own busy indicator safely.
    pub(super) fn invalidate_selection(&mut self) -> bool {
        self.generation = self.generation.wrapping_add(1);
        if let WatchState::Submitting { identity, .. } = &self.state {
            self.invalidated_submissions.push(identity.clone());
            self.state = WatchState::Idle;
            return false;
        }
        self.state = WatchState::Idle;
        true
    }

    /// Apply a write completion only to the exact active attempt and selected
    /// issue. An unknown result blocks another toggle until a successful fresh
    /// read reconciles the remote watcher state.
    pub(super) fn finish_write(
        &mut self,
        identity: WatchSubmissionIdentity,
        current_issue: Option<&IssueId>,
        result: Result<(), ApplicationError>,
    ) -> WatchCompletion {
        let is_current = matches!(
            &self.state,
            WatchState::Submitting {
                identity: current,
                ..
            } if current == &identity && current_issue == Some(identity.issue_id())
        );
        if is_current {
            return match result {
                Ok(()) => {
                    self.reconciliation_pending = false;
                    self.state = WatchState::Ready {
                        issue_id: identity.issue_id().clone(),
                        issue_key: identity.issue_key().to_owned(),
                        watching: identity.target_watching(),
                    };
                    WatchCompletion::Applied
                }
                Err(error) => {
                    let unknown = error.kind() == ErrorKind::UnknownOutcome;
                    self.reconciliation_pending = unknown;
                    let message = write_error_message(error.kind()).to_owned();
                    self.state = WatchState::Error {
                        issue_id: identity.issue_id().clone(),
                        message: message.clone(),
                        unknown,
                    };
                    WatchCompletion::Failed { message, unknown }
                }
            };
        }

        let Some(index) = self
            .invalidated_submissions
            .iter()
            .position(|invalidated| invalidated == &identity)
        else {
            return WatchCompletion::Ignored {
                busy: BusyDirective::Retain,
            };
        };
        self.invalidated_submissions.swap_remove(index);
        let superseded = self
            .last_submission
            .as_ref()
            .is_some_and(|latest| latest != &identity);
        WatchCompletion::Ignored {
            busy: if superseded {
                BusyDirective::Retain
            } else {
                BusyDirective::Release
            },
        }
    }
}

fn read_error_message(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Authentication => "Sign in again to load watch status.",
        ErrorKind::Authorization => "You do not have permission to view watch status.",
        ErrorKind::NotFound => "This issue could not be found.",
        ErrorKind::Offline => "Could not reach Jira to load watch status.",
        ErrorKind::RateLimited => "Jira is temporarily limiting requests. Try again shortly.",
        ErrorKind::Cancelled => "Watch status loading was cancelled.",
        ErrorKind::InvalidInput => "This issue has an invalid identity.",
        ErrorKind::UnknownOutcome => "Could not confirm watch status. Refresh to check it.",
        ErrorKind::Storage
        | ErrorKind::Upstream
        | ErrorKind::Notification
        | ErrorKind::Internal => "Could not load watch status. Refresh to try again.",
    }
}

fn write_error_message(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Authentication => "Sign in again to change watch status.",
        ErrorKind::Authorization => "You do not have permission to change watch status.",
        ErrorKind::NotFound => "This issue could not be found.",
        ErrorKind::Offline => "Could not reach Jira. Refresh watch status before trying again.",
        ErrorKind::RateLimited => "Jira is temporarily limiting requests. Refresh before retrying.",
        ErrorKind::Cancelled => "The watch change was cancelled.",
        ErrorKind::InvalidInput => "This issue has an invalid identity.",
        ErrorKind::UnknownOutcome => {
            "Jira may have changed watch status. Refresh before trying again."
        }
        ErrorKind::Storage
        | ErrorKind::Upstream
        | ErrorKind::Notification
        | ErrorKind::Internal => "Could not change watch status. Refresh before trying again.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str) -> IssueId {
        IssueId::new(id).expect("issue id")
    }

    fn load_ready(flow: &mut WatchFlow, id: &str, key: &str, watching: bool) {
        let issue_id = issue(id);
        let generation = flow.begin_read(issue_id.clone()).expect("read generation");
        assert!(flow.finish_read(
            Some(&issue_id),
            issue_id.clone(),
            key.to_owned(),
            generation,
            Ok(watching),
        ));
    }

    #[test]
    fn confirmation_toggles_known_state_and_submission_is_consumed_once() {
        let mut flow = WatchFlow::new();
        load_ready(&mut flow, "1", "APP-1", false);

        assert!(flow.begin_confirmation());
        assert!(matches!(
            flow.state(),
            WatchState::Confirming {
                issue_id,
                issue_key,
                target_watching: true,
            } if issue_id == &issue("1") && issue_key == "APP-1"
        ));
        let submission = flow.consume_submission().expect("write submission");
        assert!(submission.target_watching);
        assert_eq!(submission.issue_id(), &issue("1"));
        assert_eq!(submission.identity().generation(), 3);
        assert!(flow.consume_submission().is_none());
        assert!(flow.is_submitting());
    }

    #[test]
    fn cancelling_confirmation_never_creates_a_submission() {
        let mut flow = WatchFlow::new();
        load_ready(&mut flow, "1", "APP-1", true);
        assert!(flow.begin_confirmation());
        flow.cancel_confirmation();

        assert!(matches!(
            flow.state(),
            WatchState::Ready {
                issue_id,
                issue_key,
                watching: true,
            } if issue_id == &issue("1") && issue_key == "APP-1"
        ));
        assert!(flow.consume_submission().is_none());
    }

    #[test]
    fn stale_reads_are_rejected_for_older_generation_or_different_selection() {
        let mut flow = WatchFlow::new();
        let selected = issue("1");
        let old_generation = flow.begin_read(selected.clone()).expect("old generation");
        let new_generation = flow.begin_read(selected.clone()).expect("new generation");
        assert!(!flow.finish_read(
            Some(&selected),
            selected.clone(),
            "APP-1".to_owned(),
            old_generation,
            Ok(false),
        ));
        let other = issue("2");
        assert!(!flow.finish_read(
            Some(&other),
            selected.clone(),
            "APP-1".to_owned(),
            new_generation,
            Ok(false),
        ));
        assert!(matches!(
            flow.state(),
            WatchState::Loading { issue_id, generation }
                if issue_id == &selected && *generation == new_generation
        ));
    }

    #[test]
    fn unknown_write_requires_a_successful_fresh_read_before_another_confirmation() {
        let mut flow = WatchFlow::new();
        load_ready(&mut flow, "1", "APP-1", false);
        assert!(flow.begin_confirmation());
        let submission = flow.consume_submission().expect("submission");
        assert_eq!(
            flow.finish_write(
                submission.identity().clone(),
                Some(submission.issue_id()),
                Err(ApplicationError::new(
                    ErrorKind::UnknownOutcome,
                    "secret detail"
                )),
            ),
            WatchCompletion::Failed {
                message: "Jira may have changed watch status. Refresh before trying again."
                    .to_owned(),
                unknown: true,
            }
        );
        assert!(flow.reconciliation_pending());
        assert!(!flow.begin_confirmation());
        assert!(flow.consume_submission().is_none());
        flow.cancel_confirmation();
        assert!(matches!(flow.state(), WatchState::Idle));
        assert!(!flow.begin_confirmation());

        let generation = flow.begin_read(issue("1")).expect("reconciliation read");
        assert!(flow.finish_read(
            Some(&issue("1")),
            issue("1"),
            "APP-1".to_owned(),
            generation,
            Ok(true),
        ));
        assert!(!flow.reconciliation_pending());
        assert!(matches!(
            flow.state(),
            WatchState::Ready { watching: true, .. }
        ));
        assert!(flow.begin_confirmation());
        assert!(matches!(
            flow.consume_submission(),
            Some(WatchSubmission {
                target_watching: false,
                ..
            })
        ));
    }

    #[test]
    fn stale_write_completion_does_not_disturb_a_new_attempt_for_same_issue() {
        let mut flow = WatchFlow::new();
        let selected = issue("1");
        load_ready(&mut flow, "1", "APP-1", false);
        assert!(flow.begin_confirmation());
        let older = flow.consume_submission().expect("older write");
        assert!(!flow.invalidate_selection());

        load_ready(&mut flow, "1", "APP-1", false);
        assert!(flow.begin_confirmation());
        let newer = flow.consume_submission().expect("newer write");
        assert_ne!(older.identity().generation(), newer.identity().generation());
        assert!(flow.is_submitting());
        assert_eq!(
            flow.finish_write(older.identity().clone(), Some(&selected), Ok(()),),
            WatchCompletion::Ignored {
                busy: BusyDirective::Retain,
            }
        );
        assert!(flow.is_submitting());
        assert_eq!(
            flow.finish_write(newer.identity().clone(), Some(&selected), Ok(()),),
            WatchCompletion::Applied
        );
        assert!(matches!(
            flow.state(),
            WatchState::Ready { watching: true, .. }
        ));
    }

    #[test]
    fn invalidated_write_releases_busy_only_when_no_newer_attempt_owns_it() {
        let mut flow = WatchFlow::new();
        load_ready(&mut flow, "1", "APP-1", false);
        assert!(flow.begin_confirmation());
        let submission = flow.consume_submission().expect("write");
        assert!(!flow.invalidate_selection());

        assert_eq!(
            flow.finish_write(submission.identity().clone(), Some(&issue("2")), Ok(()),),
            WatchCompletion::Ignored {
                busy: BusyDirective::Release,
            }
        );
        assert_eq!(flow.state(), &WatchState::Idle);
        assert!(!flow.is_submitting());
    }
}
