//! Running a step of the locate windows on a worker thread.
//!
//! The windows own the tickets and the busy state; this is only the thread: it runs the work
//! (a panic in it becomes an error message, so a window never waits for an answer that will
//! not come) and hands the result to the UI thread through the window's event loop, unless the
//! window is gone by then.

use super::super::mesh_task::guarded;
use slint::{ComponentHandle, Weak};

/// Runs `work` on a thread of its own and `deliver` with its result on the UI thread. An error
/// means the thread could not be started (and `work` and `deliver` were dropped).
pub(super) fn spawn<W: ComponentHandle + 'static, T: Send + 'static>(
    weak: Weak<W>,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    deliver: impl FnOnce(Result<T, String>) + Send + 'static,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("locate-job".to_string())
        .spawn(move || {
            let value = guarded(work);
            let _ = weak.upgrade_in_event_loop(move |_window| deliver(value));
        })
        .map(|_| ())
}
