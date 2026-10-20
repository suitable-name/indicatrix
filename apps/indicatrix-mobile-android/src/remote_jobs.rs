//! Server jobs on the Remote page: list this phone's jobs on the endpoint, save a finished
//! picture, cancel or delete a job.
//!
//! The endpoint owns the jobs (it stores them durably and answers only the certificate
//! that submitted them), so nothing here is persisted on the phone: the list is always
//! asked for, which is what makes a job survive the phone going offline, the app being
//! killed or the activity being recreated. One request runs at a time on a worker of its
//! own; a result whose request was replaced or interrupted (leaving the page) is dropped.

use crate::{AppWindow, JobRow, RemoteModel, push::toast};
use indicatrix_mobile_core::{
    app_state::AppState,
    platform::{OutgoingFile, SaveOutcome, mime},
    remote::jobs::{self, JobId, JobInfo, JobPhase},
    settings::RemoteSettings,
    tasks::BackgroundQueue,
    viewport::render_view::export_file_name,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Runtime {
    window: slint::Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    queue: BackgroundQueue,
    /// The jobs the rows show, in row order.
    jobs: Vec<JobInfo>,
    next: u64,
    active: Option<(u64, Arc<AtomicBool>)>,
}
thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }

/// One request to the endpoint.
#[derive(Clone, Copy)]
enum Op {
    Refresh,
    Save(JobId),
    Cancel(JobId),
    Delete(JobId),
}

/// What a finished request brings back.
enum Outcome {
    Listed(Vec<JobInfo>, &'static str),
    Saved(JobInfo, Vec<u8>),
}

/// Connects the page's job buttons.
pub fn wire(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    RUNTIME.with(|value| {
        *value.borrow_mut() = Some(Runtime {
            window: window.as_weak(),
            state: Rc::clone(state),
            queue: BackgroundQueue::new("remote-jobs"),
            jobs: Vec::new(),
            next: 0,
            active: None,
        });
    });
    let model = window.global::<RemoteModel>();
    let weak = window.as_weak();
    model.on_refresh_jobs(move || {
        if let Some(window) = weak.upgrade() {
            start(&window, |_| Some(Op::Refresh));
        }
    });
    let weak = window.as_weak();
    model.on_job_save(move |row| {
        if let Some(window) = weak.upgrade() {
            start(&window, |jobs| id_at(jobs, row).map(Op::Save));
        }
    });
    let weak = window.as_weak();
    model.on_job_cancel(move |row| {
        if let Some(window) = weak.upgrade() {
            start(&window, |jobs| id_at(jobs, row).map(Op::Cancel));
        }
    });
    let weak = window.as_weak();
    model.on_job_delete(move |row| {
        if let Some(window) = weak.upgrade() {
            start(&window, |jobs| id_at(jobs, row).map(Op::Delete));
        }
    });
}

fn id_at(jobs: &[JobInfo], row: i32) -> Option<JobId> {
    usize::try_from(row)
        .ok()
        .and_then(|row| jobs.get(row))
        .map(|info| info.job_id)
}

/// Runs the request `choose` picks from the current rows, unless one is running already.
fn start(window: &AppWindow, choose: impl FnOnce(&[JobInfo]) -> Option<Op>) {
    let model = window.global::<RemoteModel>();
    // Forget Server is erasing the credentials: no request may load them now (the page
    // disables its buttons too).
    if model.get_forgetting() {
        return;
    }
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if runtime.active.is_some() {
            return;
        }
        let Some(op) = choose(&runtime.jobs) else {
            return;
        };
        runtime.next = runtime.next.wrapping_add(1);
        let id = runtime.next;
        let cancelled = Arc::new(AtomicBool::new(false));
        runtime.active = Some((id, Arc::clone(&cancelled)));
        let (platform, settings) = {
            let app = runtime.state.borrow();
            (Arc::clone(app.platform()), app.settings.remote.clone())
        };
        model.set_jobs_busy(true);
        model.set_jobs_status("Contacting the endpoint…".into());
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let flag = || cancelled.load(Ordering::Acquire);
            let result = run(platform.as_ref(), &settings, op, &flag);
            let _queued = weak.upgrade_in_event_loop(move |window| finish(&window, id, result));
        });
    });
}

fn run(
    platform: &dyn indicatrix_mobile_core::platform::Platform,
    settings: &RemoteSettings,
    op: Op,
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome, String> {
    let listed = |note| {
        jobs::list(platform, settings, cancelled)
            .map(|list| Outcome::Listed(list, note))
            .map_err(String::from)
    };
    match op {
        Op::Refresh => listed(""),
        Op::Save(job_id) => jobs::fetch(platform, settings, cancelled, job_id)
            .map(|(info, png)| Outcome::Saved(info, png))
            .map_err(String::from),
        Op::Cancel(job_id) => {
            jobs::cancel(platform, settings, cancelled, job_id).map_err(String::from)?;
            listed(" Cancel requested.")
        }
        Op::Delete(job_id) => {
            jobs::delete(platform, settings, cancelled, job_id).map_err(String::from)?;
            listed(" Job deleted.")
        }
    }
}

fn rows(list: &[JobInfo]) -> ModelRc<JobRow> {
    ModelRc::new(VecModel::from(
        list.iter()
            .map(|info| JobRow {
                title: jobs::describe(info).into(),
                note: info.note.as_str().into(),
                can_save: info.phase == JobPhase::Done,
                can_cancel: !info.phase.is_terminal(),
            })
            .collect::<Vec<_>>(),
    ))
}

/// A request ended (on the UI thread).
fn finish(window: &AppWindow, id: u64, result: Result<Outcome, String>) {
    let model = window.global::<RemoteModel>();
    let save = RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let runtime = guard.as_mut()?;
        if !runtime
            .active
            .as_ref()
            .is_some_and(|(run, cancel)| *run == id && !cancel.load(Ordering::Acquire))
        {
            return None;
        }
        runtime.active = None;
        model.set_jobs_busy(false);
        match result {
            Ok(Outcome::Listed(list, note)) => {
                model.set_jobs(rows(&list));
                let count = list.len();
                model.set_jobs_status(
                    format!(
                        "{count} server {} on the endpoint.{note}",
                        if count == 1 { "job" } else { "jobs" }
                    )
                    .into(),
                );
                runtime.jobs = list;
                None
            }
            Ok(Outcome::Saved(info, png)) => {
                model.set_jobs_status(format!("Picture ready: {}.", jobs::describe(&info)).into());
                let file = OutgoingFile {
                    suggested_name: export_file_name(&info.label),
                    mime: mime::PNG.to_owned(),
                    bytes: png,
                };
                Some((Arc::clone(runtime.state.borrow().platform()), file))
            }
            Err(message) => {
                model.set_jobs_status(message.into());
                None
            }
        }
    });
    if let Some((platform, file)) = save {
        let weak = window.as_weak();
        platform.create_document(
            file,
            Box::new(move |result| {
                let _queued = weak.upgrade_in_event_loop(move |window| match result {
                    Ok(SaveOutcome::Saved { display_name }) => {
                        toast::show(&window, &format!("Saved {display_name}"));
                    }
                    Ok(SaveOutcome::Cancelled) => {}
                    Err(error) => toast::show(&window, &error.message),
                });
            }),
        );
    }
}

/// Stops a running request (leaving the page, shutting down); its result is dropped.
pub fn interrupt() {
    let window = RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let runtime = guard.as_mut()?;
        let (_, cancel) = runtime.active.take()?;
        cancel.store(true, Ordering::Release);
        Some(runtime.window.clone())
    });
    if let Some(window) = window.and_then(|weak| weak.upgrade()) {
        let model = window.global::<RemoteModel>();
        model.set_jobs_busy(false);
        model.set_jobs_status("Request interrupted. Refresh to try again.".into());
    }
}

/// Stops a running request and drops the rows' cache (Forget Server): the endpoint's jobs
/// are no longer this phone's to show. The caller empties the page's rows.
pub fn clear() {
    interrupt();
    RUNTIME.with(|value| {
        if let Some(runtime) = value.borrow_mut().as_mut() {
            runtime.jobs.clear();
        }
    });
}

/// Ends the module with the app.
pub fn shutdown() {
    interrupt();
    RUNTIME.with(|value| drop(value.borrow_mut().take()));
}
