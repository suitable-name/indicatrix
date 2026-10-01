//! The small, shared value types the frame pipeline is built from: the orbit
//! camera pose, the facet-overlay update payload, and the per-frame pick buffer.
//!
//! Moved verbatim from the desktop's `gui::solid_preview::preview_state::types`,
//! which re-exports them.

use glam::Vec3;
use std::sync::Arc;

/// The shared `yaw`/`pitch`/`distance` orbit camera.
///
/// See [`crate::raster`]'s module doc comment for why the solid view must use exactly
/// `indicatrix::optics::raytracer::Camera::new(yaw, pitch, distance, 42.0)`, the
/// same call the path tracer uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    /// Orbit yaw, radians.
    pub yaw: f32,
    /// Orbit pitch, radians.
    pub pitch: f32,
    /// Orbit distance, model units.
    pub distance: f32,
}

/// A facet-id-keyed highlight update.
///
/// Identifies one facet under the cursor, one facet a click resolved within
/// its tier, or lists every facet belonging to a multi-selected set of tiers.
///
/// Every field names facet ids directly rather than tier indices, so applying an
/// update never needs a `Design`/[`crate::facet_map::FacetMap`] -- the caller
/// already resolved the id(s) it wants highlighted (from a pick buffer, or from
/// `FacetMap::facets_of_tier` for each multi-selected tier). `Default` (every
/// field empty/`None`) clears every highlight.
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
    /// Every facet id of the provisional (not yet committed) slice tier, outlined
    /// green and drawn above every other highlight; empty when there is none.
    pub provisional: Vec<u32>,
    /// Every facet id of a tier whose mast moved because of the drag in progress,
    /// outlined orange; empty when nothing is being dragged.
    pub moved: Vec<u32>,
}

/// The geometry of the mesh one frame was drawn from, in the frame's own pick
/// coordinates -- what the direct-manipulation handles are placed and projected with.
///
/// `corner_points` and `facet_centroids` are built once per mesh
/// ([`crate::mesh_cache::CachedMesh`]) and shared by `Arc`, never rebuilt per frame;
/// `camera` and `size` are exactly the pose and pixel size the raster used, so a
/// point projected with `Camera::new(yaw, pitch, distance, 42.0)` at `size` lands on
/// the same pixel the pick buffer holds.
#[derive(Debug, Clone)]
pub struct FrameGeometry {
    /// Every distinct simplified-ring corner of the shown mesh (deduped within 1e-6).
    pub corner_points: Arc<Vec<Vec3>>,
    /// Facet id -> mean of that facet's simplified ring, `None` for a facet with no
    /// ring; one entry per plane the mesh was built from.
    pub facet_centroids: Arc<Vec<Option<Vec3>>>,
    /// The shown mesh's bounding radius (the farthest vertex from the origin).
    pub bounding_radius: f64,
    /// The orbit pose the raster used for this frame.
    pub camera: CameraPose,
    /// The raster (= pick buffer) size in pixels the frame was drawn at.
    pub size: (u32, u32),
}

/// A finished frame's per-pixel facet-picking buffer, alongside the size needed to
/// index it (`pick[y * width + x]`, mirroring
/// [`crate::raster::SolidRasterizer::pick_at`]).
///
/// Kept by a caller so hover/click can be resolved against the LAST rendered
/// frame without holding the rasterizer itself.
#[derive(Debug, Clone)]
pub struct PickBuffer {
    /// Buffer width in pixels.
    pub width: u32,
    /// Buffer height in pixels.
    pub height: u32,
    /// `facet_id + 1` per pixel, `0` for background.
    pub pick: Vec<u32>,
}

impl PickBuffer {
    /// Same contract as [`crate::raster::SolidRasterizer::pick_at`].
    #[must_use]
    pub fn facet_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let v = self.pick[(y * self.width + x) as usize];
        (v != 0).then(|| v - 1)
    }
}
