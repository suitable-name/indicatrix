//! [`LanePool`]: N lanes rendering one image epoch against one [`SampleCursor`].
//!
//! # One run
//!
//! [`LanePool::run`] (or [`LanePool::run_into`]) spawns one scoped thread per lane.
//! Each lane loops:
//!
//! 1. size a chunk from its own [`RateModel`] under the pool's [`ChunkPolicy`]
//!    (a calibration chunk while uncalibrated);
//! 2. claim it from the epoch's cursor ([`SampleCursor::claim_any`]: a failed lane's
//!    returned remainder first, fresh samples after);
//! 3. trace it with [`WorkerLane::render_chunk`];
//! 4. merge the valid prefix into the [`Merger`] (chunk-start order, see its
//!    determinism notes) and fold the measurement into its rate model;
//! 5. on a short chunk: requeue the untraced tail for any lane, count a consecutive
//!    failure, back off, and retire after [`PoolConfig::retire_after_failures`].
//!
//! A lane with nothing to claim waits while any other lane still has a chunk in flight
//! (that chunk could fail and come back); claims and in-flight bookkeeping share one
//! lock, so no lane can conclude "all done" between another lane's claim and its
//! bookkeeping. The run ends when every lane has exited: all samples traced exactly
//! once ([`PoolStatus::Complete`]), cancelled, or every lane retired with samples
//! left ([`PoolStatus::LanesExhausted`]).
//!
//! A lane that panics inside `render_chunk` is treated as a failed chunk with nothing
//! traced; the panic does not take the pool down.

mod epoch;
mod lane_loop;
#[cfg(test)]
mod tests;

use crate::{CancelToken, ChunkPolicy, Merger, RateModel, SampleCursor, SampleRange, WorkerLane};
use epoch::Epoch;
use glam::Vec3;
use indicatrix_net::SceneState;
use std::{
    fmt,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

/// Failure handling and chunk sizing for a [`LanePool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    /// How chunks are sized from each lane's rate.
    pub policy: ChunkPolicy,
    /// Consecutive failed chunks a lane may have before it starts pausing between
    /// attempts (`1`: pause after every failure). A single dropped connection is common
    /// enough that retrying at once is often right.
    pub pause_after_failures: u32,
    /// Consecutive failed chunks after which a lane is retired for the rest of the
    /// epoch. `u32::MAX` never retires.
    pub retire_after_failures: u32,
    /// The first pause; each further consecutive failure doubles it.
    pub backoff_initial: Duration,
    /// The longest pause.
    pub backoff_max: Duration,
}

impl PoolConfig {
    /// Still export / tilt video: 22 s chunks, pause from the 2nd consecutive failure
    /// (15 s doubling to 120 s, the desktop export's schedule), retire after 5.
    pub const EXPORT: Self = Self {
        policy: ChunkPolicy::EXPORT,
        pause_after_failures: 2,
        retire_after_failures: 5,
        backoff_initial: Duration::from_secs(15),
        backoff_max: Duration::from_secs(120),
    };

    /// Interactive requests: 1.5 s chunks, short pauses (250 ms to 2 s), retire after 3.
    pub const INTERACTIVE: Self = Self {
        policy: ChunkPolicy::INTERACTIVE,
        pause_after_failures: 1,
        retire_after_failures: 3,
        backoff_initial: Duration::from_millis(250),
        backoff_max: Duration::from_secs(2),
    };

    /// The pause after `consecutive_failures` failed chunks in a row: zero below
    /// [`Self::pause_after_failures`], then `backoff_initial` doubling per further
    /// failure, capped at `backoff_max`.
    #[must_use]
    pub fn backoff(&self, consecutive_failures: u32) -> Duration {
        if consecutive_failures < self.pause_after_failures.max(1) {
            return Duration::ZERO;
        }
        let doublings = consecutive_failures - self.pause_after_failures.max(1);
        self.backoff_initial
            .saturating_mul(1u32 << doublings.min(16))
            .min(self.backoff_max)
    }
}

/// Something that happened during a [`LanePool`] run. `lane` is the index
/// [`LanePool::add_lane`] returned; [`LanePool::lane_name`] names it.
///
/// Events are delivered on the lane threads, concurrently, in no global order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolEvent {
    /// The lane's thread started claiming work.
    LaneStarted {
        /// The lane.
        lane: usize,
    },
    /// A chunk's traced prefix was merged: progress.
    ChunkMerged {
        /// The lane.
        lane: usize,
        /// The chunk as claimed.
        range: SampleRange,
        /// How many of its samples were traced and merged (a prefix).
        done: u32,
        /// The exact merged total across all lanes after this chunk.
        total_done: u32,
        /// The run's target sample count.
        target: u32,
    },
    /// A chunk ended short; its untraced tail went back to the cursor.
    LaneFailed {
        /// The lane.
        lane: usize,
        /// Why (the lane's own error text, or a pool-side reason).
        error: String,
        /// The tail returned to the cursor for any lane.
        returned: SampleRange,
        /// Failed chunks in a row, this one included.
        consecutive_failures: u32,
        /// The pause the lane now takes; `None` when it is being retired instead.
        pause: Option<Duration>,
    },
    /// The lane failed too often in a row and takes no more work this run.
    LaneRetired {
        /// The lane.
        lane: usize,
        /// Failed chunks in a row.
        consecutive_failures: u32,
    },
    /// The lane stopped normally: nothing left to claim, or cancelled.
    LaneFinished {
        /// The lane.
        lane: usize,
        /// Chunks it merged at least one sample from.
        chunks: u32,
        /// Samples it contributed.
        samples: u32,
    },
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolStatus {
    /// Every sample of the range was traced exactly once.
    Complete,
    /// The cancel token was raised; `missing` samples were not traced.
    Cancelled {
        /// Samples of the range not merged.
        missing: u32,
    },
    /// Every lane retired (or there were none) with `missing` samples untraced. A
    /// coordinator reports this to the viewer as a lost job.
    LanesExhausted {
        /// Samples of the range not merged.
        missing: u32,
    },
}

/// The result of [`LanePool::run`].
#[derive(Debug, Clone, PartialEq)]
pub struct PoolOutcome {
    /// Merged per-pixel radiance sum, `width * height` long.
    pub sum: Vec<Vec3>,
    /// Exact number of samples in `sum`.
    pub count: u32,
    /// How the run ended.
    pub status: PoolStatus,
}

/// [`LanePool::run_into`] was handed a [`Merger`] built for a different image or range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergerMismatch {
    /// `scene.width * scene.height`.
    pub expected_pixels: usize,
    /// The range's first sample.
    pub expected_first_sample: u32,
    /// The merger's pixel count.
    pub merger_pixels: usize,
    /// The merger's first sample.
    pub merger_first_sample: u32,
    /// Samples the merger already held (a fresh merger holds none).
    pub merger_samples: u32,
}

impl fmt::Display for MergerMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "merger built for {} px from sample {} holding {} samples, run needs a fresh \
             one for {} px from sample {}",
            self.merger_pixels,
            self.merger_first_sample,
            self.merger_samples,
            self.expected_pixels,
            self.expected_first_sample
        )
    }
}

impl std::error::Error for MergerMismatch {}

/// One registered lane and its rate model, which persists across runs.
struct LaneSlot {
    lane: Arc<dyn WorkerLane>,
    rate: Mutex<RateModel>,
}

/// See the module doc. Lanes and their rate models persist across runs, so a caller
/// rendering many images of one scene (a tilt video, successive coordinator jobs)
/// calibrates each lane once.
pub struct LanePool {
    config: PoolConfig,
    lanes: Vec<LaneSlot>,
}

impl LanePool {
    /// An empty pool.
    #[must_use]
    pub const fn new(config: PoolConfig) -> Self {
        Self {
            config,
            lanes: Vec::new(),
        }
    }

    /// Registers `lane` starting from `rate`, returning its index.
    pub fn add_lane(&mut self, lane: Arc<dyn WorkerLane>, rate: RateModel) -> usize {
        self.lanes.push(LaneSlot {
            lane,
            rate: Mutex::new(rate),
        });
        self.lanes.len() - 1
    }

    /// The pool's configuration.
    #[must_use]
    pub const fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// How many lanes are registered.
    #[must_use]
    pub const fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// Lane `lane`'s [`WorkerLane::name`].
    #[must_use]
    pub fn lane_name(&self, lane: usize) -> Option<&str> {
        self.lanes.get(lane).map(|slot| slot.lane.name())
    }

    /// Lane `lane`'s current rate model.
    #[must_use]
    pub fn rate(&self, lane: usize) -> Option<RateModel> {
        self.lanes
            .get(lane)
            .map(|slot| *slot.rate.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Renders `range` of `scene` with every lane and returns the merged result.
    /// Blocks until the run ends (see [`PoolStatus`]); `events` is called from the lane
    /// threads as things happen.
    pub fn run(
        &self,
        scene: &SceneState,
        range: SampleRange,
        cancel: &CancelToken,
        events: &(dyn Fn(PoolEvent) + Sync),
    ) -> PoolOutcome {
        let merger = Merger::new(pixel_count(scene), range.first_sample);
        let status = self.run_unchecked(scene, range, &merger, cancel, events);
        let (sum, count) = merger.into_parts();
        PoolOutcome { sum, count, status }
    }

    /// Like [`Self::run`], merging into a caller-owned `merger` so another thread can
    /// read progressive snapshots ([`Merger::snapshot_into`]) while the run is going.
    /// `merger` must be fresh, built with `Merger::new(width * height,
    /// range.first_sample)`.
    ///
    /// # Errors
    ///
    /// [`MergerMismatch`] when `merger` was built for another size or range; nothing
    /// is rendered then.
    pub fn run_into(
        &self,
        scene: &SceneState,
        range: SampleRange,
        merger: &Merger,
        cancel: &CancelToken,
        events: &(dyn Fn(PoolEvent) + Sync),
    ) -> Result<PoolStatus, MergerMismatch> {
        let expected_pixels = pixel_count(scene);
        let merger_samples = merger.total();
        if merger.pixel_count() != expected_pixels
            || merger.first_sample() != range.first_sample
            || merger_samples != 0
        {
            return Err(MergerMismatch {
                expected_pixels,
                expected_first_sample: range.first_sample,
                merger_pixels: merger.pixel_count(),
                merger_first_sample: merger.first_sample(),
                merger_samples,
            });
        }
        Ok(self.run_unchecked(scene, range, merger, cancel, events))
    }

    fn run_unchecked(
        &self,
        scene: &SceneState,
        range: SampleRange,
        merger: &Merger,
        cancel: &CancelToken,
        events: &(dyn Fn(PoolEvent) + Sync),
    ) -> PoolStatus {
        let epoch = Epoch {
            scene,
            cursor: SampleCursor::new(range.first_sample, range.end()),
            merger,
            cancel,
            events,
            config: &self.config,
            target: range.samples,
            pixels: merger.pixel_count(),
            sched: epoch::Sched::new(),
            lanes: &self.lanes,
        };
        if !range.is_empty() {
            thread::scope(|scope| {
                for (index, slot) in self.lanes.iter().enumerate() {
                    let epoch = &epoch;
                    scope.spawn(move || {
                        lane_loop::run_lane(epoch, index, &*slot.lane, &slot.rate);
                    });
                }
            });
        }
        let missing = range.samples.saturating_sub(merger.total());
        if missing == 0 {
            PoolStatus::Complete
        } else if cancel.is_cancelled() {
            PoolStatus::Cancelled { missing }
        } else {
            PoolStatus::LanesExhausted { missing }
        }
    }
}

/// `width * height` of `scene`.
const fn pixel_count(scene: &SceneState) -> usize {
    scene.width as usize * scene.height as usize
}
