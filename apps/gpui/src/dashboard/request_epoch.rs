use std::{marker::PhantomData, sync::Arc};

use jira_application::CancellationToken;

/// The read surfaces that own independent request epochs in the dashboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RequestSource {
    SelectedDetail,
    RemoteLookup,
}

/// An immutable identity for one cancellable presentation read.
#[derive(Clone, Debug)]
pub(super) struct RequestTicket<S, K> {
    source: S,
    key: K,
    generation: u64,
    /// Fresh per-begin identity. Generations can wrap, and tickets from
    /// separate epoch instances can otherwise have identical fields.
    identity: Arc<()>,
    cancellation: CancellationToken,
}

impl<S, K> RequestTicket<S, K> {
    #[cfg(test)]
    pub(super) fn source(&self) -> &S {
        &self.source
    }

    pub(super) fn key(&self) -> &K {
        &self.key
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// UI-owned generation and cancellation state for one family of reads.
///
/// The dashboard keeps one instance for selected detail and another for remote
/// lookup. A ticket is valid only while its source, key, and wrapping
/// generation still identify the current request in that instance.
pub(super) struct RequestEpoch<S, K> {
    generation: u64,
    current: Option<RequestTicket<S, K>>,
    _marker: PhantomData<fn() -> (S, K)>,
}

impl<S, K> Default for RequestEpoch<S, K> {
    fn default() -> Self {
        Self {
            generation: 0,
            current: None,
            _marker: PhantomData,
        }
    }
}

impl<S, K> RequestEpoch<S, K>
where
    S: Clone + PartialEq,
    K: Clone + PartialEq,
{
    pub(super) fn begin(&mut self, source: S, key: K) -> RequestTicket<S, K> {
        self.cancel_current();
        self.generation = self.generation.wrapping_add(1);
        let ticket = RequestTicket {
            source,
            key,
            generation: self.generation,
            identity: Arc::new(()),
            cancellation: CancellationToken::new(),
        };
        self.current = Some(ticket.clone());
        ticket
    }

    pub(super) fn invalidate(&mut self) {
        self.cancel_current();
        self.generation = self.generation.wrapping_add(1);
    }

    pub(super) fn is_current(&self, ticket: &RequestTicket<S, K>) -> bool {
        self.current.as_ref().is_some_and(|current| {
            current.source == ticket.source
                && current.key == ticket.key
                && current.generation == ticket.generation
                && Arc::ptr_eq(&current.identity, &ticket.identity)
        })
    }

    /// Finish only the currently owned ticket. Stale completions cannot clear
    /// the cancellation token belonging to a newer task.
    pub(super) fn finish(&mut self, ticket: &RequestTicket<S, K>) -> bool {
        if self.is_current(ticket) {
            self.current = None;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(super) fn is_idle(&self) -> bool {
        self.current.is_none()
    }

    fn cancel_current(&mut self) {
        if let Some(ticket) = self.current.take() {
            ticket.cancellation.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_cancels_prior_ticket_and_binds_source_key_and_generation() {
        let mut epochs = RequestEpoch::<RequestSource, &str>::default();
        let first = epochs.begin(RequestSource::SelectedDetail, "one");
        let second = epochs.begin(RequestSource::RemoteLookup, "two");

        assert!(first.cancellation().is_cancelled());
        assert_eq!(*second.source(), RequestSource::RemoteLookup);
        assert_eq!(*second.key(), "two");
        assert_eq!(second.generation(), 2);
        assert!(epochs.is_current(&second));
        assert!(!epochs.is_current(&first));
    }

    #[test]
    fn finish_only_clears_the_current_ticket() {
        let mut epochs = RequestEpoch::<RequestSource, &str>::default();
        let first = epochs.begin(RequestSource::SelectedDetail, "one");
        let second = epochs.begin(RequestSource::SelectedDetail, "two");

        assert!(!epochs.finish(&first));
        assert!(epochs.is_current(&second));
        assert!(epochs.finish(&second));
        assert!(!epochs.is_current(&second));
    }

    #[test]
    fn invalidate_cancels_and_bumps_once() {
        let mut epochs = RequestEpoch::<RequestSource, &str>::default();
        let ticket = epochs.begin(RequestSource::SelectedDetail, "one");
        assert_eq!(epochs.generation(), 1);

        epochs.invalidate();

        assert!(ticket.cancellation().is_cancelled());
        assert_eq!(epochs.generation(), 2);
        assert!(!epochs.is_current(&ticket));
    }

    #[test]
    fn generation_wraps_without_revalidating_stale_ticket() {
        let mut epochs = RequestEpoch::<RequestSource, &str>::default();
        epochs.generation = u64::MAX;
        let first = epochs.begin(RequestSource::SelectedDetail, "one");
        assert_eq!(first.generation(), 0);
        epochs.invalidate();
        let second = epochs.begin(RequestSource::SelectedDetail, "one");
        assert_eq!(second.generation(), 2);
        assert!(!epochs.is_current(&first));
        assert!(epochs.is_current(&second));
    }

    #[test]
    fn independent_epoch_instances_require_opaque_identity_even_with_matching_fields() {
        let mut first_epoch = RequestEpoch::<RequestSource, &str>::default();
        let mut second_epoch = RequestEpoch::<RequestSource, &str>::default();
        let first_ticket = first_epoch.begin(RequestSource::SelectedDetail, "same");
        let second_ticket = second_epoch.begin(RequestSource::SelectedDetail, "same");

        assert_eq!(first_ticket.generation(), second_ticket.generation());
        assert!(first_epoch.is_current(&first_ticket));
        assert!(second_epoch.is_current(&second_ticket));
        assert!(!first_epoch.is_current(&second_ticket));
        assert!(!second_epoch.is_current(&first_ticket));
    }

    #[test]
    fn forced_generation_reuse_cannot_validate_or_finish_the_old_ticket() {
        let mut epochs = RequestEpoch::<RequestSource, &str>::default();
        let first = epochs.begin(RequestSource::SelectedDetail, "same");
        epochs.generation = 0;
        let second = epochs.begin(RequestSource::SelectedDetail, "same");

        assert_eq!(first.generation(), second.generation());
        assert!(first.cancellation().is_cancelled());
        assert!(!epochs.is_current(&first));
        assert!(!epochs.finish(&first));
        assert!(epochs.is_current(&second));
        assert!(epochs.finish(&second));
    }
}
