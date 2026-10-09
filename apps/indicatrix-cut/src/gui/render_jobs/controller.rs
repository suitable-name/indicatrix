//! The render queue's controller: starts jobs one at a time, follows them, and applies the
//! Jobs window's commands.
//!
//! One job renders at a time, on its own thread, through the shared executor
//! ([`super::execute::execute_job_catching`]). Everything else happens on the UI thread: the
//! controller lives in a thread-local ([`with_controller`], the pattern of
//! `gui::rough_plan::host`), and the render thread reaches it only through
//! `Weak::upgrade_in_event_loop`.
//!
//! **Re-entrancy rule.** A `RefCell` borrow is never held across anything that can re-enter:
//! a Slint property write, a toast, a database call or a picker. Every method copies what it
//! needs out of `state`, `live` and `metas`, drops the borrow, and only then touches the
//! outside. The pure decisions ([`after_finish`], [`next_job`]) are free functions so they
//! are tested without a window.

use super::{
    execute::{ExecContext, execute_job_catching},
    rows::{LiveProgress, queue_view},
    wiring,
};
use crate::{
    ActivityModel, MainWindow, RenderJobsModel,
    bridge::render_thread::RenderContext,
    gui::{external_links::open_local_path, show_toast},
    settings::SettingsPersister,
};
use indicatrix_render_jobs::{
    ComputeChoice, FailureKind, JobOutcome, JobProgress, JobSink, RenderJobFile, codec,
    order::{QueueEntry, moved, moved_to_front, next_to_run},
    state::{
        APP_CLOSING_NOTE, CommandEffect, JobCommand, JobState, StopReason, command_effect,
        queue_continues_after, state_after_run,
    },
};
use indicatrix_vault::{db::sqlite::Database, model::render_job::RenderJobMeta};
use slint::{ComponentHandle, Weak};
use std::{
    cell::RefCell,
    fmt::Display,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// How often a running job's progress reaches the window, at most.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// The close guard's sentence while a job renders.
const WORK_AT_RISK: &str = "A render job is running. It stops now and waits in the queue as Paused; a tilt video keeps its finished frames.";

/// Whole seconds since the Unix epoch.
pub(super) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// What a finished run leaves behind, and whether the queue goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishPlan {
    /// The state to write.
    pub new_state: JobState,
    /// Frames finished (all of them after a finished run).
    pub frames_done: u32,
    /// What was written, for a finished run.
    pub result_path: Option<String>,
    /// The failure sentence, or the note a closing app leaves.
    pub error_text: Option<String>,
    /// Whether the queue tries to start the next job.
    pub start_next: bool,
    /// Whether the queue is still running afterwards (before the next job is looked for: a
    /// queue with nothing left stops itself).
    pub queue_running: bool,
}

/// What to write and do after a run ended with `outcome`, stopped for `stop` (the reason
/// the person asked for it, if any), on a queue that was `queue_running`.
pub fn after_finish(
    outcome: &JobOutcome,
    stop: Option<StopReason>,
    queue_running: bool,
    frames_total: u32,
) -> FinishPlan {
    let new_state = state_after_run(outcome.end(), stop);
    let error_text = match outcome {
        JobOutcome::Failed { message, .. } => Some(message.clone()),
        _ if new_state == JobState::Paused && stop == Some(StopReason::AppClosing) => {
            Some(APP_CLOSING_NOTE.to_string())
        }
        _ => None,
    };
    let result_path = match outcome {
        JobOutcome::Done { result, .. } => Some(result.to_string_lossy().into_owned()),
        _ => None,
    };
    let keeps_going = queue_running && queue_continues_after(stop);
    FinishPlan {
        new_state,
        frames_done: outcome.frames_done(frames_total),
        result_path,
        error_text,
        start_next: keeps_going,
        queue_running: keeps_going,
    }
}

/// The job a queue run takes next: the first waiting one in list order. Rows with a state
/// word this version does not know are skipped.
pub fn next_job(metas: &[RenderJobMeta]) -> Option<i64> {
    let entries: Vec<QueueEntry> = metas
        .iter()
        .filter_map(|meta| {
            JobState::parse(&meta.state).map(|state| QueueEntry {
                id: meta.job_id,
                state,
            })
        })
        .collect();
    next_to_run(&entries)
}

/// The job that renders now.
struct Running {
    job_id: i64,
    /// Set to stop the render at the next sample batch (still) or frame (video).
    cancel: Arc<AtomicBool>,
    /// Why it is being stopped, once the person asked.
    stop: Option<StopReason>,
    /// Its chip in the status strip.
    activity_id: i32,
    frames_total: u32,
    /// Whether this run holds a live-viewport pause (`export_active_count`): false for a
    /// Remote-only job, which traces nothing on this computer.
    pausing: bool,
}

struct ControllerState {
    queue_running: bool,
    current: Option<Running>,
    /// The last progress note shown as a toast, so a repeated note is shown once.
    last_note: Option<String>,
}

/// Owns the queue: what runs, and the rows the window shows.
pub struct JobController {
    pub(super) ui: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    settings_store: Arc<SettingsPersister>,
    state: RefCell<ControllerState>,
    live: RefCell<Option<LiveProgress>>,
    /// The database rows as last read, so a progress tick rebuilds the window without a query.
    metas: RefCell<Vec<RenderJobMeta>>,
}

thread_local! {
    /// The one controller of the app session (UI thread only).
    static CONTROLLER: RefCell<Option<Rc<JobController>>> = const { RefCell::new(None) };
}

/// Makes `controller` the session's controller.
pub(super) fn install(controller: Rc<JobController>) {
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(controller));
}

/// Runs `f` with the controller, or returns `None` when there is none (not set up yet, or
/// called off the UI thread). The controller is cloned out first, so `f` may call
/// `with_controller` again.
pub fn with_controller<R>(f: impl FnOnce(&JobController) -> R) -> Option<R> {
    let controller = CONTROLLER.with(|cell| cell.borrow().clone())?;
    Some(f(&controller))
}

/// The close guard's sentence while a job renders, else `None`.
pub fn work_at_risk() -> Option<String> {
    with_controller(JobController::is_running)
        .filter(|running| *running)
        .map(|_| WORK_AT_RISK.to_string())
}

/// Stops the running job because the app is closing. It is written as Paused at once, as
/// the event loop will not deliver the run's own ending.
pub fn stop_for_app_close() {
    let _ = with_controller(JobController::close_running);
}

impl JobController {
    pub(super) const fn new(
        ui: Weak<MainWindow>,
        db: Arc<Mutex<Database>>,
        render_ctx: Arc<Mutex<RenderContext>>,
        settings_store: Arc<SettingsPersister>,
    ) -> Self {
        Self {
            ui,
            db,
            render_ctx,
            settings_store,
            state: RefCell::new(ControllerState {
                queue_running: false,
                current: None,
                last_note: None,
            }),
            live: RefCell::new(None),
            metas: RefCell::new(Vec::new()),
        }
    }

    fn db(&self) -> MutexGuard<'_, Database> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_running(&self) -> bool {
        self.state.borrow().current.is_some()
    }

    /// The jobs from the database, or the last ones read when the database does not answer.
    fn load_metas(&self) -> Vec<RenderJobMeta> {
        let read = self.db().list_render_jobs();
        match read {
            Ok(metas) => metas,
            Err(error) => {
                tracing::warn!("The render job list could not be read: {error}");
                self.metas.borrow().clone()
            }
        }
    }

    fn toast(&self, message: &str, kind: &str) {
        if let Some(ui) = self.ui.upgrade() {
            show_toast(&ui, message, kind);
        }
    }

    /// Tells the person about a failed database write, and says whether it worked.
    fn checked<T, E: Display>(&self, what: &str, result: Result<T, E>) -> bool {
        match result {
            Ok(_) => true,
            Err(error) => {
                self.toast(&format!("{what}: {error}"), "error");
                false
            }
        }
    }

    // ---- What the window shows -------------------------------------------------------

    /// Reads the jobs again and shows them.
    pub fn refresh(&self) {
        let metas = self.load_metas();
        let unfinished = metas
            .iter()
            .filter(|meta| JobState::parse(&meta.state).is_none_or(|state| !state.is_terminal()))
            .count();
        *self.metas.borrow_mut() = metas;
        self.apply_view();
        if let Some(ui) = self.ui.upgrade() {
            let worker = self.settings_store.snapshot().settings.remote_worker();
            wiring::publish_script_facts(&ui, unfinished, worker.as_ref());
        }
    }

    /// Shows the rows, the queue line and the chip from what is cached.
    fn apply_view(&self) {
        let (view, job_running) = {
            let metas = self.metas.borrow();
            let live = self.live.borrow();
            let state = self.state.borrow();
            (
                queue_view(&metas, state.queue_running, live.as_ref()),
                state.current.is_some(),
            )
        };
        if let Some(ui) = self.ui.upgrade() {
            wiring::publish(&ui, &view);
            // The queue's input to `ExportRunModel.running`: one export at a time.
            ui.global::<RenderJobsModel>().set_job_running(job_running);
        }
    }

    // ---- Starting and following a job ------------------------------------------------

    /// Starts the first waiting job. Returns whether one is now running. A job that cannot
    /// be started is marked Failed and the next one is tried.
    fn start_next(&self) -> bool {
        let attempts = self.load_metas().len() + 1;
        for _ in 0..attempts {
            let metas = self.load_metas();
            let Some(id) = next_job(&metas) else {
                return false;
            };
            if self.start_job(id) {
                return true;
            }
        }
        false
    }

    fn fail_before_start(&self, id: i64, label: &str, message: &str) {
        let result = self
            .db()
            .set_render_job_state(id, "failed", Some(message), unix_now());
        self.checked("The render job list could not be updated", result);
        let name = if label.is_empty() {
            "A render job".to_string()
        } else {
            format!("Render job \"{label}\"")
        };
        self.toast(&format!("{name} failed: {message}"), "error");
    }

    /// Starts job `id`. Returns whether a render is now running.
    fn start_job(&self, id: i64) -> bool {
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        let Some((meta, job)) = self.read_job_for_start(id) else {
            return false;
        };
        let label = meta.label.clone();
        let marked = self
            .db()
            .set_render_job_state(id, "running", None, unix_now());
        if !self.checked("The render job could not be started", marked) {
            return false;
        }

        // Pauses the live viewport like every export, except a Remote-only job (nothing is
        // traced here, so the person keeps working); `on_finished` is the one decrement.
        let pausing = job.compute.target != ComputeChoice::Remote;
        let local_compute = {
            let mut guard = RenderContext::lock(&self.render_ctx);
            if pausing {
                guard.export_active_count += 1;
            }
            guard.local_compute_target
        };
        // Not cancellable from the status strip: that chip's cancel has a single handler,
        // owned by the tilt video export. Pause and Cancel are in the Jobs window.
        let activity_id = ui.global::<ActivityModel>().invoke_start_external(
            "render_job".into(),
            label.as_str().into(),
            false,
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx = ExecContext {
            base_dir: Path::new(&job.output)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            local_compute,
            // Machine configuration, read now and not frozen with the job.
            worker: self.settings_store.snapshot().settings.remote_worker(),
            compute_override: None,
            transfer_override: None,
            contribute_local_override: None,
            output_override: None,
            // A job with no finished frame starts clean (its own earlier frames, if any,
            // are cleared); a paused one goes on where it stopped.
            restart_frames: meta.frames_done == 0,
        };
        {
            let mut state = self.state.borrow_mut();
            state.current = Some(Running {
                job_id: id,
                cancel: Arc::clone(&cancel),
                stop: None,
                activity_id,
                frames_total: meta.frames_total,
                pausing,
            });
            state.last_note = None;
        }
        *self.live.borrow_mut() = None;

        self.spawn_render_thread(id, job, ctx, cancel);
        self.refresh();
        true
    }

    /// Reads job `id` and its frozen scene for a start. A job that cannot be read, decoded or
    /// completed is failed (with a toast) and `None` comes back; a deleted one is just `None`.
    fn read_job_for_start(&self, id: i64) -> Option<(RenderJobMeta, RenderJobFile)> {
        let read = self.db().get_render_job(id);
        let row = match read {
            Ok(Some(row)) => row,
            // Deleted since the list was read.
            Ok(None) => return None,
            Err(error) => {
                self.fail_before_start(id, "", &format!("The job could not be read: {error}"));
                return None;
            }
        };
        let label = row.meta.label.clone();
        let job = match codec::from_text(&row.snapshot) {
            Ok(job) => job,
            Err(error) => {
                self.fail_before_start(id, &label, &error.to_string());
                return None;
            }
        };
        // Zoning builds: the frozen scene carries no colour zones; they were stored beside the
        // job when it was queued and are put back here. Unreadable zones fail the job instead of
        // rendering the stone in its base colour.
        #[cfg(feature = "zoning")]
        let job = {
            let mut job = job;
            let attached =
                crate::gui::rough_colour::store::attach_job_zoning(&self.db(), id, &mut job.scene);
            if let Err(error) = attached {
                self.fail_before_start(id, &label, &format!("{error:#}"));
                return None;
            }
            job
        };
        Some((row.meta, job))
    }

    /// Runs the job on its own thread; a thread that cannot be spawned finishes the job as
    /// failed.
    fn spawn_render_thread(
        &self,
        id: i64,
        job: RenderJobFile,
        ctx: ExecContext,
        cancel: Arc<AtomicBool>,
    ) {
        let ui_weak = self.ui.clone();
        let spawned = std::thread::Builder::new()
            .name("render-job".to_string())
            .spawn(move || {
                let mut sink = UiSink {
                    ui: ui_weak.clone(),
                    job_id: id,
                    last_progress: None,
                };
                let outcome = execute_job_catching(&job, &ctx, &cancel, &mut sink);
                // Always delivered: `execute_job_catching` turns a panic into an outcome.
                let _ = ui_weak.upgrade_in_event_loop(move |_| {
                    with_controller(|controller| controller.on_finished(id, &outcome));
                });
            });
        if let Err(error) = spawned {
            self.on_finished(
                id,
                &JobOutcome::Failed {
                    kind: FailureKind::Render,
                    message: format!("The render could not be started: {error}"),
                    frames_done: 0,
                },
            );
        }
    }

    /// A progress tick of job `job_id` (already throttled by the sink).
    fn on_progress(&self, job_id: i64, progress: JobProgress) {
        let activity_id = {
            let state = self.state.borrow();
            match &state.current {
                Some(run) if run.job_id == job_id => run.activity_id,
                _ => return,
            }
        };
        let fraction = progress.fraction;
        let note = progress.note.clone();
        *self.live.borrow_mut() = Some(LiveProgress { job_id, progress });
        self.apply_view();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.global::<ActivityModel>()
            .invoke_progress_external(activity_id, fraction);
        if let Some(note) = note {
            let new_note = {
                let mut state = self.state.borrow_mut();
                let new_note = state.last_note.as_deref() != Some(note.as_str());
                state.last_note = Some(note.clone());
                new_note
            };
            if new_note {
                show_toast(&ui, &note, "info");
            }
        }
    }

    /// A video frame of job `job_id` is on disk: keep the count, so a crash resumes there.
    fn on_frame(&self, job_id: i64, frames_done: u32) {
        let result = self
            .db()
            .set_render_job_progress(job_id, frames_done, unix_now());
        if let Err(error) = result {
            tracing::warn!("The progress of render job {job_id} could not be stored: {error}");
        }
        if let Some(meta) = self
            .metas
            .borrow_mut()
            .iter_mut()
            .find(|meta| meta.job_id == job_id)
        {
            meta.frames_done = frames_done;
        }
    }

    /// A run ended. The single place that releases the live viewport and the status chip.
    fn on_finished(&self, job_id: i64, outcome: &JobOutcome) {
        let (running, queue_running) = {
            let mut state = self.state.borrow_mut();
            let ours = state
                .current
                .as_ref()
                .is_some_and(|run| run.job_id == job_id);
            let running = if ours { state.current.take() } else { None };
            (running, state.queue_running)
        };
        if running.as_ref().is_none_or(|run| run.pausing) {
            let mut guard = RenderContext::lock(&self.render_ctx);
            guard.export_active_count = guard.export_active_count.saturating_sub(1);
        }
        let label = self
            .metas
            .borrow()
            .iter()
            .find(|meta| meta.job_id == job_id)
            .map(|meta| meta.label.clone())
            .unwrap_or_default();
        let (stop, frames_total) = running
            .as_ref()
            .map_or((None, 1), |run| (run.stop, run.frames_total));
        if let (Some(run), Some(ui)) = (&running, self.ui.upgrade()) {
            ui.global::<ActivityModel>()
                .invoke_finish_external(run.activity_id);
        }

        let plan = after_finish(outcome, stop, queue_running, frames_total);
        let now = unix_now();
        let wrote = {
            let db = self.db();
            db.set_render_job_state(
                job_id,
                plan.new_state.as_str(),
                plan.error_text.as_deref(),
                now,
            )
            .and_then(|_| db.set_render_job_progress(job_id, plan.frames_done, now))
            .and_then(|_| db.set_render_job_result(job_id, plan.result_path.as_deref(), now))
        };
        self.checked("The render job could not be updated", wrote);
        *self.live.borrow_mut() = None;
        self.state.borrow_mut().queue_running = plan.queue_running;

        self.toast_outcome(&label, outcome);

        if plan.start_next && !self.start_next() {
            self.state.borrow_mut().queue_running = false;
            self.toast("All render jobs are done.", "success");
        }
        self.refresh();
    }

    fn toast_outcome(&self, label: &str, outcome: &JobOutcome) {
        match outcome {
            JobOutcome::Done { result, note } => {
                let shown = result.display();
                match note {
                    Some(note) => {
                        self.toast(&format!("Render job finished: {shown}. {note}"), "info");
                    }
                    None => self.toast(&format!("Render job finished: {shown}"), "success"),
                }
            }
            JobOutcome::Failed { message, .. } => {
                self.toast(
                    &format!("Render job \"{label}\" failed: {message}"),
                    "error",
                );
            }
            JobOutcome::Stopped { .. } => {}
        }
    }

    // ---- The window's commands -------------------------------------------------------

    /// A row button: applies what the state machine says the command does.
    pub fn command(&self, id: i64, command: JobCommand) {
        let metas = self.load_metas();
        let Some(meta) = metas.iter().find(|meta| meta.job_id == id) else {
            self.refresh();
            return;
        };
        let Some(state) = JobState::parse(&meta.state) else {
            // A state this version does not know: only Delete is offered.
            if command == JobCommand::Delete {
                let result = self.db().delete_render_job(id);
                self.checked("The render job could not be deleted", result);
            }
            self.refresh();
            return;
        };
        let now = unix_now();
        match command_effect(state, command) {
            CommandEffect::SetState {
                state: new_state,
                reset_progress,
            } => {
                let result = if reset_progress {
                    self.db().reset_render_job(id, now)
                } else {
                    self.db()
                        .set_render_job_state(id, new_state.as_str(), None, now)
                };
                self.checked("The render job could not be updated", result);
            }
            CommandEffect::StopRunning(reason) => self.stop_running(id, reason),
            CommandEffect::Delete => {
                let result = self.db().delete_render_job(id);
                self.checked("The render job could not be deleted", result);
            }
            CommandEffect::MoveToFront { requeue } => {
                let ids: Vec<i64> = metas.iter().map(|meta| meta.job_id).collect();
                let result = self.db().reorder_render_jobs(&moved_to_front(&ids, id));
                if self.checked("The render jobs could not be reordered", result) && requeue {
                    let result = self.db().set_render_job_state(id, "queued", None, now);
                    self.checked("The render job could not be updated", result);
                }
            }
            CommandEffect::Move(by) => {
                let ids: Vec<i64> = metas.iter().map(|meta| meta.job_id).collect();
                let result = self.db().reorder_render_jobs(&moved(&ids, id, by));
                self.checked("The render jobs could not be reordered", result);
            }
            CommandEffect::Refused(message) => self.toast(message, "info"),
        }
        self.refresh();
    }

    /// Asks the running job `id` to stop for `reason`. A row the database calls running but
    /// that nothing renders (a stale row) is written as stopped directly.
    fn stop_running(&self, id: i64, reason: StopReason) {
        let signalled = {
            let mut state = self.state.borrow_mut();
            match state.current.as_mut() {
                Some(run) if run.job_id == id => {
                    run.stop = Some(reason);
                    run.cancel.store(true, Ordering::Relaxed);
                    true
                }
                _ => false,
            }
        };
        if !signalled {
            let word = if reason == StopReason::Cancel {
                "cancelled"
            } else {
                "paused"
            };
            let result = self.db().set_render_job_state(id, word, None, unix_now());
            self.checked("The render job could not be updated", result);
        }
    }

    /// Start Queue: render the waiting jobs from the top down.
    pub fn start_queue(&self) {
        let waiting = self
            .load_metas()
            .iter()
            .any(|meta| JobState::parse(&meta.state) == Some(JobState::Queued));
        if !waiting {
            self.toast("There are no waiting jobs.", "info");
            self.refresh();
            return;
        }
        // One export at a time: a still export or a video from a dialog holds the machine.
        let direct_export = self
            .ui
            .upgrade()
            .is_some_and(|ui| crate::gui::export_run::ExportActivity::read(&ui).direct());
        if direct_export {
            self.toast(crate::gui::export_run::BUSY_MESSAGE, "info");
            self.refresh();
            return;
        }
        let stopping = {
            let state = self.state.borrow();
            state.current.as_ref().is_some_and(|run| run.stop.is_some())
        };
        if stopping {
            self.toast(
                "The running job is still stopping. Try again in a moment.",
                "info",
            );
            return;
        }
        let rendering = {
            let mut state = self.state.borrow_mut();
            state.queue_running = true;
            state.current.is_some()
        };
        if !rendering && !self.start_next() {
            self.state.borrow_mut().queue_running = false;
        }
        self.refresh();
    }

    /// Pause Queue: stop the running job (it waits as Paused) and start nothing more.
    pub fn pause_queue(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.queue_running = false;
            if let Some(run) = state.current.as_mut() {
                run.stop = Some(StopReason::PauseQueue);
                run.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.refresh();
    }

    /// Clear Finished: remove the Done and Cancelled jobs.
    pub fn clear_finished(&self) {
        let result = self.db().delete_finished_render_jobs();
        self.checked("The finished jobs could not be removed", result);
        self.refresh();
    }

    /// Show: open the folder of what job `id` wrote (a video folder opens itself).
    pub fn show_result(&self, id: i64) {
        let result_path = self
            .load_metas()
            .into_iter()
            .find(|meta| meta.job_id == id)
            .and_then(|meta| meta.result_path);
        let Some(result_path) = result_path else {
            self.toast("This job has no finished file.", "info");
            return;
        };
        let path = PathBuf::from(result_path);
        let target = if path.is_dir() {
            path
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(path)
        };
        if let Err(message) = open_local_path(&target) {
            self.toast(&message, "error");
        }
    }

    /// The app is closing: see [`stop_for_app_close`].
    fn close_running(&self) {
        let job_id = {
            let mut state = self.state.borrow_mut();
            state.queue_running = false;
            state.current.as_mut().map(|run| {
                run.stop = Some(StopReason::AppClosing);
                run.cancel.store(true, Ordering::Relaxed);
                run.job_id
            })
        };
        if let Some(id) = job_id {
            let _ =
                self.db()
                    .set_render_job_state(id, "paused", Some(APP_CLOSING_NOTE), unix_now());
        }
    }
}

/// Forwards a running job's progress to the UI thread: progress at most four times a
/// second, every finished frame.
struct UiSink {
    ui: Weak<MainWindow>,
    job_id: i64,
    last_progress: Option<Instant>,
}

impl JobSink for UiSink {
    fn progress(&mut self, progress: &JobProgress) {
        let now = Instant::now();
        // The last tick and a note are never dropped.
        let urgent = progress.fraction >= 1.0 || progress.note.is_some();
        let too_soon = self
            .last_progress
            .is_some_and(|at| now.duration_since(at) < PROGRESS_INTERVAL);
        if too_soon && !urgent {
            return;
        }
        self.last_progress = Some(now);
        let job_id = self.job_id;
        let progress = progress.clone();
        let _ = self.ui.upgrade_in_event_loop(move |_| {
            with_controller(|controller| controller.on_progress(job_id, progress));
        });
    }

    fn frame_finished(&mut self, frames_done: u32, _frames_total: u32) {
        let job_id = self.job_id;
        let _ = self.ui.upgrade_in_event_loop(move |_| {
            with_controller(|controller| controller.on_frame(job_id, frames_done));
        });
    }
}
