//! The angle, depth and index drag handles in the 2D Diagram view.
//!
//! The Diagram view draws three orthographic panels (crown, pavilion, profile) from the
//! stone's mesh, so a handle there is the same lever as in the 3D views, seen through a
//! different projection: [`panel_handles`] projects the facet's three tangent directions
//! with the panel's own projection ([`indicatrix_solid::diagram2d::project_point`]) and
//! fills the same [`HandleLayout`] the 3D code does. Everything downstream -- the
//! pixels-to-units drag mapping ([`drag_value`](super::drag_value)), snapping
//! ([`SnapMode`](super::SnapMode)), the edit intent queue and the one-undo-step gesture --
//! is shared with the 3D handles unchanged.
//!
//! # What each panel offers
//!
//! - **Crown and pavilion** (looking down or up the stone's axis): the facet's angle and
//!   depth handles both point along the radius through the facet, the angle handle with a
//!   length of `cos(angle)` and the depth handle `sin(angle)` of a unit lever, so a very
//!   steep facet (angle near 90 degrees) has no angle handle and a very flat one (near the
//!   table) has no depth handle: the projected lever would be shorter than
//!   [`MIN_FORESHORTENING`] of its true length. The tips are drawn at fixed fractions of
//!   the panel's wheel radius, the depth tip beyond the angle tip, so the two markers never
//!   sit on top of each other. The index handle is the tangent of the wheel at the facet;
//!   dragging it turns the tier about the panel centre ([`IndexRotation`]).
//! - **Profile** (the side elevation): only a facet seen edge-on, whose normal has
//!   `|z| <=` [`EDGE_ON_MAX_NORMAL_Z`], is a line there. Its angle handle runs along that
//!   line and its depth handle across it. There is no index wheel in the profile.
//!
//! # The index wheel and the reference angle
//!
//! The diagram's wheel and `FacetMap` put a facet at the azimuth `2 pi (index + reference) /
//! teeth`. [`facet_frame`] builds the facet frame with that azimuth (the 3D frame builder
//! takes the plain index), so the handles sit on the facet the panel drew.
//!
//! # Frozen mapping
//!
//! The diagram refits its scale on every replan. A drag keeps the [`HandleLayout`] it
//! started with (`DragStart::layout`) and, for the index handle, the centre of the panel at
//! the press ([`IndexRotation`]), so the mapping does not move under the pointer.

use super::{
    frame::FacetFrame,
    handles::{HandleKind, HandleLayout},
    projection::ScreenPoint,
};
use glam::Vec3;
use indicatrix_cut_core::ConstraintTier;
use indicatrix_solid::diagram2d::{DiagramLayout, PanelKind, PanelLayout, project_point};

/// A handle whose projected unit lever is shorter than this fraction of its length is not
/// offered.
///
/// Such a lever points (almost) along the view axis, so a pixel of drag would mean a huge
/// change of its quantity.
pub const MIN_FORESHORTENING: f32 = 0.15;

/// A facet counts as seen edge-on in the profile panel (a line, not an area) when the `z`
/// component of its unit normal is at most this.
pub const EDGE_ON_MAX_NORMAL_Z: f32 = 0.2;

/// The angle handle's tip, as a fraction of the panel's wheel radius, from the anchor.
const ANGLE_TIP_FRACTION: f32 = 0.32;

/// The depth handle's tip fraction on a crown or pavilion panel: beyond the angle tip.
const DEPTH_TIP_FRACTION: f32 = 0.62;

/// The index handle's tip fraction (along the wheel's tangent).
const INDEX_TIP_FRACTION: f32 = 0.32;

/// No tip is nearer than this many pixels to the anchor, however small the panel.
const MIN_TIP_PX: f32 = 26.0;

/// On a crown or pavilion panel the depth tip is at least this far beyond the angle tip.
const MIN_TIP_GAP_PX: f32 = 28.0;

/// The wheel radius used when a panel reports a degenerate one.
const MIN_WHEEL_RADIUS_PX: f32 = 40.0;

/// A pointer nearer than this to the panel centre has no usable azimuth.
const MIN_ROTATION_RADIUS_PX: f32 = 4.0;

/// Which handles a panel offers for a facet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HandleAvailability {
    /// The angle handle can be dragged here.
    pub angle: bool,
    /// The depth handle can be dragged here.
    pub depth: bool,
    /// The index handle can be dragged here (crown and pavilion panels only, and not for a
    /// tier without index-wheel positions).
    pub index: bool,
}

impl HandleAvailability {
    /// Whether `kind`'s handle is offered.
    #[must_use]
    pub const fn offers(self, kind: HandleKind) -> bool {
        match kind {
            HandleKind::Angle => self.angle,
            HandleKind::Depth => self.depth,
            HandleKind::Index => self.index,
        }
    }

    /// Whether any handle is offered.
    #[must_use]
    pub const fn any(self) -> bool {
        self.angle || self.depth || self.index
    }
}

/// The handles of one facet on one panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelHandles {
    /// The panel the handles sit on.
    pub panel: PanelKind,
    /// The handle layout in the frame's pixels. The tip of a handle the panel does not
    /// offer is NaN, so [`hit_test`](super::hit_test) never finds it.
    pub layout: HandleLayout,
    /// Which handles are offered.
    pub available: HandleAvailability,
    /// How the index handle turns the tier: `Some` on a crown or pavilion panel.
    pub rotation: Option<IndexRotation>,
}

/// The facet frame of `tier`'s facet at `index_on_gear`, with the azimuth the diagram's wheel
/// gives it.
///
/// That azimuth is `2 pi (index + reference) / teeth`, with the reference angle and tooth
/// count `layout` was drawn with. [`FacetFrame::from_tier`] takes the plain index; a design
/// with a gear reference angle would otherwise get its handles on a different azimuth than
/// its facets. This is [`FacetFrame::from_tier_with_reference`] with the layout's values,
/// which the 3D handles call with the design's.
#[must_use]
pub fn facet_frame(
    tier: &ConstraintTier,
    index_on_gear: f64,
    layout: &DiagramLayout,
    centroid: Vec3,
) -> FacetFrame {
    FacetFrame::from_tier_with_reference(
        tier,
        index_on_gear,
        layout.gear_reference_angle,
        layout.gear_teeth,
        centroid,
    )
}

/// The screen length of a vector.
fn length(v: (f32, f32)) -> f32 {
    v.0.hypot(v.1)
}

/// `v` as a unit vector, `fallback` when it has no length.
fn unit(v: (f32, f32), fallback: (f32, f32)) -> (f32, f32) {
    let len = length(v);
    if len > 1e-6 {
        (v.0 / len, v.1 / len)
    } else {
        fallback
    }
}

/// How the world vector `v` (a direction) projects on `panel` at `centroid`: the pixel
/// offset of `centroid + v` from `centroid`.
fn screen_vector(panel: &PanelLayout, centroid: Vec3, v: Vec3) -> (f32, f32) {
    let from = project_point(centroid.as_dvec3(), panel);
    let to = project_point((centroid + v).as_dvec3(), panel);
    (to.0 - from.0, to.1 - from.1)
}

/// Whether `panel` can carry handles for a facet with unit normal `normal`: the panel
/// draws the facet, or (profile) sees it edge-on.
fn panel_carries(panel: PanelKind, normal: Vec3) -> bool {
    match panel {
        PanelKind::Crown | PanelKind::Pavilion => panel.shows(normal),
        PanelKind::Profile => normal.z.abs() <= EDGE_ON_MAX_NORMAL_Z,
    }
}

/// The tip `distance` pixels from `anchor` along `dir`, NaN when the handle is not offered.
const fn tip_at(anchor: ScreenPoint, dir: (f32, f32), distance: f32, offered: bool) -> ScreenPoint {
    if offered {
        ScreenPoint::new(
            dir.0.mul_add(distance, anchor.x),
            dir.1.mul_add(distance, anchor.y),
        )
    } else {
        ScreenPoint::new(f32::NAN, f32::NAN)
    }
}

/// The handles of `frame`'s facet on `panel`, `None` when the panel does not carry the
/// facet at all (it is not drawn there, or not edge-on in the profile).
///
/// The result can still offer no handle ([`HandleAvailability::any`] is `false`): a facet
/// the panel draws but whose levers all point along the view axis.
#[must_use]
pub fn panel_handles(frame: &FacetFrame, panel: &PanelLayout) -> Option<PanelHandles> {
    if !panel_carries(panel.kind, frame.normal) {
        return None;
    }
    let centroid = frame.centroid;
    let scale = panel.scale;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let anchor_px = project_point(centroid.as_dvec3(), panel);
    let anchor = ScreenPoint::new(anchor_px.0, anchor_px.1);
    let angle_vec = screen_vector(panel, centroid, frame.tangent_theta());
    let depth_vec = screen_vector(panel, centroid, frame.normal);
    let index_vec = screen_vector(panel, centroid, frame.tangent_phi());
    let on_wheel = matches!(panel.kind, PanelKind::Crown | PanelKind::Pavilion);
    let available = HandleAvailability {
        angle: length(angle_vec) / scale >= MIN_FORESHORTENING,
        depth: length(depth_vec) / scale >= MIN_FORESHORTENING,
        index: on_wheel && !frame.is_indexless(),
    };
    let wheel = if panel.wheel_radius_px.is_finite() {
        panel.wheel_radius_px.max(MIN_WHEEL_RADIUS_PX)
    } else {
        MIN_WHEEL_RADIUS_PX
    };
    let angle_len = (ANGLE_TIP_FRACTION * wheel).max(MIN_TIP_PX);
    let index_len = (INDEX_TIP_FRACTION * wheel).max(MIN_TIP_PX);
    // Crown and pavilion: the angle and depth levers share the radial direction, so the
    // depth tip goes beyond the angle tip. The profile's two are perpendicular.
    let depth_len = if on_wheel {
        (DEPTH_TIP_FRACTION * wheel).max(angle_len + MIN_TIP_GAP_PX)
    } else {
        angle_len
    };
    let angle_dir = unit(angle_vec, (0.0, -1.0));
    let depth_dir = unit(depth_vec, (1.0, 0.0));
    let index_dir = unit(index_vec, (1.0, 0.0));
    let radius = centroid.x.hypot(centroid.z).max(1e-3);
    let tooth_arc = radius * std::f32::consts::TAU / frame.gear_teeth.max(1) as f32;
    let layout = HandleLayout {
        anchor,
        angle_tip: tip_at(anchor, angle_dir, angle_len, available.angle),
        depth_tip: tip_at(anchor, depth_dir, depth_len, available.depth),
        index_tip: tip_at(anchor, index_dir, index_len, available.index),
        angle_dir,
        depth_dir,
        index_dir,
        pixels_per_degree: length(angle_vec).to_radians(),
        pixels_per_mast_unit: length(depth_vec),
        pixels_per_tooth: length(index_vec) * tooth_arc,
    };
    let rotation = on_wheel.then_some(IndexRotation::new(
        ScreenPoint::new(panel.center_x, panel.center_y),
        panel.kind == PanelKind::Pavilion,
        frame.gear_teeth,
    ));
    Some(PanelHandles {
        panel: panel.kind,
        layout,
        available,
        rotation,
    })
}

/// The handles of `frame`'s facet on the first panel of `layout` that offers any.
///
/// The `preferred` panel (the one under the pointer) is tried first, then crown, pavilion and
/// profile. `None` when no panel does (the facet is not in the diagram, or every lever is
/// edge-on).
#[must_use]
pub fn place_handles(
    frame: &FacetFrame,
    layout: &DiagramLayout,
    preferred: Option<PanelKind>,
) -> Option<PanelHandles> {
    [
        preferred,
        Some(PanelKind::Crown),
        Some(PanelKind::Pavilion),
        Some(PanelKind::Profile),
    ]
    .into_iter()
    .flatten()
    .filter_map(|kind| layout.panel(kind))
    .find_map(|panel| panel_handles(frame, panel).filter(|handles| handles.available.any()))
}

/// The facets of `tier` to try as the handles' anchor, best first: the `remembered` facet
/// (the last one clicked) when it belongs to the tier, then the tier's own facets in order.
///
/// The Diagram view tries them in turn because a panel offers handles only for some facets
/// (the profile sees just the edge-on ones, a very steep facet has no angle handle on the
/// crown): the first candidate that carries any handle anchors them.
#[must_use]
pub fn candidate_facets(
    remembered: Option<u32>,
    tier: usize,
    facets_of_tier: &[u32],
    tier_of: impl Fn(u32) -> Option<usize>,
) -> Vec<u32> {
    let first = remembered.filter(|&id| tier_of(id) == Some(tier));
    first
        .into_iter()
        .chain(
            facets_of_tier
                .iter()
                .copied()
                .filter(|&id| Some(id) != first),
        )
        .collect()
}

/// The hit radius, in the frame's pixels, for a view zoomed by `zoom`.
///
/// The markers are drawn at a fixed size on screen, so a zoomed-in view grabs them within
/// the same screen distance, which is `1 / zoom` as far in the frame's own pixels.
#[must_use]
pub fn zoomed_hit_radius(radius_px: f32, zoom: f32) -> f32 {
    if zoom.is_finite() && zoom > 1.0 {
        radius_px / zoom
    } else {
        radius_px
    }
}

/// The index handle on a crown or pavilion panel: turning the tier about the panel centre.
///
/// The pointer's azimuth about the centre, relative to where the press was, is the turn of
/// the tier's facets about the stone's axis. One tooth is `2 pi / teeth` of it. The turn is
/// accumulated over consecutive pointer positions (each step unwrapped to at most half a
/// revolution), so a drag can go round the wheel more than once.
///
/// Crown panel: azimuth `phi` runs clockwise on screen, the same way the pointer's pixel
/// angle grows (pixel `y` points down). The pavilion panel is the mirror image: `phi` runs
/// counter-clockwise there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndexRotation {
    center: ScreenPoint,
    mirrored: bool,
    gear_teeth: u32,
    /// The pointer's pixel angle at the previous update, `None` until a usable one.
    last_angle: Option<f32>,
    /// The accumulated turn of the facets' azimuth, in radians.
    turn: f64,
}

impl IndexRotation {
    /// A rotation about `center` for a wheel of `gear_teeth` teeth; `mirrored` for the
    /// pavilion panel.
    #[must_use]
    pub const fn new(center: ScreenPoint, mirrored: bool, gear_teeth: u32) -> Self {
        Self {
            center,
            mirrored,
            gear_teeth,
            last_angle: None,
            turn: 0.0,
        }
    }

    /// The panel centre the pointer turns about.
    #[must_use]
    pub const fn center(&self) -> ScreenPoint {
        self.center
    }

    /// The pointer's pixel angle about the centre, `None` too close to the centre to tell.
    fn angle_of(&self, pointer: ScreenPoint) -> Option<f32> {
        let (dx, dy) = self.center.delta_to(pointer);
        (dx.hypot(dy) >= MIN_ROTATION_RADIUS_PX).then(|| dy.atan2(dx))
    }

    /// Starts the turn at `pointer` (the press): no turn yet.
    pub fn begin(&mut self, pointer: ScreenPoint) {
        self.turn = 0.0;
        self.last_angle = self.angle_of(pointer);
    }

    /// Follows the pointer to `pointer` and returns the turn so far in whole teeth,
    /// positive toward higher index numbers.
    ///
    /// A pointer too close to the centre to have an azimuth leaves the turn as it was.
    pub fn teeth_at(&mut self, pointer: ScreenPoint) -> i64 {
        if let Some(angle) = self.angle_of(pointer) {
            if let Some(last) = self.last_angle {
                let step = wrap_half_turn(angle - last);
                let sign: f64 = if self.mirrored { -1.0 } else { 1.0 };
                self.turn = sign.mul_add(f64::from(step), self.turn);
            }
            self.last_angle = Some(angle);
        }
        let teeth = f64::from(self.gear_teeth.max(1));
        (self.turn * teeth / std::f64::consts::TAU).round() as i64
    }
}

/// `delta` wrapped into `(-pi, pi]`.
fn wrap_half_turn(delta: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut wrapped = delta % tau;
    if wrapped > std::f32::consts::PI {
        wrapped -= tau;
    } else if wrapped <= -std::f32::consts::PI {
        wrapped += tau;
    }
    wrapped
}
