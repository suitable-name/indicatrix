//! A qualification's persistent record across crash, intentional exit and suspension.

use super::{AdmissionRecord, AdmissionState};

/// Owns the pre-test snapshot until this attempt completes or is abandoned.
#[derive(Debug, Clone, Default)]
pub struct ProbeRecord {
    /// The record to persist before dispatch and after a result.
    pub current: AdmissionRecord,
    previous: Option<AdmissionRecord>,
}

impl ProbeRecord {
    #[must_use]
    pub const fn new(current: AdmissionRecord) -> Self {
        Self {
            current,
            previous: None,
        }
    }

    /// A crash leaves this Probing record on disk. Intentional exits may restore the
    /// previous stable record without changing the still-running in-memory attempt.
    pub fn begin(&mut self) {
        self.previous = Some(self.current.clone());
        self.current.state = AdmissionState::Probing;
        self.current.reason = None;
        self.current.per_kernel_ms.clear();
        self.current.total_ms = 0.0;
        self.current.max_ms = 0.0;
    }

    pub fn settle(&mut self, state: AdmissionState, reason: Option<String>) {
        self.current.state = state;
        self.current.reason = reason;
        if state != AdmissionState::Probing {
            self.previous = None;
        }
    }

    /// Persistent record for an intentional pagehide while the attempt is active.
    #[must_use]
    pub const fn released(&self) -> Option<&AdmissionRecord> {
        self.previous.as_ref()
    }

    /// Persistent record when the same attempt resumes from the back/forward cache.
    #[must_use]
    pub fn rearmed(&self) -> Option<&AdmissionRecord> {
        self.previous.as_ref().map(|_| &self.current)
    }

    /// The worker was intentionally discarded; restore memory as well as disk so a
    /// prior Failed record keeps blocking automatic tests after an off/on cycle.
    pub fn abandon(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.current = previous;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_explicit_retry_restores_on_exit_and_rearms_only_while_active() {
        let prior = AdmissionRecord {
            state: AdmissionState::Failed,
            reason: Some("previous driver failure".to_owned()),
            total_ms: 12.0,
            ..AdmissionRecord::default()
        };
        let mut probe = ProbeRecord::new(prior.clone());
        probe.begin();
        assert_eq!(probe.current.state, AdmissionState::Probing);
        assert!(probe.current.reason.is_none());
        assert_eq!(probe.released(), Some(&prior));
        assert_eq!(probe.rearmed(), Some(&probe.current));
        // pagehide releases only the disk record: the cached page still owns the test.
        assert_eq!(probe.current.state, AdmissionState::Probing);
        probe.abandon();
        assert_eq!(probe.current, prior);
        assert!(probe.released().is_none());
        assert!(probe.rearmed().is_none());
    }

    #[test]
    fn a_crash_retains_probing_but_success_or_failure_ends_marker_ownership() {
        let mut probe = ProbeRecord::default();
        probe.begin();
        let key = crate::storage_keys::admission_key("probe-lifecycle");
        let disk = serde_json::to_string(&probe.current).unwrap();
        assert_eq!(
            super::super::probing_keys([(key.as_str(), disk.as_str())]),
            vec![key]
        );
        for (state, reason) in [
            (AdmissionState::Ok, None),
            (AdmissionState::Failed, Some("dispatch failed".to_owned())),
        ] {
            probe.settle(state, reason.clone());
            assert!(probe.released().is_none());
            assert!(probe.rearmed().is_none());
            probe.abandon();
            assert_eq!(probe.current.state, state);
            assert_eq!(probe.current.reason, reason);
            probe.begin();
        }
    }
}
