//! Where the three handles go: which facet carries them, the alignment checks that hide
//! them rather than land them on the wrong facet, and the hit-test that respects a tier
//! with no index handle.
//!
//! Shared by the desktop and the web app; each supplies its own frame (the desktop the
//! worker's `FrameGeometry`, the web the main-thread pipeline's).

use super::{
    frame::FacetFrame,
    handles::{HandleKind, HandleLayout, handle_layout, hit_test},
    projection::{ScreenPoint, ScreenSize},
};
use indicatrix::optics::raytracer::Camera;
use indicatrix_cut_core::Design;
use indicatrix_solid::{facet_map::FacetMap, preview::FrameGeometry};

/// The field of view the solid raster and the path tracer share.
pub const CAMERA_FOV_DEG: f32 = indicatrix::optics::raytracer::DEFAULT_FOV_DEG;

/// How long each handle is, as a fraction of the shown stone's bounding radius.
pub const HANDLE_LENGTH_FRACTION: f64 = 0.35;

/// What the handles are currently drawn on: the selected tier, the facet whose centroid
/// anchors them, and their layout in the pick frame of the frame they were computed
/// from.
#[derive(Debug, Clone)]
pub struct HandleTarget {
    /// The tier the handles edit.
    pub tier: usize,
    /// The anchor facet's local frame.
    pub frame: FacetFrame,
    /// The handles' pick-frame layout.
    pub layout: HandleLayout,
    /// The tier's name for hints and toasts.
    pub label: String,
    /// Whether `tier` is the provisional slice tier (an index one past the committed
    /// design's last tier) rather than a tier of the edited design.
    pub provisional: bool,
}

impl HandleTarget {
    /// The handle whose tip lies within `radius` of `p` ([`hit_kind`] on this target).
    #[must_use]
    pub fn hit(&self, p: ScreenPoint, radius: f32) -> Option<HandleKind> {
        hit_kind(&self.layout, self.frame.is_indexless(), p, radius)
    }
}

/// The facet the handles anchor on: the last clicked facet when it belongs to `tier`
/// and has a centroid, else the tier's first facet that has one (a selection made from
/// the tier table names no facet).
#[must_use]
pub fn pick_facet(
    remembered: Option<u32>,
    tier: usize,
    facets_of_tier: &[u32],
    tier_of: impl Fn(u32) -> Option<usize>,
    has_centroid: impl Fn(u32) -> bool,
) -> Option<u32> {
    remembered
        .filter(|&id| tier_of(id) == Some(tier) && has_centroid(id))
        .or_else(|| facets_of_tier.iter().copied().find(|&id| has_centroid(id)))
}

/// Whether the solved masts describe the design's tiers one to one -- a stale cache
/// (a tier was just added or removed) would put every facet id on the wrong tier.
#[must_use]
pub const fn masts_aligned(tier_count: usize, masts_len: usize) -> bool {
    tier_count == masts_len
}

/// Whether the facet map and the frame's geometry share one facet-id space.
///
/// They differ while a frame is behind the design, and under the Cut slider (the mesh
/// then holds only the first tiers' planes); the handles hide rather than land on the
/// wrong facet.
#[must_use]
pub const fn ids_aligned(map_facets: usize, centroid_count: usize) -> bool {
    map_facets == centroid_count
}

/// The handle whose tip lies within `radius` of `p`. A tier with no index-wheel
/// positions has no index handle to grab.
#[must_use]
pub fn hit_kind(
    layout: &HandleLayout,
    indexless: bool,
    p: ScreenPoint,
    radius: f32,
) -> Option<HandleKind> {
    let mut layout = *layout;
    if indexless {
        // A NaN tip is never within any radius.
        layout.index_tip = ScreenPoint::new(f32::NAN, f32::NAN);
    }
    hit_test(&layout, p, radius)
}

/// The centroid of `facet_id` in `centroids`, if it has one.
fn centroid_of(centroids: &[Option<glam::Vec3>], facet_id: u32) -> Option<glam::Vec3> {
    centroids.get(facet_id as usize).copied().flatten()
}

/// The handle target on `tier` of `design`, whose facets `map` describes.
///
/// `geometry` is the frame on screen. The anchor is the `remembered` facet when it
/// belongs to the tier, else the tier's first facet with a centroid. `None` when the
/// map and the frame hold different facet-id spaces, no facet of the tier is on screen,
/// or a handle would land behind the camera.
#[must_use]
pub fn target_for(
    geometry: &FrameGeometry,
    map: &FacetMap,
    design: &Design,
    tier: usize,
    remembered: Option<u32>,
    provisional: bool,
) -> Option<HandleTarget> {
    let tier_data = design.tiers.get(tier)?;
    let centroids = geometry.facet_centroids.as_slice();
    if !ids_aligned(map.facet_count(), centroids.len()) {
        return None;
    }
    let facet_id = pick_facet(
        remembered,
        tier,
        map.facets_of_tier(tier),
        |id| map.tier_of(id as usize),
        |id| centroid_of(centroids, id).is_some(),
    )?;
    let centroid = centroid_of(centroids, facet_id)?;
    let frame = FacetFrame::from_tier(
        tier_data,
        f64::from(map.index_on_gear(facet_id as usize)),
        design.meta.gear_teeth_abs(),
        centroid,
    );
    let pose = geometry.camera;
    let camera = Camera::new(pose.yaw, pose.pitch, pose.distance, CAMERA_FOV_DEG);
    let size = ScreenSize::new(geometry.size.0 as f32, geometry.size.1 as f32);
    let length = (HANDLE_LENGTH_FRACTION * geometry.bounding_radius) as f32;
    let layout = handle_layout(&frame, &camera, size, length)?;
    Some(HandleTarget {
        tier,
        frame,
        layout,
        label: super::gesture::tier_label(tier_data, tier),
        provisional,
    })
}
