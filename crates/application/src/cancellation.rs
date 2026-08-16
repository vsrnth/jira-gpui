use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Runtime-agnostic cooperative cancellation shared with adapter operations.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn check(&self) -> Result<(), crate::ApplicationError> {
        if self.is_cancelled() {
            Err(crate::ApplicationError::cancelled())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CancellationToken;

    #[test]
    fn clones_observe_cancellation() {
        let original = CancellationToken::new();
        let clone = original.clone();

        clone.cancel();

        assert!(original.is_cancelled());
        assert!(original.check().is_err());
    }
}
