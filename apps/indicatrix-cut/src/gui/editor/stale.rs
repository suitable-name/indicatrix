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
//! [`clear`]/[`refresh_badges`]'s own visibility.

use crate::{MainWindow, RetargetModel};
use slint::ComponentHandle;
use std::{cell::RefCell, collections::BTreeMap};

/// Which analysis result a generation stamp names. `Ord`/`PartialOrd` (needed only
/// for `BTreeMap`'s key bound, per this crate's "no `HashMap`/`HashSet` iteration
/// in a decision path" house rule) follow the derive order below, which has no
/// significance beyond satisfying the trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::gui::editor) enum ResultKind {
    /// "Retarget for material"'s held proposal (`RetargetModel.rows`/`notes`) --
    /// see the module doc comment for why this is the one result actually using
    /// this registry today.
    Retarget,
}

thread_local! {
    /// UI-thread-only bookkeeping (Slint's event loop is single-threaded, the same
    /// soundness argument `callbacks::retarget_actions::RETARGET_ASYNC`'s own doc
    /// comment makes for its own `thread_local!`) -- the generation each
    /// [`ResultKind`] was last [`stamp`]ed at, or absent for "no result to badge".
    static STAMPS: RefCell<BTreeMap<ResultKind, u64>> = const { RefCell::new(BTreeMap::new()) };
}

/// Records that `kind`'s current result was computed against `generation` --
/// called the moment a fresh result actually lands (a rebuilt Shift-mode proposal,
/// a completed async Optimize-mode search).
pub(in crate::gui::editor) fn stamp(kind: ResultKind, generation: u64) {
    STAMPS.with(|cell| {
        cell.borrow_mut().insert(kind, generation);
    });
}

/// Forgets `kind`'s stamp -- called whenever its result is discarded outright
/// (the dialog closes, the held proposal is applied/cancelled) so a badge can
/// never survive its own result.
pub(in crate::gui::editor) fn clear(kind: ResultKind) {
    STAMPS.with(|cell| {
        cell.borrow_mut().remove(&kind);
    });
}

/// Re-derives every `*_stale` Slint property this registry drives, against the
/// design's current `live_generation` -- called once per edit from
/// [`super::view::push_stale_content`], alongside that function's own direct
/// Deep-Solve/Optimize staleness pushes (see the module doc comment for why those
/// two are not routed through here).
pub(in crate::gui::editor) fn refresh_badges(ui: &MainWindow, live_generation: u64) {
    let stamped = |kind: ResultKind| STAMPS.with(|cell| cell.borrow().get(&kind).copied());
    ui.global::<RetargetModel>()
        .set_stale(super::state::result_is_stale(
            stamped(ResultKind::Retarget),
            live_generation,
        ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_with_no_stamp_is_never_stale() {
        STAMPS.with(|cell| cell.borrow_mut().clear());
        assert_eq!(
            STAMPS.with(|cell| cell.borrow().get(&ResultKind::Retarget).copied()),
            None
        );
    }

    #[test]
    fn stamping_then_reading_back_the_same_generation_round_trips() {
        STAMPS.with(|cell| cell.borrow_mut().clear());
        stamp(ResultKind::Retarget, 7);
        assert_eq!(
            STAMPS.with(|cell| cell.borrow().get(&ResultKind::Retarget).copied()),
            Some(7)
        );
    }

    #[test]
    fn clearing_a_stamped_kind_forgets_it() {
        STAMPS.with(|cell| cell.borrow_mut().clear());
        stamp(ResultKind::Retarget, 3);
        clear(ResultKind::Retarget);
        assert_eq!(
            STAMPS.with(|cell| cell.borrow().get(&ResultKind::Retarget).copied()),
            None
        );
    }

    #[test]
    fn restamping_overwrites_the_previous_generation() {
        STAMPS.with(|cell| cell.borrow_mut().clear());
        stamp(ResultKind::Retarget, 1);
        stamp(ResultKind::Retarget, 2);
        assert_eq!(
            STAMPS.with(|cell| cell.borrow().get(&ResultKind::Retarget).copied()),
            Some(2)
        );
    }

    #[test]
    fn result_is_stale_agrees_with_a_stamped_then_moved_generation() {
        // The actual decision `refresh_badges` reduces to for each kind -- exercised
        // here directly (rather than through a live `MainWindow`, which this crate's
        // test environment cannot construct -- see `activity`'s own test module doc
        // comment for the identical constraint) against `state::result_is_stale`,
        // the same pure function Deep Solve/Optimize's own staleness already uses.
        STAMPS.with(|cell| cell.borrow_mut().clear());
        stamp(ResultKind::Retarget, 5);
        let stamped = STAMPS.with(|cell| cell.borrow().get(&ResultKind::Retarget).copied());
        assert!(!super::super::state::result_is_stale(stamped, 5));
        assert!(super::super::state::result_is_stale(stamped, 6));
    }
}
