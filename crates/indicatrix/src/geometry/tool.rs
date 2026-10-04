//! Concave-facet tool volumes: the resolved primitives a stone is carved with.
//!
//! A concave (or "fantasy") facet is the surface a rotating or reciprocating
//! tool leaves behind. The kernel never sees a tier or a notation; it sees a
//! list of [`ToolPrimitive`]s, each a **convex** volume that is subtracted from
//! the flat stone. Convexity is what keeps ray intersection closed-form: every
//! primitive, swept or not, decomposes into a handful of convex pieces (ball,
//! cone frustum, wedge) whose union is itself convex, so the ray's interval
//! inside a tool is `[min c, max d]` over the pieces and no iterative solver is
//! needed.
//!
//! The layout is `repr(C)`, 80 bytes, 16-byte aligned and made only of
//! `u32`/`f32`, so it is [`bytemuck::Pod`] and mirrors the WGSL struct of the
//! same name byte for byte.

#![expect(
    clippy::many_single_char_names,
    reason = "ray and frame algebra (o, p, q, a, c, d, r, s, t, z) mirrors the derivation in the plan; longer names would hide the formulas"
)]
#![expect(
    clippy::pub_underscore_fields,
    clippy::used_underscore_binding,
    reason = "`_pad` is part of the specified 80-byte GPU layout and is read only to check it is zero"
)]

use crate::{geometry::plane::GpuFacetPlane, optics::raytracer::camera::Ray};
use glam::Vec3;

/// Upper bound on tool primitives in one stone.
///
/// One `pub const` so every validator and the shader loop bound agree. Each
/// index is one placement, not one tier; it also sizes the fixed boundary array
/// in `intersect_stone`, which is why that function never allocates.
pub const MAX_TOOL_PRIMITIVES: usize = 128;

/// Disc/quadratic guard: a ray whose discriminant is within this (relative)
/// distance of zero is tangent to the tool and reports no interval. Same guard
/// class as the slab loop's `denom`; the missed boundary is a measure-zero set
/// and the choice is deterministic.
const TANGENT_EPS: f32 = 1e-7;

/// `|denom|` below this treats a ray as parallel to a slab or plane.
const PARALLEL_EPS: f32 = 1e-7;

/// Shape of a tool.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolKind {
    /// Sphere (`SPH`, a dimple).
    Ball = 0,
    /// Capped cylinder (`CYL`, and `CIR` as a short one).
    Cylinder = 1,
    /// Cone frustum (`CON`).
    Frustum = 2,
    /// Two frusta sharing a rim plane (`DSC`, a wheel with a bevelled rim).
    Bicone = 3,
}

impl TryFrom<u32> for ToolKind {
    type Error = u32;

    /// Decodes the wire/GPU value; an unknown value is returned as the error.
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ball),
            1 => Ok(Self::Cylinder),
            2 => Ok(Self::Frustum),
            3 => Ok(Self::Bicone),
            other => Err(other),
        }
    }
}

/// How a tool moves while it cuts; the swept volume is what is subtracted.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolSweep {
    /// Pressed in without lateral motion.
    None = 0,
    /// Reciprocates along its own axis (Minkowski sum with an axial segment).
    AlongAxis = 1,
    /// Reciprocates across its axis along `sweep_dir` (Minkowski sum with a
    /// perpendicular segment).
    AcrossAxis = 2,
}

impl TryFrom<u32> for ToolSweep {
    type Error = u32;

    /// Decodes the wire/GPU value; an unknown value is returned as the error.
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::AlongAxis),
            2 => Ok(Self::AcrossAxis),
            other => Err(other),
        }
    }
}

/// A convex tool volume subtracted from the stone, in model units.
///
/// One primitive per placement. Its facet id is `planes.len() + k` where `k` is
/// its position in the slice, so the struct carries no facet bookkeeping.
///
/// `align(16)` matches the WGSL storage-buffer rule for a struct of `vec4`s, so a
/// host `&[ToolPrimitive]` uploads with no repacking (80 is a multiple of 16, so
/// the attribute adds no padding and `Pod` still holds).
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToolPrimitive {
    /// `ToolKind as u32`: 0 ball, 1 capped cylinder, 2 cone frustum, 3 bicone.
    pub kind: u32,
    /// `ToolSweep as u32`: 0 none, 1 along `axis`, 2 across `axis`.
    pub sweep_kind: u32,
    /// Must be zero (keeps the GPU layout and the cache hash canonical).
    pub _pad: [u32; 2],
    /// Centre `xyz`; `w` = radius at the centre (ball, cylinder, bicone rim).
    pub origin: [f32; 4],
    /// Unit axis `xyz`; `w` = half-length along the axis (0 for a ball).
    pub axis: [f32; 4],
    /// `x` = radius at `-half_length`, `y` = radius at `+half_length`, `z` =
    /// sweep half-stroke, `w` = 0.
    pub profile: [f32; 4],
    /// Unit sweep direction `xyz` (read only when `sweep_kind == 2`), `w` = 0.
    pub sweep_dir: [f32; 4],
}

/// Why a [`ToolPrimitive`] failed [`ToolPrimitive::validate`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToolPrimitiveError {
    /// Some float is NaN or infinite.
    NonFinite,
    /// A radius is negative, no radius is positive, or a length (half-length,
    /// half-stroke) is negative or, for a non-ball, zero.
    NonPositiveRadius,
    /// The axis, or the sweep direction when it is read, is not a unit vector
    /// to `1e-4` (or the sweep direction is not perpendicular to the axis).
    AxisNotUnit,
    /// `kind` is not a [`ToolKind`] value.
    UnknownKind(u32),
    /// `sweep_kind` is not a [`ToolSweep`] value.
    UnknownSweep(u32),
    /// A padding lane or a `w` lane that must be zero is not.
    PaddingNotZero,
}

impl std::fmt::Display for ToolPrimitiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite => f.write_str("tool primitive has a non-finite value"),
            Self::NonPositiveRadius => {
                f.write_str("tool primitive has an invalid radius or length")
            }
            Self::AxisNotUnit => f.write_str("tool primitive axis is not a unit vector"),
            Self::UnknownKind(k) => write!(f, "unknown tool kind {k}"),
            Self::UnknownSweep(k) => write!(f, "unknown tool sweep {k}"),
            Self::PaddingNotZero => f.write_str("tool primitive padding is not zero"),
        }
    }
}

impl std::error::Error for ToolPrimitiveError {}

/// The planes and tools that together bound a stone.
///
/// The single gate every "convex shortcut" tests via [`Self::is_convex`].
#[derive(Debug, Clone, Copy)]
pub struct StoneGeometry<'a> {
    /// The flat facets; `n . x + d <= 0` inside.
    pub planes: &'a [GpuFacetPlane],
    /// Convex volumes subtracted from the polyhedron the planes define.
    pub tools: &'a [ToolPrimitive],
}

impl<'a> StoneGeometry<'a> {
    /// A stone with no tools: every plane-only API wraps its slice in this.
    #[must_use]
    pub const fn planes_only(planes: &'a [GpuFacetPlane]) -> Self {
        Self { planes, tools: &[] }
    }

    /// `true` when there are no tools, i.e. the stone is the convex polyhedron.
    #[must_use]
    pub const fn is_convex(&self) -> bool {
        self.tools.is_empty()
    }

    /// Plane count plus tool count; tool `k` has facet id `planes.len() + k`.
    #[must_use]
    pub const fn facet_count(&self) -> usize {
        self.planes.len() + self.tools.len()
    }
}

/// One convex piece of a (possibly swept) tool.
///
/// Every tool decomposes into at most [`MAX_PIECES`] of these whose union is
/// the tool, which is how sweeps stay closed-form.
#[derive(Debug, Clone, Copy)]
enum Piece {
    Ball {
        c: Vec3,
        r: f32,
    },
    /// Frustum about `a` through `c`, radius `r0` at `-hl` and `r1` at `+hl`.
    Frustum {
        c: Vec3,
        a: Vec3,
        hl: f32,
        r0: f32,
        r1: f32,
    },
    /// The straight part of a stadium-section sweep: `|axial| <= hl`,
    /// `|along d| <= s`, `|across| <= r(axial)`.
    Wedge {
        c: Vec3,
        a: Vec3,
        d: Vec3,
        hl: f32,
        s: f32,
        r0: f32,
        r1: f32,
    },
}

/// Most pieces any tool needs (a bicone swept across: two frusta, twice, plus
/// two wedges).
const MAX_PIECES: usize = 6;

/// Fixed-capacity piece list, so decomposing a tool never allocates.
struct Pieces {
    items: [Piece; MAX_PIECES],
    len: usize,
}

impl Pieces {
    const fn new() -> Self {
        Self {
            items: [Piece::Ball {
                c: Vec3::ZERO,
                r: 0.0,
            }; MAX_PIECES],
            len: 0,
        }
    }

    fn push(&mut self, p: Piece) {
        debug_assert!(
            self.len < MAX_PIECES,
            "tool decomposes into too many pieces"
        );
        if self.len < MAX_PIECES {
            self.items[self.len] = p;
            self.len += 1;
        }
    }

    fn as_slice(&self) -> &[Piece] {
        &self.items[..self.len]
    }
}

/// A straight run of the axial radius profile: `z` in `[z_lo, z_hi]`, radius
/// `r_lo -> r_hi`.
#[derive(Clone, Copy)]
struct Segment {
    z_lo: f32,
    z_hi: f32,
    r_lo: f32,
    r_hi: f32,
}

impl Piece {
    /// Entry/exit parameters of `ray` inside the piece.
    fn interval(&self, ray: Ray) -> Option<(f32, f32)> {
        match *self {
            Self::Ball { c, r } => ball_interval(ray, c, r),
            Self::Frustum { c, a, hl, r0, r1 } => frustum_interval(ray, c, a, hl, r0, r1),
            Self::Wedge {
                c,
                a,
                d,
                hl,
                s,
                r0,
                r1,
            } => wedge_interval(ray, c, a, d, hl, s, r0, r1),
        }
    }

    /// A signed distance estimate (negative inside, zero on the surface; never
    /// larger in magnitude than the true distance for a point inside, and the
    /// correct sign everywhere) and the outward unit normal of the face that
    /// attains it.
    fn sd_normal(&self, p: Vec3) -> (f32, Vec3) {
        match *self {
            Self::Ball { c, r } => {
                let q = p - c;
                let len = q.length();
                (len - r, if len > 1e-12 { q / len } else { Vec3::Z })
            }
            Self::Frustum { c, a, hl, r0, r1 } => {
                let q = p - c;
                let z = q.dot(a);
                let perp = q - a * z;
                let rho = perp.length();
                let rho_hat = if rho > 1e-12 {
                    perp / rho
                } else {
                    a.any_orthonormal_vector()
                };
                let (cap_sd, cap_n) = if z >= 0.0 { (z - hl, a) } else { (-z - hl, -a) };
                let dr = r1 - r0;
                let len = (4.0 * hl).mul_add(hl, dr * dr).sqrt();
                let rm = 0.5 * (r0 + r1);
                let side_sd = z.mul_add(-dr, (rho - rm) * 2.0 * hl) / len;
                let side_n = rho_hat * (2.0 * hl / len) - a * (dr / len);
                if side_sd > cap_sd {
                    (side_sd, side_n)
                } else {
                    (cap_sd, cap_n)
                }
            }
            Self::Wedge {
                c,
                a,
                d,
                hl,
                s,
                r0,
                r1,
            } => {
                let q = p - c;
                let w = a.cross(d);
                let z = q.dot(a);
                let x = q.dot(d);
                let y = q.dot(w);
                let k = (r1 - r0) / (2.0 * hl);
                let rm = 0.5 * (r0 + r1);
                let kappa = 1.0 / k.mul_add(k, 1.0).sqrt();
                let mut best = if z >= 0.0 { (z - hl, a) } else { (-z - hl, -a) };
                let along = if x >= 0.0 { (x - s, d) } else { (-x - s, -d) };
                if along.0 > best.0 {
                    best = along;
                }
                let slant_pos = ((y - rm - k * z) * kappa, (w - a * k) * kappa);
                if slant_pos.0 > best.0 {
                    best = slant_pos;
                }
                let slant_neg = ((-y - rm - k * z) * kappa, (-w - a * k) * kappa);
                if slant_neg.0 > best.0 {
                    best = slant_neg;
                }
                best
            }
        }
    }
}

/// Roots of `a t^2 + 2 b t + c = 0` as `(lo, hi)`, or `None` when the ray is
/// tangent (relative discriminant within [`TANGENT_EPS`]).
///
/// Uses the cancellation-free form so a small tool hit from far away keeps its
/// precision. Requires `a != 0`.
fn quadratic_roots(a: f32, b: f32, c: f32) -> Option<(f32, f32)> {
    let disc = b.mul_add(b, -(a * c));
    if disc <= TANGENT_EPS * b.mul_add(b, (a * c).abs()) {
        return None;
    }
    let sq = disc.sqrt();
    let q = -(b + sq.copysign(b));
    let r1 = q / a;
    let r2 = c / q;
    Some(if r1 <= r2 { (r1, r2) } else { (r2, r1) })
}

fn ball_interval(ray: Ray, c: Vec3, r: f32) -> Option<(f32, f32)> {
    let o = ray.origin - c;
    let a = ray.dir.dot(ray.dir);
    let b = o.dot(ray.dir);
    let cc = r.mul_add(-r, o.dot(o));
    quadratic_roots(a, b, cc)
}

/// Parameter range of `ray` inside the slab `|axial| <= hl`, `None` when the
/// ray is parallel to the caps and outside them.
fn cap_range(z0: f32, dz: f32, hl: f32) -> Option<(f32, f32)> {
    if dz.abs() > PARALLEL_EPS {
        let t1 = (-hl - z0) / dz;
        let t2 = (hl - z0) / dz;
        Some(if t1 <= t2 { (t1, t2) } else { (t2, t1) })
    } else if z0.abs() <= hl {
        Some((-1e30, 1e30))
    } else {
        None
    }
}

fn clip(lo: f32, hi: f32, s0: f32, s1: f32) -> Option<(f32, f32)> {
    let l = lo.max(s0);
    let h = hi.min(s1);
    (l <= h).then_some((l, h))
}

fn frustum_interval(ray: Ray, c: Vec3, a: Vec3, hl: f32, r0: f32, r1: f32) -> Option<(f32, f32)> {
    let o = ray.origin - c;
    let z0 = o.dot(a);
    let dz = ray.dir.dot(a);
    let (s0, s1) = cap_range(z0, dz, hl)?;
    let rm = 0.5 * (r0 + r1);
    let k = (r1 - r0) / (2.0 * hl);
    let p = o - a * z0;
    let q = ray.dir - a * dz;
    let rz0 = k.mul_add(z0, rm);
    let rate = k * dz;
    let qa = rate.mul_add(-rate, q.dot(q));
    let qb = (-rate).mul_add(rz0, p.dot(q));
    let qc = rz0.mul_add(-rz0, p.dot(p));
    if qa.abs() <= PARALLEL_EPS {
        // Ray parallel to a generator: the radius condition is linear in t.
        if qb.abs() <= PARALLEL_EPS {
            return (qc <= 0.0).then_some((s0, s1));
        }
        let t = -qc / (2.0 * qb);
        return if qb > 0.0 {
            clip(-1e30, t, s0, s1)
        } else {
            clip(t, 1e30, s0, s1)
        };
    }
    let (lo, hi) = quadratic_roots(qa, qb, qc)?;
    if qa > 0.0 {
        clip(lo, hi, s0, s1)
    } else {
        // Steeper than the cone: `Q <= 0` outside the roots. The mirror nappe
        // lies beyond the caps, so at most one side survives clipping; the hull
        // keeps the result an interval if rounding lets both through.
        match (clip(-1e30, lo, s0, s1), clip(hi, 1e30, s0, s1)) {
            (Some(x), Some(y)) => Some((x.0.min(y.0), x.1.max(y.1))),
            (x, y) => x.or(y),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a wedge is fully described by its frame and two radii; a struct would only rename them"
)]
fn wedge_interval(
    ray: Ray,
    c: Vec3,
    a: Vec3,
    d: Vec3,
    hl: f32,
    s: f32,
    r0: f32,
    r1: f32,
) -> Option<(f32, f32)> {
    let w = a.cross(d);
    let k = (r1 - r0) / (2.0 * hl);
    let rm = 0.5 * (r0 + r1);
    // `(normal, offset)` with `normal . (x - c) + offset <= 0` inside; normals
    // are deliberately not normalised, the slab method only needs ratios.
    let faces = [
        (a, -hl),
        (-a, -hl),
        (d, -s),
        (-d, -s),
        (w - a * k, -rm),
        (-w - a * k, -rm),
    ];
    let o = ray.origin - c;
    let mut t_near = -1e30f32;
    let mut t_far = 1e30f32;
    for (n, e) in faces {
        let denom = n.dot(ray.dir);
        let side = n.dot(o) + e;
        if denom.abs() > PARALLEL_EPS {
            let t = -side / denom;
            if denom < 0.0 {
                t_near = t_near.max(t);
            } else {
                t_far = t_far.min(t);
            }
        } else if side > 0.0 {
            return None;
        }
    }
    (t_near <= t_far).then_some((t_near, t_far))
}

impl ToolPrimitive {
    const fn base(
        kind: ToolKind,
        centre: Vec3,
        axis: Vec3,
        rim: f32,
        half_length: f32,
        r: [f32; 2],
    ) -> Self {
        Self {
            kind: kind as u32,
            sweep_kind: ToolSweep::None as u32,
            _pad: [0; 2],
            origin: [centre.x, centre.y, centre.z, rim],
            axis: [axis.x, axis.y, axis.z, half_length],
            profile: [r[0], r[1], 0.0, 0.0],
            sweep_dir: [0.0; 4],
        }
    }

    /// A sphere. Its axis is `+Z` so an along-axis sweep has a direction.
    #[must_use]
    pub const fn ball(centre: Vec3, radius: f32) -> Self {
        Self::base(
            ToolKind::Ball,
            centre,
            Vec3::Z,
            radius,
            0.0,
            [radius, radius],
        )
    }

    /// A capped cylinder of `radius` and total length `2 * half_length`.
    #[must_use]
    pub const fn cylinder(centre: Vec3, axis: Vec3, radius: f32, half_length: f32) -> Self {
        Self::base(
            ToolKind::Cylinder,
            centre,
            axis,
            radius,
            half_length,
            [radius, radius],
        )
    }

    /// A cone frustum with radius `r_neg` at `-half_length` and `r_pos` at
    /// `+half_length`.
    #[must_use]
    pub fn frustum(centre: Vec3, axis: Vec3, r_neg: f32, r_pos: f32, half_length: f32) -> Self {
        Self::base(
            ToolKind::Frustum,
            centre,
            axis,
            0.5 * (r_neg + r_pos),
            half_length,
            [r_neg, r_pos],
        )
    }

    /// Two frusta sharing a rim of `rim_radius` at the centre, tapering to a
    /// point at each end (`r_neg = r_pos = 0`).
    #[must_use]
    pub const fn bicone(centre: Vec3, axis: Vec3, rim_radius: f32, half_length: f32) -> Self {
        Self::base(
            ToolKind::Bicone,
            centre,
            axis,
            rim_radius,
            half_length,
            [0.0, 0.0],
        )
    }

    /// Returns the tool with a sweep applied: a half-stroke along the axis, or
    /// across it along `dir` (which must be a unit vector perpendicular to the
    /// axis; ignored for [`ToolSweep::AlongAxis`] and [`ToolSweep::None`]).
    #[must_use]
    pub fn with_sweep(mut self, sweep: ToolSweep, half_stroke: f32, dir: Vec3) -> Self {
        self.sweep_kind = sweep as u32;
        self.profile[2] = half_stroke;
        self.sweep_dir = if sweep == ToolSweep::AcrossAxis {
            [dir.x, dir.y, dir.z, 0.0]
        } else {
            [0.0; 4]
        };
        self
    }

    /// The decoded shape, `None` for an unknown `kind`.
    #[must_use]
    pub fn kind(&self) -> Option<ToolKind> {
        ToolKind::try_from(self.kind).ok()
    }

    /// The decoded sweep, `None` for an unknown `sweep_kind`.
    #[must_use]
    pub fn sweep(&self) -> Option<ToolSweep> {
        ToolSweep::try_from(self.sweep_kind).ok()
    }

    /// Checks everything a producer must guarantee: every float finite, radii
    /// `>= 0` with one `> 0`, lengths `>= 0` (`> 0` for a non-ball), axis (and
    /// the sweep direction when read) unit to `1e-4` and perpendicular, pad and
    /// `w` lanes zero, kinds known.
    ///
    /// # Errors
    ///
    /// The first violated rule, as a [`ToolPrimitiveError`].
    pub fn validate(&self) -> Result<(), ToolPrimitiveError> {
        let all = self
            .origin
            .iter()
            .chain(&self.axis)
            .chain(&self.profile)
            .chain(&self.sweep_dir);
        if !all.into_iter().all(|v| v.is_finite()) {
            return Err(ToolPrimitiveError::NonFinite);
        }
        let kind = ToolKind::try_from(self.kind).map_err(ToolPrimitiveError::UnknownKind)?;
        let sweep =
            ToolSweep::try_from(self.sweep_kind).map_err(ToolPrimitiveError::UnknownSweep)?;
        if self._pad != [0; 2] || self.profile[3] != 0.0 || self.sweep_dir[3] != 0.0 {
            return Err(ToolPrimitiveError::PaddingNotZero);
        }
        let radii = [self.origin[3], self.profile[0], self.profile[1]];
        let hl = self.axis[3];
        let stroke = self.profile[2];
        if radii.iter().any(|&r| r < 0.0)
            || radii.iter().all(|&r| r <= 0.0)
            || hl < 0.0
            || (kind != ToolKind::Ball && hl <= 0.0)
            || (kind == ToolKind::Ball && hl != 0.0)
            || stroke < 0.0
        {
            return Err(ToolPrimitiveError::NonPositiveRadius);
        }
        let axis = self.axis_vec();
        if (axis.length() - 1.0).abs() > 1e-4 {
            return Err(ToolPrimitiveError::AxisNotUnit);
        }
        if sweep == ToolSweep::AcrossAxis {
            let dir = self.sweep_dir_vec();
            if (dir.length() - 1.0).abs() > 1e-4 || dir.dot(axis).abs() > 1e-4 {
                return Err(ToolPrimitiveError::AxisNotUnit);
            }
        }
        Ok(())
    }

    const fn centre(&self) -> Vec3 {
        Vec3::new(self.origin[0], self.origin[1], self.origin[2])
    }

    const fn axis_vec(&self) -> Vec3 {
        Vec3::new(self.axis[0], self.axis[1], self.axis[2])
    }

    const fn sweep_dir_vec(&self) -> Vec3 {
        Vec3::new(self.sweep_dir[0], self.sweep_dir[1], self.sweep_dir[2])
    }

    /// The profile as straight segments in axial coordinate, plus the index of
    /// the peak vertex (the plateau of an along-axis sweep sits there).
    fn segments(&self, kind: ToolKind) -> ([Segment; 2], usize, f32, f32) {
        let hl = self.axis[3];
        let rim = self.origin[3];
        let (rn, rp) = (self.profile[0], self.profile[1]);
        let seg = |z_lo, z_hi, r_lo, r_hi| Segment {
            z_lo,
            z_hi,
            r_lo,
            r_hi,
        };
        match kind {
            ToolKind::Ball | ToolKind::Cylinder => (
                [seg(-hl, hl, rim, rim), seg(0.0, 0.0, 0.0, 0.0)],
                1,
                hl,
                rim,
            ),
            ToolKind::Frustum => {
                let peak = if rp >= rn { (hl, rp) } else { (-hl, rn) };
                (
                    [seg(-hl, hl, rn, rp), seg(0.0, 0.0, 0.0, 0.0)],
                    1,
                    peak.0,
                    peak.1,
                )
            }
            ToolKind::Bicone => ([seg(-hl, 0.0, rn, rim), seg(0.0, hl, rim, rp)], 2, 0.0, rim),
        }
    }

    /// Decomposes the (swept) tool into convex pieces whose union it is.
    ///
    /// Along-axis: the radius profile of a Minkowski sum with an axial segment
    /// is the sliding-window maximum of a concave profile, i.e. each rising run
    /// shifted by `-stroke`, each falling run by `+stroke`, and a cylinder of
    /// half-length `stroke` on the peak. Across-axis: every cross-section
    /// circle becomes a stadium, i.e. two shifted copies plus the wedge between
    /// them.
    fn pieces(&self) -> Option<Pieces> {
        let kind = self.kind()?;
        let sweep = self.sweep()?;
        let c = self.centre();
        let a = self.axis_vec();
        let stroke = self.profile[2];
        let mut out = Pieces::new();
        let swept = sweep != ToolSweep::None && stroke > 0.0;
        if kind == ToolKind::Ball {
            let r = self.origin[3];
            if swept {
                let dir = if sweep == ToolSweep::AlongAxis {
                    a
                } else {
                    self.sweep_dir_vec()
                };
                out.push(Piece::Ball {
                    c: c - dir * stroke,
                    r,
                });
                out.push(Piece::Ball {
                    c: c + dir * stroke,
                    r,
                });
                out.push(Piece::Frustum {
                    c,
                    a: dir,
                    hl: stroke,
                    r0: r,
                    r1: r,
                });
            } else {
                out.push(Piece::Ball { c, r });
            }
            return Some(out);
        }
        let (segs, n, peak_z, peak_r) = self.segments(kind);
        let seg_piece = |s: &Segment, shift: Vec3| Piece::Frustum {
            c: c + a * (0.5 * (s.z_lo + s.z_hi)) + shift,
            a,
            hl: 0.5 * (s.z_hi - s.z_lo),
            r0: s.r_lo,
            r1: s.r_hi,
        };
        for s in &segs[..n] {
            if !swept {
                out.push(seg_piece(s, Vec3::ZERO));
            } else if sweep == ToolSweep::AlongAxis {
                let shift = if s.r_hi >= s.r_lo {
                    -a * stroke
                } else {
                    a * stroke
                };
                out.push(seg_piece(s, shift));
            } else {
                let dir = self.sweep_dir_vec();
                out.push(seg_piece(s, -dir * stroke));
                out.push(seg_piece(s, dir * stroke));
                out.push(Piece::Wedge {
                    c: c + a * (0.5 * (s.z_lo + s.z_hi)),
                    a,
                    d: dir,
                    hl: 0.5 * (s.z_hi - s.z_lo),
                    s: stroke,
                    r0: s.r_lo,
                    r1: s.r_hi,
                });
            }
        }
        if swept && sweep == ToolSweep::AlongAxis {
            out.push(Piece::Frustum {
                c: c + a * peak_z,
                a,
                hl: stroke,
                r0: peak_r,
                r1: peak_r,
            });
        }
        Some(out)
    }

    /// Analytic inside test, used by tests and debug assertions.
    #[must_use]
    pub fn contains(&self, p: Vec3) -> bool {
        self.pieces()
            .is_some_and(|ps| ps.as_slice().iter().any(|pc| pc.sd_normal(p).0 <= 0.0))
    }

    /// Smallest `|signed distance estimate|` over the pieces: never larger than
    /// the true distance to a piece surface, so a point with a large gap is
    /// safely away from every boundary. Test support.
    #[cfg(test)]
    fn boundary_gap(&self, p: Vec3) -> f32 {
        self.pieces().map_or(0.0, |ps| {
            ps.as_slice()
                .iter()
                .map(|pc| pc.sd_normal(p).0.abs())
                .fold(f32::INFINITY, f32::min)
        })
    }

    /// Outward normal of the TOOL at a surface point (callers negate it for the
    /// stone: the tool's outward normal points into the stone material, the
    /// stone's outward normal into the cavity).
    ///
    /// For a union the owner of the surface point is the piece whose surface it
    /// is closest to; at a seam between pieces that is a tie and the first
    /// piece wins, deterministically.
    #[must_use]
    pub fn outward_normal(&self, p: Vec3) -> Vec3 {
        let Some(ps) = self.pieces() else {
            return Vec3::Z;
        };
        let mut best = (f32::INFINITY, Vec3::Z);
        for pc in ps.as_slice() {
            let (sd, n) = pc.sd_normal(p);
            if sd.abs() < best.0 {
                best = (sd.abs(), n);
            }
        }
        best.1
    }

    /// Entry/exit parameters of the ray inside the tool, or `None`.
    ///
    /// `[min c, max d]` over the convex pieces; valid because the union of the
    /// pieces is convex (a Minkowski sum of a convex tool with a segment).
    #[must_use]
    pub fn ray_interval(&self, ray: Ray) -> Option<(f32, f32)> {
        let ps = self.pieces()?;
        let mut hull: Option<(f32, f32)> = None;
        for pc in ps.as_slice() {
            if let Some((c, d)) = pc.interval(ray) {
                hull = Some(hull.map_or((c, d), |(lo, hi)| (lo.min(c), hi.max(d))));
            }
        }
        hull
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift32, as in the other seeded generators in this workspace.
    struct Rng(u32);

    impl Rng {
        fn next_u32(&mut self) -> u32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            x
        }

        /// Uniform in `[-1, 1)`.
        fn signed(&mut self) -> f32 {
            (self.next_u32() >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        }

        fn unit_vec(&mut self) -> Vec3 {
            loop {
                let v = Vec3::new(self.signed(), self.signed(), self.signed());
                let l = v.length();
                if (0.1..=1.0).contains(&l) {
                    return v / l;
                }
            }
        }
    }

    fn base_tool(kind: ToolKind) -> ToolPrimitive {
        let centre = Vec3::new(0.1, -0.2, 0.15);
        let axis = Vec3::new(0.3, 0.5, 0.8).normalize();
        match kind {
            ToolKind::Ball => ToolPrimitive::ball(centre, 0.4),
            ToolKind::Cylinder => ToolPrimitive::cylinder(centre, axis, 0.3, 0.5),
            ToolKind::Frustum => ToolPrimitive::frustum(centre, axis, 0.2, 0.45, 0.5),
            ToolKind::Bicone => ToolPrimitive::bicone(centre, axis, 0.45, 0.5),
        }
    }

    fn with_sweep_kind(t: ToolPrimitive, sweep: ToolSweep) -> ToolPrimitive {
        let axis = t.axis_vec();
        let dir = axis.any_orthonormal_vector();
        t.with_sweep(sweep, 0.3, dir)
    }

    #[test]
    fn tool_primitive_layout_is_80_bytes_and_16_aligned() {
        assert_eq!(std::mem::size_of::<ToolPrimitive>(), 80);
        assert_eq!(std::mem::align_of::<ToolPrimitive>(), 16);
    }

    #[test]
    fn tool_kind_and_sweep_round_trip_through_u32_and_reject_unknown_values() {
        for k in [
            ToolKind::Ball,
            ToolKind::Cylinder,
            ToolKind::Frustum,
            ToolKind::Bicone,
        ] {
            assert_eq!(ToolKind::try_from(k as u32), Ok(k));
        }
        for s in [ToolSweep::None, ToolSweep::AlongAxis, ToolSweep::AcrossAxis] {
            assert_eq!(ToolSweep::try_from(s as u32), Ok(s));
        }
        assert_eq!(ToolKind::try_from(4), Err(4));
        assert_eq!(ToolSweep::try_from(9), Err(9));
    }

    #[test]
    fn tool_primitive_validate_rejects_each_bad_field() {
        let good = base_tool(ToolKind::Cylinder);
        assert_eq!(good.validate(), Ok(()));
        for sweep in [ToolSweep::AlongAxis, ToolSweep::AcrossAxis] {
            assert_eq!(with_sweep_kind(good, sweep).validate(), Ok(()));
        }

        let mut t = good;
        t.origin[1] = f32::NAN;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::NonFinite));
        let mut t = good;
        t.axis[3] = f32::INFINITY;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::NonFinite));

        let mut t = good;
        t.origin[3] = -0.1;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));
        let mut t = good;
        t.origin[3] = 0.0;
        t.profile[0] = 0.0;
        t.profile[1] = 0.0;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));
        let mut t = good;
        t.axis[3] = 0.0;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));
        let mut t = good;
        t.profile[2] = -1.0;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));

        let mut t = good;
        t.axis[0] *= 1.5;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::AxisNotUnit));
        let across = with_sweep_kind(good, ToolSweep::AcrossAxis);
        let mut t = across;
        t.sweep_dir = [t.axis[0], t.axis[1], t.axis[2], 0.0];
        assert_eq!(t.validate(), Err(ToolPrimitiveError::AxisNotUnit));

        let mut t = good;
        t.kind = 7;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::UnknownKind(7)));
        let mut t = good;
        t.sweep_kind = 5;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::UnknownSweep(5)));

        let mut t = good;
        t._pad[1] = 1;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::PaddingNotZero));
        let mut t = good;
        t.profile[3] = 1.0;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::PaddingNotZero));
        let mut t = good;
        t.sweep_dir[3] = 1.0;
        assert_eq!(t.validate(), Err(ToolPrimitiveError::PaddingNotZero));
    }

    #[test]
    fn tool_interval_agrees_with_point_classification_on_seeded_rays() {
        let mut rng = Rng(0x9E37_79B9);
        for kind in [
            ToolKind::Ball,
            ToolKind::Cylinder,
            ToolKind::Frustum,
            ToolKind::Bicone,
        ] {
            for sweep in [ToolSweep::None, ToolSweep::AlongAxis, ToolSweep::AcrossAxis] {
                let tool = with_sweep_kind(base_tool(kind), sweep);
                assert_eq!(tool.validate(), Ok(()), "{kind:?} {sweep:?}");
                let centre = tool.centre();
                let scale = tool.origin[3].max(tool.profile[0]).max(tool.profile[1]);
                let mut hit_rays = 0usize;
                for _ in 0..10_000 {
                    let from = centre + rng.unit_vec() * 3.0;
                    let aim = centre + rng.unit_vec() * (0.9 * rng.signed().abs());
                    let ray = Ray {
                        origin: from,
                        dir: (aim - from).normalize(),
                    };
                    let interval = tool.ray_interval(ray);
                    hit_rays += usize::from(interval.is_some());
                    for i in 0..64u32 {
                        let t = 1.0 + 4.0 * (i as f32 + 0.5) / 64.0;
                        let p = ray.origin + ray.dir * t;
                        if tool.boundary_gap(p) < 1e-4 * scale {
                            continue;
                        }
                        let in_interval = interval.is_some_and(|(c, d)| t >= c && t <= d);
                        assert_eq!(
                            in_interval,
                            tool.contains(p),
                            "{kind:?} {sweep:?} t={t} interval={interval:?}"
                        );
                    }
                }
                assert!(
                    hit_rays > 500,
                    "{kind:?} {sweep:?}: only {hit_rays} rays hit, the test would be vacuous"
                );
            }
        }
    }

    #[test]
    fn stone_geometry_counts_planes_and_tools_and_reports_convexity() {
        let planes = [GpuFacetPlane::new(Vec3::X, -1.0)];
        let tools = [ToolPrimitive::ball(Vec3::ZERO, 0.1)];
        let convex = StoneGeometry::planes_only(&planes);
        assert!(convex.is_convex());
        assert_eq!(convex.facet_count(), 1);
        let carved = StoneGeometry {
            planes: &planes,
            tools: &tools,
        };
        assert!(!carved.is_convex());
        assert_eq!(carved.facet_count(), 2);
    }
}
