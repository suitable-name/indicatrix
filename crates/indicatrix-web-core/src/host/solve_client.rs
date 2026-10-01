//! [`SolveClient`]: one solve-role Worker, with superseding, a hang timeout and respawn.
//! The pool holds two: the design solve's, and the analysis Worker's for the current-view
//! metrics and the tilt sweep (`WorkerPool::analysis`), so those never preempt a solve.
//!
//! The analysis client can be given an HDR map ([`SolveClient::set_hdr`]) for the metrics
//! to be scored under. It keeps the map and sends it to every Worker it starts, so a
//! Worker that is created late or replaced after a crash, a timeout or a cancel scores
//! under the same map as the one it follows.

use std::{
    cell::RefCell,
    future::Future,
    rc::{Rc, Weak},
};

use futures_channel::oneshot;
use wasm_bindgen::{JsCast, closure::Closure};

use super::{cancel_url::CancelUrl, handle::WorkerHandle, now_ms};
use crate::{
    protocol::{FromWorker, PROTOCOL_VERSION, ToWorker, WorkerRole},
    solve::{SolveRequest, SolveResponse},
    solve_error::SolveError,
};

/// How long a solve may run before its Worker is terminated and respawned: 60 s.
pub const DEFAULT_SOLVE_TIMEOUT_MS: u32 = 60_000;

/// How long an Optimize or Retarget search may go silent before its Worker is respawned.
///
/// Five minutes without a progress report. Such a search runs far longer than a
/// solve, so [`DEFAULT_SOLVE_TIMEOUT_MS`] would kill it; instead every progress report
/// re-arms this timer, and a search that reports nothing for this long is hung. (One
/// evaluation of a large, heavily meet-derived design takes several seconds natively.)
pub const ANALYSIS_INACTIVITY_TIMEOUT_MS: u32 = 300_000;

/// How long a superseded job may have run before its Worker is terminated.
///
/// An older job is abandoned by terminating the Worker, rather than letting it finish
/// while the new request waits. The threshold is the desktop's synchronous-solve limit
/// (`solve_policy::SYNC_SOLVE_TIME_LIMIT`, 500 ms). A younger job is left to finish
/// (its answer is discarded), since a restart costs more than the wait.
pub const SUPERSEDE_RESPAWN_AFTER_MS: f64 = 500.0;

/// How long a cancelled search may take to stop and answer with its best result so far
/// before its Worker is terminated instead: 30 s.
///
/// A cancelled search does not answer at once: the Worker sees the revoked cancel URL at
/// its next progress report (one evaluation of the design), then scores the design before
/// and after at full fidelity (two evaluations of over a second each natively), on one
/// wasm thread. Measured in the browser on a 12-tier design (`rbc445`, 3 free tiers): the
/// starting-point scoring takes 10.7-11.9 s and the closing one about 10 s, so
///
/// - a cancel in the middle of the search answers after ~11 s (12 s left 0.9 s of margin,
///   and lost the partial result whenever the closing scoring ran a little slow), and
/// - a cancel while the STARTING point is still being scored (the first ~11 s of a run)
///   answers after ~21 s: the Worker cannot look at the cancel flag before that scoring
///   ends, and then still owes the closing one. A 12 s grace period ended every such
///   cancel in the hard fallback and lost the partial result.
///
/// 30 s covers both with room for a slower machine. The page offers "Stop now" (the hard
/// cancel) the whole time it waits.
pub const GRACEFUL_CANCEL_MS: u32 = 30_000;

/// Restarts allowed for a Worker that crashes before it ever reports ready.
const MAX_CRASHES_BEFORE_READY: u32 = 3;

type Reply = Result<SolveResponse, SolveError>;

/// A running job's progress report (an Optimize or Retarget search streams these).
#[derive(Debug, Clone, PartialEq)]
pub struct SolveProgress {
    /// The Worker's human-readable stage line (`optimize_progress_status`).
    pub message: String,
    /// Completion fraction when the stage counts evaluations.
    pub fraction: Option<f32>,
}

/// Where a job's progress goes; called from the Worker's `onmessage`, never re-entrantly
/// from a [`SolveClient`] method.
type ProgressFn = Rc<dyn Fn(SolveProgress)>;

struct Queued {
    job_id: u64,
    design_toml: String,
    request: SolveRequest,
    sender: oneshot::Sender<Reply>,
    on_progress: Option<ProgressFn>,
    /// The search's cancel URL (a plain solve has none).
    cancel: Option<CancelUrl>,
}

struct Running {
    job_id: u64,
    /// The hang timeout this job's timer is armed with (re-armed on progress).
    timeout_ms: u32,
    started_ms: f64,
    /// `None` once superseded: the answer is then discarded.
    sender: Option<oneshot::Sender<Reply>>,
    timer: Option<(i32, Closure<dyn FnMut()>)>,
    on_progress: Option<ProgressFn>,
    /// The Worker's cancel URL; dropping it (revoking it) asks the search to stop.
    cancel: Option<CancelUrl>,
    /// A graceful cancel was requested: the timer is now [`GRACEFUL_CANCEL_MS`], and
    /// progress no longer re-arms it.
    cancelling: bool,
}

struct Inner {
    self_weak: Weak<RefCell<Self>>,
    handle: Option<WorkerHandle>,
    spawn_id: u64,
    ready: bool,
    /// Set when the Worker refused `Init`; every job fails with it until a respawn.
    failure: Option<String>,
    /// Crashes since the Worker last reported ready.
    crashes_before_ready: u32,
    retired: Vec<WorkerHandle>,
    retired_timers: Vec<Closure<dyn FnMut()>>,
    next_job_id: u64,
    running: Option<Running>,
    queued: Option<Queued>,
    timeout_ms: u32,
    /// The HDR map every Worker of this client is given once it is ready, and its id.
    hdr: Option<(u64, Rc<Vec<u8>>)>,
}

/// Replies and progress reports to deliver once the client's borrow is released.
#[derive(Default)]
struct Deliveries {
    replies: Vec<(oneshot::Sender<Reply>, Reply)>,
    progress: Vec<(ProgressFn, SolveProgress)>,
}

impl Deliveries {
    fn push(&mut self, sender: oneshot::Sender<Reply>, reply: Reply) {
        self.replies.push((sender, reply));
    }

    fn push_progress(&mut self, on_progress: ProgressFn, progress: SolveProgress) {
        self.progress.push((on_progress, progress));
    }

    fn deliver(self) {
        for (on_progress, progress) in self.progress {
            on_progress(progress);
        }
        for (sender, reply) in self.replies {
            // The receiver may be gone (the caller dropped the future); nothing to do.
            let _ = sender.send(reply);
        }
    }
}

/// The solve Worker. A cheap `Rc` handle.
#[derive(Clone)]
pub struct SolveClient {
    inner: Rc<RefCell<Inner>>,
}

impl SolveClient {
    /// Spawns the solve Worker (see [`super::WorkerPool::new`]).
    pub(super) fn new() -> Result<Self, String> {
        let inner = Rc::new_cyclic(|weak| {
            RefCell::new(Inner {
                self_weak: weak.clone(),
                handle: None,
                spawn_id: 0,
                ready: false,
                failure: None,
                crashes_before_ready: 0,
                retired: Vec::new(),
                retired_timers: Vec::new(),
                next_job_id: 1,
                running: None,
                queued: None,
                timeout_ms: DEFAULT_SOLVE_TIMEOUT_MS,
                hdr: None,
            })
        });
        inner.borrow_mut().spawn()?;
        Ok(Self { inner })
    }

    /// Gives the Worker an HDR map to hold under `id`, replacing any earlier one: the
    /// metrics jobs whose `MetricsParams::hdr_id` is `id` are scored under it.
    ///
    /// The map is posted at once when the Worker is ready (behind a running job, which
    /// finishes first), else when it becomes ready; and again to every Worker the client
    /// starts later. The Worker answers `HdrLoaded` or `HdrError`; a map it could not
    /// decode is logged to the console and the metrics stay under the preset.
    pub fn set_hdr(&self, id: u64, bytes: Rc<Vec<u8>>) {
        let mut inner = self.inner.borrow_mut();
        inner.hdr = Some((id, bytes));
        inner.send_hdr();
    }

    /// Drops the HDR map: the Worker frees its copy and the metrics are scored under the
    /// preset again.
    pub fn clear_hdr(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.hdr = None;
        if inner.ready
            && let Some(handle) = &inner.handle
            && let Err(error) = handle.post(&ToWorker::ClearHdr)
        {
            warn(&format!(
                "could not tell the analysis worker to drop its HDR map: {error}"
            ));
        }
    }

    /// Sets the hang timeout for jobs dispatched from now on (default
    /// [`DEFAULT_SOLVE_TIMEOUT_MS`]).
    pub fn set_timeout_ms(&self, timeout_ms: u32) {
        self.inner.borrow_mut().timeout_ms = timeout_ms.max(1);
    }

    /// Runs `request` on the design (`solve::design_to_toml`) in the solve Worker.
    ///
    /// A new call supersedes the previous one: the previous future resolves
    /// `Err(SolveError::Superseded)` at once. If the previous job has been running for
    /// [`SUPERSEDE_RESPAWN_AFTER_MS`] or longer, the Worker is terminated and respawned
    /// so this job starts right away; otherwise this job starts when that one ends.
    /// A job with no answer within the timeout resolves `Err(SolveError::Failed(..))`,
    /// and its Worker is terminated and respawned.
    pub fn solve(
        &self,
        design_toml: String,
        request: SolveRequest,
    ) -> impl Future<Output = Result<SolveResponse, SolveError>> + use<> {
        self.submit(design_toml, request, None)
    }

    /// [`Self::solve`], with `on_progress` called for every progress report the job
    /// streams while it runs (an Optimize or Retarget search; a plain solve reports
    /// none). The callback is dropped with the job, and never called after the job's
    /// future has resolved.
    pub fn solve_with_progress<F: Fn(SolveProgress) + 'static>(
        &self,
        design_toml: String,
        request: SolveRequest,
        on_progress: F,
    ) -> impl Future<Output = Result<SolveResponse, SolveError>> + use<F> {
        self.submit(design_toml, request, Some(Rc::new(on_progress)))
    }

    fn submit(
        &self,
        design_toml: String,
        request: SolveRequest,
        on_progress: Option<ProgressFn>,
    ) -> impl Future<Output = Result<SolveResponse, SolveError>> + use<> {
        let (sender, receiver) = oneshot::channel();
        let deliveries = {
            let mut inner = self.inner.borrow_mut();
            inner.retired.clear();
            inner.retired_timers.clear();
            let mut deliveries = Deliveries::default();
            let job_id = inner.next_job_id;
            inner.next_job_id += 1;
            if let Some(old) = inner.queued.take() {
                deliveries.push(old.sender, Err(SolveError::Superseded));
            }
            let respawn = inner.running.as_mut().is_some_and(|running| {
                if let Some(old) = running.sender.take() {
                    deliveries.push(old, Err(SolveError::Superseded));
                }
                now_ms() - running.started_ms >= SUPERSEDE_RESPAWN_AFTER_MS
            });
            let respawned = if respawn { inner.respawn() } else { Ok(()) };
            if let Err(error) = respawned {
                deliveries.push(sender, Err(SolveError::Failed(error)));
            } else {
                // Only a search or a sweep can stop early.
                let cancel = request
                    .supports_graceful_cancel()
                    .then(CancelUrl::create)
                    .flatten();
                inner.queued = Some(Queued {
                    job_id,
                    design_toml,
                    request,
                    sender,
                    on_progress,
                    cancel,
                });
                inner.dispatch(&mut deliveries);
            }
            deliveries
        };
        deliveries.deliver();
        reply_of(receiver)
    }

    /// Abandons the running and queued jobs (their futures resolve `Err`) and restarts
    /// the Worker when it was busy or had failed to start.
    ///
    /// # Errors
    ///
    /// When the replacement Worker cannot be started.
    pub fn cancel(&self) -> Result<(), String> {
        let (deliveries, result) = {
            let mut inner = self.inner.borrow_mut();
            let mut deliveries = Deliveries::default();
            if let Some(queued) = inner.queued.take() {
                deliveries.push(queued.sender, Err(SolveError::Cancelled));
            }
            let busy = inner.running.is_some();
            if let Some(sender) = inner.running.as_mut().and_then(|r| r.sender.take()) {
                deliveries.push(sender, Err(SolveError::Cancelled));
            }
            // A failed Worker gets a fresh start too.
            let result = if busy || inner.failure.is_some() {
                inner.crashes_before_ready = 0;
                inner.respawn()
            } else {
                Ok(())
            };
            (deliveries, result)
        };
        deliveries.deliver();
        result
    }

    /// Asks the running search to stop and answer with the best result it has so far
    /// (Optimize's "Cancel"); queued jobs are dropped as in [`Self::cancel`].
    ///
    /// The Worker learns of the request through the job's cancel URL (revoked here),
    /// polls it between tier decisions, and the job's future then resolves normally with
    /// the partial result. If the Worker has not answered within [`GRACEFUL_CANCEL_MS`],
    /// or the running job has no cancel URL, it is terminated as by [`Self::cancel`]
    /// (the future resolves `Err(SolveError::Cancelled)`). The Worker is also sent a
    /// [`ToWorker::Cancel`] for the job, so a Worker whose cancel URL cannot be probed
    /// (a strict content-security policy) skips whatever is queued behind it.
    ///
    /// # Errors
    ///
    /// When the replacement Worker of a hard cancel cannot be started.
    pub fn request_cancel(&self) -> Result<(), String> {
        let graceful = {
            let mut inner = self.inner.borrow_mut();
            let job_id = inner
                .running
                .as_ref()
                .filter(|r| r.cancel.is_some() && !r.cancelling && r.sender.is_some())
                .map(|r| r.job_id);
            if let Some(job_id) = job_id {
                inner.begin_graceful_cancel(job_id);
            }
            job_id.is_some()
        };
        if graceful {
            // Anything queued behind it is not wanted either.
            let queued = self.inner.borrow_mut().queued.take();
            if let Some(queued) = queued {
                let _ = queued.sender.send(Err(SolveError::Cancelled));
            }
            return Ok(());
        }
        self.cancel()
    }

    /// Whether a job is running in the Worker.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.inner.borrow().running.is_some()
    }
}

/// The hang timeout for `request`: the client's own for a plain solve or a metrics job,
/// at least [`ANALYSIS_INACTIVITY_TIMEOUT_MS`] (re-armed by progress) for a search or a
/// tilt sweep, which report as they go.
fn job_timeout_ms(request: &SolveRequest, solve_timeout_ms: u32) -> u32 {
    if request.streams_progress() {
        solve_timeout_ms.max(ANALYSIS_INACTIVITY_TIMEOUT_MS)
    } else {
        solve_timeout_ms
    }
}

/// A warning in the browser console.
fn warn(text: &str) {
    web_sys::console::warn_1(&text.into());
}

async fn reply_of(receiver: oneshot::Receiver<Reply>) -> Reply {
    receiver
        .await
        .unwrap_or_else(|_| Err(SolveError::Failed("the solve worker went away".to_string())))
}

impl Inner {
    /// Starts a fresh Worker (the old one, if any, must already be retired).
    fn spawn(&mut self) -> Result<(), String> {
        self.spawn_id += 1;
        self.ready = false;
        self.failure = None;
        let spawn_id = self.spawn_id;
        let on_message = {
            let weak = self.self_weak.clone();
            move |message: FromWorker| {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let deliveries = inner.borrow_mut().on_message(spawn_id, message);
                deliveries.deliver();
            }
        };
        let on_error = {
            let weak = self.self_weak.clone();
            move |message: String| {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let deliveries = inner.borrow_mut().on_crash(spawn_id, &message);
                deliveries.deliver();
            }
        };
        self.handle = Some(WorkerHandle::spawn(on_message, on_error)?);
        Ok(())
    }

    /// Terminates the Worker (parking its handle and the running job's timer) and
    /// starts a new one. The running job, if any, is dropped; the queued one waits for
    /// the new Worker.
    fn respawn(&mut self) -> Result<(), String> {
        if let Some(handle) = self.handle.take() {
            handle.terminate();
            self.retired.push(handle);
        }
        if let Some(running) = self.running.take() {
            self.retire_timer(running.timer);
        }
        self.spawn()
    }

    fn retire_timer(&mut self, timer: Option<(i32, Closure<dyn FnMut()>)>) {
        if let Some((id, closure)) = timer {
            if let Some(window) = web_sys::window() {
                window.clear_timeout_with_handle(id);
            }
            self.retired_timers.push(closure);
        }
    }

    /// Sends the queued job when the Worker is ready and idle.
    fn dispatch(&mut self, deliveries: &mut Deliveries) {
        if let Some(failure) = &self.failure {
            if let Some(queued) = self.queued.take() {
                deliveries.push(queued.sender, Err(SolveError::Failed(failure.clone())));
            }
            return;
        }
        if !self.ready || self.running.is_some() {
            return;
        }
        let Some(queued) = self.queued.take() else {
            return;
        };
        let Some(handle) = &self.handle else {
            deliveries.push(
                queued.sender,
                Err(SolveError::Failed("no solve worker".to_string())),
            );
            return;
        };
        let timeout_ms = job_timeout_ms(&queued.request, self.timeout_ms);
        // The cancel URL goes first: the Worker keeps it for the job that follows.
        if let Some(cancel) = &queued.cancel {
            let watch = ToWorker::WatchCancel {
                job_id: queued.job_id,
                url: cancel.url().to_owned(),
            };
            if let Err(error) = handle.post(&watch) {
                deliveries.push(queued.sender, Err(SolveError::Failed(error)));
                return;
            }
        }
        let message = ToWorker::Solve {
            job_id: queued.job_id,
            design_toml: queued.design_toml,
            request: queued.request,
        };
        if let Err(error) = handle.post(&message) {
            deliveries.push(queued.sender, Err(SolveError::Failed(error)));
            return;
        }
        let timer = self.start_timer(queued.job_id, timeout_ms);
        self.running = Some(Running {
            job_id: queued.job_id,
            timeout_ms,
            started_ms: now_ms(),
            sender: Some(queued.sender),
            timer,
            on_progress: queued.on_progress,
            cancel: queued.cancel,
            cancelling: false,
        });
    }

    /// Revokes the cancel URL of job `job_id` and shortens its timer to
    /// [`GRACEFUL_CANCEL_MS`]: the Worker either stops and answers by then, or is
    /// terminated ([`Self::on_timeout`]).
    fn begin_graceful_cancel(&mut self, job_id: u64) {
        // The message reaches the Worker when it next reads one (after the running job);
        // until then the revoked URL is the signal. A failed post changes nothing: the
        // timer below still ends the job.
        if let Some(handle) = &self.handle {
            let _ = handle.post(&ToWorker::Cancel { job_id });
        }
        let timer = self.start_timer(job_id, GRACEFUL_CANCEL_MS);
        let old = self.running.as_mut().map(|running| {
            // Dropping the URL revokes it, which is the signal.
            running.cancel = None;
            running.cancelling = true;
            running.timeout_ms = GRACEFUL_CANCEL_MS;
            std::mem::replace(&mut running.timer, timer)
        });
        if let Some(old) = old {
            self.retire_timer(old);
        }
    }

    /// Arms the hang timeout for `job_id`.
    fn start_timer(&self, job_id: u64, timeout_ms: u32) -> Option<(i32, Closure<dyn FnMut()>)> {
        let weak = self.self_weak.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deliveries = inner.borrow_mut().on_timeout(job_id, timeout_ms);
            deliveries.deliver();
        });
        let id = web_sys::window()?
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                i32::try_from(timeout_ms).unwrap_or(i32::MAX),
            )
            .ok()?;
        Some((id, closure))
    }

    fn on_timeout(&mut self, job_id: u64, timeout_ms: u32) -> Deliveries {
        let mut deliveries = Deliveries::default();
        if self.running.as_ref().is_none_or(|r| r.job_id != job_id) {
            return deliveries;
        }
        let cancelling = self.running.as_ref().is_some_and(|r| r.cancelling);
        if let Some(sender) = self.running.as_mut().and_then(|r| r.sender.take()) {
            deliveries.push(
                sender,
                Err(if cancelling {
                    // The search did not stop in time: the hard cancel.
                    SolveError::Cancelled
                } else {
                    SolveError::Failed(format!(
                        "the solve did not finish within {:.0} s; the solve worker was restarted",
                        f64::from(timeout_ms) / 1000.0
                    ))
                }),
            );
        }
        // Called from the timer's own closure: `respawn` parks it, never drops it.
        if let Err(error) = self.respawn()
            && let Some(queued) = self.queued.take()
        {
            deliveries.push(queued.sender, Err(SolveError::Failed(error)));
        }
        deliveries
    }

    fn on_message(&mut self, spawn_id: u64, message: FromWorker) -> Deliveries {
        let mut deliveries = Deliveries::default();
        if spawn_id != self.spawn_id {
            return deliveries;
        }
        match message {
            FromWorker::Loaded { .. } => {
                let init = ToWorker::Init {
                    protocol_version: PROTOCOL_VERSION,
                    role: WorkerRole::Solve,
                    worker_index: 0,
                };
                if let Some(Err(error)) = self.handle.as_ref().map(|h| h.post(&init)) {
                    self.failure = Some(error);
                    self.dispatch(&mut deliveries);
                }
            }
            FromWorker::Ready { .. } => {
                self.ready = true;
                self.crashes_before_ready = 0;
                // Before the first job, so the Worker has decoded the map when it starts it.
                self.send_hdr();
                self.dispatch(&mut deliveries);
            }
            FromWorker::SolveResult {
                job_id, response, ..
            } => {
                if self.running.as_ref().is_some_and(|r| r.job_id == job_id)
                    && let Some(running) = self.running.take()
                {
                    if let Some(sender) = running.sender {
                        deliveries.push(sender, Ok(response));
                    }
                    self.retire_timer(running.timer);
                }
                self.dispatch(&mut deliveries);
            }
            FromWorker::Error { message } => {
                if self.ready {
                    if let Some(sender) = self.running.as_mut().and_then(|r| r.sender.take()) {
                        deliveries.push(sender, Err(SolveError::Failed(message)));
                    }
                } else {
                    self.failure = Some(message);
                    self.dispatch(&mut deliveries);
                }
            }
            FromWorker::Progress {
                job_id,
                message,
                fraction,
            } => {
                // Only the current, un-superseded job's progress is of interest.
                let current = self
                    .running
                    .as_ref()
                    .filter(|r| r.job_id == job_id && r.sender.is_some())
                    .map(|r| (r.on_progress.clone(), r.timeout_ms, r.cancelling));
                if let Some((on_progress, timeout_ms, cancelling)) = current {
                    // A search that reports is alive: re-arm its hang timer -- unless it
                    // is being cancelled, when the short deadline must stand.
                    if !cancelling {
                        let timer = self.start_timer(job_id, timeout_ms);
                        let old = self
                            .running
                            .as_mut()
                            .and_then(|r| std::mem::replace(&mut r.timer, timer));
                        self.retire_timer(old);
                    }
                    if let Some(on_progress) = on_progress {
                        deliveries.push_progress(on_progress, SolveProgress { message, fraction });
                    }
                }
            }
            // A map that could not be decoded leaves the Worker scoring under the preset,
            // which the metrics' own result reports; the console says why.
            FromWorker::HdrError { id, message } => warn(&format!(
                "the analysis worker could not load HDR map {id}: {message}"
            )),
            FromWorker::Picture { .. }
            | FromWorker::PictureFailed { .. }
            | FromWorker::ChunkResult { .. }
            | FromWorker::ChunkDropped { .. }
            | FromWorker::ChunkAborted { .. }
            | FromWorker::SceneError { .. }
            | FromWorker::HdrLoaded { .. } => {}
        }
        deliveries
    }

    /// Posts the held HDR map to the Worker when it is ready; one that is not ready yet is
    /// given it when it becomes so ([`Self::on_message`]).
    fn send_hdr(&self) {
        let (true, Some((id, bytes)), Some(handle)) = (self.ready, &self.hdr, &self.handle) else {
            return;
        };
        let message = ToWorker::HdrMap {
            id: *id,
            bytes: bytes.as_ref().clone(),
        };
        if let Err(error) = handle.post(&message) {
            warn(&format!(
                "could not send an HDR map to the analysis worker: {error}"
            ));
        }
    }

    /// The Worker crashed (a panic or a script error): fail the running job and restart.
    fn on_crash(&mut self, spawn_id: u64, message: &str) -> Deliveries {
        let mut deliveries = Deliveries::default();
        if spawn_id != self.spawn_id {
            return deliveries;
        }
        if let Some(sender) = self.running.as_mut().and_then(|r| r.sender.take()) {
            deliveries.push(
                sender,
                Err(SolveError::Failed(format!(
                    "the solve worker crashed: {message}"
                ))),
            );
        }
        // A Worker that dies before it is ever ready (a missing script, say) would die
        // again on every restart: stop after a few and fail jobs with the reason.
        if !self.ready {
            self.crashes_before_ready += 1;
        }
        if self.crashes_before_ready >= MAX_CRASHES_BEFORE_READY {
            if let Some(handle) = self.handle.take() {
                handle.terminate();
                self.retired.push(handle);
            }
            if let Some(running) = self.running.take() {
                self.retire_timer(running.timer);
            }
            self.failure = Some(format!("the solve worker cannot start: {message}"));
            self.dispatch(&mut deliveries);
            return deliveries;
        }
        // Called from the Worker's own error closure: `respawn` parks the handle.
        if let Err(error) = self.respawn()
            && let Some(queued) = self.queued.take()
        {
            deliveries.push(queued.sender, Err(SolveError::Failed(error)));
        }
        deliveries
    }
}
