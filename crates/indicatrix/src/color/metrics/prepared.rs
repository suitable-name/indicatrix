//! Everything a metrics evaluation needs that does not depend on the camera pose.
//!
//! A tilt sweep evaluates the same stone under the same light 724 times, and only the camera
//! frame differs between the evaluations. [`PreparedEval`] holds the rest -- the plane arena,
//! the fan scale (a hash over every plane plus a thread-local cache lookup), the three
//! refractive indices, the lighting (a boxed `StudioRig` and the median of 96 radiance
//! lookups) and the `DIAG_FIRE_DEBUG` environment lookup -- so a sweep builds it once and
//! every pose, on any thread, only reads it. Each field is a pure function of
//! `(planes, tools, material, environment)`, so building it once changes no result.

use super::{fan::FanGeometry, lighting::ExitLighting, ray_trace::StoneArena};
use crate::{
    geometry::tool::{StoneGeometry, ToolPrimitive},
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, build_plane_soa},
    },
    simd::PlanesSoA32,
};

/// The pose-independent half of one metrics evaluation; see the module docs.
///
/// Plain data (no interior mutability), so a sweep shares one by reference across its
/// worker threads.
pub(super) struct PreparedEval<'a> {
    /// SIMD slab arena of the stone's planes: every ray any pose fires intersects it.
    plane_soa: PlanesSoA32,
    /// The tools subtracted from the polyhedron (empty for a planar stone).
    tools: &'a [ToolPrimitive],
    /// Where the fan's rays start, scaled to the stone.
    pub(super) fan: FanGeometry,
    /// Refractive index at the d-line (clamped to at least 1.1).
    pub(super) nd: f32,
    /// Refractive index at the F-line, clamped defensively like `nd` so the entry
    /// refraction never hits the pathological `EntryBlocked` case.
    pub(super) n_f: f32,
    /// Refractive index at the C-line, clamped like `n_f`.
    pub(super) n_c: f32,
    /// The illumination an exit direction is judged against: the radiance the tracer lights
    /// the image with, relative to its ambient level (see `ExitLighting`).
    pub(super) lighting: ExitLighting<'a>,
    /// The material name when `DIAG_FIRE_DEBUG` is set (the diagnostic lines print it),
    /// `None` otherwise. Looked up once here, not once per pose.
    pub(super) diag_material: Option<String>,
}

impl<'a> PreparedEval<'a> {
    /// Prepares `geom` made of `material` under `environment`; `None` when there are no
    /// planes to trace (every pose then reports the neutral placeholder values).
    pub(super) fn new(
        geom: StoneGeometry<'a>,
        material: &GemMaterial,
        environment: EnvironmentSource<'a>,
    ) -> Option<Self> {
        let planes = geom.planes;
        if planes.is_empty() {
            return None;
        }
        Some(Self {
            plane_soa: build_plane_soa(planes),
            tools: geom.tools,
            // The fan is scaled to the stone once (the measurement itself is also reused
            // while the design is unchanged -- see `FanGeometry`).
            fan: FanGeometry::for_planes(planes),
            nd: material.dispersion.evaluate(589.3).max(1.1),
            n_f: material.dispersion.evaluate(486.1).max(1.001),
            n_c: material.dispersion.evaluate(656.3).max(1.001),
            lighting: ExitLighting::new(environment),
            diag_material: std::env::var("DIAG_FIRE_DEBUG")
                .is_ok()
                .then(|| material.name.clone()),
        })
    }

    /// The stone every ray of every pose is traced against.
    pub(super) const fn stone(&self) -> StoneArena<'_> {
        StoneArena {
            plane_soa: &self.plane_soa,
            tools: self.tools,
        }
    }
}
