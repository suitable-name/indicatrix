//! Observational counters only: never participate in candidate acceptance or ordering.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

/// Inclusive phase timings; solve and validation time are included in projection time.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SearchDiagnostics {
    /// Nanoseconds by phase: baseline, projection, solves, validation, Fast optics, Full optics, final checks.
    pub phase_ns: [u64; 7],
    /// Number of calls to each phase in the same order.
    pub phase_calls: [u64; 7],
    /// Projection iterations, including candidates rejected before optics.
    pub iterations: u64,
    /// Proposals rejected by projection.
    pub rejected: u64,
    /// How often a turned tier whose inward reach could not be read was granted the
    /// half-length fallback by a policy that follows the turn (one per such tier per
    /// assessment; see `PreservationPolicy::bounds_follow_turn`). Zero means every turn
    /// allowance came from a reach read in the original geometry.
    #[serde(default)]
    pub turn_reach_fallbacks: u64,
}
#[derive(Debug, Default)]
pub struct Telemetry {
    times: [AtomicU64; 7],
    calls: [AtomicU64; 7],
    iterations: AtomicU64,
    rejected: AtomicU64,
    turn_reach_fallbacks: AtomicU64,
}
// Measurements are not part of the immutable geometry's identity.
impl PartialEq for Telemetry {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Telemetry {
    pub(crate) fn phase(&self, phase: usize) -> Phase<'_> {
        self.calls[phase].fetch_add(1, Ordering::Relaxed);
        #[cfg(not(target_arch = "wasm32"))]
        let start = Some(Instant::now());
        #[cfg(target_arch = "wasm32")]
        let start = None; // std::Instant is unavailable on browser wasm; counts still work.
        Phase {
            telemetry: self,
            slot: phase,
            start,
        }
    }
    pub(crate) fn iteration(&self) {
        self.iterations.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn rejected(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }
    /// Records `count` turned tiers granted the half-length reach fallback.
    pub(crate) fn turn_reach_fallbacks(&self, count: usize) {
        if count > 0 {
            self.turn_reach_fallbacks
                .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
        }
    }
    pub(crate) fn snapshot(&self) -> SearchDiagnostics {
        SearchDiagnostics {
            phase_ns: std::array::from_fn(|i| self.times[i].load(Ordering::Relaxed)),
            phase_calls: std::array::from_fn(|i| self.calls[i].load(Ordering::Relaxed)),
            iterations: self.iterations.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            turn_reach_fallbacks: self.turn_reach_fallbacks.load(Ordering::Relaxed),
        }
    }
}
pub struct Phase<'a> {
    telemetry: &'a Telemetry,
    slot: usize,
    start: Option<Instant>,
}
impl Drop for Phase<'_> {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        let ns = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.telemetry.times[self.slot].fetch_add(ns, Ordering::Relaxed);
    }
}
