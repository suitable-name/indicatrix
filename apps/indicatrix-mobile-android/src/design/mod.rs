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

pub mod concave;
pub mod deep_solve;
pub mod document;
pub mod history;
pub mod optimization;
pub mod props;
pub mod render;
pub mod retarget;
pub mod solve;
pub mod tiers;
pub mod variants;
pub mod viewport;

/// The app state as the UI thread holds it.
pub type Shared = Rc<RefCell<AppState>>;

/// Connects every Design surface's callbacks, and the shell's single dialog-answer
/// callback, which offers each answer to the surfaces in turn.
pub fn wire(window: &AppWindow, state: &Shared) {
    document::wire(window, state);
    deep_solve::wire(window, state);
    variants::wire(window, state);
    optimization::wire(window, state);
    retarget::wire(window, state);
    concave::wire(window, state);
    history::wire(window, state);
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
            let claimed = match dialog_owner(id) {
                DialogOwner::Remote => crate::remote::handle_dialog_result(&window, id, confirmed),
                DialogOwner::Library => {
                    crate::library::handle_dialog_result(&window, &state, id, confirmed)
                }
                DialogOwner::Cut => {
                    crate::cut::handle_dialog_result(&window, &state, id, confirmed)
                }
                DialogOwner::Document => {
                    document::handle_dialog_result(&window, &state, id, confirmed)
                }
            };
            if !claimed {
                tracing::debug!(id, confirmed, "dialog answer nobody claimed");
            }
        }
    });
}

/// Which surface answers a shell dialog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DialogOwner {
    /// The document (unsaved-changes questions, Clear Recent Items).
    Document,
    /// The Cut destination (Reset Progress, Delete All Cutting Progress).
    Cut,
    /// The Library (Delete, import failed).
    Library,
    /// The Remote page (Forget Server).
    Remote,
}

/// The owner of dialog `id`: ids below 2000 belong to the document (1001-1003), 2000-2999
/// to the Cut destination (2001-2002), 3000-3099 to the Library and 4000-4099 to the
/// Remote page (4001). An id from 2000 up that no range names goes to the Cut destination
/// as before (which does not claim it, so the shell logs it); a new page's range must be
/// added here before its dialogs can be answered.
const fn dialog_owner(id: i32) -> DialogOwner {
    match id {
        4000..=4099 => DialogOwner::Remote,
        3000..=3099 => DialogOwner::Library,
        2000..=2999 | 3100..=3999 | 4100.. => DialogOwner::Cut,
        _ => DialogOwner::Document,
    }
}

/// Ends the Design surfaces' worker threads (solve, raster) and drops every
/// thread-local handle on the app state. Call on the UI thread once the event loop has
/// ended: the state (and its settings writer) must be dropped while their threads still
/// exist, not by the process's thread-local teardown, where joining aborts on Windows.
pub fn shutdown() {
    solve::shutdown();
    history::shutdown();
    deep_solve::shutdown();
    variants::shutdown();
    optimization::shutdown();
    retarget::shutdown();
    concave::shutdown();
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
    history::push(window, state);
    deep_solve::push(window, state);
    optimization::push(window, state);
    retarget::push(window, state);
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
    state.borrow_mut().operations.invalidate();
    solve::schedule_auto(window, state);
    refresh(window, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialog_id_reaches_the_surface_that_asked() {
        // The ids are the ones the surfaces use: 1001-1002 unsaved changes and 1003 Clear
        // Recent Items (document), 2001 Reset Progress and 2002 Delete All Cutting
        // Progress (Cut), 3001 Delete and 3002 import failed (Library).
        for id in [1001, 1002, 1003] {
            assert_eq!(dialog_owner(id), DialogOwner::Document, "dialog {id}");
        }
        for id in [2001, 2002] {
            assert_eq!(dialog_owner(id), DialogOwner::Cut, "dialog {id}");
        }
        for id in [3001, 3002] {
            assert_eq!(dialog_owner(id), DialogOwner::Library, "dialog {id}");
        }
        assert_eq!(
            dialog_owner(crate::remote::DIALOG_FORGET),
            DialogOwner::Remote
        );
    }

    #[test]
    fn the_range_edges_stay_with_their_owner() {
        assert_eq!(dialog_owner(1999), DialogOwner::Document);
        assert_eq!(dialog_owner(2000), DialogOwner::Cut);
        assert_eq!(dialog_owner(2999), DialogOwner::Cut);
        assert_eq!(dialog_owner(3000), DialogOwner::Library);
        assert_eq!(dialog_owner(3099), DialogOwner::Library);
        assert_eq!(dialog_owner(3100), DialogOwner::Cut);
        assert_eq!(dialog_owner(4000), DialogOwner::Remote);
        assert_eq!(dialog_owner(4099), DialogOwner::Remote);
        assert_eq!(dialog_owner(4100), DialogOwner::Cut);
    }
}
