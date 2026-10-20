//! One worker, one replacement slot, one retained result. No render/UI lock while tracing.
use super::snapshot::OpticalSnapshot;
use indicatrix::color::metrics::{
    OpticalImage, OpticalImageError, OpticalImageRequest, evaluate_optical_image,
};
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard,
    atomic::{AtomicU64, Ordering},
};
/// The optional channels of a pass; each costs extra traces, so each is traced only while
/// its layer is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channels {
    /// The F/C companion traces of returned rays.
    pub fire: bool,
    /// The five yaw-offset centre rays of the scintillation temporal term.
    pub scintillation: bool,
}
#[derive(Clone)]
pub struct Job {
    pub serial: u64,
    pub scene: Arc<OpticalSnapshot>,
    pub width: u32,
    pub height: u32,
    pub channels: Channels,
}
pub struct Completed {
    pub serial: u64,
    pub result: Result<Arc<OpticalImage>, OpticalImageError>,
    pub refined: bool,
}
#[derive(Default)]
struct State {
    pending: Option<Job>,
    result: Option<Completed>,
    closed: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    latest: AtomicU64,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
pub struct Worker(Arc<Shared>);
/// The two passes of one job: a small single-sample draft, then the refinement.
const PASSES: [(u32, u32, bool); 2] = [(128, 1, false), (320, 4, true)];
/// Size of a pass: the longer edge becomes `edge` pixels, never larger than the frame.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the scaled edge is positive, rounded and bounded by the frame size"
)]
fn pass_extent(frame: u32, edge: u32, longest: u32) -> u32 {
    let scale = (edge as f32 / longest.max(1) as f32).min(1.0);
    (frame as f32 * scale).round().max(1.0) as u32
}
impl Worker {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            latest: AtomicU64::new(0),
        });
        let bg = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("indicatrix-optical".to_string())
            .spawn(move || run(&bg));
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the optical diagnostics worker; the overlay stays empty");
        }
        Self(shared)
    }
    pub fn submit(&self, job: Job) {
        self.0.latest.store(job.serial, Ordering::Release);
        self.0.lock().pending = Some(job);
        self.0.wake.notify_all();
    }
    pub fn cancel(&self, serial: u64) {
        self.0.latest.store(serial, Ordering::Release);
        {
            let mut state = self.0.lock();
            state.pending = None;
            state.result = None;
        }
        // A worker holding a draft for pickup must notice the cancel at once.
        self.0.wake.notify_all();
    }
    /// Takes the finished pass, if any. Taking a draft releases the worker to refine it.
    pub fn take(&self) -> Option<Completed> {
        let taken = self.0.lock().result.take();
        if taken.is_some() {
            self.0.wake.notify_all();
        }
        taken
    }
}
fn run(bg: &Shared) {
    loop {
        let job = {
            let mut state = bg.lock();
            while state.pending.is_none() && !state.closed {
                state = bg
                    .wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.closed {
                break;
            }
            state.pending.take().expect("pending work")
        };
        let longest = job.width.max(job.height);
        for (edge, samples, refined) in PASSES {
            let camera = job.scene.camera();
            let request = OpticalImageRequest {
                camera: &camera,
                width: pass_extent(job.width, edge, longest),
                height: pass_extent(job.height, edge, longest),
                samples,
                projection_size: Some([job.width, job.height]),
                tile_origin: None,
                geometry: job.scene.geometry(),
                material: &job.scene.material,
                environment: job.scene.environment(),
                want_fire: job.channels.fire,
                want_scintillation: job.channels.scintillation,
                pose: Some(job.scene.pose),
            };
            let result = evaluate_optical_image(&request, || {
                bg.latest.load(Ordering::Acquire) != job.serial
            })
            .map(Arc::new);
            if bg.latest.load(Ordering::Acquire) != job.serial {
                break;
            }
            let completed = Completed {
                serial: job.serial,
                result,
                refined,
            };
            if !deliver(bg, completed) {
                break;
            }
        }
    }
}
/// Puts a finished pass into the slot. Returns `false` when the job is over: it was
/// superseded, the worker is closing, or the pass failed.
fn deliver(bg: &Shared, completed: Completed) -> bool {
    let serial = completed.serial;
    let failed = completed.result.is_err();
    let refined = completed.refined;
    let mut state = bg.lock();
    if bg.latest.load(Ordering::Acquire) != serial || state.closed {
        return false;
    }
    state.result = Some(completed);
    if failed {
        return false;
    }
    if !refined {
        // Hold the draft until the UI took it (or the job was superseded): the
        // slot holds one result, and the refinement must not overwrite an
        // unpublished draft. `take`, `cancel`, `submit` and `Drop` all notify.
        while bg.latest.load(Ordering::Acquire) == serial && state.result.is_some() && !state.closed
        {
            state = bg
                .wake
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    drop(state);
    true
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.latest.fetch_add(1, Ordering::AcqRel);
        self.0.lock().closed = true;
        self.0.wake.notify_all();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use std::time::{Duration, Instant};
    fn job(serial: u64) -> Job {
        let mut ctx = RenderContext::default();
        let mut width_cache = crate::bridge::frame_cache::stone_width::StoneWidthCache::new();
        Job {
            serial,
            scene: Arc::new(
                OpticalSnapshot::capture(&mut ctx, &mut width_cache).expect("default scene"),
            ),
            width: 16,
            height: 12,
            channels: Channels::default(),
        }
    }
    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn holds_result(worker: &Worker) -> bool {
        worker.0.lock().result.is_some()
    }
    #[test]
    fn draft_is_held_until_taken_then_refined() {
        let worker = Worker::new();
        worker.submit(job(1));
        wait_until("the draft", || holds_result(&worker));
        // Left untaken, the draft stays: the refinement must not replace it.
        std::thread::sleep(Duration::from_millis(100));
        let draft = worker.take().expect("the held draft");
        assert_eq!(draft.serial, 1);
        assert!(!draft.refined);
        assert!(draft.result.is_ok());
        // Taking it releases the worker, which now delivers the refined pass.
        wait_until("the refinement", || holds_result(&worker));
        let refined = worker.take().expect("the refinement");
        assert_eq!(refined.serial, 1);
        assert!(refined.refined);
    }
    #[test]
    fn the_scintillation_channel_reaches_the_pass_with_the_scene_pose() {
        let worker = Worker::new();
        let mut scintillating = job(1);
        scintillating.channels.scintillation = true;
        worker.submit(scintillating);
        wait_until("the draft", || holds_result(&worker));
        let draft = worker.take().expect("the draft");
        let map = draft.result.expect("a pose accompanies the request");
        assert!(map.scint_computed);
        assert!(!map.fire_computed, "channels are independent");
    }
    #[test]
    fn cancel_releases_a_held_draft() {
        let worker = Worker::new();
        worker.submit(job(1));
        wait_until("the draft", || holds_result(&worker));
        worker.cancel(2);
        assert!(!holds_result(&worker), "cancel drops the held draft");
        // The worker left its wait: a later job is picked up and drafted.
        worker.submit(job(3));
        wait_until("the next draft", || holds_result(&worker));
        let next = worker.take().expect("the next draft");
        assert_eq!(next.serial, 3);
        assert!(!next.refined);
    }
}
