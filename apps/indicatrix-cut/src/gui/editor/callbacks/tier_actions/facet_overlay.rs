//! Shared facet-overlay state resubmitted by every hover/select/multi-select path.
//!
//! [`resubmit_facet_overlay`] and [`facet_map_from_aligned_solve`] are the two
//! primitives every selection, per-facet, and solid-picking callback in this
//! module group goes through so the merge-then-resubmit sequence (and the "never
//! trust a stale solve" guard) is never duplicated or done slightly differently at
//! two call sites.
//!
//! The overlay state itself lives in `gui::solid_preview::facet_selection`, below the
//! editor, because the Diagram view's pointer callbacks merge into the very same overlay
//! (a diagram hover used to replace it and wipe the clicked facet's outline).

use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;

use crate::gui::{
    editor::auto_solve,
    solid_preview::{facet_map::FacetMap, facet_selection},
};

pub(in crate::gui::editor) use facet_selection::{resubmit_facet_overlay, set_hovered_facet};

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
///
/// The map numbers the facets of the frame on screen: with the Cut slider cutting a design
/// back, that frame holds only some of the tiers' planes and the finished stone's
/// numbering would light the wrong facets ([`facet_map_for_drawn_tiers`]).
pub(super) fn facet_map_from_aligned_solve(design: &Design) -> FacetMap {
    let solved = auto_solve::solid_last_solved()
        .and_then(|cache| {
            cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        // the shared cache is now generation-tagged
        // (`(u64, Vec<SolvedTier>)`) -- only the masts themselves matter here.
        .filter(|(_, solved)| solved.len() == design.tiers.len())
        .map(|(_, solved)| solved)
        .unwrap_or_default();
    let drawn = facet_selection::drawn_tiers();
    facet_map_for_drawn_tiers(design, &solved, drawn.as_deref().map(Vec::as_slice))
}

/// The facet map of the frame whose planes hold the flat tiers `drawn` marks (`None`: the
/// finished stone). Pure, so the cut numbering is tested without a window.
#[must_use]
pub(super) fn facet_map_for_drawn_tiers(
    design: &Design,
    solved: &[SolvedTier],
    drawn: Option<&[bool]>,
) -> FacetMap {
    FacetMap::from_design_cut(design, solved, &[], drawn)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The map of a design cut back to its first tiers numbers fewer facets than the
    /// finished one, and a tier the cut hides has none.
    #[test]
    fn a_cut_back_map_counts_only_the_drawn_tiers_facets() {
        let design = Design::concave_fixture();
        let solved = design.solve().expect("the fixture solves");
        let finished = facet_map_for_drawn_tiers(&design, &solved, None);
        let mut drawn = vec![false; design.tiers.len()];
        drawn[0] = true;
        let cut = facet_map_for_drawn_tiers(&design, &solved, Some(&drawn));
        assert!(cut.facet_count() < finished.facet_count());
        assert_eq!(
            cut.facet_count(),
            cut.preform_plane_count() + cut.facets_of_tier(0).len()
        );
        assert!(
            cut.facets_of_tier(1).is_empty(),
            "a hidden tier has no facets"
        );
    }
}
