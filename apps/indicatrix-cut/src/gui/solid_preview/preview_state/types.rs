//! The small, shared value types the rest of this module's request/response
//! plumbing is built from: the orbit camera pose, the facet-overlay update
//! payload, and the per-frame pick buffer bundle.

use indicatrix::geometry::meet_solver::SolvedTier;
use std::sync::{Arc, Mutex};

/// The shared cache backing [`super::request::ReplanRequest::last_solved`] across edits.
///
/// See the parent module's doc comment ("Where `last_solved` lives") for why a plain
/// `Arc<Mutex<..>>` rather than a field on `gui::editor::state::EditorState`.
/// Constructed once in `gui::mod::build_main_window`, shared between the sink
/// (writer) and `gui::editor`'s edit callbacks (reader).
pub type SolidLastSolved = Arc<Mutex<Option<Vec<SolvedTier>>>>;

/// The shared `yaw`/`pitch`/`distance` orbit camera.
///
/// See `raster.rs`'s module doc comment for why the solid view must use exactly
/// `indicatrix::optics::raytracer::Camera::new(yaw, pitch, distance, 42.0)`, the
/// same call the path tracer uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

/// A facet-id-keyed highlight update.
///
/// Identifies one facet under the cursor, one facet a click resolved within
/// its tier, or lists every facet belonging to a multi-selected set of tiers.
///
/// Every field names facet ids directly rather than tier indices, so applying an
/// update never needs a `Design`/`facet_map::FacetMap` on the worker thread -- the
/// caller already resolved the id(s) it wants highlighted (from a pick buffer, or
/// from `FacetMap::facets_of_tier` for each multi-selected tier) before calling
/// [`super::SolidPreviewState::request_facet_overlay`]. `Default` (every field
/// empty/`None`) clears every highlight.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FacetOverlay {
    /// The one facet under the cursor, or `None` when nothing is hovered.
    pub hovered: Option<u32>,
    /// The one facet a click identified within its (possibly multi-facet) tier
    /// selection, or `None`.
    pub selected_facet: Option<u32>,
    /// Every facet id belonging to a multi-selected tier; empty when nothing is
    /// multi-selected.
    pub multi_selected: Vec<u32>,
}

/// A finished frame's per-pixel facet-picking buffer, alongside the size needed to
/// index it (`pick[y * width + x]`, mirroring `raster::SolidRasterizer::pick_at`).
///
/// Handed to [`super::PreviewSink::apply`] so a real implementation can stash it for
/// hover/click callbacks to read against the LAST rendered frame, without holding
/// the worker thread's own `SolidRasterizer` past this call.
#[derive(Debug, Clone)]
pub struct PickBuffer {
    pub width: u32,
    pub height: u32,
    /// `pub` (rather than a constructor) so `render_request`/`build_diagram_outputs`
    /// (in `super::render`, a sibling module) can build one directly from a
    /// rasterizer's or diagram frame's own pick buffer.
    pub pick: Vec<u32>,
}

impl PickBuffer {
    /// Same contract as [`raster::SolidRasterizer::pick_at`].
    #[must_use]
    pub fn facet_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let v = self.pick[(y * self.width + x) as usize];
        (v != 0).then(|| v - 1)
    }
}

/// The Solid viewport's shared pick-buffer/hover-text/facet-tier state.
///
/// The last rendered frame's own [`PickBuffer`], its per-facet hover strings, and
/// its facet-id-to-tier table -- all indexed by facet id and written together by
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
}
