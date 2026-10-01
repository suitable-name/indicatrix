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
    /// The last rendered frame's facet-id-to-tier table, indexed by facet id.
    pub facet_tier: Arc<Mutex<Vec<Option<usize>>>>,
    /// The last rendered frame's mesh geometry, camera pose and raster size
    /// ([`FrameGeometry`]); stored in the same UI-thread closure that swaps `pick`,
    /// so it always belongs to the pick buffer above. `None` until a solid closed.
    pub geometry: Arc<Mutex<Option<FrameGeometry>>>,
}
