//! Colour zoning: absorption that changes from place to place inside one stone or rough
//! (the `zoning` feature; plan 2026-10-09 sections 7.1, 7.2 and 8).
//!
//! A [`ZonedAbsorption`] is a base zone plus up to [`MAX_ZONES`] shaped zones. Each zone has its
//! own [`AbsorptionTensor`] in per-millimetre units ([`ZoneAbsorption`]). Later zones override
//! earlier ones where they overlap, and zone 0 (the base) fills everything else. Zones share the
//! refractive index, so path geometry is unchanged; only the optical depth of a straight
//! segment changes, from `alpha(lambda) * length` to `sum_z alpha_z(lambda) * length_z`.
//!
//! # Units and frames
//!
//! * All geometry (offsets, radii, axis points, mesh vertices, the softness width) is in
//!   millimetres in the zone's own frame. [`ZonedAbsorption::frame`] maps that frame into the
//!   frame of the segments you ask about (rough frame, then stone frame after
//!   [`ZonedAbsorption::transformed`]). A segment is always given in the OUTER frame.
//! * The tracer works in model units. Multiply model-unit points by the model-unit to millimetre
//!   scale before calling the kernels here, or use [`ZonedAbsorption::scaled`] to bring the
//!   geometry into model units (the "zone geometry relative to stone" rule for library
//!   materials). Absorption coefficients are per millimetre and are never touched by `scaled`.
//!
//! # Contents
//!
//! * the types of plan section 7.1 and [`ZonedAbsorption::validate`];
//! * path-length kernels: [`zone_lengths`] (f64 reference), [`zone_lengths_f32`] (the f32 twin
//!   whose operation order a WGSL port copies), and the reusable [`ZoneKernel`] /
//!   [`ZoneKernelF32`] that skip the per-call set-up;
//! * [`segment_optical_depth`], the one function the CPU tracer will call per segment.
//!
//! Nothing here is wired into the tracer, the GPU layout or `GemMaterial`.

mod kernels;
mod mesh;
#[cfg(test)]
mod tests;

pub use kernels::{
    ExportedKernel, ExportedZone, ZoneKernel, ZoneKernelF32, zone_lengths, zone_lengths_f32,
};
#[allow(
    unused_imports,
    reason = "read by the GPU zone table and its WGSL-constant test"
)]
pub(crate) use kernels::{K_CYL, K_HALF, K_NONE, K_PRISM, K_SECTOR, K_SLAB, SOFT_SUBDIV};

use crate::optics::{
    absorption::AbsorptionTensor, birefringence::AbsorptionTensor3, materials::AbsorptionUnit,
    raytracer::spectral_absorption,
};
use glam::{DQuat, DVec3, Vec3};

/// The most zones (besides the base) a [`ZonedAbsorption`] may hold (owner decision 3).
pub const MAX_ZONES: usize = 4;
/// The fewest sides of a [`ZoneShape::CoaxialPrism`].
pub const MIN_PRISM_SIDES: u32 = 3;
/// The most sides of a [`ZoneShape::CoaxialPrism`].
pub const MAX_PRISM_SIDES: u32 = 12;
/// The most triangles a [`ZoneShape::MeshShell`] may hold. The shell is tested by brute force
/// (every triangle against the segment line), which at this size costs far less than a BVH
/// would save.
pub const MAX_MESH_TRIANGLES: usize = 512;

/// A rigid transform: rotate by `rotation`, then add `translation` (millimetres).
///
/// `ZoneFrame::point(p) = rotation * p + translation`, the same convention as the rig `Rigid`
/// in `indicatrix-cut-core` (whose rotation vector converts with
/// [`ZoneFrame::from_rotation_vector`]).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ZoneFrame {
    /// Unit quaternion.
    pub rotation: DQuat,
    /// Translation in millimetres.
    pub translation: DVec3,
}

impl ZoneFrame {
    /// No rotation, no translation.
    pub const IDENTITY: Self = Self {
        rotation: DQuat::IDENTITY,
        translation: DVec3::ZERO,
    };

    /// From a rotation vector (axis times angle, radians) and a translation, the layout the
    /// rig `Rigid` stores.
    #[must_use]
    pub fn from_rotation_vector(rotation_vector: [f64; 3], translation: [f64; 3]) -> Self {
        Self {
            rotation: DQuat::from_scaled_axis(DVec3::from_array(rotation_vector)),
            translation: DVec3::from_array(translation),
        }
    }

    /// A point of the inner frame in the outer frame.
    #[must_use]
    pub fn point(&self, p: DVec3) -> DVec3 {
        self.rotation * p + self.translation
    }

    /// A direction of the inner frame in the outer frame.
    #[must_use]
    pub fn direction(&self, d: DVec3) -> DVec3 {
        self.rotation * d
    }

    /// A point of the outer frame in the inner frame.
    #[must_use]
    pub fn inverse_point(&self, p: DVec3) -> DVec3 {
        self.rotation.inverse() * (p - self.translation)
    }

    /// The inverse transform.
    #[must_use]
    pub fn inverse(&self) -> Self {
        let rotation = self.rotation.inverse();
        Self {
            rotation,
            translation: -(rotation * self.translation),
        }
    }

    /// `self` after `inner`: `compose(a, b).point(p) == a.point(b.point(p))`.
    #[must_use]
    pub fn compose(&self, inner: &Self) -> Self {
        Self {
            rotation: (self.rotation * inner.rotation).normalize(),
            translation: self.rotation * inner.translation + self.translation,
        }
    }
}

impl Default for ZoneFrame {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// The geometry of one zone, in the zone frame ([`ZonedAbsorption::frame`]), millimetres.
///
/// Directions (`normal`, `axis_dir`) must be unit vectors ([`ZonedAbsorption::validate`]
/// checks; the kernels do not normalise).
///
/// "Inside" is where the zone applies:
///
/// * `HalfSpace`: `normal . p >= offset`;
/// * `Slab`: `offset_min <= normal . p <= offset_max`;
/// * `CoaxialCylinder`: `r_in <= r <= r_out`, `r` the distance from the axis (`r_in = 0` is a
///   solid rod, `r_in > 0` a tube wall); infinite along the axis;
/// * `CoaxialPrism`: the same with the regular `n_sides`-gon whose apothem (centre to side
///   distance) is the radius, so `r_in`/`r_out` are apothems; infinite along the axis. The
///   side normals point at angles `phase + 2 pi k / n_sides` about the axis, measured from
///   the reference direction of the axis (see below);
/// * `Sector`: the wedge swept counter-clockwise (about `axis_dir`) from `angle_from` to
///   `angle_to`, radians, measured from the reference direction of the axis; unbounded in
///   radius and along the axis. A span of `2 pi` is all space;
/// * `MeshShell`: the inside of the closed triangle shell.
///
/// The reference direction `u` of an axis is deterministic: take the world X axis (the Y axis
/// when `|axis_dir.x| > 0.9`), remove its component along `axis_dir`, normalise. The second
/// direction is `v = axis_dir x u`. Angles and `phase` are measured from `u` towards `v`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ZoneShape {
    /// Everything on the `normal` side of a plane (layers, bicolour).
    HalfSpace {
        /// Unit normal.
        normal: DVec3,
        /// Plane offset along the normal, mm.
        offset: f64,
    },
    /// The band between two parallel planes.
    Slab {
        /// Unit normal.
        normal: DVec3,
        /// Lower plane offset, mm.
        offset_min: f64,
        /// Upper plane offset, mm.
        offset_max: f64,
    },
    /// A round tube or rod around an axis (round watermelon).
    CoaxialCylinder {
        /// A point of the axis, mm.
        axis_point: DVec3,
        /// Unit axis direction.
        axis_dir: DVec3,
        /// Inner radius, mm (0 for a solid rod).
        r_in: f64,
        /// Outer radius, mm.
        r_out: f64,
    },
    /// A regular prism around an axis (trigonal or hexagonal watermelon, tourmaline).
    CoaxialPrism {
        /// A point of the axis, mm.
        axis_point: DVec3,
        /// Unit axis direction.
        axis_dir: DVec3,
        /// Number of sides, 3 to 12.
        n_sides: u32,
        /// Inner apothem, mm (0 for a solid prism).
        r_in: f64,
        /// Outer apothem, mm.
        r_out: f64,
        /// Angle of the first side normal from the reference direction, radians.
        phase: f64,
    },
    /// A wedge around an axis (ametrine, trapiche).
    Sector {
        /// A point of the axis, mm.
        axis_point: DVec3,
        /// Unit axis direction.
        axis_dir: DVec3,
        /// Start angle, radians.
        angle_from: f64,
        /// End angle, radians (greater than `angle_from`, at most a full turn later).
        angle_to: f64,
    },
    /// A freeform closed triangle shell (sharp edge only, f64 only).
    MeshShell {
        /// Vertex positions, mm.
        vertices: Vec<[f64; 3]>,
        /// Triangles as vertex indices, consistently wound (closed, watertight).
        triangles: Vec<[u32; 3]>,
    },
}

/// The absorption of one zone: a tensor of absorption bands in per-millimetre units.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ZoneAbsorption {
    /// Band sets per eigenmode, as for [`GemMaterial::absorption`](crate::optics::GemMaterial).
    pub tensor: AbsorptionTensor,
    /// Must be [`AbsorptionUnit::PerMm`] (zoned materials are always physical).
    pub unit: AbsorptionUnit,
}

/// How a pleochroic zone tensor is evaluated along a segment.
///
/// The electric-field direction of the ray's assigned eigenmode and the host crystal's c axis,
/// exactly what the tracer feeds `birefringence::assigned_mode_alpha` (see
/// `raytracer::absorption::channel_absorption_alphas_assigned`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PleochroicMode {
    /// The assigned eigenmode's world-space E-field direction (unit).
    pub e_mode_hat: Vec3,
    /// The crystal's optical (gamma) axis (unit).
    pub c_axis: Vec3,
}

impl ZoneAbsorption {
    /// A per-millimetre zone absorption.
    #[must_use]
    pub const fn per_mm(tensor: AbsorptionTensor) -> Self {
        Self {
            tensor,
            unit: AbsorptionUnit::PerMm,
        }
    }

    /// The absorption coefficient at `lambda_nm`, per millimetre.
    ///
    /// * Not pleochroic: the `o_ray` band sum (`o_ray == e_ray` for such tensors).
    /// * Pleochroic and `mode == Some`: the same evaluation the tracer uses on an anisotropic
    ///   host, `AbsorptionTensor3::{uniaxial, biaxial}(..., c_axis).quadratic_form(e_mode_hat)`.
    /// * Pleochroic and `mode == None`: the orientation mean of the principal coefficients,
    ///   `(2 o + e) / 3` (uniaxial) or `(o + beta + e) / 3` (biaxial). This is the value for a
    ///   randomly oriented crystal or an unpolarised average; it is NOT what the tracer does
    ///   per bounce, so the tracer passes `Some`.
    ///
    /// Bands are evaluated in `f32`, like the tracer.
    #[must_use]
    pub fn alpha(&self, lambda_nm: f64, mode: Option<&PleochroicMode>) -> f64 {
        let lambda = lambda_nm as f32;
        let tensor = &self.tensor;
        let o = spectral_absorption(&tensor.o_ray, lambda);
        if !tensor.is_pleochroic {
            return f64::from(o);
        }
        let e = spectral_absorption(&tensor.e_ray, lambda);
        let beta = tensor
            .beta_ray
            .as_ref()
            .map(|bands| spectral_absorption(bands, lambda));
        mode.map_or_else(
            || {
                let mean = beta.map_or_else(|| 2.0f32.mul_add(o, e) / 3.0, |b| (o + b + e) / 3.0);
                f64::from(mean)
            },
            |m| {
                let tensor3 = beta.map_or_else(
                    || AbsorptionTensor3::uniaxial(o, e, m.c_axis),
                    |b| AbsorptionTensor3::biaxial(o, b, e, m.c_axis),
                );
                f64::from(tensor3.quadratic_form(m.e_mode_hat))
            },
        )
    }

    /// The tracer's branch for an isotropic-by-symmetry host (`!ctx.is_anisotropic`): the
    /// midpoint of the quadratic forms of the two eigen directions. For a non-pleochroic
    /// tensor this equals [`Self::alpha`].
    #[must_use]
    pub fn alpha_midpoint(
        &self,
        lambda_nm: f64,
        eigen_a: Vec3,
        eigen_b: Vec3,
        c_axis: Vec3,
    ) -> f64 {
        let lambda = lambda_nm as f32;
        let tensor = &self.tensor;
        let o = spectral_absorption(&tensor.o_ray, lambda);
        if !tensor.is_pleochroic {
            return f64::from(o);
        }
        let e = spectral_absorption(&tensor.e_ray, lambda);
        let tensor3 = tensor
            .beta_ray
            .as_ref()
            .map(|bands| spectral_absorption(bands, lambda))
            .map_or_else(
                || AbsorptionTensor3::uniaxial(o, e, c_axis),
                |b| AbsorptionTensor3::biaxial(o, b, e, c_axis),
            );
        f64::from(f32::midpoint(
            tensor3.quadratic_form(eigen_a),
            tensor3.quadratic_form(eigen_b),
        ))
    }

    fn validate(&self, zone: Option<usize>) -> Result<(), ZoningError> {
        if self.unit != AbsorptionUnit::PerMm {
            return Err(ZoningError::NotPerMm { zone });
        }
        let bad_band = |bands: &[crate::optics::absorption::AbsorptionBand]| {
            bands.iter().any(|b| {
                !(b.center_nm.is_finite()
                    && b.width_nm.is_finite()
                    && b.width_nm > 0.0
                    && b.peak.is_finite()
                    && b.peak >= 0.0)
            })
        };
        let t = &self.tensor;
        if bad_band(&t.o_ray)
            || bad_band(&t.e_ray)
            || t.beta_ray.as_ref().is_some_and(|b| bad_band(b))
        {
            return Err(ZoningError::BadBands { zone });
        }
        Ok(())
    }
}

/// One shaped zone: where ([`ZoneShape`]) and what ([`ZoneAbsorption`]).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Zone {
    /// Where the zone applies.
    pub shape: ZoneShape,
    /// What the zone absorbs.
    pub absorption: ZoneAbsorption,
}

/// Colour zoning of one stone or rough: a base zone plus ordered, overriding zones.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ZonedAbsorption {
    /// Zone geometry frame relative to the stone or rough frame the segments are given in.
    pub frame: ZoneFrame,
    /// The outermost / default zone (zone index 0 in length arrays).
    pub base: ZoneAbsorption,
    /// Ordered zones (indices 1.. in length arrays); later ones override earlier ones.
    pub zones: Vec<Zone>,
    /// 0 for sharp boundaries; otherwise the width in mm of the smoothstep blend centred on
    /// each boundary. Must be 0 when a [`ZoneShape::MeshShell`] is present.
    pub boundary_softness_mm: f32,
}

impl ZonedAbsorption {
    /// One zone (the base) and nothing else: identity frame, sharp, no shaped zones.
    #[must_use]
    pub const fn new(base: ZoneAbsorption) -> Self {
        Self {
            frame: ZoneFrame::IDENTITY,
            base,
            zones: Vec::new(),
            boundary_softness_mm: 0.0,
        }
    }

    /// The absorption of zone `index` in length-array order (0 is the base).
    #[must_use]
    pub fn zone_absorption(&self, index: usize) -> Option<&ZoneAbsorption> {
        if index == 0 {
            Some(&self.base)
        } else {
            self.zones.get(index - 1).map(|z| &z.absorption)
        }
    }

    /// Checks the zone count, finiteness, shape parameters, the closed and consistently
    /// wound mesh shells, the softness rule and the per-millimetre unit of every zone.
    ///
    /// The kernels assume this passed. They never panic on invalid input, but their results
    /// are then unspecified.
    ///
    /// # Errors
    /// The first problem found, see [`ZoningError`].
    pub fn validate(&self) -> Result<(), ZoningError> {
        if self.zones.len() > MAX_ZONES {
            return Err(ZoningError::TooManyZones {
                count: self.zones.len(),
            });
        }
        if !self.boundary_softness_mm.is_finite() || self.boundary_softness_mm < 0.0 {
            return Err(ZoningError::BadSoftness);
        }
        let q = self.frame.rotation;
        if !q.is_finite() || (q.length() - 1.0).abs() > 1e-6 || !self.frame.translation.is_finite()
        {
            return Err(ZoningError::BadFrame);
        }
        self.base.validate(None)?;
        for (i, zone) in self.zones.iter().enumerate() {
            zone.absorption.validate(Some(i + 1))?;
            zone.shape.validate(i + 1)?;
        }
        let has_mesh = self
            .zones
            .iter()
            .any(|z| matches!(z.shape, ZoneShape::MeshShell { .. }));
        if has_mesh && self.boundary_softness_mm > 0.0 {
            return Err(ZoningError::SoftMeshShell);
        }
        Ok(())
    }

    /// The same zoning expressed in a frame that sits `rigid` away: `rigid` maps the current
    /// outer frame (the rough frame) into the new one (the stone frame). Only the zone frame
    /// changes (`new = rigid o old`), so lengths of a segment moved by `rigid` are unchanged.
    #[must_use]
    pub fn transformed(&self, rigid: &ZoneFrame) -> Self {
        let mut out = self.clone();
        out.frame = rigid.compose(&self.frame);
        out
    }

    /// The same zoning with every length multiplied by `factor` (frame translation, offsets,
    /// radii, axis points, mesh vertices and the softness width). Used for "zone geometry
    /// relative to stone" library materials: the geometry is stored for a unit-width stone and
    /// scaled by the stone width. Angles and the per-millimetre coefficients are unchanged.
    /// `factor` must be finite and positive ([`Self::validate`] catches the negative radii
    /// that a bad factor produces).
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        let mut out = self.clone();
        out.frame.translation *= factor;
        out.boundary_softness_mm = (f64::from(self.boundary_softness_mm) * factor) as f32;
        for zone in &mut out.zones {
            match &mut zone.shape {
                ZoneShape::HalfSpace { offset, .. } => *offset *= factor,
                ZoneShape::Slab {
                    offset_min,
                    offset_max,
                    ..
                } => {
                    *offset_min *= factor;
                    *offset_max *= factor;
                }
                ZoneShape::CoaxialCylinder {
                    axis_point,
                    r_in,
                    r_out,
                    ..
                }
                | ZoneShape::CoaxialPrism {
                    axis_point,
                    r_in,
                    r_out,
                    ..
                } => {
                    *axis_point *= factor;
                    *r_in *= factor;
                    *r_out *= factor;
                }
                ZoneShape::Sector { axis_point, .. } => *axis_point *= factor,
                ZoneShape::MeshShell { vertices, .. } => {
                    for v in vertices.iter_mut() {
                        for c in v.iter_mut() {
                            *c *= factor;
                        }
                    }
                }
            }
        }
        out
    }
}

/// `Ok` when `ok`, otherwise [`ZoningError::NotFinite`] for `zone`.
const fn require_finite(zone: usize, ok: bool, what: &'static str) -> Result<(), ZoningError> {
    if ok {
        Ok(())
    } else {
        Err(ZoningError::NotFinite {
            zone: Some(zone),
            what,
        })
    }
}

/// `Ok` when `v` is a finite unit vector, otherwise [`ZoningError::NotUnit`] for `zone`.
fn require_unit(zone: usize, v: DVec3, what: &'static str) -> Result<(), ZoningError> {
    if v.is_finite() && (v.length() - 1.0).abs() <= 1e-6 {
        Ok(())
    } else {
        Err(ZoningError::NotUnit { zone, what })
    }
}

/// The inner and outer radius of a tube or prism: `0 <= r_in < r_out`.
fn require_radii(zone: usize, r_in: f64, r_out: f64) -> Result<(), ZoningError> {
    if r_in < 0.0 || r_in >= r_out {
        return Err(ZoningError::BadRadii { zone });
    }
    Ok(())
}

impl ZoneShape {
    fn validate(&self, zone: usize) -> Result<(), ZoningError> {
        match self {
            Self::HalfSpace { normal, offset } => {
                require_finite(zone, normal.is_finite() && offset.is_finite(), "half space")?;
                require_unit(zone, *normal, "normal")
            }
            Self::Slab {
                normal,
                offset_min,
                offset_max,
            } => {
                require_finite(
                    zone,
                    normal.is_finite() && offset_min.is_finite() && offset_max.is_finite(),
                    "slab",
                )?;
                require_unit(zone, *normal, "normal")?;
                if offset_min >= offset_max {
                    return Err(ZoningError::BadSlab { zone });
                }
                Ok(())
            }
            Self::CoaxialCylinder {
                axis_point,
                axis_dir,
                r_in,
                r_out,
            } => {
                require_finite(
                    zone,
                    axis_point.is_finite() && r_in.is_finite() && r_out.is_finite(),
                    "cylinder",
                )?;
                require_unit(zone, *axis_dir, "axis_dir")?;
                require_radii(zone, *r_in, *r_out)
            }
            Self::CoaxialPrism {
                axis_point,
                axis_dir,
                n_sides,
                r_in,
                r_out,
                phase,
            } => {
                require_finite(
                    zone,
                    axis_point.is_finite()
                        && r_in.is_finite()
                        && r_out.is_finite()
                        && phase.is_finite(),
                    "prism",
                )?;
                require_unit(zone, *axis_dir, "axis_dir")?;
                if !(MIN_PRISM_SIDES..=MAX_PRISM_SIDES).contains(n_sides) {
                    return Err(ZoningError::BadSideCount {
                        zone,
                        n_sides: *n_sides,
                    });
                }
                require_radii(zone, *r_in, *r_out)
            }
            Self::Sector {
                axis_point,
                axis_dir,
                angle_from,
                angle_to,
            } => {
                require_finite(
                    zone,
                    axis_point.is_finite() && angle_from.is_finite() && angle_to.is_finite(),
                    "sector",
                )?;
                require_unit(zone, *axis_dir, "axis_dir")?;
                let span = angle_to - angle_from;
                if span <= 0.0 || span > std::f64::consts::TAU + 1e-9 {
                    return Err(ZoningError::BadAngles { zone });
                }
                Ok(())
            }
            Self::MeshShell {
                vertices,
                triangles,
            } => mesh::check(vertices, triangles)
                .map_err(|problem| ZoningError::BadMesh { zone, problem }),
        }
    }
}

/// Why a [`ZonedAbsorption`] is invalid.
///
/// `zone` is `None` for the base zone and the 1-based zone index otherwise (the same index
/// the length arrays use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoningError {
    /// More than [`MAX_ZONES`] shaped zones.
    TooManyZones {
        /// The number given.
        count: usize,
    },
    /// A parameter is NaN or infinite.
    NotFinite {
        /// Which zone.
        zone: Option<usize>,
        /// Which parameter group.
        what: &'static str,
    },
    /// A direction is not a unit vector (tolerance 1e-6).
    NotUnit {
        /// Which zone (1-based).
        zone: usize,
        /// Which direction.
        what: &'static str,
    },
    /// The zone's unit is not [`AbsorptionUnit::PerMm`].
    NotPerMm {
        /// Which zone.
        zone: Option<usize>,
    },
    /// An absorption band has a non-finite value, a non-positive width or a negative peak.
    BadBands {
        /// Which zone.
        zone: Option<usize>,
    },
    /// A slab with `offset_min >= offset_max`.
    BadSlab {
        /// Which zone (1-based).
        zone: usize,
    },
    /// A cylinder or prism with `r_in < 0` or `r_in >= r_out`.
    BadRadii {
        /// Which zone (1-based).
        zone: usize,
    },
    /// A prism with fewer than 3 or more than 12 sides.
    BadSideCount {
        /// Which zone (1-based).
        zone: usize,
        /// The number given.
        n_sides: u32,
    },
    /// A sector whose span is not in `(0, 2 pi]`.
    BadAngles {
        /// Which zone (1-based).
        zone: usize,
    },
    /// The softness width is negative or not finite.
    BadSoftness,
    /// The frame rotation is not a finite unit quaternion, or the translation is not finite.
    BadFrame,
    /// A mesh shell is present and `boundary_softness_mm > 0` (mesh edges are sharp only).
    SoftMeshShell,
    /// The mesh shell is not a valid closed, consistently wound triangle mesh.
    BadMesh {
        /// Which zone (1-based).
        zone: usize,
        /// What is wrong.
        problem: MeshProblem,
    },
}

/// What is wrong with a [`ZoneShape::MeshShell`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshProblem {
    /// Fewer than 4 triangles.
    TooFewTriangles,
    /// More than [`MAX_MESH_TRIANGLES`] triangles.
    TooManyTriangles,
    /// A triangle index is outside the vertex list.
    IndexOutOfRange,
    /// A vertex is NaN or infinite.
    NotFinite,
    /// A triangle repeats a vertex or has no area.
    DegenerateTriangle,
    /// Some edge has no opposite partner: the shell has a hole.
    NotClosed,
    /// Two triangles use the same directed edge (flipped neighbour, or an edge shared by
    /// more than two triangles).
    InconsistentWinding,
    /// The enclosed volume is zero.
    ZeroVolume,
}

fn zone_label(zone: Option<usize>) -> String {
    zone.map_or_else(|| "base zone".to_owned(), |i| format!("zone {i}"))
}

impl core::fmt::Display for ZoningError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooManyZones { count } => {
                write!(f, "{count} zones given, at most {MAX_ZONES} are allowed")
            }
            Self::NotFinite { zone, what } => {
                write!(f, "{}: {what} has a non-finite value", zone_label(*zone))
            }
            Self::NotUnit { zone, what } => {
                write!(f, "zone {zone}: {what} is not a unit vector")
            }
            Self::NotPerMm { zone } => {
                write!(
                    f,
                    "{}: zoned absorption must be per millimetre",
                    zone_label(*zone)
                )
            }
            Self::BadBands { zone } => write!(
                f,
                "{}: an absorption band has a non-finite value, a non-positive width or a negative peak",
                zone_label(*zone)
            ),
            Self::BadSlab { zone } => {
                write!(f, "zone {zone}: slab offset_min must be below offset_max")
            }
            Self::BadRadii { zone } => {
                write!(f, "zone {zone}: radii must satisfy 0 <= r_in < r_out")
            }
            Self::BadSideCount { zone, n_sides } => write!(
                f,
                "zone {zone}: a prism has {MIN_PRISM_SIDES} to {MAX_PRISM_SIDES} sides, not {n_sides}"
            ),
            Self::BadAngles { zone } => {
                write!(f, "zone {zone}: sector span must be in (0, 2 pi]")
            }
            Self::BadSoftness => write!(f, "boundary softness must be finite and not negative"),
            Self::BadFrame => write!(
                f,
                "zone frame must be a finite unit rotation and translation"
            ),
            Self::SoftMeshShell => {
                write!(
                    f,
                    "a mesh shell has sharp edges only; set the boundary softness to 0"
                )
            }
            Self::BadMesh { zone, problem } => {
                write!(f, "zone {zone}: mesh shell problem {problem:?}")
            }
        }
    }
}

impl std::error::Error for ZoningError {}

/// The optical depth of the straight segment `from` to `to` (outer-frame points, in the
/// units of the zone geometry, mm): `sum_z alpha_z(lambda) * length_z`.
///
/// This is the isotropic / orientation-mean form, [`ZoneAbsorption::alpha`] with
/// `mode = None`. For a pleochroic tensor along a ray whose eigenmode is known use
/// [`segment_optical_depth_mode`].
#[must_use]
pub fn segment_optical_depth(z: &ZonedAbsorption, from: DVec3, to: DVec3, lambda_nm: f64) -> f64 {
    segment_optical_depth_mode(z, from, to, lambda_nm, None)
}

/// As [`segment_optical_depth`], with the pleochroic evaluation the tracer uses
/// (`mode = Some`, see [`ZoneAbsorption::alpha`]).
#[must_use]
pub fn segment_optical_depth_mode(
    z: &ZonedAbsorption,
    from: DVec3,
    to: DVec3,
    lambda_nm: f64,
    mode: Option<&PleochroicMode>,
) -> f64 {
    optical_depth_from_lengths(z, &zone_lengths(z, from, to), lambda_nm, mode)
}

/// `sum_z alpha_z(lambda) * lengths[z]` for lengths already computed (by [`ZoneKernel`], say).
#[must_use]
pub fn optical_depth_from_lengths(
    z: &ZonedAbsorption,
    lengths: &[f64; MAX_ZONES + 1],
    lambda_nm: f64,
    mode: Option<&PleochroicMode>,
) -> f64 {
    let mut depth = z.base.alpha(lambda_nm, mode) * lengths[0];
    for (i, zone) in z.zones.iter().take(MAX_ZONES).enumerate() {
        depth = f64::mul_add(
            zone.absorption.alpha(lambda_nm, mode),
            lengths[i + 1],
            depth,
        );
    }
    depth
}
