//! [`Epoch`]: the state one [`super::LanePool`] run shares between its lane threads,
//! and the claim / settle / pause steps that keep the in-flight bookkeeping consistent.

use super::{PoolConfig, PoolEvent};
use crate::{CancelToken, Merger, SampleCursor, SampleRange};
use indicatrix_net::SceneState;
use std::{
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// How often a waiting lane re-checks the cancel token even without a wake-up.
const POLL: Duration = Duration::from_millis(25);

/// The in-flight counter and the condition variable idle lanes wait on.
pub(super) struct Sched {
    /// Chunks claimed and not yet settled. Guarded together with every claim, so an
    /// idle lane never sees "nothing claimable and nothing in flight" while a claim is
    /// between the cursor and this counter.
    in_flight: Mutex<u32>,
    wake: Condvar,
}

impl Sched {
    pub(super) const fn new() -> Self {
        Self {
            in_flight: Mutex::new(0),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, u32> {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'g>(&self, guard: MutexGuard<'g, u32>, timeout: Duration) -> MutexGuard<'g, u32> {
        self.wake
            .wait_timeout(guard, timeout)
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }
}

/// Everything one run's lanes share. Built by `LanePool::run_unchecked`.
pub(super) struct Epoch<'a> {
    pub(super) scene: &'a SceneState,
    pub(super) cursor: SampleCursor,
    pub(super) merger: &'a Merger,
    pub(super) cancel: &'a CancelToken,
    pub(super) events: &'a (dyn Fn(PoolEvent) + Sync),
    pub(super) config: &'a PoolConfig,
    /// The run's sample count.
    pub(super) target: u32,
    /// `width * height`.
    pub(super) pixels: usize,
    pub(super) sched: Sched,
}

impl Epoch<'_> {
    /// Delivers `event` to the run's observer.
    pub(super) fn emit(&self, event: PoolEvent) {
        (self.events)(event);
    }

    /// Claims up to `want` samples, waiting while nothing is claimable but another
    /// lane still has a chunk in flight (it may fail and return a remainder). `None`
    /// when cancelled or when nothing is left and nothing can come back.
    pub(super) fn claim(&self, want: u32) -> Option<SampleRange> {
        let mut in_flight = self.sched.lock();
        loop {
            if self.cancel.is_cancelled() {
                return None;
            }
            if let Some((first, count)) = self.cursor.claim_any(want) {
                *in_flight += 1;
                return Some(SampleRange::new(first, count));
            }
            if *in_flight == 0 {
                return None;
            }
            in_flight = self.sched.wait(in_flight, POLL);
        }
    }

    /// Ends a claimed chunk of which the prefix `done` was merged: returns the tail to
    /// the cursor for any lane and wakes every waiting lane.
    pub(super) fn settle(&self, range: SampleRange, done: u32) {
        let tail = range.after_prefix(done);
        let mut in_flight = self.sched.lock();
        self.cursor.requeue(tail.first_sample, tail.samples);
        *in_flight -= 1;
        drop(in_flight);
        self.sched.wake.notify_all();
    }

    /// A failed lane's pause. `true` when the full pause elapsed and work may still
    /// come; `false` (stop now) when cancelled, or when nothing is claimable and no
    /// chunk is in flight -- the other lanes finished everything meanwhile.
    pub(super) fn pause(&self, total: Duration) -> bool {
        let deadline = Instant::now() + total;
        let mut in_flight = self.sched.lock();
        loop {
            if self.cancel.is_cancelled() || (*in_flight == 0 && self.cursor.fully_claimed()) {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            in_flight = self.sched.wait(in_flight, (deadline - now).min(POLL));
        }
    }
}
