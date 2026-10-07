//! The facet highlight and the selected facet, shared by the Solid and the Diagram views.
//!
//! Both views draw the same [`FacetOverlay`] (hovered facet, the clicked facet, the
//! multi-selected tiers' facets, the slice tier's and the drag followers' outlines) and
//! both hand the manipulation handles the same "last clicked facet" as their anchor. The
//! state lives here, below `gui::editor`, so the Diagram view's pointer callbacks
//! (`super::diagram_wiring`, which has no editor dependency) update exactly what the Solid
//! view's do.
//!
//! # Merge, never replace
//!
//! [`SolidPreviewState::request_facet_overlay`] replaces the WHOLE overlay. Every writer
//! therefore mutates its own field of the cached copy and resubmits the merged whole
//! ([`resubmit_facet_overlay`]): a hover never wipes the clicked facet's outline or the
//! multi-select, and a click never wipes the drag followers. The pure merge steps
//! ([`apply_hover`], [`apply_click`], [`apply_cleared_pick`]) are what the tests pin.

use super::preview_state::{FacetOverlay, FacetOwners, SolidPreviewState};
use crate::{EditorModel, MainWindow};
use slint::{ComponentHandle, Model};
use std::{cell::RefCell, sync::Arc};

/// The tier table's row counts right now, as `(flat, concave)`.
///
/// A click's facet owner is checked and positioned against them
/// ([`FacetOwners::table_position`]). Read from the table itself, so a stale frame (drawn
/// before an edit changed the tier counts) can never select a row that is not there.
#[must_use]
pub fn tier_row_counts(ui: &MainWindow) -> (usize, usize) {
    let concave = ui
        .global::<EditorModel>()
        .get_tiers()
        .iter()
        .filter(|row| row.kind == 1)
        .count();
    let total = ui.global::<EditorModel>().get_tiers().row_count();
    (total - concave, concave)
}

/// The tier-table row a click on `facet_id` selects, from the frame's `owners` and the
/// table as it is now (`None`: the facet belongs to no selectable tier).
#[must_use]
pub fn clicked_tier_row(ui: &MainWindow, owners: &FacetOwners, facet_id: u32) -> Option<usize> {
    let (flat_count, concave_count) = tier_row_counts(ui);
    owners.table_position(facet_id as usize, flat_count, concave_count)
}

thread_local! {
    /// The last [`FacetOverlay`] submitted to the solid preview's worker thread.
    /// UI-thread-only, like every other piece of Slint-adjacent state.
    static FACET_OVERLAY: RefCell<FacetOverlay> = const {
        RefCell::new(FacetOverlay {
            hovered: None,
            selected_facet: None,
            multi_selected: Vec::new(),
            provisional: Vec::new(),
            moved: Vec::new(),
        })
    };
    /// The last clicked facet's id (either view): the facet the manipulation handles
    /// (`gui::editor::manipulate`) sit on. A selection made from the tier table leaves it
    /// stale or unset, which the handles resolve by checking it belongs to the selected
    /// tier and falling back to the tier's first facet. UI-thread-only.
    static SELECTED_FACET_ID: RefCell<Option<u32>> = const { RefCell::new(None) };
    /// Which flat tiers the frame on screen contains when the Cut slider has the stone cut
    /// back (`None`: the finished stone) -- see [`note_drawn_tiers`]. UI-thread-only.
    static DRAWN_TIERS: RefCell<Option<Arc<Vec<bool>>>> = const { RefCell::new(None) };
}

/// Makes `hovered` the hovered facet of `overlay`; every other field stays.
pub const fn apply_hover(overlay: &mut FacetOverlay, hovered: Option<u32>) {
    overlay.hovered = hovered;
}

/// Makes `facet` the clicked facet of `overlay`; every other field stays.
pub const fn apply_click(overlay: &mut FacetOverlay, facet: u32) {
    overlay.selected_facet = Some(facet);
}

/// A click that missed every facet: the clicked facet and the hover are gone.
///
/// The multi-selected tiers and the two outlines are not this click's to clear (the
/// selection change that follows clears the multi-select itself).
pub const fn apply_cleared_pick(overlay: &mut FacetOverlay) {
    overlay.hovered = None;
    overlay.selected_facet = None;
}

/// Mutates the cached [`FacetOverlay`] via `mutate`, then resubmits the merged whole.
///
/// The one place every overlay-touching callback goes through, so the merge-then-resubmit
/// sequence is never duplicated or done slightly differently at two call sites.
pub fn resubmit_facet_overlay(
    preview_state: &SolidPreviewState,
    mutate: impl FnOnce(&mut FacetOverlay),
) {
    let overlay = FACET_OVERLAY.with(|cell| {
        let mut overlay = cell.borrow_mut();
        mutate(&mut overlay);
        overlay.clone()
    });
    // The two outlines a replan would otherwise wipe are also handed to the worker's
    // draw step directly (`SolidPreviewState::set_outlines`).
    preview_state.set_outlines(&overlay.provisional, &overlay.moved);
    preview_state.request_facet_overlay(overlay);
}

/// Makes `facet` the hovered facet via [`resubmit_facet_overlay`], unless it already is.
///
/// Compared against the cached overlay, which holds what the worker last received from here.
/// A pointer moving within one facet, or across the background beside it, changes nothing on
/// screen, so it must not re-raster the whole view.
pub fn set_hovered_facet(preview_state: &SolidPreviewState, facet: Option<u32>) {
    if FACET_OVERLAY.with(|cell| cell.borrow().hovered) == facet {
        return;
    }
    resubmit_facet_overlay(preview_state, |overlay| apply_hover(overlay, facet));
}

/// A click landed on `facet` (either view): it becomes the clicked facet, in the overlay
/// and as the handles' anchor ([`selected_facet_id`]).
pub fn select_clicked_facet(preview_state: &SolidPreviewState, facet: u32) {
    set_selected_facet_id(Some(facet));
    resubmit_facet_overlay(preview_state, |overlay| apply_click(overlay, facet));
}

/// A click missed every facet: nothing is clicked or hovered any more.
pub fn clear_clicked_facet(preview_state: &SolidPreviewState) {
    set_selected_facet_id(None);
    resubmit_facet_overlay(preview_state, apply_cleared_pick);
}

/// The last clicked facet's id, if the last click landed on a facet.
#[must_use]
pub fn selected_facet_id() -> Option<u32> {
    SELECTED_FACET_ID.with(|cell| *cell.borrow())
}

/// Records (or, with `None`, forgets) the last clicked facet.
pub fn set_selected_facet_id(facet: Option<u32>) {
    SELECTED_FACET_ID.with(|cell| *cell.borrow_mut() = facet);
}

/// The sink records which flat tiers the frame it just stored contains
/// (`FrameGeometry::visible_tiers`; `None` for the finished stone).
///
/// The facet ids of a frame number the facets of exactly those tiers, so a facet map that
/// turns a tier into facet ids for an outline must be built for the same set
/// ([`drawn_tiers`]): under a cutting-order cut of a design with concave tiers the drawn
/// planes are not a prefix of the finished stone's, and the finished stone's numbering
/// would light the wrong facets.
pub fn note_drawn_tiers(tiers: Option<Arc<Vec<bool>>>) {
    DRAWN_TIERS.with(|cell| *cell.borrow_mut() = tiers);
}

/// The tiers of the frame on screen, as last recorded by [`note_drawn_tiers`].
#[must_use]
pub fn drawn_tiers() -> Option<Arc<Vec<bool>>> {
    DRAWN_TIERS.with(|cell| cell.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An overlay with something in every field.
    fn busy() -> FacetOverlay {
        FacetOverlay {
            hovered: Some(1),
            selected_facet: Some(2),
            multi_selected: vec![3, 4],
            provisional: vec![5],
            moved: vec![6, 7],
        }
    }

    #[test]
    fn a_hover_keeps_the_clicked_facet_the_multi_select_and_both_outlines() {
        let mut merged = busy();
        apply_hover(&mut merged, Some(9));
        assert_eq!(merged.hovered, Some(9));
        assert_eq!(merged.selected_facet, Some(2));
        assert_eq!(merged.multi_selected, vec![3, 4]);
        assert_eq!(merged.provisional, vec![5]);
        assert_eq!(merged.moved, vec![6, 7]);
        apply_hover(&mut merged, None);
        assert_eq!(
            merged.selected_facet,
            Some(2),
            "leaving a facet keeps the click"
        );
    }

    #[test]
    fn a_click_keeps_the_hover_the_multi_select_and_both_outlines() {
        let mut merged = busy();
        apply_click(&mut merged, 8);
        assert_eq!(merged.selected_facet, Some(8));
        assert_eq!(merged.hovered, Some(1));
        assert_eq!(merged.multi_selected, vec![3, 4]);
        assert_eq!(merged.provisional, vec![5]);
        assert_eq!(merged.moved, vec![6, 7]);
    }

    #[test]
    fn a_click_that_misses_clears_only_the_pick() {
        let mut merged = busy();
        apply_cleared_pick(&mut merged);
        assert_eq!((merged.hovered, merged.selected_facet), (None, None));
        assert_eq!(merged.multi_selected, vec![3, 4]);
        assert_eq!(merged.provisional, vec![5]);
        assert_eq!(merged.moved, vec![6, 7]);
    }

    #[test]
    fn merging_into_an_empty_overlay_changes_one_field() {
        let mut hovered = FacetOverlay::default();
        apply_hover(&mut hovered, Some(3));
        assert_eq!(
            hovered,
            FacetOverlay {
                hovered: Some(3),
                ..FacetOverlay::default()
            }
        );
        let mut cleared = FacetOverlay::default();
        apply_cleared_pick(&mut cleared);
        assert_eq!(cleared, FacetOverlay::default());
    }

    #[test]
    fn the_drawn_tiers_are_remembered_until_the_next_frame_replaces_them() {
        note_drawn_tiers(None);
        assert_eq!(drawn_tiers(), None);
        note_drawn_tiers(Some(Arc::new(vec![true, false, true])));
        assert_eq!(drawn_tiers().as_deref(), Some(&vec![true, false, true]));
        note_drawn_tiers(None);
        assert_eq!(drawn_tiers(), None, "a finished frame forgets the cut");
    }

    #[test]
    fn the_selected_facet_id_is_one_value_for_both_views() {
        set_selected_facet_id(None);
        assert_eq!(selected_facet_id(), None);
        set_selected_facet_id(Some(17));
        assert_eq!(selected_facet_id(), Some(17));
        set_selected_facet_id(None);
        assert_eq!(selected_facet_id(), None);
    }
}
