//! The Design destination's Rust side: one module per lane surface, tied together here.
//!
//! Every submodule exposes the same two entry points:
//! - `wire(window, state)`: connects its Slint callbacks (called once at start);
//! - `push(window, state)`: writes its Slint properties from the state (called by
//!   [`refresh`] after anything changed; must be cheap and must not borrow mutably).
//!
//! Code that changes the design (`state.session`) calls [`after_edit`]; code that
//! changes only what is shown (selection, camera, a setting) calls [`refresh`].
//! `solve` additionally offers `request_solve` and calls [`refresh`] when a solve lands.

use std::{cell::RefCell, rc::Rc};

use indicatrix_mobile_core::app_state::AppState;
use slint::ComponentHandle;

use crate::{AppWindow, Shell};

pub mod document;
pub mod props;
pub mod render;
pub mod solve;
pub mod tiers;
pub mod viewport;

/// The app state as the UI thread holds it.
pub type Shared = Rc<RefCell<AppState>>;

/// Connects every Design surface's callbacks, and the shell's single dialog-answer
/// callback, which offers each answer to the surfaces in turn.
pub fn wire(window: &AppWindow, state: &Shared) {
    document::wire(window, state);
    viewport::wire(window, state);
    tiers::wire(window, state);
    solve::wire(window, state);
    props::wire(window, state);
    render::wire(window, state);
    crate::library::wire(window, state);

    window.global::<Shell>().on_dialog_result({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move |id, confirmed| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            // Ids 2000-2999 belong to the Cut destination, 3000-3099 to the Library.
            let claimed = if (3000..3100).contains(&id) {
                crate::library::handle_dialog_result(&window, &state, id, confirmed)
            } else if id >= 2000 {
                crate::cut::handle_dialog_result(&window, &state, id, confirmed)
            } else {
                document::handle_dialog_result(&window, &state, id, confirmed)
            };
            if !claimed {
                tracing::debug!(id, confirmed, "dialog answer nobody claimed");
            }
        }
    });
}

/// Ends the Design surfaces' worker threads (solve, raster) and drops every
/// thread-local handle on the app state. Call on the UI thread once the event loop has
/// ended: the state (and its settings writer) must be dropped while their threads still
/// exist, not by the process's thread-local teardown, where joining aborts on Windows.
pub fn shutdown() {
    solve::shutdown();
    viewport::shutdown();
    render::shutdown();
    crate::library::shutdown();
    crate::cut::mode::shutdown();
    document::forget_state();
    props::forget();
}

/// Pushes every Design surface from the state.
pub fn push_all(window: &AppWindow, state: &AppState) {
    document::push(window, state);
    viewport::push(window, state);
    tiers::push(window, state);
    solve::push(window, state);
    props::push(window, state);
    render::push(window, state);
    // The Cut destination shows the same design's instructions, so it refreshes with it;
    // the Library shows its save state.
    crate::cut::push(window, state);
    crate::library::push(window, state);
}

/// Re-pushes every Design surface. Call after a change that does not touch the design
/// itself (selection, camera, settings, a finished solve).
pub fn refresh(window: &AppWindow, state: &Shared) {
    push_all(window, &state.borrow());
}

/// Call after any change to `state.session` (an edit, undo/redo, a new or opened
/// design): schedules the auto-solve (if the budget allows) and re-pushes everything.
pub fn after_edit(window: &AppWindow, state: &Shared) {
    solve::schedule_auto(window, state);
    refresh(window, state);
}
