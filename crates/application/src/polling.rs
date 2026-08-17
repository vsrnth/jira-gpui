//! Small, deterministic scheduling policy for background synchronization.

use std::time::Duration;

use crate::{ApplicationError, ErrorKind};

const NORMAL_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const TRANSIENT_BACKOFF_START: Duration = Duration::from_secs(30);
const TRANSIENT_BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);
const RATE_LIMIT_MIN: Duration = Duration::from_secs(30);
const RATE_LIMIT_MAX: Duration = Duration::from_secs(60 * 60);

/// Default automatic-poll scheduling policy.
///
/// The policy only calculates delays. A shell or scheduler decides whether to
/// sleep, cancel, or persist the resulting state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefaultPollingPolicy;

impl DefaultPollingPolicy {
    /// Return the normal interval after a successful synchronization.
    pub fn next_delay_after_success(&self) -> Duration {
        NORMAL_POLL_INTERVAL
    }

    /// Return the next automatic-poll delay after a synchronization failure.
    ///
    /// `consecutive_failures` is intended to be 1-based. A zero value is
    /// treated as the first failure so callers cannot accidentally request an
    /// immediate retry or trigger arithmetic overflow.
    pub fn next_delay_after_failure(
        &self,
        error: &ApplicationError,
        consecutive_failures: u32,
    ) -> Option<Duration> {
        match error.kind() {
            ErrorKind::Offline | ErrorKind::Upstream => {
                Some(transient_backoff(consecutive_failures))
            }
            ErrorKind::RateLimited => Some(
                error
                    .retry_after()
                    .map_or_else(|| transient_backoff(consecutive_failures), clamp_rate_limit),
            ),
            ErrorKind::Authentication
            | ErrorKind::Authorization
            | ErrorKind::Cancelled
            | ErrorKind::InvalidInput
            | ErrorKind::NotFound
            | ErrorKind::Storage
            | ErrorKind::Notification
            | ErrorKind::UnknownOutcome
            | ErrorKind::Internal => None,
        }
    }
}

fn transient_backoff(consecutive_failures: u32) -> Duration {
    // Five doublings reach 960 seconds, which is then clamped to 15 minutes.
    // Limiting the loop before doing arithmetic keeps even u32::MAX harmless.
    let doublings = consecutive_failures.saturating_sub(1).min(5);
    let mut seconds = TRANSIENT_BACKOFF_START.as_secs();
    for _ in 0..doublings {
        seconds = seconds
            .saturating_mul(2)
            .min(TRANSIENT_BACKOFF_MAX.as_secs());
    }
    Duration::from_secs(seconds)
}

fn clamp_rate_limit(delay: Duration) -> Duration {
    delay.max(RATE_LIMIT_MIN).min(RATE_LIMIT_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(kind: ErrorKind) -> ApplicationError {
        ApplicationError::new(kind, "safe message")
    }

    #[test]
    fn success_uses_five_minute_interval() {
        assert_eq!(
            DefaultPollingPolicy.next_delay_after_success(),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn offline_and_upstream_use_exponential_backoff() {
        let policy = DefaultPollingPolicy;
        for kind in [ErrorKind::Offline, ErrorKind::Upstream] {
            assert_eq!(
                policy.next_delay_after_failure(&error(kind), 1),
                Some(Duration::from_secs(30))
            );
            assert_eq!(
                policy.next_delay_after_failure(&error(kind), 2),
                Some(Duration::from_secs(60))
            );
            assert_eq!(
                policy.next_delay_after_failure(&error(kind), 3),
                Some(Duration::from_secs(120))
            );
        }
    }

    #[test]
    fn zero_failure_count_is_first_failure_and_large_count_is_capped() {
        let policy = DefaultPollingPolicy;
        let upstream = error(ErrorKind::Upstream);
        assert_eq!(
            policy.next_delay_after_failure(&upstream, 0),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            policy.next_delay_after_failure(&upstream, 6),
            Some(Duration::from_secs(900))
        );
        assert_eq!(
            policy.next_delay_after_failure(&upstream, u32::MAX),
            Some(Duration::from_secs(900))
        );
    }

    #[test]
    fn rate_limit_retry_after_is_clamped_or_uses_backoff() {
        let policy = DefaultPollingPolicy;
        let minimum = ApplicationError::rate_limited("rate limit", Some(Duration::ZERO));
        let exact = ApplicationError::rate_limited("rate limit", Some(Duration::from_secs(600)));
        let maximum =
            ApplicationError::rate_limited("rate limit", Some(Duration::from_secs(60 * 60 + 1)));
        let absent = ApplicationError::rate_limited("rate limit", None);

        assert_eq!(
            policy.next_delay_after_failure(&minimum, 1),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            policy.next_delay_after_failure(&exact, 1),
            Some(Duration::from_secs(600))
        );
        assert_eq!(
            policy.next_delay_after_failure(&maximum, 1),
            Some(Duration::from_secs(60 * 60))
        );
        assert_eq!(
            policy.next_delay_after_failure(&absent, 3),
            Some(Duration::from_secs(120))
        );
    }

    #[test]
    fn non_transient_failures_pause_automatic_polling() {
        let policy = DefaultPollingPolicy;
        for kind in [
            ErrorKind::Authentication,
            ErrorKind::Authorization,
            ErrorKind::Cancelled,
            ErrorKind::InvalidInput,
            ErrorKind::NotFound,
            ErrorKind::Storage,
            ErrorKind::Notification,
            ErrorKind::UnknownOutcome,
            ErrorKind::Internal,
        ] {
            assert_eq!(policy.next_delay_after_failure(&error(kind), 1), None);
        }
    }

    #[test]
    fn policy_ignores_error_text_and_does_not_expose_it() {
        let policy = DefaultPollingPolicy;
        let error = ApplicationError::new(
            ErrorKind::Upstream,
            "token=secret path=/private/cache.sqlite",
        );
        assert_eq!(
            policy.next_delay_after_failure(&error, 1),
            Some(Duration::from_secs(30))
        );
    }
}
