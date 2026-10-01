//! A shared generation-stamp registry for analysis results that have no other
//! natural home to track their own "computed against generation N" bookkeeping in.
//!
//! # Why this exists alongside `state::result_is_stale`, not instead of it
//!
//! Deep Solve and Optimize already stamp their own result generation directly on
//! [`super::state::EditorState`] (`deep_solve_result_generation`/
//! `pending_optimize`'s stored generation) and read it back in
//! [`super::view::push_stale_content`] -- that machinery predates this module and
//! is not migrated here: `EditorState`'s fields are immutable, and duplicating a
//! second stamp for the same two results would be duplication. This module instead
//! covers the ONE remaining result that has no `EditorState` field of its own to
//! live on: the Retarget proposal, whose generation is produced from TWO different
//! call sites (`callbacks::retarget_actions`'s Shift-mode intent handler and its
//! async Optimize-mode completion handler) that cannot otherwise share a single
//! owner without a new `EditorState` field. A `thread_local!` map, keyed by
//! [`ResultKind`], gives both call sites one shared place to stamp into and gives
//! [`refresh_badges`] one shared place to read back from, entirely in this module.
//!
//! Tilt curves (`ui/models/tilt.slint`'s `cached_curve_*`/`curves_stale`) are
//! deliberately NOT stamped here: `gui::tilt` sits outside `gui::editor`'s module
//! tree (Rust privacy: `mod activity;`/`mod auto_solve;` in `gui/editor/mod.rs`
//! are visible only within that subtree), so it cannot reach this registry. Instead,
//! `gui::tilt::tilt_profile` keys tilt-curve staleness off its own already-existing
//! `AxesCacheKey`/`hash_planes` comparison (the exact planes/material/light the
//! curve was swept against), which is self-contained and more precise than a generic
//! edit-generation counter would be.
//! Trace staleness (`ViewportModel.trace_stale`) is likewise left as-is: it already
//! has its own generation home on `RenderContext::planes_owner`
//! (`bridge::render_thread::context::RenderContext::traced_planes_are_stale`),
//! read directly by `view::push_trace_staleness`.
//!
//! Reachable only from within `gui::editor` -- see [`ResultKind`]/[`stamp`]/
//! [`clear`]/[`refresh_badges`]'s own visibility. The registry itself
//! (`ResultKind`/`StaleStamps`) lives in `indicatrix_editor::stale`, shared with the
//! web app; this module keeps the UI-thread holder and the Slint badge push.

use crate::{MainWindow, RetargetModel};
pub(in crate::gui::editor) use indicatrix_editor::stale::ResultKind;
use indicatrix_editor::stale::StaleStamps;
use slint::ComponentHandle;
use std::cell::RefCell;

thread_local! {
    /// UI-thread-only bookkeeping (Slint's event loop is single-threaded, the same
    /// soundness argument `callbacks::retarget_actions::RETARGET_ASYNC`'s own doc
    /// comment makes for its own `thread_local!`) -- the generation each
    /// [`ResultKind`] was last [`stamp`]ed at, or absent for "no result to badge".
    static STAMPS: RefCell<StaleStamps> = const { RefCell::new(StaleStamps::new()) };
}

/// Records that `kind`'s current result was computed against `generation` --
/// called the moment a fresh result actually lands (a rebuilt Shift-mode proposal,
/// a completed async Optimize-mode search).
pub(in crate::gui::editor) fn stamp(kind: ResultKind, generation: u64) {
    STAMPS.with(|cell| cell.borrow_mut().stamp(kind, generation));
}

/// Forgets `kind`'s stamp -- called whenever its result is discarded outright
/// (the dialog closes, the held proposal is applied/cancelled) so a badge can
/// never survive its own result.
pub(in crate::gui::editor) fn clear(kind: ResultKind) {
    STAMPS.with(|cell| cell.borrow_mut().clear(kind));
}

/// Re-derives every `*_stale` Slint property this registry drives, against the
/// design's current `live_generation` -- called once per edit from
/// [`super::view::push_stale_content`], alongside that function's own direct
/// Deep-Solve/Optimize staleness pushes (see the module doc comment for why those
/// two are not routed through here).
pub(in crate::gui::editor) fn refresh_badges(ui: &MainWindow, live_generation: u64) {
    let stale = STAMPS.with(|cell| {
        cell.borrow()
            .is_stale(ResultKind::Retarget, live_generation)
    });
    ui.global::<RetargetModel>().set_stale(stale);
}
