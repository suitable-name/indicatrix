//! [`CancelToken`]: one shared cancellation flag for an epoch or a job.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A cloneable cancellation flag. Every clone observes the same flag, so the owner of
/// a job keeps one clone and hands others to the lanes working on it.
///
/// Cancellation is one-way: once raised it stays raised for the token's lifetime.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, not-yet-cancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Raises the flag for every clone of this token.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether [`Self::cancel`] has been called on any clone.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// The underlying flag, for code that takes a plain `&AtomicBool` (the export's
    /// existing dispatch helpers, the tracer's cancellable entry points).
    #[must_use]
    pub fn as_flag(&self) -> &AtomicBool {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::CancelToken;

    #[test]
    fn every_clone_sees_a_cancel() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
        assert!(clone.as_flag().load(std::sync::atomic::Ordering::Relaxed));
    }
}
