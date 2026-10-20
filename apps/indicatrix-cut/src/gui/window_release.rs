//! Letting go of what a previous main window left in this thread's thread-locals.
//!
//! The application has one main window, so the thread-locals that remember the vault, the render
//! context, the settings writer or the editor state for their module's callbacks are written once
//! and live as long as the process. The UI harness builds one main window per scenario on the same
//! thread: a thread-local that still holds the previous window's `Arc<Mutex<Database>>` keeps that
//! window's database file open (the profile cannot be deleted, a fresh run opens the old
//! catalogue), and a kept secondary window answers with the previous window's callbacks.
//!
//! [`release_window_state`] clears every such holder. [`build_main_window`] calls it first
//! (nothing is set yet in the one-window application, so it does nothing there); the harness calls
//! it again after it dropped a window, so the last window's handles go too.
//!
//! [`live_holders`] names the holders that are still set afterwards, so a database handle that
//! survives a shutdown can be traced.
//!
//! [`build_main_window`]: super::build_main_window

use super::{batch, editor, help, library::search, pickers, render, render_jobs, rough_plan};

/// Clears the module-level handles (database, render context, settings writer, editor state) and
/// the kept secondary windows that a previous main window left behind, and cancels the library
/// search timers it left pending. Safe to call at any time on the UI thread; calling it with no
/// previous window does nothing.
pub fn release_window_state() {
    // The windows that belong to the main window go first, so nothing is looking at the handles.
    rough_plan::close_planner_window();
    help::close_help_window();
    search::cancel_pending_searches();
    pickers::release_window_state();
    editor::release_window_state();
    batch::tilt::release_window_state();
    batch::preview::release_window_state();
    batch::preview_cache::release_window_state();
    render::design_lighting::release_window_state();
    render::camera_handles::release_camera();
    render_jobs::controller::release_window_state();
    {
        super::rough_colour::material_zones::release_window_state();
        super::rough_colour::adopt_link::release_window_state();
    }
}

/// The holders of the design library handle, the editor state or a kept window that are still
/// set, by `module NAME (what it holds)`. Empty after a complete [`release_window_state`]; the UI
/// harness prints it when a database handle survives a shutdown (the survivor is then either one
/// of these, or a background thread or a queued event-loop closure, which are not listed).
/// UI thread only.
#[cfg(feature = "ui-harness")]
#[must_use]
pub fn live_holders() -> Vec<&'static str> {
    let mut held = Vec::new();
    pickers::note_holders(&mut held);
    editor::note_holders(&mut held);
    batch::tilt::note_holders(&mut held);
    batch::preview::note_holders(&mut held);
    batch::preview_cache::note_holders(&mut held);
    render::design_lighting::note_holders(&mut held);
    render_jobs::controller::note_holders(&mut held);
    rough_plan::note_holders(&mut held);
    held
}
