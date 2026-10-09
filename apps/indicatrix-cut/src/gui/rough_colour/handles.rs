//! The 3D handles of a colour zone in the Rough Planner's rough view (`zoning` feature only).
//!
//! Window-free: this module knows zones, a camera and pixels, nothing about Slint. The planner's
//! view (`gui::rough_plan::zoning_hooks`) draws what [`overlay`] returns, hit-tests with
//! [`pick_handle`] and turns a drag into a zone edit with [`drag_edit`]; the wizard
//! (`wizard::actions`) applies that edit with `apply_with_locks`, so a handle drag is exactly a
//! sequence of `ZoneEdit`s and a locked parameter has no handle.
//!
//! # Handles of the selected zone
//!
//! | shape | handles |
//! |---|---|
//! | half space | offset (at the plane's foot point, moves along the normal), normal arrow tip |
//! | slab | lower and upper offset (along the normal), normal arrow tip |
//! | cylinder, prism | axis point, axis arrow tip, outer radius (and inner radius when it is not 0) |
//! | sector | axis point, axis arrow tip, start angle and end angle |
//! | mesh shell | none |
//!
//! The arrow tip is dragged freely in the plane facing the camera and sets the direction to
//! "foot to tip"; the axis point is dragged the same way; offsets and radii move along a fixed
//! 3D line and the pointer's travel is projected on that line's picture; an angle moves along
//! the tangent of its circle, one radian per radius of travel. The gesture is relative to where
//! it started, so the mapping from pixels to value is fixed for the whole drag.
//!
//! This repeats the pattern of the Solid viewport's handles (`gui::editor::manipulate`: a hit
//! radius around each handle, a gesture that stays relative to its start, one undo step per
//! gesture) but not their code, which is bound to tiers and the solid preview.

use glam::{DVec3, Vec3};
use indicatrix::optics::{
    raytracer::Camera,
    zoning::{ZoneShape, ZonedAbsorption},
};
use indicatrix_cut_core::rough_plan::colour_fit::zones::{ZoneEdit, ZoneLocks, ZoneParameter};
use indicatrix_solid::raster::project_point;

/// How close, in logical pixels, the pointer must be to a handle to grab it.
pub const HIT_RADIUS_PX: f32 = 9.0;

/// The radius of a drawn handle marker, in logical pixels.
pub const MARKER_RADIUS_PX: f32 = 5.0;

/// The length of an arrow, as a share of the rough's extent.
const ARROW_FRACTION: f64 = 0.3;

/// The radius at which the angle handles of a sector sit, as a share of the rough's extent.
const SECTOR_RADIUS_FRACTION: f64 = 0.4;

/// How far, in world units, a probe moves to measure pixels per millimetre.
const PROBE_WORLD: f64 = 0.05;

/// What a handle changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    /// The tip of the axis (or normal) arrow.
    Axis,
    /// The axis point of a cylinder, prism or sector.
    AxisPoint,
    /// The offset of a half space.
    Offset,
    /// The lower offset of a slab.
    OffsetMin,
    /// The upper offset of a slab.
    OffsetMax,
    /// The inner radius.
    InnerRadius,
    /// The outer radius.
    OuterRadius,
    /// The start angle of a sector.
    AngleFrom,
    /// The end angle of a sector.
    AngleTo,
}

impl HandleKind {
    /// What the hint strip says while the pointer is over (or drags) the handle.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Axis => "Drag to turn the zone's axis or normal",
            Self::AxisPoint => "Drag to move the zone's axis",
            Self::Offset => "Drag to move the zone's plane",
            Self::OffsetMin => "Drag to move the zone's lower plane",
            Self::OffsetMax => "Drag to move the zone's upper plane",
            Self::InnerRadius => "Drag to change the zone's inner radius",
            Self::OuterRadius => "Drag to change the zone's outer radius",
            Self::AngleFrom => "Drag to turn the start of the wedge",
            Self::AngleTo => "Drag to turn the end of the wedge",
        }
    }
}

/// How dragging a handle changes the zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drag {
    /// The handle moves along `axis` (a unit vector, mm frame); `parameter` changes by `per_mm`
    /// for every millimetre of travel along it.
    Along {
        /// The line the handle moves on.
        axis: DVec3,
        /// The parameter it sets.
        parameter: ZoneParameter,
        /// The parameter's value when the drag starts.
        start: f64,
        /// Parameter units per millimetre of travel.
        per_mm: f64,
    },
    /// The handle moves in the plane facing the camera: the zone's axis point.
    Point,
    /// The tip of an arrow whose foot stays at `foot` moves in the plane facing the camera; the
    /// direction becomes foot to tip.
    Tip {
        /// The foot of the arrow, mm.
        foot: DVec3,
    },
}

/// One handle of the selected zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    /// The zone it belongs to (1-based).
    pub zone: usize,
    /// What it is.
    pub kind: HandleKind,
    /// Where it is, in the rough frame, mm.
    pub position: DVec3,
    /// How a drag changes the zone.
    pub drag: Drag,
}

/// The reference direction `u` and the second direction `v = axis x u` of an axis, as the zone
/// kernels define them.
///
/// `u` is the world X axis (Y when the axis is nearly along X) with its component along the axis
/// removed, normalised.
#[must_use]
pub fn axis_reference(axis: DVec3) -> (DVec3, DVec3) {
    let seed = if axis.x.abs() > 0.9 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let u = (seed - axis * seed.dot(axis)).normalize_or_zero();
    (u, axis.cross(u))
}

/// The handles of zone `zone` (1-based) of `zoned`, for a rough `extent_mm` across. A parameter
/// that is locked has no handle; an unknown zone has none.
#[must_use]
pub fn handles_for(
    zoned: &ZonedAbsorption,
    locks: &ZoneLocks,
    zone: usize,
    extent_mm: f64,
) -> Vec<Handle> {
    let Some(entry) = zone.checked_sub(1).and_then(|i| zoned.zones.get(i)) else {
        return Vec::new();
    };
    let extent = if extent_mm.is_finite() && extent_mm > 0.0 {
        extent_mm
    } else {
        10.0
    };
    let arrow = ARROW_FRACTION * extent;
    let free = |parameter: ZoneParameter| !locks.is_locked(zone, parameter);
    let mut out = Vec::new();
    let mut push = |kind: HandleKind, position: DVec3, drag: Drag| {
        out.push(Handle {
            zone,
            kind,
            position,
            drag,
        });
    };
    match &entry.shape {
        ZoneShape::HalfSpace { normal, offset } => {
            half_space_handles(&mut push, &free, *normal, *offset, arrow);
        }
        ZoneShape::Slab {
            normal,
            offset_min,
            offset_max,
        } => {
            slab_handles(&mut push, &free, *normal, (*offset_min, *offset_max), arrow);
        }
        ZoneShape::CoaxialCylinder {
            axis_point,
            axis_dir,
            r_in,
            r_out,
        } => {
            axis_handles(&mut push, &free, *axis_point, *axis_dir, arrow);
            let (u, _) = axis_reference(*axis_dir);
            radius_handles(&mut push, &free, *axis_point, u, *r_in, *r_out);
        }
        ZoneShape::CoaxialPrism {
            axis_point,
            axis_dir,
            r_in,
            r_out,
            phase,
            ..
        } => {
            axis_handles(&mut push, &free, *axis_point, *axis_dir, arrow);
            let (u, v) = axis_reference(*axis_dir);
            let side = u * phase.cos() + v * phase.sin();
            radius_handles(&mut push, &free, *axis_point, side, *r_in, *r_out);
        }
        ZoneShape::Sector {
            axis_point,
            axis_dir,
            angle_from,
            angle_to,
        } => {
            axis_handles(&mut push, &free, *axis_point, *axis_dir, arrow);
            let radius = SECTOR_RADIUS_FRACTION * extent;
            let edges = (*angle_from, *angle_to);
            angle_handles(&mut push, &free, (*axis_point, *axis_dir), edges, radius);
        }
        ZoneShape::MeshShell { .. } => {}
    }
    out
}

/// The offset handle and the normal arrow of a half space.
fn half_space_handles(
    push: &mut impl FnMut(HandleKind, DVec3, Drag),
    free: &impl Fn(ZoneParameter) -> bool,
    normal: DVec3,
    offset: f64,
    arrow: f64,
) {
    let foot = normal * offset;
    if free(ZoneParameter::Offset) {
        push(
            HandleKind::Offset,
            foot,
            Drag::Along {
                axis: normal,
                parameter: ZoneParameter::Offset,
                start: offset,
                per_mm: 1.0,
            },
        );
    }
    if free(ZoneParameter::Direction) {
        push(HandleKind::Axis, foot + normal * arrow, Drag::Tip { foot });
    }
}

/// The two offset handles and the normal arrow of a slab (`offsets` is `(min, max)`).
fn slab_handles(
    push: &mut impl FnMut(HandleKind, DVec3, Drag),
    free: &impl Fn(ZoneParameter) -> bool,
    normal: DVec3,
    offsets: (f64, f64),
    arrow: f64,
) {
    let (offset_min, offset_max) = offsets;
    if free(ZoneParameter::OffsetMin) {
        push(
            HandleKind::OffsetMin,
            normal * offset_min,
            Drag::Along {
                axis: normal,
                parameter: ZoneParameter::OffsetMin,
                start: offset_min,
                per_mm: 1.0,
            },
        );
    }
    if free(ZoneParameter::OffsetMax) {
        push(
            HandleKind::OffsetMax,
            normal * offset_max,
            Drag::Along {
                axis: normal,
                parameter: ZoneParameter::OffsetMax,
                start: offset_max,
                per_mm: 1.0,
            },
        );
    }
    if free(ZoneParameter::Direction) {
        let foot = normal * offset_min.midpoint(offset_max);
        push(HandleKind::Axis, foot + normal * arrow, Drag::Tip { foot });
    }
}

/// The two edge handles of a sector (`axis` is `(axis_point, axis_dir)`, `edges` the angles
/// `(from, to)`), `radius` mm from the axis point.
fn angle_handles(
    push: &mut impl FnMut(HandleKind, DVec3, Drag),
    free: &impl Fn(ZoneParameter) -> bool,
    axis: (DVec3, DVec3),
    edges: (f64, f64),
    radius: f64,
) {
    let (axis_point, axis_dir) = axis;
    let (u, v) = axis_reference(axis_dir);
    for (kind, parameter, angle) in [
        (HandleKind::AngleFrom, ZoneParameter::AngleFrom, edges.0),
        (HandleKind::AngleTo, ZoneParameter::AngleTo, edges.1),
    ] {
        if !free(parameter) {
            continue;
        }
        let radial = u * angle.cos() + v * angle.sin();
        let tangent = v * angle.cos() - u * angle.sin();
        push(
            kind,
            axis_point + radial * radius,
            Drag::Along {
                axis: tangent,
                parameter,
                start: angle,
                per_mm: 1.0 / radius,
            },
        );
    }
}

/// The axis point and the axis arrow of a cylinder, prism or sector.
fn axis_handles(
    push: &mut impl FnMut(HandleKind, DVec3, Drag),
    free: &impl Fn(ZoneParameter) -> bool,
    axis_point: DVec3,
    axis_dir: DVec3,
    arrow: f64,
) {
    if free(ZoneParameter::AxisPoint) {
        push(HandleKind::AxisPoint, axis_point, Drag::Point);
    }
    if free(ZoneParameter::Direction) {
        push(
            HandleKind::Axis,
            axis_point + axis_dir * arrow,
            Drag::Tip { foot: axis_point },
        );
    }
}

/// The radius handles of a cylinder or prism: `side` is the unit direction they move along.
fn radius_handles(
    push: &mut impl FnMut(HandleKind, DVec3, Drag),
    free: &impl Fn(ZoneParameter) -> bool,
    axis_point: DVec3,
    side: DVec3,
    r_in: f64,
    r_out: f64,
) {
    if free(ZoneParameter::ROut) {
        push(
            HandleKind::OuterRadius,
            axis_point + side * r_out,
            Drag::Along {
                axis: side,
                parameter: ZoneParameter::ROut,
                start: r_out,
                per_mm: 1.0,
            },
        );
    }
    // At 0 the inner handle would sit on the axis point; the number panel sets a first inner
    // radius.
    if r_in > 0.0 && free(ZoneParameter::RIn) {
        push(
            HandleKind::InnerRadius,
            axis_point + side * r_in,
            Drag::Along {
                axis: side,
                parameter: ZoneParameter::RIn,
                start: r_in,
                per_mm: 1.0,
            },
        );
    }
}

/// The rough view's camera and size, and the map from the rough frame (mm) to the view's world
/// units: `world = (mm - centre) * scale`.
#[derive(Clone, Copy)]
pub struct ViewMap<'a> {
    /// The camera of the frame.
    pub camera: &'a Camera,
    /// The size the screen coordinates are in (pixels of the frame or logical pixels: whatever
    /// the caller's cursor is in).
    pub size: (u32, u32),
    /// The centre of the rough frame that is the world origin, mm.
    pub centre: DVec3,
    /// World units per millimetre.
    pub scale: f64,
}

impl ViewMap<'_> {
    /// A point of the rough frame in world units.
    #[must_use]
    pub fn world(&self, mm: DVec3) -> Vec3 {
        ((mm - self.centre) * self.scale).as_vec3()
    }

    /// The screen position of a point of the rough frame, `None` behind the camera.
    #[must_use]
    pub fn screen(&self, mm: DVec3) -> Option<(f32, f32)> {
        project_point(self.camera, self.world(mm), self.size.0, self.size.1).map(|p| (p.0, p.1))
    }

    /// The same map for a different frame size (a camera ray is the same in any size).
    #[must_use]
    pub const fn with_size(self, size: (u32, u32)) -> Self {
        Self { size, ..self }
    }

    /// Millimetres of the rough frame for one world unit.
    fn probe_mm(&self) -> f64 {
        PROBE_WORLD / self.scale
    }

    /// The camera's right and up directions as unit vectors (the frame is not rotated or
    /// scaled differently per axis, so directions are the same in mm and in world units).
    fn right_up(&self) -> (DVec3, DVec3) {
        (self.camera.right.as_dvec3(), self.camera.up.as_dvec3())
    }
}

/// The handle nearest to `cursor` within `radius` pixels (the first of equals), as an index into
/// `handles`.
#[must_use]
pub fn pick_handle(
    view: &ViewMap<'_>,
    handles: &[Handle],
    cursor: (f32, f32),
    radius: f32,
) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (index, handle) in handles.iter().enumerate() {
        let Some(at) = view.screen(handle.position) else {
            continue;
        };
        let distance = (at.0 - cursor.0).hypot(at.1 - cursor.1);
        if distance <= radius && best.is_none_or(|(nearest, _)| distance < nearest) {
            best = Some((distance, index));
        }
    }
    best.map(|(_, index)| index)
}

/// Pixels on the screen per millimetre along the direction `axis` at `at` (a vector in pixels),
/// or `None` when the point or its probe is behind the camera.
fn pixels_per_mm(view: &ViewMap<'_>, at: DVec3, axis: DVec3) -> Option<(f64, f64)> {
    let probe = view.probe_mm();
    let a = view.screen(at)?;
    let b = view.screen(at + axis * probe)?;
    Some((f64::from(b.0 - a.0) / probe, f64::from(b.1 - a.1) / probe))
}

/// How far, in millimetres, a cursor travel of `(dx, dy)` pixels moves a point on the line
/// through `at` along `axis`.
fn travel_along(view: &ViewMap<'_>, at: DVec3, axis: DVec3, dx: f64, dy: f64) -> Option<f64> {
    let (sx, sy) = pixels_per_mm(view, at, axis)?;
    let length_squared = sx.mul_add(sx, sy * sy);
    if length_squared < 1e-12 {
        return None;
    }
    Some(dx.mul_add(sx, dy * sy) / length_squared)
}

/// How far, in the rough frame, a cursor travel of `(dx, dy)` pixels moves a point at `at` in the
/// plane facing the camera.
fn travel_in_plane(view: &ViewMap<'_>, at: DVec3, dx: f64, dy: f64) -> Option<DVec3> {
    let (right, up) = view.right_up();
    let (rx, ry) = pixels_per_mm(view, at, right)?;
    let pixels_per_mm = rx.hypot(ry);
    if pixels_per_mm < 1e-9 {
        return None;
    }
    // Screen y grows downwards, the camera's up vector upwards.
    Some(right * (dx / pixels_per_mm) - up * (dy / pixels_per_mm))
}

/// The zone edit that dragging `handle` from `start` to `cursor` (both in the pixels of `view`)
/// asks for.
///
/// `None` when the mapping is degenerate (the handle is behind the camera, or an
/// axis points at it). `start` is where the pointer went down; the edit is always relative to the
/// handle's position at that time, so applying it to the zoning from before the drag gives the
/// zoning under the pointer.
#[must_use]
pub fn drag_edit(
    view: &ViewMap<'_>,
    handle: &Handle,
    start: (f32, f32),
    cursor: (f32, f32),
) -> Option<ZoneEdit> {
    let dx = f64::from(cursor.0 - start.0);
    let dy = f64::from(cursor.1 - start.1);
    match handle.drag {
        Drag::Along {
            axis,
            parameter,
            start: value,
            per_mm,
        } => {
            let moved = travel_along(view, handle.position, axis, dx, dy)?;
            let mut next = moved.mul_add(per_mm, value);
            if matches!(parameter, ZoneParameter::RIn | ZoneParameter::ROut) {
                next = next.max(0.0);
            }
            Some(ZoneEdit::SetParameter {
                zone: handle.zone,
                parameter,
                value: next,
            })
        }
        Drag::Point => {
            let moved = travel_in_plane(view, handle.position, dx, dy)?;
            Some(ZoneEdit::SetAxisPoint {
                zone: handle.zone,
                point: handle.position + moved,
            })
        }
        Drag::Tip { foot } => {
            let moved = travel_in_plane(view, handle.position, dx, dy)?;
            let direction = handle.position + moved - foot;
            (direction.length() > 1e-9).then_some(ZoneEdit::SetDirection {
                zone: handle.zone,
                direction,
            })
        }
    }
}

// --- Drawing ---------------------------------------------------------------------------------

/// A line segment of the overlay, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Start.
    pub from: (f32, f32),
    /// End.
    pub to: (f32, f32),
}

/// A handle marker of the overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    /// Centre, pixels.
    pub at: (f32, f32),
    /// Whether the pointer is over it or drags it.
    pub lit: bool,
}

/// What to draw for a set of handles: arrow shafts from the foot to the tip, and a marker per
/// handle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    /// The shafts.
    pub segments: Vec<Segment>,
    /// The markers.
    pub markers: Vec<Marker>,
}

/// The overlay of `handles` seen through `view`; `lit` is the index of the handle the pointer is
/// over or drags.
#[must_use]
pub fn overlay(view: &ViewMap<'_>, handles: &[Handle], lit: Option<usize>) -> Overlay {
    let mut out = Overlay::default();
    for (index, handle) in handles.iter().enumerate() {
        let Some(at) = view.screen(handle.position) else {
            continue;
        };
        if let Drag::Tip { foot } = handle.drag
            && let Some(from) = view.screen(foot)
        {
            out.segments.push(Segment { from, to: at });
        }
        out.markers.push(Marker {
            at,
            lit: lit == Some(index),
        });
    }
    out
}

/// Draws `overlay` into an RGBA8 buffer of `width` x `height` pixels in `colour`. `scale` is the
/// buffer's pixels per overlay pixel (the overlay is in logical pixels, the frame in physical
/// ones).
///
/// Pixels outside the buffer are skipped.
pub fn draw_overlay(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    overlay: &Overlay,
    colour: [u8; 3],
    scale: f32,
) {
    let mut put = |x: i64, y: i64, rgb: [u8; 3]| {
        if x < 0 || y < 0 || x >= i64::from(width) || y >= i64::from(height) {
            return;
        }
        let at = (y as usize * width as usize + x as usize) * 4;
        if let Some(pixel) = rgba.get_mut(at..at + 4) {
            pixel[..3].copy_from_slice(&rgb);
            pixel[3] = 0xff;
        }
    };
    for segment in &overlay.segments {
        let (from, to) = (scaled(segment.from, scale), scaled(segment.to, scale));
        let steps = (to.0 - from.0)
            .abs()
            .max((to.1 - from.1).abs())
            .ceil()
            .max(1.0) as i64;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let x = (to.0 - from.0).mul_add(t, from.0).round() as i64;
            let y = (to.1 - from.1).mul_add(t, from.1).round() as i64;
            put(x, y, colour);
        }
    }
    let radius = MARKER_RADIUS_PX * scale;
    for marker in &overlay.markers {
        let centre = scaled(marker.at, scale);
        let fill = if marker.lit { [255, 255, 255] } else { colour };
        let reach = radius.ceil() as i64 + 1;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let distance = (dx as f32).hypot(dy as f32);
                let x = centre.0.round() as i64 + dx;
                let y = centre.1.round() as i64 + dy;
                if distance <= radius - 1.0 {
                    put(x, y, fill);
                } else if distance <= radius {
                    // A dark ring so a marker shows on a light picture too.
                    put(x, y, [16, 18, 24]);
                }
            }
        }
    }
}

fn scaled(point: (f32, f32), scale: f32) -> (f32, f32) {
    (point.0 * scale, point.1 * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::optics::{
        absorption::AbsorptionTensor,
        zoning::{Zone, ZoneAbsorption},
    };
    use indicatrix_cut_core::rough_plan::colour_fit::zones::{apply, apply_with_locks};

    const SIZE: (u32, u32) = (800, 600);

    fn clear() -> ZoneAbsorption {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()))
    }

    fn zoned_with(shape: ZoneShape) -> ZonedAbsorption {
        let mut zoned = ZonedAbsorption::new(clear());
        zoned.zones.push(Zone {
            shape,
            absorption: clear(),
        });
        zoned
    }

    fn view(camera: &Camera) -> ViewMap<'_> {
        ViewMap {
            camera,
            size: SIZE,
            centre: DVec3::ZERO,
            scale: 0.1,
        }
    }

    fn camera() -> Camera {
        Camera::new(0.6, 0.45, 3.0, 42.0)
    }

    fn cylinder() -> ZonedAbsorption {
        zoned_with(ZoneShape::CoaxialCylinder {
            axis_point: DVec3::new(0.5, -0.5, 0.0),
            axis_dir: DVec3::Z,
            r_in: 0.0,
            r_out: 3.0,
        })
    }

    #[test]
    fn each_shape_offers_its_handles_and_a_mesh_shell_none() {
        let none = ZoneLocks::new();
        let kinds = |zoned: &ZonedAbsorption| -> Vec<HandleKind> {
            handles_for(zoned, &none, 1, 10.0)
                .iter()
                .map(|h| h.kind)
                .collect()
        };
        assert_eq!(
            kinds(&zoned_with(ZoneShape::HalfSpace {
                normal: DVec3::X,
                offset: 1.0
            })),
            vec![HandleKind::Offset, HandleKind::Axis]
        );
        assert_eq!(
            kinds(&zoned_with(ZoneShape::Slab {
                normal: DVec3::Y,
                offset_min: -1.0,
                offset_max: 1.0
            })),
            vec![
                HandleKind::OffsetMin,
                HandleKind::OffsetMax,
                HandleKind::Axis
            ]
        );
        assert_eq!(
            kinds(&cylinder()),
            vec![
                HandleKind::AxisPoint,
                HandleKind::Axis,
                HandleKind::OuterRadius
            ]
        );
        let tube = zoned_with(ZoneShape::CoaxialCylinder {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            r_in: 1.0,
            r_out: 3.0,
        });
        assert_eq!(
            kinds(&tube),
            vec![
                HandleKind::AxisPoint,
                HandleKind::Axis,
                HandleKind::OuterRadius,
                HandleKind::InnerRadius
            ]
        );
        assert_eq!(
            kinds(&zoned_with(ZoneShape::Sector {
                axis_point: DVec3::ZERO,
                axis_dir: DVec3::Z,
                angle_from: 0.0,
                angle_to: 1.0
            })),
            vec![
                HandleKind::AxisPoint,
                HandleKind::Axis,
                HandleKind::AngleFrom,
                HandleKind::AngleTo
            ]
        );
        let shell = zoned_with(ZoneShape::MeshShell {
            vertices: Vec::new(),
            triangles: Vec::new(),
        });
        assert_eq!(handles_for(&shell, &none, 1, 10.0), [] as [Handle; 0]);
        assert!(
            handles_for(&cylinder(), &none, 0, 10.0).is_empty(),
            "the base has none"
        );
        assert!(
            handles_for(&cylinder(), &none, 2, 10.0).is_empty(),
            "no such zone"
        );
    }

    #[test]
    fn a_locked_parameter_shows_no_handle() {
        let zoned = cylinder();
        let (_, locks) = apply_with_locks(
            &zoned,
            &ZoneLocks::new(),
            &ZoneEdit::Lock {
                zone: 1,
                parameter: ZoneParameter::ROut,
            },
        )
        .unwrap();
        let kinds: Vec<_> = handles_for(&zoned, &locks, 1, 10.0)
            .iter()
            .map(|h| h.kind)
            .collect();
        assert_eq!(kinds, vec![HandleKind::AxisPoint, HandleKind::Axis]);
        let (_, locks) = apply_with_locks(
            &zoned,
            &locks,
            &ZoneEdit::Lock {
                zone: 1,
                parameter: ZoneParameter::Direction,
            },
        )
        .unwrap();
        let kinds: Vec<_> = handles_for(&zoned, &locks, 1, 10.0)
            .iter()
            .map(|h| h.kind)
            .collect();
        assert_eq!(kinds, vec![HandleKind::AxisPoint]);
    }

    #[test]
    fn handles_sit_where_the_numbers_say() {
        let none = ZoneLocks::new();
        let handles = handles_for(&cylinder(), &none, 1, 10.0);
        let outer = handles
            .iter()
            .find(|h| h.kind == HandleKind::OuterRadius)
            .unwrap();
        let (u, _) = axis_reference(DVec3::Z);
        assert!((outer.position - (DVec3::new(0.5, -0.5, 0.0) + u * 3.0)).length() < 1e-12);
        let axis = handles.iter().find(|h| h.kind == HandleKind::Axis).unwrap();
        assert!((axis.position - DVec3::new(0.5, -0.5, 3.0)).length() < 1e-12);
        // The reference direction is unit and perpendicular to the axis, as the kernels define it.
        assert!((u.length() - 1.0).abs() < 1e-12 && u.dot(DVec3::Z).abs() < 1e-12);
        let (u_x, _) = axis_reference(DVec3::X);
        assert!((u_x - DVec3::Y).length() < 1e-12, "a near-X axis takes Y");
    }

    #[test]
    fn the_pointer_grabs_the_nearest_handle_in_reach_only() {
        let camera = camera();
        let map = view(&camera);
        let handles = handles_for(&cylinder(), &ZoneLocks::new(), 1, 10.0);
        let outer_index = handles
            .iter()
            .position(|h| h.kind == HandleKind::OuterRadius)
            .unwrap();
        let at = map.screen(handles[outer_index].position).unwrap();
        assert_eq!(
            pick_handle(&map, &handles, (at.0 + 3.0, at.1 - 2.0), HIT_RADIUS_PX),
            Some(outer_index)
        );
        assert_eq!(
            pick_handle(&map, &handles, (at.0 + 60.0, at.1), HIT_RADIUS_PX),
            None
        );
    }

    #[test]
    fn dragging_the_radius_handle_along_its_line_changes_the_radius_by_the_travel() {
        let camera = camera();
        let map = view(&camera);
        let zoned = cylinder();
        let handles = handles_for(&zoned, &ZoneLocks::new(), 1, 10.0);
        let outer = handles
            .iter()
            .find(|h| h.kind == HandleKind::OuterRadius)
            .unwrap();
        // The cursor goes where the handle would be 1 mm further out: the radius becomes 4.
        let Drag::Along { axis, .. } = outer.drag else {
            panic!("a radius handle moves along a line");
        };
        let start = map.screen(outer.position).unwrap();
        let end = map.screen(outer.position + axis).unwrap();
        let edit = drag_edit(&map, outer, start, end).unwrap();
        let ZoneEdit::SetParameter {
            zone,
            parameter,
            value,
        } = edit
        else {
            panic!("a radius drag sets a parameter");
        };
        assert_eq!((zone, parameter), (1, ZoneParameter::ROut));
        assert!((value - 4.0).abs() < 0.1, "radius {value}");
        // Applying the edit through the same entry point the wizard uses changes the shape.
        let applied = apply(&zoned, &edit).unwrap();
        let ZoneShape::CoaxialCylinder { r_out, .. } = applied.zones[0].shape else {
            panic!("still a cylinder");
        };
        assert_eq!(r_out, value);
        // Dragging far the other way never makes a negative radius.
        let back = map.screen(outer.position - axis * 50.0).unwrap();
        let ZoneEdit::SetParameter { value, .. } = drag_edit(&map, outer, start, back).unwrap()
        else {
            panic!("a parameter");
        };
        assert!(value >= 0.0);
    }

    #[test]
    fn dragging_an_offset_handle_moves_the_plane_along_its_normal() {
        let camera = camera();
        let map = view(&camera);
        let zoned = zoned_with(ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 1.0,
        });
        let handles = handles_for(&zoned, &ZoneLocks::new(), 1, 10.0);
        let offset = handles
            .iter()
            .find(|h| h.kind == HandleKind::Offset)
            .unwrap();
        let start = map.screen(offset.position).unwrap();
        let end = map.screen(offset.position + DVec3::X * 2.0).unwrap();
        let ZoneEdit::SetParameter {
            parameter, value, ..
        } = drag_edit(&map, offset, start, end).unwrap()
        else {
            panic!("a parameter");
        };
        assert_eq!(parameter, ZoneParameter::Offset);
        assert!((value - 3.0).abs() < 0.25, "offset {value}");
        // No travel, no change.
        let ZoneEdit::SetParameter { value, .. } = drag_edit(&map, offset, start, start).unwrap()
        else {
            panic!("a parameter");
        };
        assert!((value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn an_angle_handle_turns_by_the_travel_over_the_radius() {
        let camera = camera();
        let map = view(&camera);
        let zoned = zoned_with(ZoneShape::Sector {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Z,
            angle_from: 0.0,
            angle_to: 1.0,
        });
        let handles = handles_for(&zoned, &ZoneLocks::new(), 1, 10.0);
        let to = handles
            .iter()
            .find(|h| h.kind == HandleKind::AngleTo)
            .unwrap();
        let Drag::Along { axis, per_mm, .. } = to.drag else {
            panic!("an angle handle moves along its tangent");
        };
        assert!(
            (per_mm - 0.25).abs() < 1e-12,
            "one radian per radius of 4 mm"
        );
        let start = map.screen(to.position).unwrap();
        let end = map.screen(to.position + axis * 0.4).unwrap();
        let ZoneEdit::SetParameter { value, .. } = drag_edit(&map, to, start, end).unwrap() else {
            panic!("a parameter");
        };
        assert!((value - 1.1).abs() < 0.02, "angle {value}");
    }

    #[test]
    fn the_axis_point_and_the_arrow_tip_follow_the_pointer_in_the_view_plane() {
        let camera = camera();
        let map = view(&camera);
        let zoned = cylinder();
        let handles = handles_for(&zoned, &ZoneLocks::new(), 1, 10.0);
        let point = handles
            .iter()
            .find(|h| h.kind == HandleKind::AxisPoint)
            .unwrap();
        let start = map.screen(point.position).unwrap();
        // Dragging right and up on the screen moves the point along the camera's right and up.
        let edit = drag_edit(&map, point, start, (start.0 + 40.0, start.1 - 30.0)).unwrap();
        let ZoneEdit::SetAxisPoint { point: moved, .. } = edit else {
            panic!("an axis point");
        };
        let delta = moved - point.position;
        let right = camera.right.as_dvec3();
        let up = camera.up.as_dvec3();
        assert!(delta.dot(right) > 0.0 && delta.dot(up) > 0.0);
        assert!(
            delta.dot(camera.forward.as_dvec3()).abs() < 1e-6,
            "no depth change"
        );
        // The tip: the direction becomes foot to tip, and a drag back onto the foot is refused.
        let tip = handles.iter().find(|h| h.kind == HandleKind::Axis).unwrap();
        let tip_screen = map.screen(tip.position).unwrap();
        let ZoneEdit::SetDirection { direction, .. } =
            drag_edit(&map, tip, tip_screen, (tip_screen.0 + 25.0, tip_screen.1)).unwrap()
        else {
            panic!("a direction");
        };
        assert!(
            direction.normalize().dot(DVec3::Z) < 1.0 - 1e-6,
            "the axis turned"
        );
        let applied = apply(&zoned, &ZoneEdit::SetDirection { zone: 1, direction }).unwrap();
        let ZoneShape::CoaxialCylinder { axis_dir, .. } = applied.zones[0].shape else {
            panic!("a cylinder");
        };
        assert!((axis_dir.length() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_degenerate_view_gives_no_edit() {
        let camera = camera();
        let map = ViewMap {
            scale: 0.1,
            ..view(&camera)
        };
        let handles = handles_for(&cylinder(), &ZoneLocks::new(), 1, 10.0);
        // A handle behind the camera cannot be dragged.
        let behind = Handle {
            position: camera.origin.as_dvec3() * 20.0,
            ..handles[0]
        };
        assert!(drag_edit(&map, &behind, (0.0, 0.0), (10.0, 10.0)).is_none());
    }

    #[test]
    fn the_overlay_has_a_shaft_per_arrow_and_lights_the_hovered_marker() {
        let camera = camera();
        let map = view(&camera);
        let handles = handles_for(&cylinder(), &ZoneLocks::new(), 1, 10.0);
        let drawn = overlay(&map, &handles, Some(1));
        assert_eq!(drawn.markers.len(), handles.len());
        assert_eq!(drawn.segments.len(), 1, "only the axis arrow has a shaft");
        assert_eq!(
            drawn.markers.iter().map(|m| m.lit).collect::<Vec<_>>(),
            vec![false, true, false]
        );
    }

    #[test]
    fn drawing_colours_the_marker_pixels_and_stays_inside_the_buffer() {
        let (w, h) = (40_u32, 30_u32);
        let mut rgba = vec![0_u8; (w * h * 4) as usize];
        let drawn = Overlay {
            segments: vec![Segment {
                from: (8.0, 8.0),
                to: (30.0, 20.0),
            }],
            markers: vec![
                Marker {
                    at: (20.0, 15.0),
                    lit: false,
                },
                // Half outside the buffer: skipped without a panic.
                Marker {
                    at: (-2.0, 1.0),
                    lit: true,
                },
            ],
        };
        draw_overlay(&mut rgba, w, h, &drawn, [255, 200, 0], 1.0);
        let at = |x: usize, y: usize| &rgba[(y * w as usize + x) * 4..(y * w as usize + x) * 4 + 4];
        assert_eq!(at(20, 15), &[255, 200, 0, 255], "marker centre");
        assert_eq!(at(8, 8), &[255, 200, 0, 255], "shaft start");
        assert_eq!(at(39, 29), &[0, 0, 0, 0], "untouched corner");
        // A frame drawn at twice the logical size scales the overlay.
        let mut big = vec![0_u8; (80 * 60 * 4) as usize];
        draw_overlay(&mut big, 80, 60, &drawn, [255, 200, 0], 2.0);
        assert_eq!(
            &big[(30 * 80 + 40) * 4..(30 * 80 + 40) * 4 + 4],
            &[255, 200, 0, 255]
        );
    }
}
