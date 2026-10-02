//! [`Epoch`]: the state one [`super::LanePool`] run shares between its lane threads,
//! and the claim / settle / pause steps that keep the in-flight bookkeeping consistent.
//!
//! A claimed chunk is a [`Claim`] guard: dropping it without settling (a panic unwinding
//! out of the lane's loop) returns the whole range to the cursor and decrements the
//! in-flight count, so a panicking lane can never leave the others waiting for a chunk
//! that no longer has an owner.

use super::{LaneSlot, PoolConfig, PoolEvent};
use crate::{CancelToken, Merger, RateModel, SampleCursor, SampleRange};
use indicatrix_net::SceneState;
use std::{
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock},
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

/// How many lane threads of a run are still going, and whether the roster is closed to
/// newcomers. The roster closes the moment the last lane leaves, so a lane fed in after
/// the run has finished is refused (and dropped) instead of starting.
pub(super) struct Roster {
    state: Mutex<RosterState>,
}

struct RosterState {
    active: usize,
    closed: bool,
}

impl Roster {
    /// A roster with `active` lanes about to start (closed already when there are none).
    pub(super) const fn new(active: usize) -> Self {
        Self {
            state: Mutex::new(RosterState {
                active,
                closed: active == 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, RosterState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether every lane has left.
    pub(super) fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Counts one more lane in; `false` when the run has already ended.
    pub(super) fn try_join(&self) -> bool {
        let mut state = self.lock();
        if state.closed {
            return false;
        }
        state.active += 1;
        true
    }

    /// A lane thread finished: counts it out and closes the roster with the last one.
    pub(super) fn leave(&self) {
        let mut state = self.lock();
        state.active = state.active.saturating_sub(1);
        if state.active == 0 {
            state.closed = true;
        }
    }

    /// Counts the calling lane out unless it is the only one left (`true`: it left).
    /// Check and decrement are one step, so two lanes removed together can never empty
    /// the pool between them.
    pub(super) fn leave_unless_last(&self) -> bool {
        let mut state = self.lock();
        let leave = state.active > 1;
        state.active -= usize::from(leave);
        drop(state);
        leave
    }
}

/// A chunk claimed from the cursor and not yet settled; see the module doc.
pub(super) struct Claim<'e> {
    epoch: &'e Epoch<'e>,
    range: SampleRange,
    settled: bool,
}

impl Claim<'_> {
    /// The claimed sample range.
    pub(super) const fn range(&self) -> SampleRange {
        self.range
    }

    /// Settles the chunk: the prefix `done` was merged, the tail goes back to the cursor.
    pub(super) fn settle(mut self, done: u32) {
        self.settled = true;
        self.epoch.release(self.range, done);
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.epoch.release(self.range, 0);
        }
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
    /// Every registered lane's rate model (this one included), for [`Self::want`]'s
    /// share-of-what-is-left computation. Grows when lanes join a running epoch; a
    /// lane's position is its event index and never changes.
    pub(super) lanes: &'a RwLock<Vec<Arc<LaneSlot>>>,
    /// Which lane threads are still going.
    pub(super) roster: Roster,
}

impl Epoch<'_> {
    /// How many lanes are registered so far.
    pub(super) fn lane_count(&self) -> usize {
        self.lanes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Registers a lane that joins the running epoch and returns its index.
    pub(super) fn push_lane(&self, slot: Arc<LaneSlot>) -> usize {
        let mut lanes = self.lanes.write().unwrap_or_else(PoisonError::into_inner);
        lanes.push(slot);
        lanes.len() - 1
    }

    /// Delivers `event` to the run's observer.
    pub(super) fn emit(&self, event: PoolEvent) {
        (self.events)(event);
    }

    /// The next chunk size for a lane whose own rate model is `rate`: the policy's
    /// calibration chunk while uncalibrated, otherwise a tail-aware, share-aware size
    /// (see [`crate::ChunkPolicy::tail_aware_samples`]) from this lane's current rate,
    /// the run's outstanding sample count, and every lane's summed rate.
    ///
    /// A [`RateModel::staggered`] lane's first calibrated size is scaled by its one-shot
    /// factor ([`RateModel::take_first_chunk_scale`], clamped to the policy's bounds), so
    /// it starts out of phase with the other lanes of its worker; every later size is
    /// the plain one.
    pub(super) fn want(&self, rate: &Mutex<RateModel>) -> u32 {
        let (current, scale) = {
            let mut model = rate.lock().unwrap_or_else(PoisonError::into_inner);
            if !model.is_calibrated() {
                return self.config.policy.first_chunk_samples();
            }
            (model.rate(), model.take_first_chunk_scale())
        };
        let remaining = self.target.saturating_sub(self.merger.total());
        let size = self
            .config
            .policy
            .tail_aware_samples(current, remaining, self.sum_rates());
        scale.map_or(size, |scale| self.config.policy.scaled_samples(size, scale))
    }

    /// Sum of every lane's current rate (guess or calibrated estimate) -- cheap for the
    /// handful of lanes one job has.
    fn sum_rates(&self) -> f64 {
        self.lanes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|slot| {
                slot.rate
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .rate()
            })
            .sum()
    }

    /// Claims up to `want` samples, waiting while nothing is claimable but another
    /// lane still has a chunk in flight (it may fail and return a remainder). `None`
    /// when cancelled or when nothing is left and nothing can come back.
    pub(super) fn claim(&self, want: u32) -> Option<Claim<'_>> {
        let mut in_flight = self.sched.lock();
        loop {
            if self.cancel.is_cancelled() {
                return None;
            }
            if let Some((first, count)) = self.cursor.claim_any(want) {
                *in_flight += 1;
                return Some(Claim {
                    epoch: self,
                    range: SampleRange::new(first, count),
                    settled: false,
                });
            }
            if *in_flight == 0 {
                return None;
            }
            in_flight = self.sched.wait(in_flight, POLL);
        }
    }

    /// Ends a claimed chunk of which the prefix `done` was merged: returns the tail to
    /// the cursor for any lane and wakes every waiting lane.
    fn release(&self, range: SampleRange, done: u32) {
        let tail = range.after_prefix(done);
        let mut in_flight = self.sched.lock();
        self.cursor.requeue(tail.first_sample, tail.samples);
        *in_flight = in_flight.saturating_sub(1);
        drop(in_flight);
        self.sched.wake.notify_all();
    }

    /// Waits until the merge frontier moves past `seen`, the run is cancelled, or no
    /// other lane has a chunk in flight (nothing left that could advance it). Always
    /// waits at least one poll interval, so a lane whose merge was refused can never spin.
    pub(super) fn wait_for_frontier(&self, seen: u32) {
        let earliest_exit = Instant::now() + POLL;
        let mut in_flight = self.sched.lock();
        loop {
            if self.cancel.is_cancelled() || self.merger.frontier() != seen {
                return;
            }
            if *in_flight == 0 && Instant::now() >= earliest_exit {
                return;
            }
            in_flight = self.sched.wait(in_flight, POLL);
        }
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
