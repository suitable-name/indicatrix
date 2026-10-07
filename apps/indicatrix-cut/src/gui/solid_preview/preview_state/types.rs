//! The small, shared value types the rest of this module's request/response
//! plumbing is built from: the orbit camera pose, the facet-overlay update
//! payload, and the per-frame pick buffer (all three moved to
//! `indicatrix_solid::preview` and re-exported here at their old paths), plus the
//! desktop's own shared-state handles.

use indicatrix::geometry::meet_solver::SolvedTier;
use std::sync::{Arc, Mutex};

pub use indicatrix_solid::preview::{CameraPose, FacetOverlay, FrameGeometry, PickBuffer};

/// The shared cache backing [`super::request::ReplanRequest::last_solved`] across edits.
///
/// See the parent module's doc comment ("Where `last_solved` lives") for why a plain
/// `Arc<Mutex<..>>` rather than a field on `gui::editor::state::EditorState`.
/// Constructed once in `gui::mod::build_main_window`, shared between the sink
/// (writer) and `gui::editor`'s edit callbacks (reader).
///
/// stamped with the `gui::editor::state::EditorState::generation` value
/// the cached masts describe, alongside the masts themselves -- every writer
/// (`gui::SlintSolidSink::apply`, `gui::editor::auto_solve::apply::
/// apply_background_solve_result`, `gui::editor::view::viewport::
/// refresh_viewport`) previously overwrote this cache unconditionally, so a
/// slow in-flight solve for an OLDER design could finish and clobber a NEWER
/// design's already-cached masts. A reader that used to match the bare
/// `Option<Vec<SolvedTier>>` payload directly now needs the tuple's second
/// element instead, filtering on the first (the generation) itself when an
/// exact match matters to it -- see `gui::editor::native_io::solve::
/// cached_solve_matching` for the one caller outside `gui::editor::auto_solve`
/// that does exactly this.
pub type SolidLastSolved = Arc<Mutex<Option<(u64, Vec<SolvedTier>)>>>;

/// Which tier owns each facet id of one frame, flat and concave.
///
/// The two tables travel and are stored together, so a click never reads them from
/// different frames.
///
/// `flat[id]` is `FacetMap::tier_of` (unchanged: `None` for a tool facet), `concave[id]`
/// the index into `Design::concave_tiers` (`Some` only for a tool facet). The tier TABLE
/// lists the concave tiers after the flat ones, so a concave index `c` sits at row
/// `design.tiers.len() + c`; [`table_position_of`] does that conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FacetOwners {
    /// Facet id -> flat tier index.
    pub flat: Vec<Option<usize>>,
    /// Facet id -> concave tier index.
    pub concave: Vec<Option<usize>>,
}

impl FacetOwners {
    /// The tier-table row a click on `facet_id` selects, for a design that currently
    /// has `flat_count` flat and `concave_count` concave tiers; see [`table_position_of`].
    #[must_use]
    pub fn table_position(
        &self,
        facet_id: usize,
        flat_count: usize,
        concave_count: usize,
    ) -> Option<usize> {
        table_position_of(
            self.flat.get(facet_id).copied().flatten(),
            self.concave.get(facet_id).copied().flatten(),
            flat_count,
            concave_count,
        )
    }
}

/// The tier-table row a click on a facet selects. A flat owner wins and maps to its own
/// index; otherwise a concave owner `c` maps to row `flat_count + c`. A stale owner (an
/// index past the design's CURRENT tier count, because the design changed after the
/// frame was drawn) yields `None`, so a click never selects the wrong or a missing row.
#[must_use]
pub const fn table_position_of(
    flat: Option<usize>,
    concave: Option<usize>,
    flat_count: usize,
    concave_count: usize,
) -> Option<usize> {
    if let Some(tier) = flat {
        return Some(tier);
    }
    match concave {
        Some(tier) if tier < concave_count => Some(flat_count + tier),
        _ => None,
    }
}

/// The Solid viewport's shared pick-buffer/hover-text/facet-tier/geometry state.
///
/// The last rendered frame's own [`PickBuffer`], its per-facet hover strings, its
/// facet-id-to-tier table (all indexed by facet id) and its [`FrameGeometry`] --
/// written together by
/// `gui::SlintSolidSink::apply` as each frame lands. Bundled into one struct,
/// rather than three parameters threaded separately through
/// `gui::editor::setup_editor_callbacks`/`setup_editor_secondary_callbacks`, since
/// all three always travel together and are read together by the Solid
/// viewport's own hover/click callbacks
/// (`gui::editor::callbacks::tier_actions::setup_solid_facet_hover_callback`/
/// `setup_solid_facet_click_callback`).
pub struct SolidPickState {
    /// The last rendered frame's per-pixel facet-picking buffer.
    pub pick: Arc<Mutex<Option<PickBuffer>>>,
    /// The last rendered frame's per-facet hover strings, indexed by facet id.
    pub hover_text: Arc<Mutex<Vec<String>>>,
    /// The last rendered frame's facet-id-to-tier tables (flat and concave), indexed by
    /// facet id.
    pub facet_owners: Arc<Mutex<FacetOwners>>,
    /// The last rendered frame's mesh geometry, camera pose and raster size
    /// ([`FrameGeometry`]); stored in the same UI-thread closure that swaps `pick`,
    /// so it always belongs to the pick buffer above. `None` until a solid closed.
    pub geometry: Arc<Mutex<Option<FrameGeometry>>>,
}

#[cfg(test)]
mod tests {
    use super::{FacetOwners, table_position_of};

    #[test]
    fn a_flat_owner_keeps_its_own_index() {
        assert_eq!(table_position_of(Some(3), None, 8, 2), Some(3));
        // A flat owner wins even if a concave one were also listed.
        assert_eq!(table_position_of(Some(3), Some(0), 8, 2), Some(3));
    }

    #[test]
    fn a_concave_owner_sits_after_the_flat_rows() {
        assert_eq!(table_position_of(None, Some(0), 8, 2), Some(8));
        assert_eq!(table_position_of(None, Some(1), 8, 2), Some(9));
    }

    #[test]
    fn a_stale_concave_owner_selects_nothing() {
        // The design lost a concave tier after the frame was drawn.
        assert_eq!(table_position_of(None, Some(2), 8, 2), None);
        assert_eq!(table_position_of(None, Some(0), 8, 0), None);
        assert_eq!(table_position_of(None, None, 8, 2), None);
    }

    #[test]
    fn owners_resolve_a_facet_id_and_ignore_an_out_of_range_one() {
        let owners = FacetOwners {
            flat: vec![Some(0), Some(1), None],
            concave: vec![None, None, Some(1)],
        };
        assert_eq!(owners.table_position(1, 8, 2), Some(1));
        assert_eq!(owners.table_position(2, 8, 2), Some(9));
        assert_eq!(owners.table_position(3, 8, 2), None);
    }
}
