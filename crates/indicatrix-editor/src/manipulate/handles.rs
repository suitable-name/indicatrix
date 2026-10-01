//! The three drag handles of the selected facet -- angle, depth, index -- placed on the
//! pick frame, and hit-testing them.

use super::{
    frame::FacetFrame,
    projection::{ScreenPoint, ScreenSize, project},
};
use glam::Vec3;
use indicatrix::optics::raytracer::Camera;

/// How close (pixels) the pointer must be to a handle's tip to grab it.
pub const HANDLE_HIT_RADIUS_PX: f32 = 12.0;

/// The world-space step (mast units) `pixels_per_mast_unit` is measured over; the
/// measured distance is scaled by `1 / MAST_PROBE` = 10.
const MAST_PROBE: f32 = 0.1;

/// A tip closer than this (pixels) to the anchor has no usable screen direction.
const MIN_TIP_LENGTH_PX: f32 = 1e-3;

/// Which of the three handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandleKind {
    /// Tilts the whole tier: changes its angle.
    Angle,
    /// Moves the whole tier in or out along its normal: changes its mast.
    Depth,
    /// Turns the whole tier around the index wheel: changes its index positions.
    Index,
}

/// Where the three handles of one facet sit on screen, and how many pixels of pointer
/// travel along each handle's direction equal one unit of its quantity.
///
/// Every field is in the pick frame. The directions are unit screen vectors from
/// [`Self::anchor`] to the tip; a handle pointing straight along the view axis (its tip
/// on top of the anchor) gets a fixed fallback direction and near-zero pixels-per-unit,
/// which `drag_value` treats as "cannot be dragged here".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandleLayout {
    /// The facet centroid's pixel: the handles fan out from here.
    pub anchor: ScreenPoint,
    /// Tip of the angle handle (centroid + `tangent_theta` * length).
    pub angle_tip: ScreenPoint,
    /// Tip of the depth handle (centroid + normal * length).
    pub depth_tip: ScreenPoint,
    /// Tip of the index handle (centroid + `tangent_phi` * length).
    pub index_tip: ScreenPoint,
    /// Unit screen direction anchor to angle tip.
    pub angle_dir: (f32, f32),
    /// Unit screen direction anchor to depth tip.
    pub depth_dir: (f32, f32),
    /// Unit screen direction anchor to index tip.
    pub index_dir: (f32, f32),
    /// Pixels of travel along `angle_dir` per degree of angle.
    pub pixels_per_degree: f32,
    /// Pixels of travel along `depth_dir` per mast unit.
    pub pixels_per_mast_unit: f32,
    /// Pixels of travel along `index_dir` per index-wheel tooth.
    pub pixels_per_tooth: f32,
}

impl HandleLayout {
    /// The tip pixel of `kind`'s handle.
    #[must_use]
    pub const fn tip(&self, kind: HandleKind) -> ScreenPoint {
        match kind {
            HandleKind::Angle => self.angle_tip,
            HandleKind::Depth => self.depth_tip,
            HandleKind::Index => self.index_tip,
        }
    }

    /// The unit screen direction of `kind`'s handle.
    #[must_use]
    pub const fn dir(&self, kind: HandleKind) -> (f32, f32) {
        match kind {
            HandleKind::Angle => self.angle_dir,
            HandleKind::Depth => self.depth_dir,
            HandleKind::Index => self.index_dir,
        }
    }
}

/// The unit screen direction `anchor` to `tip`, or `fallback` when they coincide.
fn unit_dir(anchor: ScreenPoint, tip: ScreenPoint, fallback: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = anchor.delta_to(tip);
    let len = dx.hypot(dy);
    if len > MIN_TIP_LENGTH_PX {
        (dx / len, dy / len)
    } else {
        fallback
    }
}

/// Places `frame`'s three handles, each `handle_len_world` long in world space.
///
/// `angle_tip = project(centroid + tangent_theta * L)`, `depth_tip = project(centroid +
/// normal * L)`, `index_tip = project(centroid + tangent_phi * L)`.
///
/// - `pixels_per_degree` is `|angle_tip - anchor| / L * pi / 180`: the tip moves `L`
///   world units per radian of facet angle. (The shared contract wrote the formula
///   without the `/ L`; the two agree for `L = 1`, and dividing keeps a dragged tip
///   under the pointer for any handle length.)
/// - `pixels_per_mast_unit` is `10 * |project(centroid + normal * 0.1) - anchor|`.
/// - `pixels_per_tooth` is `|project(centroid + tangent_phi * r * 2 pi / gear) -
///   anchor|`, `r = hypot(centroid.x, centroid.z)` (at least `1e-3`).
///
/// `None` when the centroid or any probe point is behind the camera, the frame is not
/// positive, or `handle_len_world` is not a positive finite length.
#[must_use]
pub fn handle_layout(
    frame: &FacetFrame,
    camera: &Camera,
    size: ScreenSize,
    handle_len_world: f32,
) -> Option<HandleLayout> {
    if !handle_len_world.is_finite() || handle_len_world <= 0.0 {
        return None;
    }
    let len = handle_len_world;
    let centroid: Vec3 = frame.centroid;
    let anchor = project(camera, centroid, size)?;
    let angle_tip = project(camera, centroid + frame.tangent_theta() * len, size)?;
    let depth_tip = project(camera, centroid + frame.normal * len, size)?;
    let index_tip = project(camera, centroid + frame.tangent_phi() * len, size)?;
    let mast_probe = project(camera, centroid + frame.normal * MAST_PROBE, size)?;
    let radius = centroid.x.hypot(centroid.z).max(1e-3);
    let tooth_arc = radius * std::f32::consts::TAU / frame.gear_teeth.max(1) as f32;
    let tooth_probe = project(camera, centroid + frame.tangent_phi() * tooth_arc, size)?;
    Some(HandleLayout {
        anchor,
        angle_tip,
        depth_tip,
        index_tip,
        angle_dir: unit_dir(anchor, angle_tip, (0.0, -1.0)),
        depth_dir: unit_dir(anchor, depth_tip, (1.0, 0.0)),
        index_dir: unit_dir(anchor, index_tip, (1.0, 0.0)),
        pixels_per_degree: (anchor.distance(angle_tip) / len).to_radians(),
        pixels_per_mast_unit: 10.0 * anchor.distance(mast_probe),
        pixels_per_tooth: anchor.distance(tooth_probe),
    })
}

/// Which handle tip, if any, lies within `radius_px` of `p`; the nearest wins (the
/// angle handle, then depth, then index on an exact tie).
#[must_use]
pub fn hit_test(layout: &HandleLayout, p: ScreenPoint, radius_px: f32) -> Option<HandleKind> {
    [HandleKind::Angle, HandleKind::Depth, HandleKind::Index]
        .into_iter()
        .map(|kind| (kind, layout.tip(kind).distance(p)))
        .filter(|&(_, distance)| distance <= radius_px)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(kind, _)| kind)
}
