//! Shared facet-overlay state resubmitted by every hover/select/multi-select path.
//!
//! [`resubmit_facet_overlay`] and [`facet_map_from_aligned_solve`] are the two
//! primitives every selection, per-facet, and solid-picking callback in this
//! module group goes through so the merge-then-resubmit sequence (and the "never
//! trust a stale solve" guard) is never duplicated or done slightly differently at
//! two call sites.

use std::cell::RefCell;

use indicatrix_cut_core::Design;

use crate::gui::{
    editor::auto_solve,
    solid_preview::{
        facet_map::FacetMap,
        preview_state::{FacetOverlay, SolidPreviewState},
    },
};

thread_local! {
    /// The last [`FacetOverlay`] submitted to the solid preview's worker thread --
    /// mirrored here since [`SolidPreviewState::request_facet_overlay`] replaces the
    /// WHOLE overlay on every call (there is no "just update the hover field"
    /// entry point): the hover/click/multi-select/selected-tier-changed callbacks
    /// each mutate only their own field of this cached copy and resubmit the
    /// merged whole, so hovering a facet never clears the click/multi-select
    /// highlight and vice versa. UI-thread-only, same reasoning as
    /// `auto_solve::RUNTIME`'s own `thread_local!`.
    static FACET_OVERLAY: RefCell<FacetOverlay> = const {
        RefCell::new(FacetOverlay {
            hovered: None,
            selected_facet: None,
            multi_selected: Vec::new(),
        })
    };
}

/// Mutates the cached [`FacetOverlay`] via `mutate`, then resubmits the merged
/// whole to `preview_state` -- the one place every overlay-touching callback below
/// goes through, so the merge-then-resubmit sequence is never duplicated or done
/// slightly differently at two call sites.
pub(super) fn resubmit_facet_overlay(
    preview_state: &SolidPreviewState,
    mutate: impl FnOnce(&mut FacetOverlay),
) {
    let overlay = FACET_OVERLAY.with(|cell| {
        let mut overlay = cell.borrow_mut();
        mutate(&mut overlay);
        overlay.clone()
    });
    preview_state.request_facet_overlay(overlay);
}

/// Builds a [`FacetMap`] from `design` and the last-solved mast cache, but only
/// when that cache is aligned with `design`'s CURRENT tier count. An unfiltered
/// `solid_last_solved().unwrap_or_default()` here fed
/// `FacetMap::from_design` a stale solve (wrong length) whenever a tier had just
/// been added or removed and the background solve had not caught up yet;
/// `facet_map.rs`'s own fallback for a missing tier is mast 0, so every
/// highlight/hover/pick landed on the wrong facet until the next frame. Falls
/// back to `FacetMap::default()` (no facets) rather than a stale solve, same
/// reasoning `retarget_actions::setup_snapshot_callbacks`'s length filter uses
/// for its own `solid_last_solved` read.
pub(super) fn facet_map_from_aligned_solve(design: &Design) -> FacetMap {
    let solved = auto_solve::solid_last_solved()
        .and_then(|cache| {
            cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .filter(|solved| solved.len() == design.tiers.len())
        .unwrap_or_default();
    FacetMap::from_design(design, &solved)
}
