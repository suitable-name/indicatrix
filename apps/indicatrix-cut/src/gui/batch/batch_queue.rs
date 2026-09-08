//! A shared work queue that lets a LOCAL worker and a REMOTE dispatcher process the
//! same catalogue-wide batch (`gui::preview_batch`, `gui::tilt_batch`) CONCURRENTLY,
//! each claiming the next unclaimed item as soon as it is free -- the mechanism behind
//! `settings::model::LiveComputeTarget::Both`.
//!
//! Work is handed out one item at a time, never split into sub-ranges: a preview is
//! one fixed-camera render and a tilt-curve set needs all 4 axis sweeps, so there is
//! nothing smaller to divide. [`WorkQueue::claim_shared`] needs no throughput
//! measurement to balance the lanes -- whichever finishes its current claim first
//! simply claims the next, so a faster host naturally does more work and a slower one
//! never blocks the other.
//!
//! A remote failure is requeued for LOCAL only, via [`WorkQueue::return_to_local`],
//! never back to the shared pool: an item failing for a persistent reason (e.g. a
//! missing worker capability) would otherwise have the remote lane spin forever
//! re-failing it. This guarantees such an item is retried at most once, by the lane
//! that hasn't already shown it can't do it.
//!
//! The local lane must not exit just because [`WorkQueue::claim_local`] finds both
//! piles momentarily empty -- the remote lane could still be mid-item and about to
//! push a fresh failure. Each batch module pairs this queue with an `AtomicBool`
//! ("remote lane done") set only after the remote lane's claim loop has permanently
//! ended; the local loop stops only once both piles are empty AND that flag is set.

use crate::settings::LiveComputeTarget;
use std::{
    collections::VecDeque,
    sync::{Mutex, PoisonError},
};

/// How many LOCAL lanes to run concurrently: one less than available parallelism,
/// floored at `1`, reserving a core for the UI thread and the `Mutex<Database>`
/// queries every lane makes (the live viewport's own tracing is separately paused for
/// the batch's duration via `RenderContext::export_active`).
#[must_use]
pub fn local_lane_count() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .saturating_sub(1)
        .max(1)
}

/// See this module's doc comment. `T` is a small, `Send`-able description of one unit
/// of work -- never the rendered/computed result, which each lane produces and
/// persists on its own.
pub struct WorkQueue<T> {
    /// Fresh work either lane may claim.
    shared: Mutex<VecDeque<T>>,
    /// Work a remote attempt already failed, reserved for the local lane alone.
    local_retry: Mutex<VecDeque<T>>,
}

impl<T> WorkQueue<T> {
    pub fn new(items: impl IntoIterator<Item = T>) -> Self {
        Self {
            shared: Mutex::new(items.into_iter().collect()),
            local_retry: Mutex::new(VecDeque::new()),
        }
    }

    /// Claims the next item either lane may take. Used by both the local worker and
    /// the remote dispatcher to draw fresh (never-attempted) work.
    pub fn claim_shared(&self) -> Option<T> {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Claims the next item for the LOCAL lane only: a previously remote-failed item
    /// first (so it doesn't sit behind an arbitrarily long run of fresh work), falling
    /// back to the shared pool once that retry pile is empty.
    pub fn claim_local(&self) -> Option<T> {
        let retried = self
            .local_retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        if let Some(item) = retried {
            return Some(item);
        }
        self.claim_shared()
    }

    /// Returns `item` to the queue for GUARANTEED local processing after a remote
    /// attempt failed -- never re-offered to the remote lane.
    pub fn return_to_local(&self, item: T) {
        self.local_retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(item);
    }
}

/// Which lane(s) a batch should run for `target`, given whether a remote worker is
/// configured -- shared by `gui::preview_batch::spawn_preview_batch` and
/// `gui::tilt_batch::spawn_tilt_batch` so the two batches can't drift apart on it.
///
/// - `LocalOnly`: local only. `RemoteOnly`: remote only, even with no worker configured
///   (see `run_remote`'s doc comment).
/// - `Both`: local always; remote only if a worker is configured (a no-op-beyond-local
///   otherwise, per `settings::model::worker::LiveComputeTarget`'s own doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanePlan {
    pub run_local: bool,
    /// Whether the remote lane should run AT ALL -- for `RemoteOnly` this is `true`
    /// even with no worker configured, so it still drains the queue and tallies every
    /// item `failed` rather than leaving the dialog looking like it did nothing.
    pub run_remote: bool,
    /// Only `true` for `LiveComputeTarget::Both` -- gates whether a remote failure is
    /// requeued for guaranteed local processing, vs. tallied `failed` immediately
    /// (`RemoteOnly`'s "never silently fall back" requirement).
    pub fallback_to_local: bool,
}

impl LanePlan {
    #[must_use]
    pub const fn for_target(target: LiveComputeTarget, worker_configured: bool) -> Self {
        let run_remote = matches!(target, LiveComputeTarget::RemoteOnly)
            || (matches!(target, LiveComputeTarget::Both) && worker_configured);
        Self {
            run_local: !matches!(target, LiveComputeTarget::RemoteOnly),
            run_remote,
            fallback_to_local: matches!(target, LiveComputeTarget::Both),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_shared_drains_fifo() {
        let queue = WorkQueue::new([1, 2, 3]);
        assert_eq!(queue.claim_shared(), Some(1));
        assert_eq!(queue.claim_shared(), Some(2));
        assert_eq!(queue.claim_shared(), Some(3));
        assert_eq!(queue.claim_shared(), None);
    }

    #[test]
    fn claim_local_prefers_the_retry_pile_over_fresh_work() {
        let queue = WorkQueue::new(["fresh-1", "fresh-2"]);
        queue.return_to_local("retried");
        assert_eq!(queue.claim_local(), Some("retried"));
        assert_eq!(queue.claim_local(), Some("fresh-1"));
        assert_eq!(queue.claim_local(), Some("fresh-2"));
        assert_eq!(queue.claim_local(), None);
    }

    #[test]
    fn returned_items_are_invisible_to_claim_shared() {
        let queue: WorkQueue<i32> = WorkQueue::new(std::iter::empty());
        queue.return_to_local(42);
        assert_eq!(queue.claim_shared(), None);
        assert_eq!(queue.claim_local(), Some(42));
    }

    // ---- `LanePlan` -----------------------------------------------------

    #[test]
    fn local_only_never_runs_remote() {
        for worker_configured in [false, true] {
            let plan = LanePlan::for_target(LiveComputeTarget::LocalOnly, worker_configured);
            assert!(plan.run_local);
            assert!(!plan.run_remote);
            assert!(!plan.fallback_to_local);
        }
    }

    #[test]
    fn remote_only_always_runs_remote_and_never_falls_back() {
        for worker_configured in [false, true] {
            let plan = LanePlan::for_target(LiveComputeTarget::RemoteOnly, worker_configured);
            assert!(!plan.run_local);
            assert!(plan.run_remote);
            assert!(!plan.fallback_to_local);
        }
    }

    #[test]
    fn both_runs_remote_only_when_a_worker_is_configured() {
        let with_worker = LanePlan::for_target(LiveComputeTarget::Both, true);
        assert!(with_worker.run_local);
        assert!(with_worker.run_remote);
        assert!(with_worker.fallback_to_local);

        // No worker configured: `Both` degrades to local-only.
        let without_worker = LanePlan::for_target(LiveComputeTarget::Both, false);
        assert!(without_worker.run_local);
        assert!(!without_worker.run_remote);
        // Depends only on target, not whether remote actually runs; never consulted
        // since `run_remote` is false here.
        assert!(without_worker.fallback_to_local);
    }

    #[test]
    fn empty_queue_claims_are_none_on_both_paths() {
        let queue: WorkQueue<i32> = WorkQueue::new(std::iter::empty());
        assert_eq!(queue.claim_shared(), None);
        assert_eq!(queue.claim_local(), None);
    }

    // ---- N concurrent claimants -------------------------------------------
    //
    // Proves under real contention that many lanes hammering `claim_shared`/
    // `claim_local` at once never lose or duplicate an item.

    #[test]
    fn n_concurrent_claim_shared_callers_each_get_a_disjoint_subset() {
        const ITEM_COUNT: i32 = 2_000;
        const LANE_COUNT: usize = 16;

        let queue = WorkQueue::new(0..ITEM_COUNT);
        let claimed: Mutex<Vec<i32>> = Mutex::new(Vec::with_capacity(ITEM_COUNT as usize));

        std::thread::scope(|scope| {
            for _ in 0..LANE_COUNT {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    while let Some(item) = queue.claim_shared() {
                        mine.push(item);
                    }
                    claimed.lock().unwrap().extend(mine);
                });
            }
        });

        let mut claimed = claimed.into_inner().unwrap();
        assert_eq!(
            claimed.len(),
            ITEM_COUNT as usize,
            "no item lost or duplicated"
        );
        claimed.sort_unstable();
        claimed.dedup();
        assert_eq!(
            claimed.len(),
            ITEM_COUNT as usize,
            "every item claimed exactly once, by exactly one lane"
        );
    }

    #[test]
    fn n_concurrent_claim_local_callers_never_duplicate_across_retry_and_shared_piles() {
        const FRESH_COUNT: i32 = 1_000;
        const RETRIED_COUNT: i32 = 500;
        const LANE_COUNT: usize = 16;

        let queue = WorkQueue::new(0..FRESH_COUNT);
        for retried in FRESH_COUNT..(FRESH_COUNT + RETRIED_COUNT) {
            queue.return_to_local(retried);
        }
        let claimed: Mutex<Vec<i32>> =
            Mutex::new(Vec::with_capacity((FRESH_COUNT + RETRIED_COUNT) as usize));

        std::thread::scope(|scope| {
            for _ in 0..LANE_COUNT {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    while let Some(item) = queue.claim_local() {
                        mine.push(item);
                    }
                    claimed.lock().unwrap().extend(mine);
                });
            }
        });

        let mut claimed = claimed.into_inner().unwrap();
        let expected = (FRESH_COUNT + RETRIED_COUNT) as usize;
        assert_eq!(claimed.len(), expected, "no item lost or duplicated");
        claimed.sort_unstable();
        claimed.dedup();
        assert_eq!(
            claimed.len(),
            expected,
            "every fresh and retried item claimed exactly once"
        );
    }

    #[test]
    fn local_lane_count_is_at_least_one_and_leaves_headroom() {
        let n = local_lane_count();
        assert!(n >= 1);
        let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        assert!(n <= available.saturating_sub(1).max(1));
    }
}
