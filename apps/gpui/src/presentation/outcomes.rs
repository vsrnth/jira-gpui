use std::fmt;

use jira_application::{ErrorKind, MAX_COMMENT_BYTES, MAX_COMMENT_CHARS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FeedbackSeverity {
    Info,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FeedbackCertainty {
    Definite,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryDirective {
    None,
    Retry,
    Refresh,
    InvalidateWorkspace,
    PauseTeam,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutcomeCopy {
    message: &'static str,
    severity: FeedbackSeverity,
    certainty: FeedbackCertainty,
    recovery: RecoveryDirective,
}

impl fmt::Display for OutcomeCopy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl OutcomeCopy {
    pub(crate) const fn new(
        message: &'static str,
        severity: FeedbackSeverity,
        certainty: FeedbackCertainty,
        recovery: RecoveryDirective,
    ) -> Self {
        Self {
            message,
            severity,
            certainty,
            recovery,
        }
    }

    pub(crate) const fn message(self) -> &'static str {
        self.message
    }

    pub(crate) const fn severity(self) -> FeedbackSeverity {
        self.severity
    }

    #[cfg(test)]
    pub(crate) const fn certainty(self) -> FeedbackCertainty {
        self.certainty
    }

    pub(crate) const fn is_unknown(self) -> bool {
        matches!(self.certainty, FeedbackCertainty::Unknown)
    }

    pub(crate) const fn recovery(self) -> RecoveryDirective {
        self.recovery
    }

    pub(crate) fn to_owned(self) -> String {
        self.message.to_owned()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadSurface {
    Sync,
    Detail,
    Lookup,
}

pub(crate) fn read_error_copy(surface: ReadSurface, kind: ErrorKind) -> OutcomeCopy {
    use ErrorKind::*;
    let message = match (surface, kind) {
        (ReadSurface::Sync, Authentication) => "Refresh failed · Jira authentication was rejected",
        (ReadSurface::Sync, Authorization) => "Refresh failed · Jira authorization was denied",
        (ReadSurface::Sync, RateLimited) => "Refresh paused · Jira rate limit reached",
        (ReadSurface::Sync, Offline) => "Refresh failed · Jira is unreachable",
        (ReadSurface::Sync, Cancelled) => "Refresh cancelled",
        (ReadSurface::Sync, InvalidInput) => "Refresh failed · invalid request",
        (ReadSurface::Sync, NotFound) => "Refresh failed · Jira site was not found",
        (ReadSurface::Sync, Upstream) => "Refresh failed · Jira returned an error",
        (ReadSurface::Sync, Storage | Notification | Internal | UnknownOutcome) => {
            "Refresh failed · local application error"
        }
        (ReadSurface::Detail, Authentication) => {
            "Issue details unavailable · Jira authentication was rejected"
        }
        (ReadSurface::Detail, Authorization) => {
            "Issue details unavailable · Jira authorization was denied"
        }
        (ReadSurface::Detail, NotFound) => "Issue details unavailable · Jira issue was not found",
        (ReadSurface::Detail, RateLimited) => "Issue details unavailable · Jira rate limit reached",
        (ReadSurface::Detail, Offline) => "Issue details unavailable · Jira is unreachable",
        (ReadSurface::Detail, Cancelled) => "Issue details request cancelled",
        (
            ReadSurface::Detail,
            InvalidInput | Upstream | Storage | Notification | Internal | UnknownOutcome,
        ) => "Issue details unavailable · Jira returned an error",
        (ReadSurface::Lookup, Authentication) => "Jira lookup failed · authentication was rejected",
        (ReadSurface::Lookup, Authorization) => "Jira lookup failed · authorization was denied",
        (ReadSurface::Lookup, NotFound) => "Jira lookup · issue was not found",
        (ReadSurface::Lookup, RateLimited) => "Jira lookup paused · rate limit reached",
        (ReadSurface::Lookup, Offline) => "Jira lookup failed · Jira is unreachable",
        (ReadSurface::Lookup, Cancelled) => "Jira lookup cancelled",
        (
            ReadSurface::Lookup,
            InvalidInput | Upstream | Storage | Notification | Internal | UnknownOutcome,
        ) => "Jira lookup failed · request was not completed",
    };
    OutcomeCopy::new(
        message,
        FeedbackSeverity::Error,
        FeedbackCertainty::Definite,
        RecoveryDirective::Retry,
    )
}

pub(crate) fn lookup_workspace_unavailable_copy() -> OutcomeCopy {
    OutcomeCopy::new(
        "Jira lookup unavailable · live workspace is not ready",
        FeedbackSeverity::Error,
        FeedbackCertainty::Definite,
        RecoveryDirective::Retry,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommentOutcomeKind {
    ValidationEmpty,
    ValidationByteLimit,
    ValidationCharacterLimit,
    WorkspaceUnavailable,
    Error(ErrorKind),
}

pub(crate) fn comment_outcome_copy(kind: CommentOutcomeKind) -> OutcomeCopy {
    let (message, certainty, recovery) = match kind {
        CommentOutcomeKind::ValidationEmpty => (
            "Comment not posted · enter a non-empty comment",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::ValidationByteLimit => (
            "Comment not posted · comment exceeds the byte limit",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::ValidationCharacterLimit => (
            "Comment not posted · comment exceeds the character limit",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::WorkspaceUnavailable => (
            "Comment not posted · live Jira workspace is not ready",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::Error(ErrorKind::Authentication) => (
            "Comment not posted · Jira authentication was rejected",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::Error(ErrorKind::Authorization) => (
            "Comment not posted · Jira denied comment permission",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::Error(ErrorKind::NotFound) => (
            "Comment not posted · the Jira issue was not found",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::Error(ErrorKind::RateLimited) => (
            "Comment not posted · Jira rate limit reached; try later",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::Error(ErrorKind::InvalidInput) => (
            "Comment not posted · the comment text is invalid",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        CommentOutcomeKind::Error(ErrorKind::UnknownOutcome) => (
            "Jira may have accepted this comment. Refresh comments before retrying.",
            FeedbackCertainty::Unknown,
            RecoveryDirective::Refresh,
        ),
        CommentOutcomeKind::Error(_) => (
            "Comment not posted · Jira returned an error",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
    };
    OutcomeCopy::new(message, FeedbackSeverity::Error, certainty, recovery)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IssueEditPhase {
    Lookup,
    Write,
}

pub(crate) fn issue_edit_error_copy(kind: ErrorKind, phase: IssueEditPhase) -> OutcomeCopy {
    let (message, certainty, recovery) = match kind {
        ErrorKind::UnknownOutcome => (
            "Jira may have accepted this change. Refresh Jira before another attempt.",
            FeedbackCertainty::Unknown,
            RecoveryDirective::Refresh,
        ),
        ErrorKind::Authentication => (
            "Change not applied · Jira authentication was rejected",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        ErrorKind::Authorization => (
            "Change not applied · Jira denied permission",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        ErrorKind::NotFound => (
            "Change not applied · the Jira issue was not found",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        ErrorKind::RateLimited => (
            "Change not applied · Jira rate limit reached; try later",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        ErrorKind::Offline => (
            "Change not applied · Jira is unreachable",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        ErrorKind::InvalidInput => (
            "Change not applied · Jira rejected the requested change",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        ErrorKind::Cancelled => (
            "Change cancelled",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
        ErrorKind::Storage
        | ErrorKind::Upstream
        | ErrorKind::Notification
        | ErrorKind::Internal
            if matches!(phase, IssueEditPhase::Lookup) =>
        {
            (
                "Jira options unavailable · request was not completed",
                FeedbackCertainty::Definite,
                RecoveryDirective::Retry,
            )
        }
        _ => (
            "Change not applied · Jira returned an error",
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
    };
    OutcomeCopy::new(message, FeedbackSeverity::Error, certainty, recovery)
}

pub(crate) fn issue_edit_workspace_unavailable_copy() -> OutcomeCopy {
    OutcomeCopy::new(
        "Change unavailable · live Jira workspace is not ready",
        FeedbackSeverity::Error,
        FeedbackCertainty::Definite,
        RecoveryDirective::Retry,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScopeOutcomeKind {
    Invalid,
    Preparation,
    RefreshRestored,
    RefreshRollbackFailed,
    PreferenceSaveRestored,
    PreferenceSaveRollbackFailed,
}

pub(crate) fn scope_outcome_copy(kind: ScopeOutcomeKind) -> OutcomeCopy {
    let (message, recovery) = match kind {
        ScopeOutcomeKind::Invalid => (
            "Scope is invalid; check the expression and ORDER BY rule",
            RecoveryDirective::Retry,
        ),
        ScopeOutcomeKind::Preparation => (
            "Scope could not be prepared locally",
            RecoveryDirective::Retry,
        ),
        ScopeOutcomeKind::RefreshRestored => (
            "Jira rejected the scope; the previous scope remains active",
            RecoveryDirective::Retry,
        ),
        ScopeOutcomeKind::RefreshRollbackFailed => (
            "Jira rejected the scope and the previous scope could not be restored",
            RecoveryDirective::InvalidateWorkspace,
        ),
        ScopeOutcomeKind::PreferenceSaveRestored => (
            "Scope applied remotely, but settings could not be saved locally",
            RecoveryDirective::Retry,
        ),
        ScopeOutcomeKind::PreferenceSaveRollbackFailed => (
            "Settings could not be saved and the previous scope could not be restored",
            RecoveryDirective::InvalidateWorkspace,
        ),
    };
    OutcomeCopy::new(
        message,
        FeedbackSeverity::Error,
        FeedbackCertainty::Definite,
        recovery,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TeamInvalidInputKind {
    TooManyMembers,
    InvalidAccount,
    UnsafeAccount,
    InvalidEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TeamOutcomeKind {
    InvalidInput(TeamInvalidInputKind),
    Search,
    EmailNotFound,
    EmailAmbiguous,
    Normalization,
    Preparation,
    RefreshRestored,
    RefreshRollbackFailed,
    PreferenceSaveRestored,
    PreferenceSaveRollbackFailed,
}

pub(crate) fn team_outcome_copy(kind: TeamOutcomeKind) -> OutcomeCopy {
    let (message, recovery) = match kind {
        TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::TooManyMembers) => (
            "Team tracker accepts at most 100 members",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::InvalidAccount) => (
            "Enter a valid Jira account ID or Atlassian email",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::UnsafeAccount) => (
            "Jira account IDs cannot contain quote or backslash characters",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::InvalidEntry) => (
            "Team tracker entries must be short, single-line values",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::Search => (
            "Jira user search failed; existing team remains active",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::EmailNotFound => (
            "Email did not resolve to one active Jira user",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::EmailAmbiguous => (
            "Email matched multiple active Jira users; enter an account ID instead",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::Normalization => (
            "Team tracker entries are invalid or exceed the member limit",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::Preparation => (
            "Team configuration could not be applied; existing team remains active",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::RefreshRestored => (
            "Team refresh failed; existing team remains active",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::RefreshRollbackFailed => (
            "Team refresh failed and the previous team could not be restored; team tracker paused",
            RecoveryDirective::PauseTeam,
        ),
        TeamOutcomeKind::PreferenceSaveRestored => (
            "Team refreshed but could not be saved locally; existing team remains active",
            RecoveryDirective::Retry,
        ),
        TeamOutcomeKind::PreferenceSaveRollbackFailed => (
            "Team settings could not be saved and the previous team could not be restored; team tracker paused",
            RecoveryDirective::PauseTeam,
        ),
    };
    OutcomeCopy::new(
        message,
        FeedbackSeverity::Error,
        FeedbackCertainty::Definite,
        recovery,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SavedLoginOutcomeKind {
    Deleted,
    Absent,
    Error,
}

pub(crate) fn saved_login_outcome_copy(kind: SavedLoginOutcomeKind) -> OutcomeCopy {
    let (message, severity) = match kind {
        SavedLoginOutcomeKind::Deleted => (
            "Saved Jira login forgotten. This session remains connected.",
            FeedbackSeverity::Info,
        ),
        SavedLoginOutcomeKind::Absent => (
            "No saved Jira login was present. This session remains connected.",
            FeedbackSeverity::Info,
        ),
        SavedLoginOutcomeKind::Error => (
            "Saved Jira login could not be removed from the system keyring.",
            FeedbackSeverity::Error,
        ),
    };
    OutcomeCopy::new(
        message,
        severity,
        FeedbackCertainty::Definite,
        RecoveryDirective::None,
    )
}

pub(crate) fn comment_validation_kind(body: &str) -> Option<CommentOutcomeKind> {
    let body = body.trim();
    if body.is_empty() {
        Some(CommentOutcomeKind::ValidationEmpty)
    } else if body.len() > MAX_COMMENT_BYTES {
        Some(CommentOutcomeKind::ValidationByteLimit)
    } else if body.chars().count() > MAX_COMMENT_CHARS {
        Some(CommentOutcomeKind::ValidationCharacterLimit)
    } else {
        None
    }
}
