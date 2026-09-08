//! The optical objective [`optimize_design`](super::optimize_design) minimizes:
//! [`ObjectiveWeights`]/[`ObjectiveComponents`]/[`ObjectiveFidelity`], the
//! plane-format conversion `indicatrix::color::metrics` needs, and
//! [`evaluate_objective`] itself. See the parent module's doc comment for the
//! measured per-evaluation cost this is built against.

use glam::{DVec3, Vec3};
use indicatrix::{
    color::metrics::{
        PROFILE_AZIMUTHS_DEG, TILT_ANGLES_DEG, evaluate_full_axis_profile_at_azimuth,
        evaluate_gem_optical_metrics,
    },
    geometry::GpuFacetPlane,
    optics::materials::GemMaterial,
};

/// The canonical camera-independent light pose every fixed-pose measurement in this
/// module uses.
///
/// The same `(light_yaw, light_pitch)` value
/// `bridge::preview_render::PREVIEW_LIGHT_YAW`/`PREVIEW_LIGHT_PITCH` uses. Duplicated
/// as a plain `f32` rather than imported since `bridge::preview_render`
/// lives in `apps/indicatrix-cut`, a crate this one must not depend on. Keeping the
/// same numeric convention means an optimized design's objective is measured under
/// the same illumination the editor's own live preview already shows the user.
pub const CANONICAL_LIGHT_YAW: f32 = 0.85;
pub const CANONICAL_LIGHT_PITCH: f32 = 0.95;

/// User-visible, user-adjustable weights for [`ObjectiveComponents`].
///
/// There is no single correct trade-off between a brilliant, wide-open stone (low
/// windowing) and one that holds its light at odd angles (low extinction, strong tilt
/// performance). All three default to `1.0` (equal weight); [`ObjectiveWeights::score`]
/// normalizes by their sum, so scaling all three by the same factor is a no-op and
/// only their RELATIVE sizes matter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectiveWeights {
    /// Weight on `windowing_pct` (lower is better -- light leaking straight through the
    /// pavilion instead of returning to the eye).
    pub windowing: f32,
    /// Weight on `extinction_pct` (lower is better -- light trapped/absorbed instead of
    /// returned).
    pub extinction: f32,
    /// Weight on `100.0 - tilt_brilliance_pct` (so a HIGHER `tilt_brilliance_pct`
    /// always reduces the score).
    pub tilt_brilliance: f32,
}

impl Default for ObjectiveWeights {
    fn default() -> Self {
        Self {
            windowing: 1.0,
            extinction: 1.0,
            tilt_brilliance: 1.0,
        }
    }
}

impl ObjectiveWeights {
    /// Combines `components` into a single 0-100 score (LOWER is better, matching
    /// every `_pct` field's polarity after `tilt_brilliance_pct` is flipped to a
    /// loss). Weights are normalized by their own sum so `score` always stays in
    /// `[0, 100]` regardless of the absolute weight values a user picks. Falls back
    /// to equal weighting when all three are non-positive, rather than dividing by
    /// zero.
    #[must_use]
    pub fn score(&self, components: &ObjectiveComponents) -> f32 {
        let sum = self.windowing + self.extinction + self.tilt_brilliance;
        let (w_win, w_ext, w_tilt) = if sum > 1e-6 {
            (self.windowing, self.extinction, self.tilt_brilliance)
        } else {
            (1.0, 1.0, 1.0)
        };
        let norm = (w_win + w_ext + w_tilt).max(1e-6);
        let weighted = w_ext.mul_add(components.extinction_pct, w_win * components.windowing_pct);
        w_tilt.mul_add(100.0 - components.tilt_brilliance_pct, weighted) / norm
    }
}

/// The individual optical measurements [`evaluate_objective`] reports, BEFORE they are
/// weighted into a single score.
///
/// If the optimizer improved windowing by wrecking extinction, the user must be able
/// to see that -- always reported alongside the blended [`ObjectiveWeights::score`],
/// never instead of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectiveComponents {
    pub windowing_pct: f32,
    pub extinction_pct: f32,
    /// Mean `brilliance_pct` across every sample point [`ObjectiveFidelity`] evaluates
    /// (HIGHER is better -- unlike the other two fields, this is not itself a loss;
    /// [`ObjectiveWeights::score`] flips its polarity).
    pub tilt_brilliance_pct: f32,
}

/// How thoroughly [`evaluate_objective`] samples a design's tilt behavior -- see this
/// module's own doc comment's "Cost first" section for the measured numbers behind this
/// split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveFidelity {
    /// A single canonical pose (table-up, `cam_yaw = 0`) -- one call to
    /// [`evaluate_gem_optical_metrics`], ~1.9 ms. Carries NO tilt information at all
    /// (a stone can look perfect table-up and window badly at 30 degrees) -- traded
    /// away deliberately so a search can afford many more candidates per unit
    /// wall-clock time. Intended for [`super::optimize_design`]'s inner search loop
    /// only, never for the before/after report handed to the user.
    Fast,
    /// The full 4-axis, 181-point `-90..=90` degree tilt sweep -- the exact same
    /// curve the Tilt Performance dialog and the catalogue's own performance
    /// filters are built from. `windowing_pct`/`extinction_pct`/`tilt_brilliance_pct`
    /// are each the plain mean across all 4 x 181 = 724 sample points. Measured at
    /// ~1.3-1.4 s per call -- see the module doc comment.
    Full,
}

/// Converts a [`crate::design::Design`]'s own half-space plane representation
/// (`Design::planes_from_solved`'s `(DVec3, f64)`, `n . x <= m`) to the
/// `GpuFacetPlane` representation `indicatrix::color::metrics` takes, inverting
/// exactly the sign convention `GpuFacetPlane::to_halfspace_f64` documents on
/// itself -- `d = -m` here recovers the same plane, narrowed to `f32` for the
/// raytracer.
pub(super) fn to_gpu_planes(planes: &[(DVec3, f64)]) -> Vec<GpuFacetPlane> {
    planes
        .iter()
        .map(|&(n, m)| GpuFacetPlane::new(Vec3::new(n.x as f32, n.y as f32, n.z as f32), -m as f32))
        .collect()
}

/// Measures a solved design's optical performance at the requested [`ObjectiveFidelity`].
///
/// Takes plane geometry directly (not a [`crate::design::Design`]) so a caller that
/// already solved and meshed a candidate never pays for a second solve just to
/// score it -- the same "already-solved" convention
/// [`crate::manufacturability::check_manufacturability`] uses.
#[must_use]
pub fn evaluate_objective(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    fidelity: ObjectiveFidelity,
) -> ObjectiveComponents {
    match fidelity {
        ObjectiveFidelity::Fast => {
            let m = evaluate_gem_optical_metrics(
                planes,
                material,
                0.0,
                90.0f32.to_radians(),
                CANONICAL_LIGHT_YAW,
                CANONICAL_LIGHT_PITCH,
            );
            ObjectiveComponents {
                windowing_pct: m.windowing_pct,
                extinction_pct: m.extinction_pct,
                tilt_brilliance_pct: m.brilliance_pct,
            }
        }
        ObjectiveFidelity::Full => {
            let mut brilliance_sum = 0.0f64;
            let mut extinction_sum = 0.0f64;
            let mut windowing_sum = 0.0f64;
            let mut n = 0u32;
            for &azimuth in &PROFILE_AZIMUTHS_DEG {
                let (brilliance, extinction, windowing) = evaluate_full_axis_profile_at_azimuth(
                    planes,
                    material,
                    azimuth,
                    CANONICAL_LIGHT_YAW,
                    CANONICAL_LIGHT_PITCH,
                );
                for i in 0..TILT_ANGLES_DEG.len() {
                    brilliance_sum += f64::from(brilliance[i]);
                    extinction_sum += f64::from(extinction[i]);
                    windowing_sum += f64::from(windowing[i]);
                    n += 1;
                }
            }
            let n = f64::from(n.max(1));
            ObjectiveComponents {
                windowing_pct: (windowing_sum / n) as f32,
                extinction_pct: (extinction_sum / n) as f32,
                tilt_brilliance_pct: (brilliance_sum / n) as f32,
            }
        }
    }
}
