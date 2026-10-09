//! Path-length kernels: how much of a straight segment lies in each zone.
//!
//! # One source, two precisions
//!
//! The kernel code is written once, in the macro `zone_kernel_impl!` below, and instantiated
//! twice: `k64` (f64, the reference) and `k32` (f32, the GPU reference). Both therefore have
//! the identical operation order, the same fixed-size arrays and no heap use, which is what a
//! WGSL port copies. Operation order notes for the port:
//!
//! * no fused multiply-add anywhere (plain `a * b + c`);
//! * a segment is `p(t) = p0 + t * dv`, `t` in `[0, 1]`, in the zone frame (the segment is
//!   moved there with the transposed frame rotation, rows `rows[i]`, minus `origin`);
//! * every interval routine returns at most [`SPAN_CAP`] sorted spans in `t`;
//! * sorting is a fixed insertion sort; Gauss-Legendre rules are the 8-point constants below;
//! * all lengths are reported as fractions of `t` times `|dv|`.
//!
//! # Semantics
//!
//! Zones override in order: at each point the LAST zone containing it wins, else the base.
//! The lengths therefore partition the segment and sum to its length.
//!
//! * Sharp (`boundary_softness_mm == 0`): the segment is cut at the exact endpoints of every
//!   zone's span set (segment intersect shape, solved analytically per shape) and each piece
//!   goes to the last zone whose spans contain its midpoint.
//! * Soft (`> 0`): each zone has a smoothstep weight `w_z(p) = S(clamp(sd_z(p) / width + 0.5))`
//!   with `S(x) = 3x^2 - 2x^3` and `sd_z` the signed depth into the zone (positive inside, zero
//!   on the sharp boundary). Weights are applied in order, `m = (1 - w) m_prev + w e_z`, so
//!   the weight of zone `j` is `w_j * prod_{k > j} (1 - w_k)` and the base gets
//!   `prod (1 - w_z)`; they sum to 1. The integral over the segment is Gauss-Legendre (8
//!   points) over sub-intervals cut at every point where some zone's weight starts or stops
//!   changing (the span ends of `sd >= -width/2` and `sd >= +width/2`, found analytically,
//!   plus the slab mid-plane and the cylinder closest approach, where `sd` has a kink). Inside
//!   a band the sub-intervals are further cut into [`SOFT_SUBDIV`] equal parts. For planes
//!   (half space, slab) the integrand on each piece is a polynomial of degree 3 per zone
//!   (at most 12 for the product of four), so the 8-point rule is exact to rounding, which
//!   equals the closed form; the tests compare against it. For cylinders, prisms and sectors
//!   `sd` is not linear in `t` (cylinder: square root; prism and sector: kinks) and the rule
//!   is an approximation with the error bound given by the tests.
//! * `sd` for a prism uses the polygon gauge (the largest side-normal projection), i.e. the
//!   distance measured along side normals; near a prism corner it is not the Euclidean
//!   distance. For a sector it is the distance to the nearer of the two bounding planes of the
//!   wedge (exact inside a convex wedge).
//! * Mesh shells are sharp only and f64 only (validation rejects softness with a mesh). The
//!   shell is tested by brute force over at most 512 triangles: the whole LINE through the
//!   segment is intersected with every triangle, crossings closer than 1e-12 in `t` are
//!   merged (so a line through a shared edge counts once), and the line is inside between the
//!   1st and 2nd crossing, 3rd and 4th, and so on. A line that exactly grazes a vertex of the
//!   shell without crossing is counted as a crossing; that degenerate case is not handled.

use super::{
    MAX_ZONES, ZoneShape, ZonedAbsorption,
    mesh::{self, MeshData},
};
use glam::{DVec3, Vec3};

// The shape discriminants are also the GPU zone table's `kind` values
// (`shaders/zoning/*.wgsl` repeats them; a test compares the two).
pub const K_NONE: u32 = 0;
pub const K_HALF: u32 = 1;
pub const K_SLAB: u32 = 2;
pub const K_CYL: u32 = 3;
pub const K_PRISM: u32 = 4;
pub const K_SECTOR: u32 = 5;

/// The most spans one zone's intersection with a segment can have in the analytic shapes
/// (a tube wall, or the complement of a convex wedge, gives two).
const SPAN_CAP: usize = 2;
/// How many equal parts a Gauss-Legendre piece inside a boundary band is cut into.
pub const SOFT_SUBDIV: usize = 16;

/// One flattened zone as plain `f32` rows: what the GPU zone table
/// (`renderer::buffers::GpuZoneTable`) is filled from.
///
/// The field meanings are those of the kernel's private `Flat` (see `zone_kernel_impl!`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportedZone {
    /// `K_*` discriminant (0 = unused slot).
    pub kind: u32,
    /// Prism side count.
    pub n_sides: u32,
    /// Sector: the span is above pi (the wedge is the complement of a convex one).
    pub wide: bool,
    /// Sector: the span is a full turn.
    pub full: bool,
    /// Axis point.
    pub p: [f32; 3],
    /// Plane normal or axis direction.
    pub d: [f32; 3],
    /// Axis reference direction `u`.
    pub u: [f32; 3],
    /// Axis reference direction `v`.
    pub v: [f32; 3],
    /// Half space: offset. Slab: `offset_min`, `offset_max`. Tube/prism: `r_in`, `r_out`,
    /// prism `phase`.
    pub x: [f32; 3],
    /// Sector: unit direction of `angle_from` in the `(u, v)` plane.
    pub e0: [f32; 2],
    /// Sector: unit direction of `angle_to` in the `(u, v)` plane.
    pub e1: [f32; 2],
}

/// The whole flattened kernel set as plain `f32` data (see [`ExportedZone`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportedKernel {
    /// The shaped zones, in order (unused slots have `kind == 0`).
    pub zones: [ExportedZone; MAX_ZONES],
    /// How many of `zones` are used.
    pub count: usize,
    /// Boundary softness in mm (0 = sharp).
    pub softness: f32,
    /// Rows of the transposed frame rotation: `local_i = rows[i] . (p - origin)`.
    pub rows: [[f32; 3]; 3],
    /// Frame translation (the zone frame's origin in the segments' frame).
    pub origin: [f32; 3],
}

/// The reference direction of an axis; see [`super::ZoneShape`].
pub(super) fn perp_basis(axis: DVec3) -> (DVec3, DVec3) {
    let a = if axis.x.abs() > 0.9 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let u = (a - axis * axis.dot(a)).normalize_or_zero();
    (u, axis.cross(u))
}

macro_rules! zone_kernel_impl {
    ($modname:ident, $f:ty, $pi:expr) => {
        #[allow(
            clippy::unnecessary_cast,
            clippy::suboptimal_flops,
            clippy::excessive_precision,
            clippy::needless_range_loop,
            clippy::many_single_char_names,
            clippy::too_many_lines,
            clippy::cast_lossless,
            clippy::float_cmp,
            clippy::manual_memcpy,
            dead_code
        )]
        pub(super) mod $modname {
            use super::{
                ExportedKernel, ExportedZone, K_CYL, K_HALF, K_NONE, K_PRISM, K_SECTOR, K_SLAB,
                SOFT_SUBDIV, SPAN_CAP, perp_basis,
            };
            use crate::optics::zoning::{MAX_ZONES, ZoneShape, ZonedAbsorption};
            use glam::{DMat3, DVec3};

            type F = $f;
            pub(super) type V3 = [F; 3];

            const TWO_PI: F = 2.0 * $pi;

            /// 8-point Gauss-Legendre nodes on [-1, 1], ascending.
            const GL_X: [F; 8] = [
                -0.960_289_856_497_536_3,
                -0.796_666_477_413_626_7,
                -0.525_532_409_916_329_0,
                -0.183_434_642_495_649_8,
                0.183_434_642_495_649_8,
                0.525_532_409_916_329_0,
                0.796_666_477_413_626_7,
                0.960_289_856_497_536_3,
            ];
            /// The matching weights (sum 2).
            const GL_W: [F; 8] = [
                0.101_228_536_290_376_3,
                0.222_381_034_453_374_5,
                0.313_706_645_877_887_3,
                0.362_683_783_378_362_0,
                0.362_683_783_378_362_0,
                0.313_706_645_877_887_3,
                0.222_381_034_453_374_5,
                0.101_228_536_290_376_3,
            ];

            const BP_SHARP: usize = 2 + MAX_ZONES * SPAN_CAP * 2;
            const BP_SOFT: usize = 2 + MAX_ZONES * (2 * SPAN_CAP * 2 + 1);

            const fn v3(v: DVec3) -> V3 {
                [v.x as F, v.y as F, v.z as F]
            }

            fn dot(a: V3, b: V3) -> F {
                a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
            }

            fn cross2(a: [F; 2], b: [F; 2]) -> F {
                a[0] * b[1] - a[1] * b[0]
            }

            /// Up to `SPAN_CAP` sorted, disjoint spans of the segment parameter `t` in [0, 1].
            #[derive(Clone, Copy)]
            pub(super) struct Spans {
                n: usize,
                s: [[F; 2]; SPAN_CAP],
            }

            impl Spans {
                const EMPTY: Self = Self {
                    n: 0,
                    s: [[0.0; 2]; SPAN_CAP],
                };

                /// The span `[lo, hi]` clipped to [0, 1]; empty when it has no length.
                fn one(lo: F, hi: F) -> Self {
                    let lo = lo.max(0.0);
                    let hi = hi.min(1.0);
                    if hi > lo {
                        let mut out = Self::EMPTY;
                        out.s[0] = [lo, hi];
                        out.n = 1;
                        out
                    } else {
                        Self::EMPTY
                    }
                }

                fn full() -> Self {
                    Self::one(0.0, 1.0)
                }

                const fn push(&mut self, lo: F, hi: F) {
                    if self.n < SPAN_CAP {
                        self.s[self.n] = [lo, hi];
                        self.n += 1;
                    }
                }

                fn intersect(self, other: Self) -> Self {
                    let mut out = Self::EMPTY;
                    for i in 0..self.n {
                        for j in 0..other.n {
                            let lo = self.s[i][0].max(other.s[j][0]);
                            let hi = self.s[i][1].min(other.s[j][1]);
                            if hi > lo {
                                out.push(lo, hi);
                            }
                        }
                    }
                    out
                }

                fn subtract_one(self, lo: F, hi: F) -> Self {
                    if hi <= lo {
                        return self;
                    }
                    let mut out = Self::EMPTY;
                    for i in 0..self.n {
                        let a = self.s[i][0];
                        let b = self.s[i][1];
                        let left_hi = b.min(lo);
                        if left_hi > a {
                            out.push(a, left_hi);
                        }
                        let right_lo = a.max(hi);
                        if b > right_lo {
                            out.push(right_lo, b);
                        }
                    }
                    out
                }

                fn subtract(self, other: Self) -> Self {
                    let mut cur = self;
                    for j in 0..other.n {
                        cur = cur.subtract_one(other.s[j][0], other.s[j][1]);
                    }
                    cur
                }

                fn contains(self, t: F) -> bool {
                    for i in 0..self.n {
                        if t >= self.s[i][0] && t <= self.s[i][1] {
                            return true;
                        }
                    }
                    false
                }
            }

            /// `{ t in [0, 1] : a + b t >= 0 }`.
            fn ge0(a: F, b: F) -> Spans {
                if b == 0.0 {
                    return if a >= 0.0 {
                        Spans::full()
                    } else {
                        Spans::EMPTY
                    };
                }
                let t = -a / b;
                if b > 0.0 {
                    Spans::one(t, 1.0)
                } else {
                    Spans::one(0.0, t)
                }
            }

            /// `{ t : |q0 + t qd| <= r }`, solved about the closest approach (no cancellation).
            fn disc(q0: [F; 2], qd: [F; 2], r: F) -> Spans {
                if r <= 0.0 {
                    return Spans::EMPTY;
                }
                let a = qd[0] * qd[0] + qd[1] * qd[1];
                if a == 0.0 {
                    let c = q0[0] * q0[0] + q0[1] * q0[1];
                    return if c <= r * r {
                        Spans::full()
                    } else {
                        Spans::EMPTY
                    };
                }
                let tc = -(q0[0] * qd[0] + q0[1] * qd[1]) / a;
                let cx = q0[0] + tc * qd[0];
                let cy = q0[1] + tc * qd[1];
                let rem = r * r - (cx * cx + cy * cy);
                if rem <= 0.0 {
                    return Spans::EMPTY;
                }
                let half = (rem / a).sqrt();
                Spans::one(tc - half, tc + half)
            }

            /// `{ t : gauge(q0 + t qd) <= r }` for the regular polygon of apothem `r`.
            fn polygon(q0: [F; 2], qd: [F; 2], n_sides: u32, phase: F, r: F) -> Spans {
                if r <= 0.0 {
                    return Spans::EMPTY;
                }
                let mut s = Spans::full();
                for k in 0..n_sides {
                    let ang = phase + TWO_PI * (k as F) / (n_sides as F);
                    let sa = ang.sin();
                    let ca = ang.cos();
                    let m0 = ca * q0[0] + sa * q0[1];
                    let md = ca * qd[0] + sa * qd[1];
                    s = s.intersect(ge0(r - m0, -md));
                }
                s
            }

            fn polygon_gauge(q: [F; 2], n_sides: u32, phase: F) -> F {
                let mut g = -F::MAX;
                for k in 0..n_sides {
                    let ang = phase + TWO_PI * (k as F) / (n_sides as F);
                    let m = ang.cos() * q[0] + ang.sin() * q[1];
                    if m > g {
                        g = m;
                    }
                }
                g
            }

            /// One zone, flattened: the form a GPU zone table holds.
            #[derive(Clone, Copy)]
            pub(super) struct Flat {
                kind: u32,
                n_sides: u32,
                /// Axis point.
                p: V3,
                /// Plane normal or axis direction.
                d: V3,
                /// Axis reference directions.
                u: V3,
                v: V3,
                /// Half space: offset. Slab: `offset_min`. Tube/prism: `r_in`.
                x0: F,
                /// Slab: `offset_max`. Tube/prism: `r_out`.
                x1: F,
                /// Prism: phase.
                x2: F,
                /// Sector: unit direction of `angle_from` and `angle_to` in the (u, v) plane.
                e0: [F; 2],
                e1: [F; 2],
                /// Sector: span above pi (the wedge is the complement of a convex one).
                wide: bool,
                /// Sector: span of a full turn.
                full: bool,
            }

            impl Flat {
                const NONE: Self = Self {
                    kind: K_NONE,
                    n_sides: 0,
                    p: [0.0; 3],
                    d: [0.0; 3],
                    u: [0.0; 3],
                    v: [0.0; 3],
                    x0: 0.0,
                    x1: 0.0,
                    x2: 0.0,
                    e0: [0.0; 2],
                    e1: [0.0; 2],
                    wide: false,
                    full: false,
                };

                fn set_axis(&mut self, point: DVec3, dir: DVec3) {
                    let (u, v) = perp_basis(dir);
                    self.p = v3(point);
                    self.d = v3(dir);
                    self.u = v3(u);
                    self.v = v3(v);
                }

                fn from_shape(shape: &ZoneShape) -> Self {
                    let mut z = Self::NONE;
                    match shape {
                        ZoneShape::HalfSpace { normal, offset } => {
                            z.kind = K_HALF;
                            z.d = v3(*normal);
                            z.x0 = *offset as F;
                        }
                        ZoneShape::Slab {
                            normal,
                            offset_min,
                            offset_max,
                        } => {
                            z.kind = K_SLAB;
                            z.d = v3(*normal);
                            z.x0 = *offset_min as F;
                            z.x1 = *offset_max as F;
                        }
                        ZoneShape::CoaxialCylinder {
                            axis_point,
                            axis_dir,
                            r_in,
                            r_out,
                        } => {
                            z.kind = K_CYL;
                            z.set_axis(*axis_point, *axis_dir);
                            z.x0 = *r_in as F;
                            z.x1 = *r_out as F;
                        }
                        ZoneShape::CoaxialPrism {
                            axis_point,
                            axis_dir,
                            n_sides,
                            r_in,
                            r_out,
                            phase,
                        } => {
                            z.kind = K_PRISM;
                            z.set_axis(*axis_point, *axis_dir);
                            z.n_sides = *n_sides;
                            z.x0 = *r_in as F;
                            z.x1 = *r_out as F;
                            z.x2 = *phase as F;
                        }
                        ZoneShape::Sector {
                            axis_point,
                            axis_dir,
                            angle_from,
                            angle_to,
                        } => {
                            z.kind = K_SECTOR;
                            z.set_axis(*axis_point, *axis_dir);
                            let span = *angle_to - *angle_from;
                            z.wide = span > std::f64::consts::PI;
                            z.full = span >= std::f64::consts::TAU - 1e-12;
                            z.e0 = [angle_from.cos() as F, angle_from.sin() as F];
                            z.e1 = [angle_to.cos() as F, angle_to.sin() as F];
                        }
                        ZoneShape::MeshShell { .. } => {}
                    }
                    z
                }
            }

            /// The segment's 2-D position about the zone axis: `(q0, qd)` with `q(t) = q0 + t qd`.
            fn q_of(z: &Flat, p0: V3, dv: V3) -> ([F; 2], [F; 2]) {
                let r0 = [p0[0] - z.p[0], p0[1] - z.p[1], p0[2] - z.p[2]];
                ([dot(r0, z.u), dot(r0, z.v)], [dot(dv, z.u), dot(dv, z.v)])
            }

            /// `{ t in [0, 1] : sd_z(p(t)) >= c }`; `c = 0` is the sharp zone.
            fn zone_spans(z: &Flat, p0: V3, dv: V3, c: F) -> Spans {
                match z.kind {
                    K_HALF => ge0(dot(z.d, p0) - z.x0 - c, dot(z.d, dv)),
                    K_SLAB => {
                        let np = dot(z.d, p0);
                        let nd = dot(z.d, dv);
                        ge0(np - z.x0 - c, nd).intersect(ge0(z.x1 - np - c, -nd))
                    }
                    K_CYL | K_PRISM => {
                        let (q0, qd) = q_of(z, p0, dv);
                        let outer = if z.kind == K_CYL {
                            disc(q0, qd, z.x1 - c)
                        } else {
                            polygon(q0, qd, z.n_sides, z.x2, z.x1 - c)
                        };
                        if z.x0 > 0.0 && z.x0 + c > 0.0 {
                            let inner = if z.kind == K_CYL {
                                disc(q0, qd, z.x0 + c)
                            } else {
                                polygon(q0, qd, z.n_sides, z.x2, z.x0 + c)
                            };
                            outer.subtract(inner)
                        } else {
                            outer
                        }
                    }
                    K_SECTOR => {
                        if z.full {
                            return Spans::full();
                        }
                        let (q0, qd) = q_of(z, p0, dv);
                        if z.wide {
                            // Complement of the convex wedge from angle_to round to angle_from.
                            let w = ge0(cross2(z.e1, q0) + c, cross2(z.e1, qd))
                                .intersect(ge0(cross2(q0, z.e0) + c, cross2(qd, z.e0)));
                            Spans::full().subtract(w)
                        } else {
                            ge0(cross2(z.e0, q0) - c, cross2(z.e0, qd))
                                .intersect(ge0(cross2(q0, z.e1) - c, cross2(qd, z.e1)))
                        }
                    }
                    _ => Spans::EMPTY,
                }
            }

            /// Signed depth into the zone at `p(t)` (positive inside, zero on the boundary).
            // The WGSL port is sqrt(q.x * q.x + q.y * q.y); hypot would round differently.
            #[allow(clippy::imprecise_flops)]
            fn depth(z: &Flat, p0: V3, dv: V3, t: F) -> F {
                match z.kind {
                    K_HALF => {
                        let p = [p0[0] + t * dv[0], p0[1] + t * dv[1], p0[2] + t * dv[2]];
                        dot(z.d, p) - z.x0
                    }
                    K_SLAB => {
                        let p = [p0[0] + t * dv[0], p0[1] + t * dv[1], p0[2] + t * dv[2]];
                        let s = dot(z.d, p);
                        (s - z.x0).min(z.x1 - s)
                    }
                    K_CYL | K_PRISM => {
                        let (q0, qd) = q_of(z, p0, dv);
                        let q = [q0[0] + t * qd[0], q0[1] + t * qd[1]];
                        let g = if z.kind == K_CYL {
                            (q[0] * q[0] + q[1] * q[1]).sqrt()
                        } else {
                            polygon_gauge(q, z.n_sides, z.x2)
                        };
                        let outer = z.x1 - g;
                        if z.x0 > 0.0 {
                            outer.min(g - z.x0)
                        } else {
                            outer
                        }
                    }
                    K_SECTOR => {
                        if z.full {
                            return F::MAX;
                        }
                        let (q0, qd) = q_of(z, p0, dv);
                        let q = [q0[0] + t * qd[0], q0[1] + t * qd[1]];
                        if z.wide {
                            -cross2(z.e1, q).min(cross2(q, z.e0))
                        } else {
                            cross2(z.e0, q).min(cross2(q, z.e1))
                        }
                    }
                    _ => -F::MAX,
                }
            }

            fn smoothstep_weight(sd: F, inv_width: F) -> F {
                let x = (sd * inv_width + 0.5).clamp(0.0, 1.0);
                x * x * (3.0 - 2.0 * x)
            }

            fn sort_small(a: &mut [F]) {
                for i in 1..a.len() {
                    let mut j = i;
                    while j > 0 && a[j - 1] > a[j] {
                        a.swap(j - 1, j);
                        j -= 1;
                    }
                }
            }

            /// The whole kernel set for one `ZonedAbsorption`, flattened.
            #[derive(Clone, Copy)]
            pub(super) struct Kernel {
                zones: [Flat; MAX_ZONES],
                count: usize,
                softness: F,
                /// Rows of the transposed frame rotation: `local_i = rows[i] . (p - origin)`.
                rows: [V3; 3],
                origin: V3,
            }

            impl Kernel {
                pub(super) fn new(z: &ZonedAbsorption) -> Self {
                    let rot = DMat3::from_quat(z.frame.rotation.normalize());
                    let mut zones = [Flat::NONE; MAX_ZONES];
                    let count = z.zones.len().min(MAX_ZONES);
                    for i in 0..count {
                        zones[i] = Flat::from_shape(&z.zones[i].shape);
                    }
                    Self {
                        zones,
                        count,
                        softness: f64::from(z.boundary_softness_mm) as F,
                        rows: [v3(rot.x_axis), v3(rot.y_axis), v3(rot.z_axis)],
                        origin: v3(z.frame.translation),
                    }
                }

                /// The flattened kernel as plain `f32` data, for the GPU zone table.
                pub(super) fn export(&self) -> ExportedKernel {
                    let v = |a: V3| [a[0] as f32, a[1] as f32, a[2] as f32];
                    let mut zones = [ExportedZone {
                        kind: K_NONE,
                        n_sides: 0,
                        wide: false,
                        full: false,
                        p: [0.0; 3],
                        d: [0.0; 3],
                        u: [0.0; 3],
                        v: [0.0; 3],
                        x: [0.0; 3],
                        e0: [0.0; 2],
                        e1: [0.0; 2],
                    }; MAX_ZONES];
                    for i in 0..self.count {
                        let z = &self.zones[i];
                        zones[i] = ExportedZone {
                            kind: z.kind,
                            n_sides: z.n_sides,
                            wide: z.wide,
                            full: z.full,
                            p: v(z.p),
                            d: v(z.d),
                            u: v(z.u),
                            v: v(z.v),
                            x: [z.x0 as f32, z.x1 as f32, z.x2 as f32],
                            e0: [z.e0[0] as f32, z.e0[1] as f32],
                            e1: [z.e1[0] as f32, z.e1[1] as f32],
                        };
                    }
                    ExportedKernel {
                        zones,
                        count: self.count,
                        softness: self.softness as f32,
                        rows: [v(self.rows[0]), v(self.rows[1]), v(self.rows[2])],
                        origin: v(self.origin),
                    }
                }

                fn localise_point(&self, p: V3) -> V3 {
                    let r = [
                        p[0] - self.origin[0],
                        p[1] - self.origin[1],
                        p[2] - self.origin[2],
                    ];
                    [
                        dot(self.rows[0], r),
                        dot(self.rows[1], r),
                        dot(self.rows[2], r),
                    ]
                }

                /// The segment in the zone frame: start point and `to - from`.
                pub(super) fn local(&self, from: V3, to: V3) -> (V3, V3) {
                    let a = self.localise_point(from);
                    let b = self.localise_point(to);
                    (a, [b[0] - a[0], b[1] - a[1], b[2] - a[2]])
                }

                /// The sharp spans of zone `j` (0-based among the shaped zones).
                pub(super) fn zone_spans_vec(&self, j: usize, p0: V3, dv: V3) -> Vec<(F, F)> {
                    let s = zone_spans(&self.zones[j], p0, dv, 0.0);
                    s.s[..s.n].iter().map(|&span| <(F, F)>::from(span)).collect()
                }

                /// Lengths per zone (index 0 base) of the segment `from` to `to`, outer frame.
                pub(super) fn lengths(&self, from: V3, to: V3) -> [F; MAX_ZONES + 1] {
                    let (p0, dv) = self.local(from, to);
                    self.lengths_local(p0, dv)
                }

                pub(super) fn lengths_local(&self, p0: V3, dv: V3) -> [F; MAX_ZONES + 1] {
                    let mut out = [0.0; MAX_ZONES + 1];
                    let len = dot(dv, dv).sqrt();
                    if len <= 0.0 || len.is_nan() {
                        return out;
                    }
                    let frac = if self.softness > 0.0 {
                        self.fractions_soft(p0, dv)
                    } else {
                        self.fractions_sharp(p0, dv)
                    };
                    for i in 0..=MAX_ZONES {
                        out[i] = frac[i] * len;
                    }
                    out
                }

                fn fractions_sharp(&self, p0: V3, dv: V3) -> [F; MAX_ZONES + 1] {
                    let mut spans = [Spans::EMPTY; MAX_ZONES];
                    let mut bp: [F; BP_SHARP] = [0.0; BP_SHARP];
                    bp[1] = 1.0;
                    let mut nb = 2;
                    for j in 0..self.count {
                        spans[j] = zone_spans(&self.zones[j], p0, dv, 0.0);
                        for i in 0..spans[j].n {
                            bp[nb] = spans[j].s[i][0];
                            bp[nb + 1] = spans[j].s[i][1];
                            nb += 2;
                        }
                    }
                    sort_small(&mut bp[..nb]);
                    let mut out = [0.0; MAX_ZONES + 1];
                    for i in 0..nb - 1 {
                        let a = bp[i];
                        let b = bp[i + 1];
                        if b <= a {
                            continue;
                        }
                        let mid = 0.5 * (a + b);
                        let mut idx = 0;
                        for j in 0..self.count {
                            if spans[j].contains(mid) {
                                idx = j + 1;
                            }
                        }
                        out[idx] += b - a;
                    }
                    out
                }

                fn fractions_soft(&self, p0: V3, dv: V3) -> [F; MAX_ZONES + 1] {
                    let half = 0.5 * self.softness;
                    let inv_width = 1.0 / self.softness;
                    let mut bp: [F; BP_SOFT] = [0.0; BP_SOFT];
                    bp[1] = 1.0;
                    let mut nb = 2;
                    for j in 0..self.count {
                        let z = &self.zones[j];
                        for level in 0..2 {
                            let c = if level == 0 { -half } else { half };
                            let s = zone_spans(z, p0, dv, c);
                            for i in 0..s.n {
                                bp[nb] = s.s[i][0];
                                bp[nb + 1] = s.s[i][1];
                                nb += 2;
                            }
                        }
                        // Kinks of the signed depth.
                        if z.kind == K_SLAB {
                            let nd = dot(z.d, dv);
                            if nd != 0.0 {
                                let t = (0.5 * (z.x0 + z.x1) - dot(z.d, p0)) / nd;
                                if t > 0.0 && t < 1.0 {
                                    bp[nb] = t;
                                    nb += 1;
                                }
                            }
                        } else if z.kind == K_CYL {
                            let (q0, qd) = q_of(z, p0, dv);
                            let a = qd[0] * qd[0] + qd[1] * qd[1];
                            if a > 0.0 {
                                let t = -(q0[0] * qd[0] + q0[1] * qd[1]) / a;
                                if t > 0.0 && t < 1.0 {
                                    bp[nb] = t;
                                    nb += 1;
                                }
                            }
                        }
                    }
                    sort_small(&mut bp[..nb]);

                    let mut acc = [0.0; MAX_ZONES + 1];
                    for i in 0..nb - 1 {
                        let a = bp[i];
                        let b = bp[i + 1];
                        if b <= a {
                            continue;
                        }
                        let mid = 0.5 * (a + b);
                        let mut banded = false;
                        for j in 0..self.count {
                            if depth(&self.zones[j], p0, dv, mid).abs() < half {
                                banded = true;
                            }
                        }
                        let parts = if banded { SOFT_SUBDIV } else { 1 };
                        let h = (b - a) / (parts as F);
                        for part in 0..parts {
                            let centre = a + h * (part as F) + 0.5 * h;
                            for g in 0..8 {
                                let t = centre + 0.5 * h * GL_X[g];
                                let wgt = 0.5 * h * GL_W[g];
                                let mut w = [0.0; MAX_ZONES];
                                for j in 0..self.count {
                                    w[j] = smoothstep_weight(
                                        depth(&self.zones[j], p0, dv, t),
                                        inv_width,
                                    );
                                }
                                let mut rest: F = 1.0;
                                for j in (0..self.count).rev() {
                                    acc[j + 1] += wgt * w[j] * rest;
                                    rest *= (1.0 - w[j]).max(0.0);
                                }
                                acc[0] += wgt * rest;
                            }
                        }
                    }
                    acc
                }
            }
        }
    };
}

zone_kernel_impl!(k64, f64, std::f64::consts::PI);
zone_kernel_impl!(k32, f32, std::f32::consts::PI);

/// Partition of `[0, 1]` for the dynamic sharp path (used when a mesh shell is present):
/// `spans[j]` are the sorted spans of shaped zone `j`, all inside `[0, 1]`.
fn partition_dyn(spans: &[Vec<(f64, f64)>]) -> [f64; MAX_ZONES + 1] {
    let mut bp: Vec<f64> = vec![0.0, 1.0];
    for zone in spans {
        for &(a, b) in zone {
            bp.push(a);
            bp.push(b);
        }
    }
    bp.sort_by(f64::total_cmp);
    let mut out = [0.0; MAX_ZONES + 1];
    for pair in bp.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b <= a {
            continue;
        }
        let mid = f64::midpoint(a, b);
        let mut idx = 0;
        for (j, zone) in spans.iter().enumerate() {
            if zone.iter().any(|&(lo, hi)| mid >= lo && mid <= hi) {
                idx = j + 1;
            }
        }
        out[idx] += b - a;
    }
    out
}

/// The f64 reference kernel for one [`ZonedAbsorption`], built once and queried per segment.
///
/// Building flattens the zones (and copies mesh shells); querying does no allocation unless a
/// mesh shell is present. Assumes [`ZonedAbsorption::validate`] passed.
#[derive(Clone)]
pub struct ZoneKernel {
    flat: k64::Kernel,
    meshes: Vec<Option<MeshData>>,
    has_mesh: bool,
}

impl ZoneKernel {
    /// Flattens `z`. Zones beyond [`MAX_ZONES`] are ignored.
    #[must_use]
    pub fn new(z: &ZonedAbsorption) -> Self {
        let meshes: Vec<Option<MeshData>> = z
            .zones
            .iter()
            .take(MAX_ZONES)
            .map(|zone| match &zone.shape {
                ZoneShape::MeshShell {
                    vertices,
                    triangles,
                } => Some(MeshData {
                    vertices: vertices.clone(),
                    triangles: triangles.clone(),
                }),
                _ => None,
            })
            .collect();
        let has_mesh = meshes.iter().any(Option::is_some);
        Self {
            flat: k64::Kernel::new(z),
            meshes,
            has_mesh,
        }
    }

    /// Length of the segment `from` to `to` (outer frame) inside each zone: index 0 is the
    /// base zone, then the shaped zones in order. Entries are non-negative and sum to the
    /// segment length.
    #[must_use]
    pub fn lengths(&self, from: DVec3, to: DVec3) -> [f64; MAX_ZONES + 1] {
        let a = from.to_array();
        let b = to.to_array();
        if !self.has_mesh {
            return self.flat.lengths(a, b);
        }
        let (p0, dv) = self.flat.local(a, b);
        let len = f64::mul_add(dv[2], dv[2], f64::mul_add(dv[1], dv[1], dv[0] * dv[0])).sqrt();
        let mut out = [0.0; MAX_ZONES + 1];
        if len <= 0.0 || len.is_nan() {
            return out;
        }
        let mut spans: Vec<Vec<(f64, f64)>> = Vec::with_capacity(self.meshes.len());
        for (j, mesh_data) in self.meshes.iter().enumerate() {
            spans.push(mesh_data.as_ref().map_or_else(
                || self.flat.zone_spans_vec(j, p0, dv),
                |m| mesh::line_spans(m, p0, dv),
            ));
        }
        let frac = partition_dyn(&spans);
        for (o, f) in out.iter_mut().zip(frac) {
            *o = f * len;
        }
        out
    }
}

/// The f32 twin of [`ZoneKernel`]: the GPU reference. Same code, single precision, no
/// allocation, no mesh shells.
#[derive(Clone, Copy)]
pub struct ZoneKernelF32 {
    flat: k32::Kernel,
}

impl ZoneKernelF32 {
    /// `None` when `z` has a [`ZoneShape::MeshShell`] (f64 only).
    #[must_use]
    pub fn new(z: &ZonedAbsorption) -> Option<Self> {
        if z.zones
            .iter()
            .any(|zone| matches!(zone.shape, ZoneShape::MeshShell { .. }))
        {
            return None;
        }
        Some(Self {
            flat: k32::Kernel::new(z),
        })
    }

    /// As [`ZoneKernel::lengths`], in `f32`.
    #[must_use]
    pub fn lengths(&self, from: Vec3, to: Vec3) -> [f32; MAX_ZONES + 1] {
        self.flat.lengths(from.to_array(), to.to_array())
    }

    /// The flattened zones, frame and softness as plain `f32` data: what the GPU zone table
    /// (`renderer::buffers::GpuZoneTable`) is encoded from, so the GPU kernel works from the
    /// very numbers this kernel does.
    #[must_use]
    pub fn export(&self) -> ExportedKernel {
        self.flat.export()
    }
}

/// Length of the segment `from` to `to` inside each zone of `z`, f64 reference.
///
/// Index 0 is the base zone, then the shaped zones in order. Non-negative and summing to the
/// segment length. Builds a [`ZoneKernel`] per call; build one yourself when querying many segments.
#[must_use]
pub fn zone_lengths(z: &ZonedAbsorption, from: DVec3, to: DVec3) -> [f64; MAX_ZONES + 1] {
    ZoneKernel::new(z).lengths(from, to)
}

/// The f32 twin of [`zone_lengths`] with the exact operation order a WGSL port copies (see the
/// module notes). `None` when `z` contains a mesh shell, which only the f64 path supports.
#[must_use]
pub fn zone_lengths_f32(z: &ZonedAbsorption, from: Vec3, to: Vec3) -> Option<[f32; MAX_ZONES + 1]> {
    ZoneKernelF32::new(z).map(|k| k.lengths(from, to))
}
