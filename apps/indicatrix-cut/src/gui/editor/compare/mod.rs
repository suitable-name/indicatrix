//! The visual before/after comparison: a separate OS window
//! (`ui/compare_window.slint`'s `CompareWindow`), or the pane embedded in the
//! Retarget dialog (`ui/components/compare_view.slint`'s `CompareView`, backed by the
//! main window's own `CompareModel` instance), showing the design before and after
//! a Retarget proposal, an Optimize result, or against the held design snapshot --
//! side by side, under a draggable split divider (window only), or as a difference
//! overlay on the "after" side (removed material red, added green, outlines). Both
//! sides are turned by ONE shared orbit camera, in the flat solid view (instant) or
//! the CPU path tracer (a modest fixed quality, started only after the pose has
//! settled and the mouse button is released). Only one session is live at a time,
//! on one of those two surfaces.
//!
//! # Keep/Discard never reimplement Apply
//!
//! "Keep after" invokes the originating feature's own Apply handler
//! (`RetargetModel.apply()` -> `callbacks::retarget_actions::apply::
//! setup_retarget_apply_callback`, `EditorModel.optimize_apply()` ->
//! `callbacks::solve_actions::optimize_outcome::setup_optimize_apply_callback`), after
//! [`origins::guard_still_matches`] confirms that handler would commit exactly what
//! was compared -- so undo, dirty and generation bookkeeping are the Apply button's
//! own. "Discard" closes the window and switches that feature's viewport ghost
//! preview off; "Close" just closes. A snapshot comparison offers only Close.
//!
//! # Module layout
//!
//! [`session`] (Slint-free: the two solved sides, the shared pose and its
//! arithmetic, the status line, the stale-frame book), [`render`] (the solid and
//! traced workers and their pure render functions), [`origins`] (gathering each
//! origin's sides and the Keep guard), [`host`] (the two surfaces, the live session
//! and frame delivery), and [`wiring`] (which surface a request opens on, and every
//! Slint callback). `tests` covers the first two without Slint.

mod host;
mod origins;
mod overlay;
mod render;
mod session;
#[cfg(test)]
mod tests;
mod wiring;

use super::{state::EditorState, view::SolidLastSolved};
use crate::{
    MainWindow, bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::SolidPreviewState,
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

pub(in crate::gui) use wiring::close_compare_window;

/// Wires the compare window's three entry points (`MainWindow`'s own
/// `CompareModel.open_retarget`/`open_optimize`/`open_snapshot`) and the Retarget
/// dialog's embedded pane (`open_retarget_embedded`/`close_embedded`). The window
/// itself is created lazily on the first open.
pub(in crate::gui::editor) fn setup_compare_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let deps = wiring::CompareDeps {
        ui: ui.as_weak(),
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
    };
    wiring::setup_entry_callbacks(ui, &deps);
}
