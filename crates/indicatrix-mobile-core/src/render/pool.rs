//! The render worker pool: the state the worker threads share, the epoch they trace, and
//! the frames they publish.
//!
//! # Epochs
//!
//! One epoch is one scene at one size. Its frame is cut into `P` interleaved partitions
//! (`P` is the pool's thread cap, fixed for the pool's life); every thread takes the next
//! chunk from the epoch's [`ChunkPlanner`], traces it against the epoch's shared
//! [`OwnedScene`] with an abort check, and merges the sums into the epoch's
//! [`Accumulator`]. A pass (all partitions for one sample range) is merged only when it is
//! complete, so every pixel shows the same sample count. Starting a new epoch changes the
//! atomic epoch id: running chunks notice within a few milliseconds, give their work back
//! and their results, if any, are dropped as stale.
//!
//! # Frames
//!
//! After every merged pass the committing thread tone-maps the running mean into a frame
//! slot (newest wins, never an older pass over a newer one). When the target is reached
//! and denoise is on, the denoised picture follows. The UI polls the slot.
//!
//! # Locking
//!
//! `Shared::inner` guards the epoch and the controls and is never held while tracing,
//! tone-mapping or denoising. The frame slot and the denoiser have their own locks and
//! are never taken while `inner` is held.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use glam::Vec3;
use indicatrix::renderer::tonemap::tonemap_to_rgba_with_threads;
use indicatrix_web_core::{
    display::{DenoiseFrame, Denoiser},
    render::{
        Accumulator, ChunkAssignment, ChunkOutcome, ChunkPlanner, ChunkRange, trace_chunk_abortable,
    },
    scene::OwnedScene,
};

use super::{PauseReason, RenderFrame, RenderStatus};

/// How long an idle worker sleeps before it looks for work again (a wake-up is also sent
/// whenever work may have appeared).
const IDLE_WAIT: Duration = Duration::from_millis(500);

/// The smoothing of the measured per-thread trace rate.
const RATE_SMOOTHING: f64 = 0.5;

/// Locks `mutex`, recovering the data of a poisoned lock: a panicking tracer must not take
/// the whole pool down with it.
fn relock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the current epoch stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// Nothing runs.
    Idle,
    /// The scene is being built or traced.
    Active,
    /// The target is reached.
    Done,
    /// The epoch ended with this message.
    Failed(String),
}

/// What limits the workers, set by the user, the lifecycle and the governor.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches set by the user, the lifecycle and the governor"
)]
pub(super) struct Control {
    /// The threads the pool runs (and the partitions of a frame).
    pub(super) max_workers: usize,
    /// How many of them may trace now (the governor's decision).
    pub(super) workers_allowed: usize,
    /// The user paused tracing.
    pub(super) user_paused: bool,
    /// The app is in the background.
    pub(super) background: bool,
    /// The device is too hot to trace.
    pub(super) hot: bool,
    /// Whether the settled picture is denoised.
    pub(super) denoise: bool,
}

impl Control {
    /// Why tracing is paused, if it is (user first: it is the one the user can undo).
    pub(super) const fn paused_reason(&self) -> Option<PauseReason> {
        if self.user_paused {
            Some(PauseReason::User)
        } else if self.background {
            Some(PauseReason::Background)
        } else if self.hot {
            Some(PauseReason::Hot)
        } else {
            None
        }
    }
}

/// One scene being traced.
struct Epoch {
    id: u64,
    scene: Arc<OwnedScene>,
    planner: ChunkPlanner,
    acc: Accumulator,
    /// The partition the next chunk search starts at, so the partitions are served evenly.
    cursor: u32,
    /// Per partition: a chunk of it is being traced. One at a time keeps every hand-back
    /// ([`ChunkPlanner::unassign`]) valid.
    busy: Vec<bool>,
    target: u32,
    finished: bool,
}

/// A chunk handed to a thread.
pub(super) struct Job {
    epoch_id: u64,
    assignment: ChunkAssignment,
    scene: Arc<OwnedScene>,
}

/// A copy of the merged frame, taken under the lock and shown outside it.
pub(super) struct Follow {
    epoch_id: u64,
    width: u32,
    height: u32,
    samples: u32,
    target: u32,
    sum: Vec<Vec3>,
    scene: Arc<OwnedScene>,
    /// The target is reached: this is the settled picture.
    last: bool,
    /// Denoise the settled picture.
    denoise: bool,
}

/// Everything behind `Shared::inner`.
struct Inner {
    control: Control,
    phase: Phase,
    samples: u32,
    target: u32,
    epoch: Option<Epoch>,
    /// The planner of the epoch before, kept for its measured chunk times.
    prev_planner: Option<ChunkPlanner>,
    /// Measured pixel-samples per millisecond of one thread.
    rate: Option<f64>,
}

impl Inner {
    /// The next chunk for thread `index`, if it may trace and there is one.
    fn take_job(&mut self, index: usize) -> Option<Job> {
        if self.phase != Phase::Active
            || self.control.paused_reason().is_some()
            || index >= self.control.workers_allowed
        {
            return None;
        }
        let epoch = self.epoch.as_mut()?;
        let partitions = epoch.planner.partitions();
        let completed = epoch.acc.completed_passes();
        for step in 0..partitions {
            let partition = (epoch.cursor + step) % partitions;
            let slot = partition as usize;
            if epoch.busy.get(slot).copied().unwrap_or(true) {
                continue;
            }
            if let Some(assignment) = epoch.planner.next_chunk(partition, completed) {
                epoch.cursor = (partition + 1) % partitions;
                epoch.busy[slot] = true;
                return Some(Job {
                    epoch_id: epoch.id,
                    assignment,
                    scene: Arc::clone(&epoch.scene),
                });
            }
        }
        None
    }

    /// Takes the result of a chunk. Returns the frame to show when it completed a pass.
    fn on_chunk(
        &mut self,
        job: &Job,
        sums: Option<Vec<Vec3>>,
        elapsed: Duration,
    ) -> Option<Follow> {
        let epoch = self
            .epoch
            .as_mut()
            .filter(|epoch| epoch.id == job.epoch_id)?;
        let assignment = job.assignment;
        if let Some(busy) = epoch.busy.get_mut(assignment.first_pixel as usize) {
            *busy = false;
        }
        let Some(sums) = sums else {
            // Aborted: hand the chunk back so a resumed epoch traces it.
            epoch.planner.unassign(&assignment);
            return None;
        };
        let millis = elapsed.as_secs_f64() * 1000.0;
        epoch
            .planner
            .record_timing(assignment.first_pixel, assignment.spp, millis);
        let work = sums.len() as f64 * f64::from(assignment.spp);
        let outcome = epoch.acc.add_chunk(
            epoch.id,
            assignment.first_pixel,
            assignment.stride,
            assignment.sample_offset,
            assignment.spp,
            sums,
        );
        let mut committed = false;
        match outcome {
            ChunkOutcome::Committed { .. } => {
                committed = true;
                epoch.finished = epoch.planner.is_done(epoch.acc.completed_passes());
                self.samples = epoch.acc.sample_count();
                if epoch.finished {
                    self.phase = Phase::Done;
                }
            }
            ChunkOutcome::Rejected(reason) => {
                tracing::warn!(?reason, "the render accumulator refused a chunk");
                self.phase = Phase::Failed("The render stopped unexpectedly.".to_owned());
            }
            ChunkOutcome::Pending | ChunkOutcome::Stale => {}
        }
        if millis > 0.0 && work > 0.0 {
            let sample = work / millis;
            self.rate = Some(
                self.rate
                    .map_or(sample, |old| RATE_SMOOTHING.mul_add(sample - old, old)),
            );
        }
        if committed { self.snapshot() } else { None }
    }

    /// A copy of the merged frame of the current epoch.
    fn snapshot(&self) -> Option<Follow> {
        let epoch = self.epoch.as_ref()?;
        Some(Follow {
            epoch_id: epoch.id,
            width: epoch.acc.width(),
            height: epoch.acc.height(),
            samples: epoch.acc.sample_count(),
            target: epoch.target,
            sum: epoch.acc.sum().to_vec(),
            scene: Arc::clone(&epoch.scene),
            last: epoch.finished,
            denoise: self.control.denoise,
        })
    }
}

/// The newest frame and what was published last, for the "never go backwards" rule.
#[derive(Default)]
struct FrameSlot {
    latest: Option<RenderFrame>,
    /// `(epoch, samples, denoised)` of the newest frame ever published.
    last: Option<(u64, u32, bool)>,
}

/// The state every render thread and the owning `RenderState` share.
pub(super) struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
    frames: Mutex<FrameSlot>,
    /// The epoch the threads must trace; 0 when none.
    current_epoch: AtomicU64,
    /// Tracing is paused: running chunks stop at their next check.
    halt: AtomicBool,
    /// The pool is shutting down.
    closed: AtomicBool,
    denoiser: Mutex<Denoiser>,
}

impl Shared {
    /// A pool state for `max_workers` threads (none spawned yet).
    pub(super) fn new(max_workers: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                control: Control {
                    max_workers,
                    workers_allowed: max_workers,
                    user_paused: false,
                    background: false,
                    hot: false,
                    denoise: true,
                },
                phase: Phase::Idle,
                samples: 0,
                target: 0,
                epoch: None,
                prev_planner: None,
                rate: None,
            }),
            wake: Condvar::new(),
            frames: Mutex::new(FrameSlot::default()),
            current_epoch: AtomicU64::new(0),
            halt: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            denoiser: Mutex::new(Denoiser::new()),
        }
    }

    /// Whether the pool was shut down.
    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Ends the pool: the threads leave their loops.
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.current_epoch.store(0, Ordering::Release);
        {
            let mut inner = relock(&self.inner);
            inner.epoch = None;
            inner.phase = Phase::Idle;
        }
        self.clear_frames();
        self.wake.notify_all();
    }

    /// Whether a chunk of `epoch` should stop now.
    fn abort_requested(&self, epoch: u64) -> bool {
        self.is_closed()
            || self.halt.load(Ordering::Acquire)
            || self.current_epoch.load(Ordering::Acquire) != epoch
    }

    fn clear_frames(&self) {
        *relock(&self.frames) = FrameSlot::default();
    }

    /// Changes the controls under the lock, refreshes the halt flag and wakes the threads.
    pub(super) fn update_control(&self, edit: impl FnOnce(&mut Control)) {
        {
            let mut inner = relock(&self.inner);
            edit(&mut inner.control);
            self.halt
                .store(inner.control.paused_reason().is_some(), Ordering::Release);
        }
        self.wake.notify_all();
    }

    /// The controls as they stand.
    pub(super) fn control(&self) -> Control {
        relock(&self.inner).control.clone()
    }

    /// Starts epoch `id` towards `target` samples; the scene follows through
    /// [`Self::install_scene`].
    pub(super) fn begin_epoch(&self, id: u64, target: u32, denoise: bool) {
        self.current_epoch.store(id, Ordering::Release);
        {
            let mut inner = relock(&self.inner);
            if let Some(old) = inner.epoch.take() {
                inner.prev_planner = Some(old.planner);
            }
            inner.phase = Phase::Active;
            inner.samples = 0;
            inner.target = target;
            inner.control.denoise = denoise;
            self.halt
                .store(inner.control.paused_reason().is_some(), Ordering::Release);
        }
        self.clear_frames();
        self.wake.notify_all();
    }

    /// Hands epoch `id` its built scene (or the reason there is none). Dropped when the
    /// epoch was replaced meanwhile.
    pub(super) fn install_scene(&self, id: u64, built: Result<OwnedScene, String>, target: u32) {
        {
            let mut inner = relock(&self.inner);
            if self.is_closed() || self.current_epoch.load(Ordering::Acquire) != id {
                return;
            }
            match built {
                Ok(scene) => {
                    let (width, height) = (scene.width(), scene.height());
                    let partitions = u32::try_from(inner.control.max_workers).unwrap_or(1).max(1);
                    let pixels = width.saturating_mul(height);
                    let planner = inner.prev_planner.take().map_or_else(
                        || ChunkPlanner::new(partitions, pixels, target),
                        |previous| {
                            ChunkPlanner::with_rates_from(&previous, partitions, pixels, target)
                        },
                    );
                    inner.epoch = Some(Epoch {
                        id,
                        scene: Arc::new(scene),
                        planner,
                        acc: Accumulator::new(id, width, height, partitions),
                        cursor: 0,
                        busy: vec![false; partitions as usize],
                        target,
                        finished: false,
                    });
                }
                Err(message) => inner.phase = Phase::Failed(message),
            }
        }
        self.wake.notify_all();
    }

    /// Cancels the epoch and goes idle, keeping the threads.
    pub(super) fn stop(&self) {
        self.current_epoch.store(0, Ordering::Release);
        {
            let mut inner = relock(&self.inner);
            if let Some(old) = inner.epoch.take() {
                inner.prev_planner = Some(old.planner);
            }
            inner.phase = Phase::Idle;
            inner.samples = 0;
        }
        self.clear_frames();
        self.wake.notify_all();
    }

    /// Cancels the epoch and reports `message` as the reason there is nothing to show.
    pub(super) fn fail_now(&self, message: &str) {
        self.stop();
        relock(&self.inner).phase = Phase::Failed(message.to_owned());
    }

    /// Marks the running epoch `id` as failed (a thread panicked).
    fn fail(&self, id: u64, message: &str) {
        if self.current_epoch.load(Ordering::Acquire) == id {
            relock(&self.inner).phase = Phase::Failed(message.to_owned());
        }
    }

    /// The pool's status.
    pub(super) fn status(&self) -> RenderStatus {
        let inner = relock(&self.inner);
        match &inner.phase {
            Phase::Idle => RenderStatus::Idle,
            Phase::Failed(message) => RenderStatus::Failed(message.clone()),
            Phase::Done => RenderStatus::Done {
                samples: inner.samples,
            },
            Phase::Active => inner.control.paused_reason().map_or(
                RenderStatus::Tracing {
                    samples: inner.samples,
                    target: inner.target,
                },
                RenderStatus::Paused,
            ),
        }
    }

    /// The measured rate of one thread in pixel-samples per millisecond.
    pub(super) fn rate_per_thread(&self) -> Option<f64> {
        relock(&self.inner).rate
    }

    /// The newest frame of the current epoch, once.
    pub(super) fn take_frame(&self) -> Option<RenderFrame> {
        let current = self.current_epoch.load(Ordering::Acquire);
        relock(&self.frames)
            .latest
            .take()
            .filter(|frame| frame.epoch == current)
    }

    /// Switches denoise. Returns the settled frame to re-show when the epoch is done.
    pub(super) fn set_denoise(&self, on: bool) -> Option<Follow> {
        let mut inner = relock(&self.inner);
        inner.control.denoise = on;
        if inner.phase == Phase::Done {
            inner.snapshot()
        } else {
            None
        }
    }

    /// Shows the settled picture of `follow`, denoised or plain, whatever was shown.
    pub(super) fn show_settled(&self, follow: &Follow, denoised: bool) {
        if denoised {
            self.show_denoised(follow, true);
        } else {
            self.publish(plain_frame(follow), true);
        }
    }

    /// Publishes `frame` unless a newer picture of its epoch was published before.
    fn publish(&self, frame: RenderFrame, force: bool) {
        let mut slots = relock(&self.frames);
        if self.current_epoch.load(Ordering::Acquire) != frame.epoch {
            return;
        }
        if !force
            && let Some((epoch, samples, denoised)) = slots.last
            && epoch == frame.epoch
            && (samples > frame.samples
                || (samples == frame.samples && denoised && !frame.denoised))
        {
            return;
        }
        slots.last = Some((frame.epoch, frame.samples, frame.denoised));
        slots.latest = Some(frame);
    }

    fn show_denoised(&self, follow: &Follow, force: bool) {
        let picture = catch_unwind(AssertUnwindSafe(|| {
            relock(&self.denoiser).denoised_rgba(&DenoiseFrame {
                guide_key: follow.epoch_id,
                width: follow.width,
                height: follow.height,
                camera: follow.scene.camera(),
                planes: follow.scene.planes(),
                n_d: follow.scene.material().dispersion.n_d(),
                sample_count: follow.samples,
                sum: &follow.sum,
            })
        }));
        if let Ok(rgba) = picture {
            self.publish(
                RenderFrame {
                    rgba,
                    denoised: true,
                    ..plain_frame_without_pixels(follow)
                },
                force,
            );
        } else {
            tracing::warn!("the denoiser panicked; the plain picture stays");
        }
    }

    /// Shows a merged pass: the plain picture, then the denoised one when it is the last.
    fn show(&self, follow: &Follow) {
        self.publish(plain_frame(follow), false);
        if follow.last && follow.denoise {
            self.show_denoised(follow, false);
        }
    }

    /// Takes the result of a traced chunk and shows what it completed.
    fn complete(&self, job: &Job, sums: Option<Vec<Vec3>>, elapsed: Duration) {
        let follow = relock(&self.inner).on_chunk(job, sums, elapsed);
        self.wake.notify_all();
        if let Some(follow) = follow {
            self.show(&follow);
        }
    }

    /// Blocks until there is a chunk for thread `index`; `None` when the pool is closed.
    fn next_job(&self, index: usize) -> Option<Job> {
        let mut inner = relock(&self.inner);
        loop {
            if self.is_closed() {
                return None;
            }
            if let Some(job) = inner.take_job(index) {
                return Some(job);
            }
            inner = self
                .wake
                .wait_timeout(inner, IDLE_WAIT)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The test view of the running epoch's merged sum and sample count.
    #[cfg(test)]
    pub(super) fn debug_sum(&self) -> Option<(u32, Vec<Vec3>)> {
        let inner = relock(&self.inner);
        inner
            .epoch
            .as_ref()
            .map(|epoch| (epoch.acc.sample_count(), epoch.acc.sum().to_vec()))
    }
}

/// The tone-mapped mean of the merged sum.
fn mean_rgba(sum: &[Vec3], samples: u32) -> Vec<u8> {
    // One thread: the pool's own threads are what the governor budgets.
    tonemap_to_rgba_with_threads(sum, 1.0 / samples.max(1) as f32, 1)
}

/// The plain picture of `follow`.
fn plain_frame(follow: &Follow) -> RenderFrame {
    RenderFrame {
        rgba: mean_rgba(&follow.sum, follow.samples),
        ..plain_frame_without_pixels(follow)
    }
}

/// A frame record of `follow` with no pixels yet.
const fn plain_frame_without_pixels(follow: &Follow) -> RenderFrame {
    RenderFrame {
        epoch: follow.epoch_id,
        width: follow.width,
        height: follow.height,
        rgba: Vec::new(),
        samples: follow.samples,
        target: follow.target,
        denoised: false,
    }
}

/// The body of render thread `index`: trace chunks until the pool closes.
pub(super) fn worker_loop(shared: &Shared, index: usize) {
    while let Some(job) = shared.next_job(index) {
        let assignment = job.assignment;
        let range = ChunkRange {
            first_pixel: assignment.first_pixel,
            stride: assignment.stride,
            sample_offset: assignment.sample_offset,
            spp: assignment.spp,
        };
        let aborted = || shared.abort_requested(job.epoch_id);
        let started = Instant::now();
        let traced = catch_unwind(AssertUnwindSafe(|| {
            trace_chunk_abortable(&job.scene, range, &aborted)
        }));
        let elapsed = started.elapsed();
        if let Ok(sums) = traced {
            shared.complete(&job, sums, elapsed);
        } else {
            tracing::error!("a render thread panicked while tracing");
            shared.fail(job.epoch_id, "The render stopped unexpectedly.");
        }
    }
}
