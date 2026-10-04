//! The provisional `v0` frame for concave tools (plan §4.4 and §11a).
//!
//! **Every convention that is still pending confirmation by Arya Akhavan lives in
//! this one file** and is read only by [`super::Design::concave_tools_from_solved`]
//! (and its siblings). Changing a decision therefore means editing this file plus
//! a reader shim keyed on [`CONCAVE_FRAME_VERSION`], which the native file records
//! next to its concave tiers. Each item below names the §11a row it implements.
//!
//! The geometry is `f64` throughout; [`primitive_for`] narrows to `f32` exactly
//! once, like `to_halfspace_f64` does for planes in the opposite direction. The one
//! exception is the frame normal `n` ([`dop_frame`]): it is built with the very
//! `f32` arithmetic the flat facet normals use, so a concave tier on the same
//! (φ, index) as a flat facet gets that facet's normal bit for bit.

#![expect(
    clippy::many_single_char_names,
    reason = "frame algebra (u, v, n, x, y, z) follows the derivation in plan §4.4; longer names would hide the formulas"
)]

use glam::{DVec3, Vec3};
use indicatrix::geometry::{
    cuts::StandardGemCuts,
    plane::tier_is_crown_side,
    stone_metrics::measure_solid_with_vertices,
    tool::{ToolPrimitive, ToolSweep},
};

use crate::design::{ConcaveTier, ConcaveTool, ToolMotion};

/// The frame this file implements.
///
/// Must equal `indicatrix_formats::native::CONCAVE_FRAME_V0`, which the native
/// file writes next to concave tiers and a reader checks before trusting a design
/// (§11a, "consequences": an unknown frame string is refused, never reinterpreted).
pub const CONCAVE_FRAME_VERSION: &str = "v0";

/// Reciprocation half-stroke for `CYL`, `CON`, `CIR` and `DSC`, times `W`.
///
/// Along the tool axis (§11a, Q2 reciprocation). The stroke length is never
/// written in the notation, so it is "enough to clear the stone".
pub const RECIP_HALF_STROKE_RATIO: f64 = 2.0;

/// Reciprocation half-stroke for `SPH`, times the tool diameter `D · W`.
///
/// §11a, Q2 reciprocation: a ball dimple is a short rocking motion, not a pass
/// over the whole stone.
pub const SPH_RECIP_HALF_STROKE_RATIO: f64 = 0.5;

/// Rim width of a `CIR` tool, times its diameter `D · W`.
///
/// Taken as the cylinder's **full** length (§11a, Q2 `CIR`): the rim of a flat
/// wheel.
pub const CIR_RIM_WIDTH_RATIO: f64 = 0.1;

/// Half-length of a `CYL` tool, times the stone width `W`.
///
/// §11a, Q2 `CIR`: "`CYL` is the long cylinder". One `W` each way makes the
/// cylinder span the stone, so a plunged cylinder is a straight groove with no
/// stroke.
pub const CYL_HALF_LENGTH_RATIO: f64 = 1.0;

/// Included angle assumed for a `CON` or `DSC` whose angle is absent. Validation
/// rejects that case, so this only keeps the resolver total (§11a, Q2 cone/disc
/// angle: the angle is the **included** angle).
const FALLBACK_INCLUDED_ANGLE_DEG: f64 = 90.0;

/// Vertices whose support value is within this fraction of the stone width `W`
/// of the best count as tied (§11a, Q1 frame origin: ties go to the
/// lexicographically smallest vertex), so float noise on a facet's own vertices
/// cannot pick a different corner on a different platform.
///
/// The flat normals are `f32`, so the corners of one flat facet differ in `v · n`
/// by about `1e-7 · W` along any direction that is not that facet's exact normal;
/// `1e-6 · W` is above that noise and far below any real feature of a stone.
const SUPPORT_TIE_REL_EPS: f64 = 1e-6;

/// A dop frame `(u, v, n)`, right-handed (`u × v = n`).
pub type DopFrame = (DVec3, DVec3, DVec3);

/// The dop frame at (φ, index) (§11a, Q1 frame axes).
///
/// `n` is the would-be flat facet normal that `StandardGemCuts` builds for the
/// same (φ, index), `u` is the unit projection of the girdle-ward direction
/// (`−sign(φ)·Y`) onto the facet plane, so it points down the slope, and
/// `v = n × u`. The tuple is `(u, v, n)`.
///
/// `index` and `gear_reference_angle` are passed separately because the flat path
/// narrows each to `f32` before adding them in `StandardGemCuts::index_to_azimuth`;
/// doing the same here (and the same `f32` sin/cos and normalisation as
/// `StandardGemCuts::from_asc_schedule`) makes `n` equal the flat facet's normal
/// exactly. `u` and `v` are then built from `n` in `f64`, so `(u, v)` is
/// orthonormal while `n` carries the flat normal's `f32` rounding (about `6e-8`
/// off unit length). φ must satisfy `0 < |φ| < 90`, which `ConcaveTier::validate`
/// guarantees; outside that range `u` falls back to `+X` rather than a NaN.
#[must_use]
pub fn dop_frame(
    angle_deg: f64,
    index: f64,
    gear_reference_angle: f64,
    gear_teeth: i32,
) -> DopFrame {
    let gear = narrow(f64::from(gear_teeth.unsigned_abs().max(1)));
    let theta = narrow(angle_deg.abs()).to_radians();
    let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
    let phi = StandardGemCuts::index_to_azimuth(narrow(index), gear, narrow(gear_reference_angle));
    let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
    let crown = tier_is_crown_side(angle_deg);
    let n_flat = Vec3::new(
        sin_theta * cos_phi,
        if crown { cos_theta } else { -cos_theta },
        sin_theta * sin_phi,
    )
    .normalize();
    let n = DVec3::new(
        f64::from(n_flat.x),
        f64::from(n_flat.y),
        f64::from(n_flat.z),
    );
    let n_unit = n.normalize_or(DVec3::Y);
    let girdle_ward = if crown { DVec3::NEG_Y } else { DVec3::Y };
    let u = (girdle_ward - n_unit * girdle_ward.dot(n_unit)).normalize_or(DVec3::X);
    (u, n_unit.cross(u), n)
}

/// The vertex of `vertices` furthest along `n`; ties (within `1e-6 · width`)
/// go to the lexicographically smallest `(x, y, z)` by `f64::total_cmp`.
///
/// `width` is the stone width `W`. Split from [`contact_point`] so the resolver
/// can enumerate the stone's vertices once and reuse them for every placement.
#[must_use]
pub fn support_vertex(vertices: &[DVec3], n: DVec3, width: f64) -> Option<DVec3> {
    let best = vertices.iter().map(|v| v.dot(n)).max_by(f64::total_cmp)?;
    let eps = SUPPORT_TIE_REL_EPS * width.abs();
    vertices
        .iter()
        .copied()
        .filter(|v| v.dot(n) >= best - eps)
        .min_by(|a, b| {
            a.x.total_cmp(&b.x)
                .then(a.y.total_cmp(&b.y))
                .then(a.z.total_cmp(&b.z))
        })
}

/// Support point of the convex polytope `planes` (`n · x ≤ m`) in direction `n`:
/// the first point a tool pressed along `−n` would touch (§11a, Q1 frame origin).
///
/// `None` when the planes do not bound a solid.
#[must_use]
pub fn contact_point(planes: &[(DVec3, f64)], n: DVec3) -> Option<DVec3> {
    let (metrics, vertices) = measure_solid_with_vertices(planes)?;
    support_vertex(&vertices, n, metrics.width_axis)
}

/// Tool centre: `c0 + W·(X·u + Y·v − Z·n)` (§11a, Q1 frame axes: `Z > 0` moves
/// the tool **into** the stone). `frame` is `(u, v, n)`.
///
/// For `CON` the "centre" is the cone's apex (see [`primitive_for`]), so `Z` is
/// the depth of a conical pit.
#[must_use]
pub fn tool_centre(c0: DVec3, width: f64, frame: DopFrame, displacement: [f64; 3]) -> DVec3 {
    let (u, v, n) = frame;
    let [x, y, z] = displacement;
    c0 + width * (u * x + v * y - n * z)
}

/// Tool axis (§11a, Q1 θ and Q2 plunge): `cos θ·u + sin θ·v`, or `−n` for a
/// plunged `CON`.
///
/// A plunged cone is a conical pit, so θ is ignored. θ is normalised to
/// `[0, 360)` here, not at validation, so the stored value is what the author
/// typed.
#[must_use]
pub fn tool_axis(tier: &ConcaveTier, frame: DopFrame) -> DVec3 {
    let (u, v, n) = frame;
    if tier.tool == ConcaveTool::Cone && tier.motion == ToolMotion::Plunge {
        return -n;
    }
    let theta = tier.tool_azimuth_deg.rem_euclid(360.0).to_radians();
    let (sin_theta, cos_theta) = theta.sin_cos();
    (u * cos_theta + v * sin_theta).normalize_or(u)
}

/// The one `f64 -> f32` narrowing of the resolver.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the kernel works in f32; the resolver is f64 and narrows once, here"
)]
const fn narrow(x: f64) -> f32 {
    x as f32
}

const fn narrow3(v: DVec3) -> Vec3 {
    Vec3::new(narrow(v.x), narrow(v.y), narrow(v.z))
}

/// Builds the primitive for one placement from the resolved centre, axis and
/// stone width (§11a, Q2 `CIR`, cone/disc angle, reciprocation, plunge).
///
/// - `CYL`: cylinder of radius `W·D/2`, half-length `W` times
///   [`CYL_HALF_LENGTH_RATIO`].
/// - `CIR`: the same radius, full length `D·W` times [`CIR_RIM_WIDTH_RATIO`].
/// - `CON`: a frustum from the base (radius `W·D/2`) to the **apex at `centre`**,
///   the apex lying along `+axis`; its height is `r / tan(angle / 2)`.
/// - `DSC`: a bicone with rim radius `W·D/2` whose two faces meet at the rim in
///   the included angle, i.e. each tapers over `r · tan(angle / 2)`.
/// - `SPH`: a ball of radius `W·D/2`.
///
/// A reciprocating tool is swept along its axis by the half-stroke above; a
/// plunged one is not swept. `width` is the flat stone's width `W`.
#[must_use]
pub fn primitive_for(tier: &ConcaveTier, centre: DVec3, axis: DVec3, width: f64) -> ToolPrimitive {
    let diameter = width * tier.diameter_ratio;
    let radius = 0.5 * diameter;
    let half_angle =
        (tier.tool_angle_deg.unwrap_or(FALLBACK_INCLUDED_ANGLE_DEG) * 0.5).to_radians();
    let axis_f = narrow3(axis);
    let mut tool = match tier.tool {
        ConcaveTool::Cylinder => ToolPrimitive::cylinder(
            narrow3(centre),
            axis_f,
            narrow(radius),
            narrow(CYL_HALF_LENGTH_RATIO * width),
        ),
        ConcaveTool::Circle => ToolPrimitive::cylinder(
            narrow3(centre),
            axis_f,
            narrow(radius),
            narrow(0.5 * CIR_RIM_WIDTH_RATIO * diameter),
        ),
        ConcaveTool::Cone => {
            let half_height = 0.5 * radius / half_angle.tan();
            ToolPrimitive::frustum(
                narrow3(centre - axis * half_height),
                axis_f,
                narrow(radius),
                0.0,
                narrow(half_height),
            )
        }
        ConcaveTool::Disc => ToolPrimitive::bicone(
            narrow3(centre),
            axis_f,
            narrow(radius),
            narrow(radius * half_angle.tan()),
        ),
        ConcaveTool::Sphere => {
            let mut ball = ToolPrimitive::ball(narrow3(centre), narrow(radius));
            if tier.motion == ToolMotion::Reciprocating {
                // A ball has no axis of its own; the stroke direction rides in it.
                ball.axis = [axis_f.x, axis_f.y, axis_f.z, 0.0];
            }
            ball
        }
    };
    if tier.motion == ToolMotion::Reciprocating {
        let half_stroke = if tier.tool == ConcaveTool::Sphere {
            SPH_RECIP_HALF_STROKE_RATIO * diameter
        } else {
            RECIP_HALF_STROKE_RATIO * width
        };
        tool = tool.with_sweep(ToolSweep::AlongAxis, narrow(half_stroke), Vec3::ZERO);
    }
    tool
}
