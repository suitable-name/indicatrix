//! Background work on a mesh rough: reading and building an imported OBJ file, and scaling
//! a mesh rough to a weighed carat.
//!
//! Both rebuild a mesh (the hull, the watertight check, the search tree of up to 50,000
//! triangles), which takes seconds for a real scan, so they run on a thread of their own
//! ([`spawn`]) and only their result is applied on the UI thread, in the same way as the
//! saved-plan tasks (`saved::spawn_task`). While one runs the window shows what is going
//! on and locks the base picker, Fit to weight and Plan.
//!
//! Every job has a ticket. A result is applied only when its ticket is the one the window
//! is still waiting for: a newer job, a reset ([`cancel`]), the opening of a saved plan
//! or a closed window makes an older result stale, and it is dropped.

use super::{
    host::{Host, on_host},
    saved::{panic_message, show_error},
};
use crate::RoughPlanModel;
use slint::ComponentHandle;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};
use tracing::warn;

/// The next ticket. Process-wide, so a result that outlives its window (the planner window
/// is created anew after it was closed) can never match a ticket of the new one.
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

/// Shown when something that needs the finished mesh is asked for while a mesh job runs.
pub(super) const MESH_BUSY_MESSAGE: &str =
    "The mesh is still being read or scaled. Wait for it to finish first.";

/// Shown when a result arrives while a plan runs: the model must not change under a run.
const PLAN_RUNNING_MESSAGE: &str = "A plan started before the mesh was ready, so the mesh was not used. Do it again when the plan has finished.";

/// Which mesh job the window is waiting for, if any.
#[derive(Debug, Default)]
pub(super) struct MeshJobs {
    /// The ticket of the job whose result is still wanted.
    current: Option<u64>,
}

impl MeshJobs {
    /// Starts a job: the ticket it must present with its result. A job that is still
    /// running is superseded, and its result will be dropped.
    pub(super) fn begin(&mut self) -> u64 {
        let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
        self.current = Some(ticket);
        ticket
    }

    /// Whether the result of job `ticket` is still wanted. Answers `true` once: the job is
    /// finished then, and the window is not busy any more.
    pub(super) fn finish(&mut self, ticket: u64) -> bool {
        if self.current == Some(ticket) {
            self.current = None;
            true
        } else {
            false
        }
    }

    /// Drops the job that is running, if any, so that its result is ignored. Returns
    /// whether there was one.
    pub(super) const fn cancel(&mut self) -> bool {
        self.current.take().is_some()
    }

    /// Whether a job is running.
    #[must_use]
    pub(super) const fn is_busy(&self) -> bool {
        self.current.is_some()
    }
}

/// Runs `work`, turning a panic into an error message for the window: a bug in a mesh
/// routine must not leave the window waiting for an answer that never comes.
pub(super) fn guarded<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|payload| {
        let message = panic_message(&*payload);
        warn!("Rough planner: a mesh task panicked: {message}");
        Err(format!(
            "it stopped unexpectedly ({message}); nothing was changed"
        ))
    })
}

/// Shows what the background mesh job is doing (`Some`), or that none runs (`None`).
fn show_busy(host: &Host, text: Option<&str>) {
    let model = host.window.global::<RoughPlanModel>();
    model.set_mesh_busy_text(text.unwrap_or_default().into());
    model.set_mesh_busy(text.is_some());
}

/// Gives up the mesh job that is running, if any: its result will be dropped and the window
/// is not busy any more. Called when something else replaces the model (New, opening a
/// saved plan).
pub(super) fn cancel(host: &Host) {
    let was_running = host.session.borrow_mut().mesh_jobs.cancel();
    if was_running {
        show_busy(host, None);
    }
}

/// Whether a mesh job is running.
pub(super) fn is_busy(host: &Host) -> bool {
    host.session.borrow().mesh_jobs.is_busy()
}

/// Runs `work` on a worker thread, then `done` with its result on the UI thread, unless the
/// job was superseded or cancelled meanwhile (the window shows `busy_text` until then).
///
/// A panic in `work` reaches `done` as an error. A message is shown in the window if the
/// thread cannot be started.
pub(super) fn spawn<T: Send + 'static>(
    host: &Rc<Host>,
    busy_text: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&Rc<Host>, Result<T, String>) + Send + 'static,
) {
    let ticket = host.session.borrow_mut().mesh_jobs.begin();
    show_busy(host, Some(busy_text));
    let weak = host.window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("rough-mesh".to_string())
        .spawn(move || {
            let value = guarded(work);
            let _ = weak.upgrade_in_event_loop(move |_window| {
                on_host(|host| deliver(host, ticket, value, done));
            });
        });
    if let Err(error) = spawned {
        warn!("Rough planner: could not start the mesh thread: {error}");
        cancel(host);
        show_error(host, &format!("Could not start a background task: {error}"));
    }
}

/// The UI-thread end of a job: applies the result when its ticket is still the wanted one.
fn deliver<T>(
    host: &Rc<Host>,
    ticket: u64,
    value: Result<T, String>,
    done: impl FnOnce(&Rc<Host>, Result<T, String>),
) {
    let wanted = host.session.borrow_mut().mesh_jobs.finish(ticket);
    if !wanted {
        return;
    }
    show_busy(host, None);
    if host.window.global::<RoughPlanModel>().get_running() {
        show_error(host, PLAN_RUNNING_MESSAGE);
        return;
    }
    done(host, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_is_wanted_until_it_finishes_and_only_once() {
        let mut jobs = MeshJobs::default();
        assert!(!jobs.is_busy());
        let ticket = jobs.begin();
        assert!(jobs.is_busy());
        assert!(jobs.finish(ticket), "the job that was waited for");
        assert!(!jobs.is_busy());
        assert!(!jobs.finish(ticket), "its result is not applied twice");
    }

    #[test]
    fn a_newer_job_makes_the_older_result_stale() {
        let mut jobs = MeshJobs::default();
        let first = jobs.begin();
        let second = jobs.begin();
        assert_ne!(first, second);
        assert!(!jobs.finish(first), "a second import supersedes the first");
        assert!(jobs.is_busy(), "the second job is still wanted");
        assert!(jobs.finish(second));
    }

    #[test]
    fn a_cancelled_job_is_dropped_and_the_window_is_not_busy() {
        let mut jobs = MeshJobs::default();
        assert!(!jobs.cancel(), "nothing to cancel");
        let ticket = jobs.begin();
        assert!(jobs.cancel());
        assert!(!jobs.is_busy());
        assert!(!jobs.finish(ticket), "a reset or a load dropped the result");
    }

    #[test]
    fn a_ticket_from_another_window_never_matches() {
        // The planner window is created anew after it was closed; its tickets come from the
        // same counter, so a result of the old window cannot be taken for a new job.
        let mut old_window = MeshJobs::default();
        let old = old_window.begin();
        let mut new_window = MeshJobs::default();
        let fresh = new_window.begin();
        assert!(!new_window.finish(old));
        assert!(new_window.finish(fresh));
    }

    #[test]
    fn a_panic_in_the_work_becomes_a_message() {
        let result: Result<u32, String> = guarded(|| panic!("the parse hit a bug"));
        let message = result.expect_err("a panic is an error, not a crash");
        assert!(message.contains("the parse hit a bug"), "{message}");
        assert!(message.contains("nothing was changed"), "{message}");
    }

    #[test]
    fn work_that_does_not_panic_keeps_its_own_result() {
        assert_eq!(guarded(|| Ok::<u32, String>(7)), Ok(7));
        assert_eq!(
            guarded(|| Err::<u32, String>("a plain failure".to_string())),
            Err("a plain failure".to_string())
        );
    }
}
