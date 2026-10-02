//! Lanes that join a run already in progress: [`LaneFeed`] and the thread that polls it.
//!
//! A late lane claims from the same [`crate::SampleCursor`] as the lanes that started
//! with the run (a requeued range first, fresh samples after) and merges into the same
//! [`crate::Merger`], so it takes part in the epoch exactly like any other lane. The
//! merger folds chunks in ascending `first_sample` order whichever lane traced them, so a
//! lane's start time cannot change which samples end up in the image, only which lane
//! traced which chunk.

use super::{LaneSlot, epoch::Epoch, lane_loop};
use crate::{RateModel, WorkerLane};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

/// How long the feeder thread waits inside one [`LaneFeed::next`] call: also the longest a
/// finished run waits for the feeder to notice it is over.
const FEED_WAIT: Duration = Duration::from_millis(50);

/// A source of lanes that join a run in progress (see
/// [`LanePool::run_into_fed`](super::LanePool::run_into_fed)).
pub trait LaneFeed: Sync {
    /// Waits up to `wait` for lanes to add and returns them with their starting rate
    /// models, or an empty list. The first returned lane gets the event index
    /// `first_index`, the next one `first_index + 1`, and so on, so an implementation
    /// can set up whatever it keys by lane index before the lane starts.
    ///
    /// Called repeatedly from one thread until the run ends; lanes returned after the
    /// last lane has left are dropped unused.
    fn next(&self, first_index: usize, wait: Duration) -> Vec<(Arc<dyn WorkerLane>, RateModel)>;
}

/// Starts lane `index` on its own scoped thread; the roster counts it out when it ends.
pub(super) fn spawn_lane<'scope, 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    epoch: &'env Epoch<'env>,
    index: usize,
    slot: Arc<LaneSlot>,
) {
    scope.spawn(move || {
        let left = lane_loop::run_lane(epoch, index, &*slot.lane, &slot.rate);
        if !left {
            epoch.roster.leave();
        }
    });
}

/// The feeder thread's body: asks `feed` for lanes until the run ends or is cancelled and
/// starts each one that arrives while the roster is still open.
pub(super) fn feed_lanes<'scope, 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    epoch: &'env Epoch<'env>,
    feed: &'env dyn LaneFeed,
) {
    while !epoch.roster.is_closed() && !epoch.cancel.is_cancelled() {
        let late = feed.next(epoch.lane_count(), FEED_WAIT);
        for (lane, rate) in late {
            if !epoch.roster.try_join() {
                return;
            }
            let slot = Arc::new(LaneSlot {
                lane,
                rate: Mutex::new(rate),
            });
            let index = epoch.push_lane(Arc::clone(&slot));
            spawn_lane(scope, epoch, index, slot);
        }
    }
}
