//! The raster queue: jobs out to the `viewport` background thread, frames back in.
//!
//! One [`RasterWorker`] lives behind a mutex on a dedicated queue. Every job carries a
//! [`Generation`] token; a job that a newer one superseded while it waited is skipped by
//! the worker before it does any work, so a burst of camera moves costs one picture. A
//! finished frame hops back to the UI thread with `upgrade_in_event_loop`, becomes a
//! Slint image there and lands in the viewport state.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::{Duration, Instant},
};

use indicatrix_cut_core::Design;
use indicatrix_mobile_core::{
    app_state::AppState,
    tasks::{BackgroundQueue, Generation},
    viewport::{
        FrameKey, JobInputs,
        frame::{RasterFrame, RasterJob, RasterWorker},
    },
};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, Timer};

use crate::{AppWindow, ViewportModel, design::Shared};

/// How long a frame waits when the state is borrowed at the moment it arrives.
const RETRY: Duration = Duration::from_millis(16);

/// Everything the UI thread keeps for the raster queue.
struct Runtime {
    queue: BackgroundQueue,
    worker: Arc<Mutex<RasterWorker>>,
    generation: Generation,
    /// The key of the newest job submitted, to skip a push that changed nothing.
    last_key: Option<FrameKey>,
    /// A snapshot of the design for jobs, kept while the session does not change.
    snapshot: Option<Snapshot>,
    window: slint::Weak<AppWindow>,
    state: Shared,
}

/// A design copy and what it was taken of.
struct Snapshot {
    /// The address of the session's generation counter (a new session has a new one).
    counter: usize,
    generation: u64,
    design: Arc<Design>,
}

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

/// The time since the first call: stamps the history's coalescing windows and the
/// gesture timing.
pub(super) fn since_start() -> Duration {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed()
}

/// Starts the queue and remembers the window and the state for frames that arrive later.
pub(super) fn install(window: &AppWindow, state: &Shared) {
    let _started = since_start();
    RUNTIME.with(|cell| {
        *cell.borrow_mut() = Some(Runtime {
            queue: BackgroundQueue::new("viewport"),
            worker: Arc::new(Mutex::new(RasterWorker::new())),
            generation: Generation::default(),
            last_key: None,
            snapshot: None,
            window: window.as_weak(),
            state: Rc::clone(state),
        });
    });
}

/// Ends the raster thread and forgets the state. Call on the UI thread when the event
/// loop has ended (see `design::shutdown`).
pub fn shutdown() {
    let runtime = RUNTIME.with(|cell| cell.borrow_mut().take());
    drop(runtime);
}

/// The app state, from the UI thread (for callbacks that only have a timer or an event
/// loop closure).
pub(super) fn shared() -> Option<Shared> {
    RUNTIME.with(|cell| {
        let guard = cell.try_borrow().ok()?;
        guard.as_ref().map(|runtime| Rc::clone(&runtime.state))
    })
}

/// Queues a new picture when what the current one depends on changed.
pub(super) fn request(window: &AppWindow, state: &AppState) {
    RUNTIME.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return;
        };
        if let Some(runtime) = guard.as_mut() {
            runtime.request(window, state);
        }
    });
}

impl Runtime {
    fn request(&mut self, window: &AppWindow, state: &AppState) {
        let (width, height) = state.viewport.view_size;
        if width < 1.0 || height < 1.0 {
            // The viewport has not been laid out yet; its first size triggers a push.
            return;
        }
        if state.viewport.mode.is_render() {
            // The Render mode shows the ray-traced picture; the raster is not on show.
            // Leaving the mode compares the frame key again and draws if it moved.
            return;
        }
        let session = &state.session;
        let generation = session.current_generation();
        let selected = state
            .selected_tier
            .filter(|&tier| tier < session.design.tiers.len());
        let solved = state.solve.current_tiers(generation);
        let key = state.viewport.frame_key(
            generation,
            solved.is_some(),
            selected,
            &session.multi_selected,
        );
        if self.last_key.as_ref() == Some(&key) {
            return;
        }
        self.last_key = Some(key);
        let inputs = JobInputs {
            design: self.design_for(session),
            generation,
            solved: solved.map(<[_]>::to_vec),
            selected_tier: selected,
            multi_selected: session.multi_selected.clone(),
        };
        let token = self.generation.advance();
        let job = state.viewport.job(token, inputs);
        window.global::<ViewportModel>().set_busy(true);
        self.submit(job);
    }

    /// A shared copy of the session's design, made again only when the session moved.
    fn design_for(&mut self, session: &indicatrix_editor::EditorSession) -> Arc<Design> {
        let counter = Arc::as_ptr(&session.generation) as usize;
        let generation = session.current_generation();
        if let Some(snapshot) = &self.snapshot
            && snapshot.counter == counter
            && snapshot.generation == generation
        {
            return Arc::clone(&snapshot.design);
        }
        let design = Arc::new(session.design.clone());
        self.snapshot = Some(Snapshot {
            counter,
            generation,
            design: Arc::clone(&design),
        });
        design
    }

    fn submit(&self, job: RasterJob) {
        let worker = Arc::clone(&self.worker);
        let generation = self.generation.clone();
        let weak = self.window.clone();
        self.queue.submit(move || {
            let token = job.token;
            {
                let mut worker = worker.lock().unwrap_or_else(PoisonError::into_inner);
                worker.run(&job, &|| generation.is_current(token), &mut |frame| {
                    deliver(&weak, frame);
                });
            }
            // Queued after every frame of the job, so the busy bar goes out last.
            let _queued = weak.upgrade_in_event_loop(move |window| job_done(&window, token));
        });
    }
}

/// Runs on the worker thread: makes the pixel buffer and hops to the UI thread.
fn deliver(weak: &slint::Weak<AppWindow>, frame: RasterFrame) {
    let buffer = (!frame.rgba.is_empty()).then(|| {
        let (width, height) = frame.size;
        let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
        let bytes = buffer.make_mut_bytes();
        if bytes.len() == frame.rgba.len() {
            bytes.copy_from_slice(&frame.rgba);
        }
        buffer
    });
    if let Err(error) = weak.upgrade_in_event_loop(move |window| land(&window, frame, buffer)) {
        tracing::debug!(%error, "a viewport frame was dropped: the UI thread is gone");
    }
}

/// Runs on the UI thread: shows the picture and hands the frame to the viewport state.
fn land(window: &AppWindow, frame: RasterFrame, buffer: Option<SharedPixelBuffer<Rgba8Pixel>>) {
    let Some(state) = shared() else {
        return;
    };
    let Ok(mut app) = state.try_borrow_mut() else {
        // The state is busy (an edit is being applied): try again in a moment.
        let weak = window.as_weak();
        Timer::single_shot(RETRY, move || {
            if let Some(window) = weak.upgrade() {
                land(&window, frame, buffer);
            }
        });
        return;
    };
    let model = window.global::<ViewportModel>();
    if let Some(buffer) = buffer {
        model.set_frame(Image::from_rgba8(buffer));
        model.set_has_frame(true);
    }
    let _outcome = app.viewport.land(frame);
    drop(app);
    super::push(window, &state.borrow());
}

/// Runs on the UI thread after a job ended: the viewport is idle again unless a newer
/// job was queued meanwhile.
fn job_done(window: &AppWindow, token: u64) {
    if token_is_current(token) {
        window.global::<ViewportModel>().set_busy(false);
    }
}

/// Whether `token` is the newest job's.
fn token_is_current(token: u64) -> bool {
    RUNTIME.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|runtime| runtime.generation.is_current(token))
            })
            .unwrap_or(true)
    })
}
