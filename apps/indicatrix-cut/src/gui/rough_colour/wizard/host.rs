//! The wizard window: created on the first open, hidden (never destroyed) when closed so its
//! photos and results survive, like the locate window.
//!
//! One thread-local holds it; callbacks reach
//! it through [`on_host`]. No `RefCell` borrow is held across a file dialog or a job: handlers read
//! what they need, drop the borrow, then act.

use super::{
    actions,
    context::PlannerLink,
    state::{Job, State},
    view_model,
};
use crate::RoughColourWindow;
use slint::{CloseRequestResponse, ComponentHandle};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tracing::warn;

/// The next job ticket, process wide.
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

/// The window and its state.
pub struct Host {
    /// The window.
    pub window: RoughColourWindow,
    /// The planner and the locate window, as closures.
    pub link: PlannerLink,
    /// What the wizard remembers.
    pub state: RefCell<State>,
}

thread_local! {
    /// The one wizard window of the app session, once it was opened.
    static WIZARD: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

/// Runs `f` with the host, or returns `None` when the window does not exist.
pub fn with_host<R>(f: impl FnOnce(&Rc<Host>) -> R) -> Option<R> {
    let host = WIZARD.with(|cell| cell.borrow().clone())?;
    Some(f(&host))
}

/// The wizard window when it exists. UI harness only.
#[cfg(feature = "ui-harness")]
#[must_use]
pub fn harness_window() -> Option<RoughColourWindow> {
    with_host(|host| host.window.clone_strong())
}

/// [`with_host`] for callbacks that return nothing.
pub fn on_host(f: impl FnOnce(&Rc<Host>)) {
    let _ = with_host(f);
}

/// Hides the window for good (the planner is closing). A running job is asked to stop first, so
/// a fit does not keep every core busy for a window nobody can see.
pub fn close() {
    if let Some(host) = WIZARD.with(|cell| cell.borrow_mut().take()) {
        cancel_job(&host);
        let _ = host.window.hide();
    }
}

/// The Rough Planner saved its plan: an open wizard takes the plan id and redraws (Accept
/// becomes available). Does nothing when the wizard was never opened.
pub fn plan_saved() {
    on_host(actions::plan_saved);
}

/// What closing the Rough Planner would lose in the wizard.
///
/// A sentence when a job is running or a fit has not been accepted (stored with the saved plan),
/// `None` otherwise or when the wizard was never opened.
#[must_use]
pub fn work_at_risk() -> Option<String> {
    let host = WIZARD.with(|cell| cell.borrow().clone())?;
    // A handler that is writing the state right now cannot be read; it counts as holding nothing
    // (the same rule as the planner's own `planner_work_at_risk`).
    let state = host.state.try_borrow().ok()?;
    if state.job.is_some() {
        Some("The rough color wizard is still working on a step.".to_owned())
    } else if state.fit.is_some() && !state.fit_accepted {
        Some("The rough color wizard has a fit that was not accepted.".to_owned())
    } else {
        None
    }
}

/// Opens the wizard: creates it on the first call, otherwise shows the existing one.
pub fn open(link: &PlannerLink) {
    let Some(host) = with_host(Rc::clone).or_else(|| create(link)) else {
        return;
    };
    actions::refresh_from_planner(&host);
    if let Err(error) = host.window.show() {
        warn!("Could not show the rough colour window: {error}");
        return;
    }
    host.window.window().set_minimized(false);
}

fn create(link: &PlannerLink) -> Option<Rc<Host>> {
    let window = match RoughColourWindow::new() {
        Ok(window) => window,
        Err(error) => {
            warn!("Could not create the rough colour window: {error}");
            return None;
        }
    };
    crate::gui::preferences::bind_rough_colour_window_theme(&window);
    window
        .window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);
    window.set_show_host_picker(crate::gui::zoning_ui::host_picker_visible());
    let host = Rc::new(Host {
        window,
        link: link.clone(),
        state: RefCell::new(State::new()),
    });
    actions::wire(&host);
    WIZARD.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&host)));
    Some(host)
}

// --- Messages and jobs -----------------------------------------------------------------------

/// Shows a status line (clears the error).
pub fn set_status(host: &Host, text: &str) {
    host.window.set_error_text("".into());
    host.window.set_status_text(text.into());
}

/// Shows an error line.
pub fn set_error(host: &Host, text: &str) {
    host.window.set_error_text(text.into());
}

/// Called by a running job to show its progress.
pub fn set_progress(host: &Host, text: &str, fraction: f32) {
    host.window.set_busy_text(text.into());
    host.window.set_progress(fraction);
}

/// What a job's work closure gets to report progress with: a line and a fraction.
pub type ProgressSink = Arc<dyn Fn(String, f32) + Send + Sync>;

/// Starts `work` on a thread of its own and runs `done` with its result on the UI thread, unless
/// another job started meanwhile. A panic in `work` reaches `done` as an error.
pub fn spawn_job<T: Send + 'static>(
    host: &Rc<Host>,
    busy_text: &str,
    work: impl FnOnce(Arc<AtomicBool>, ProgressSink) -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&Rc<Host>, Result<T, String>) + Send + 'static,
) {
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    // A job that is still running is superseded: stop it, its result would be dropped anyway.
    if let Some(previous) = host.state.borrow_mut().job.replace(Job {
        ticket,
        cancel: Arc::clone(&cancel),
    }) {
        previous.cancel.store(true, Ordering::Relaxed);
    }
    host.window.set_busy(true);
    host.window.set_busy_text(busy_text.into());
    host.window.set_progress(0.0);
    view_model::push_steps(host);
    let weak = host.window.as_weak();
    let sink_weak = weak.clone();
    let sink: ProgressSink = Arc::new(move |text, fraction| {
        let _ = sink_weak.upgrade_in_event_loop(move |_| {
            on_host(|host| {
                if host.state.borrow().job.is_some() {
                    set_progress(host, &text, fraction);
                }
            });
        });
    });
    let spawned = std::thread::Builder::new()
        .name("rough-colour-job".to_string())
        .spawn(move || {
            let value = catch_unwind(AssertUnwindSafe(|| work(cancel, sink)))
                .unwrap_or_else(|_| Err("The step stopped unexpectedly.".to_owned()));
            let _ = weak.upgrade_in_event_loop(move |_| {
                on_host(|host| deliver(host, ticket, value, done));
            });
        });
    if let Err(error) = spawned {
        warn!("Rough colour: could not start the worker thread: {error}");
        end_job(host);
        set_error(host, &format!("Could not start a background task: {error}"));
    }
}

fn end_job(host: &Rc<Host>) {
    host.state.borrow_mut().job = None;
    host.window.set_busy(false);
    host.window.set_busy_text("".into());
    host.window.set_progress(0.0);
}

fn deliver<T>(
    host: &Rc<Host>,
    ticket: u64,
    value: Result<T, String>,
    done: impl FnOnce(&Rc<Host>, Result<T, String>),
) {
    let current = host.state.borrow().job.as_ref().map(|j| j.ticket);
    if current != Some(ticket) {
        return;
    }
    end_job(host);
    done(host, value);
    view_model::push_steps(host);
}

/// Raises the cancel flag of the running job.
pub fn cancel_job(host: &Rc<Host>) {
    if let Some(job) = &host.state.borrow().job {
        job.cancel.store(true, Ordering::Relaxed);
    }
}
