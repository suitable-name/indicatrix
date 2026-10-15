//! The local picture export: a scene snapshot at export size, traced to a sample target
//! and encoded as a PNG with an ICC profile for the chosen color space.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use glam::Vec3;
use indicatrix::color::ColorSpace;
use indicatrix_web_core::{
    display::export_png,
    render::{
        Accumulator, ChunkAssignment, ChunkOutcome, ChunkPlanner, ChunkRange, trace_chunk_abortable,
    },
    scene::OwnedScene,
};

/// The message of a cancelled export.
pub const CANCELLED: &str = "The export was cancelled.";

/// One traced chunk: its assignment, its sums (`None` when it was aborted) and its time in
/// milliseconds.
type Traced = (ChunkAssignment, Option<Vec<Vec3>>, f64);

/// A picture export that owns everything it needs: the scene snapshot, the size and the
/// sample target. Send it to a thread and call [`Self::run`].
pub struct ExportJob {
    scene: Arc<OwnedScene>,
    width: u32,
    height: u32,
    spp: u32,
    color_space: ColorSpace,
    workers: usize,
    /// Measured pixel-samples per millisecond of one thread, if the live render has run.
    rate_per_thread: Option<f64>,
}

impl ExportJob {
    /// A job over `scene` (already built at `width` x `height`).
    pub(super) fn new(
        scene: Arc<OwnedScene>,
        spp: u32,
        color_space: ColorSpace,
        workers: usize,
        rate_per_thread: Option<f64>,
    ) -> Self {
        Self {
            width: scene.width(),
            height: scene.height(),
            scene,
            spp,
            color_space,
            workers,
            rate_per_thread,
        }
    }

    /// The picture's width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The picture's height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Samples per pixel the export traces.
    #[must_use]
    pub const fn spp(&self) -> u32 {
        self.spp
    }

    /// A guess of how long [`Self::run`] takes, from the live render's measured speed;
    /// `None` when the live render has not traced anything yet.
    #[must_use]
    pub fn estimate_seconds(&self) -> Option<f32> {
        let rate = self.rate_per_thread.filter(|rate| *rate > 0.0)?;
        let work = f64::from(self.width) * f64::from(self.height) * f64::from(self.spp);
        let millis = work / (rate * self.workers.max(1) as f64);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the estimate is shown as f32 seconds; the value is far inside the f32 range"
        )]
        let seconds = (millis / 1000.0) as f32;
        Some(seconds)
    }

    /// Traces the picture and encodes it. Blocks; run it on a thread of its own. It uses
    /// the render thread count and reports the fraction done (0.0 to 1.0) after every pass.
    /// Setting `cancel` ends it within a fraction of a second.
    ///
    /// # Errors
    ///
    /// [`CANCELLED`] when `cancel` was set; otherwise a sentence about what failed (a
    /// thread panicked, the PNG encoder failed).
    pub fn run(&self, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        let partitions = u32::try_from(self.workers).unwrap_or(1).max(1);
        let pixels = self.width.saturating_mul(self.height);
        let mut planner = ChunkPlanner::new(partitions, pixels, self.spp);
        let mut acc = Accumulator::new(0, self.width, self.height, partitions);
        progress(0.0);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.to_owned());
            }
            // All partitions of one pass, so the pass completes before the next starts.
            let round: Vec<ChunkAssignment> = (0..partitions)
                .filter_map(|partition| planner.next_chunk(partition, acc.completed_passes()))
                .collect();
            if round.is_empty() {
                break;
            }
            for (assignment, sums, millis) in self.trace_round(&round, cancel)? {
                let sums = sums.ok_or_else(|| CANCELLED.to_owned())?;
                planner.record_timing(assignment.first_pixel, assignment.spp, millis);
                let outcome = acc.add_chunk(
                    0,
                    assignment.first_pixel,
                    assignment.stride,
                    assignment.sample_offset,
                    assignment.spp,
                    sums,
                );
                if let ChunkOutcome::Rejected(reason) = outcome {
                    return Err(format!("The export stopped unexpectedly ({reason:?})."));
                }
            }
            progress((acc.sample_count() as f32 / self.spp.max(1) as f32).min(1.0));
        }
        export_png(
            self.width,
            self.height,
            acc.sample_count(),
            acc.sum(),
            self.color_space,
            None,
        )
    }

    /// Traces one chunk per partition of `round` on scoped threads.
    fn trace_round(
        &self,
        round: &[ChunkAssignment],
        cancel: &AtomicBool,
    ) -> Result<Vec<Traced>, String> {
        let scene: &OwnedScene = &self.scene;
        std::thread::scope(|scope| {
            #[expect(
                clippy::needless_collect,
                reason = "every thread must be spawned before the first join, or the round runs serially"
            )]
            let handles: Vec<_> = round
                .iter()
                .map(|&assignment| {
                    scope.spawn(move || {
                        let range = ChunkRange {
                            first_pixel: assignment.first_pixel,
                            stride: assignment.stride,
                            sample_offset: assignment.sample_offset,
                            spp: assignment.spp,
                        };
                        let started = Instant::now();
                        let sums =
                            trace_chunk_abortable(scene, range, &|| cancel.load(Ordering::Relaxed));
                        (assignment, sums, started.elapsed().as_secs_f64() * 1000.0)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "The export stopped unexpectedly.".to_owned())
                })
                .collect()
        })
    }
}
