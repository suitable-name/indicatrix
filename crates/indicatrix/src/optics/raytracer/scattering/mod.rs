//! Henyey-Greenstein volumetric scattering and the frosted-facet BSDF.
//!
//! [`maybe_scatter_or_extinguish`]'s per-bounce extinction/scatter estimator, the HG
//! phase function and its importance sampler, and [`apply_frosted_bounce`]'s diffuse
//! reflect/transmit dispatch.
//!
//! Split from a single `scattering.rs` into this module tree by seam: [`hg`] holds the
//! Henyey-Greenstein volumetric estimator and its own next-event-estimation
//! contribution, [`frosted`] holds the frosted-facet BSDF and its exterior-side NEE
//! contribution, and this file keeps the shared [`NeeContext`]/[`balance_heuristic`]/
//! [`frosted_orthonormal_basis`] plumbing both submodules build on. Every path
//! reachable as `scattering::X` before the split stays reachable at exactly that path
//! via the re-exports below.

use super::environment::EnvironmentSource;
use glam::Vec3;

// `pub`, not private: a `pub(crate)` item inside a private module is only reachable
// via this file's re-export, which clippy's `redundant_pub_crate` (nursery) then flags
// as suspicious -- see `raytracer::mod`'s own identical comment for this exact pattern.
// Making the submodule `pub` resolves it without widening any item's own effective
// visibility, still capped at its declared visibility.
pub mod frosted;
pub mod hg;
#[cfg(test)]
mod tests;

// hg.rs
pub(super) use hg::{ScatterStepOutcome, try_scatter_step};
pub(crate) use hg::{maybe_scatter_or_extinguish, sample_henyey_greenstein_direction};
// Used only by the GPU twin checks (`renderer::gpu::transport_check`) and tests.
#[cfg(any(test, feature = "gpu"))]
pub(crate) use hg::henyey_greenstein_phase;
#[cfg(feature = "gpu")]
pub(crate) use hg::nee_contribution_hg_scatter;

// frosted.rs
#[cfg(feature = "gpu")]
pub(crate) use frosted::nee_contribution_frosted_exterior;
pub(crate) use frosted::{apply_frosted_bounce, cosine_weighted_hemisphere};

/// Most boundary crossings the Henyey-Greenstein NEE shadow probe follows on a concave
/// stone before it drops the sample. A groove or dimple costs two crossings (out and
/// back in) per exit, so four covers one cavity plus the final exit.
pub(crate) const HG_NEE_MAX_CROSSINGS: usize = 4;

/// Bundles the per-trace next-event-estimation inputs
/// [`nee_contribution_hg_scatter`] and [`nee_contribution_frosted_exterior`] need -- the
/// environment (only [`EnvironmentSource::HdrMap`] has an importance distribution to
/// sample; see `sample_environment_for_nee`), the intersection arena a shadow ray from
/// an interior scattering point probes against (the exterior frosted NEE uses it only for
/// a concave stone's shadow probe, see that function's doc comment), and whether NEE is switched on for this trace at all.
///
/// `enabled = false` (every procedural-rig scene, and any trace that otherwise opts out)
/// skips every NEE draw entirely -- no RNG consumption, no accumulator touched -- so
/// those traces stay bit-identical to one traced with NEE support absent altogether.
/// `Copy`: every field is either a
/// reference or an `EnvironmentSource` (itself `Copy`), so passing by value is as cheap
/// as passing `&NeeContext<'_>` and avoids an extra indirection at each of this struct's
/// several call sites.
///
/// `pub(crate)`, not `pub(super)`: `renderer::gpu::transport_check`'s Tier 2 ULP check
/// for [`apply_frosted_bounce`] constructs an `enabled: false` instance directly (its
/// existing base-physics comparison predates NEE and stays that way until a dedicated
/// NEE harness lands -- see that check's own doc comment), mirroring why
/// `apply_frosted_bounce` itself is `pub(crate)`.
#[derive(Clone, Copy)]
pub(crate) struct NeeContext<'a> {
    pub(crate) environment: EnvironmentSource<'a>,
    pub(crate) plane_soa: &'a crate::simd::PlanesSoA32,
    /// Convex volumes subtracted from the polyhedron `plane_soa` holds; empty for a
    /// planar stone, which keeps every NEE path on its old convex shortcut. With tools
    /// the shadow probes are real: the exterior NEE must find the sampled direction
    /// unoccluded, and the HG probe walks up to [`HG_NEE_MAX_CROSSINGS`] boundaries.
    pub(crate) tools: &'a [crate::geometry::tool::ToolPrimitive],
    /// `optics::raytracer::transport::trace_spectral_ray_inner`'s own `enable_nee`
    /// parameter -- `true` only at every PUBLIC entry point when `environment` is
    /// `HdrMap` or the analytic `DaylightSun` (`environment_supports_nee`; every other
    /// `Studio` rig has no light-sampling technique to draw from), or when this crate's
    /// own tests force it explicitly for an on/off A-B comparison (mirroring
    /// `ExitSplitCtx::enabled`'s identical precedent). Under the sun only the frosted
    /// exterior NEE fires: the Henyey-Greenstein estimator is HDR-only (see
    /// `nee_contribution_hg_scatter`).
    pub(crate) enabled: bool,
}

/// Balance-heuristic MIS weight for a light-sampling technique with pdf `pdf_a` against a
/// competing (BSDF/phase) technique with pdf `pdf_b`, evaluated at a direction drawn from
/// technique `a` -- `w_a = pdf_a / (pdf_a + pdf_b)`. `0.0` if both densities are
/// non-positive (a genuinely degenerate case: e.g. an all-black environment row, or a
/// direction the phase function assigns zero density), contributing nothing rather than
/// dividing by zero. Symmetric in the sense Veach's balance heuristic requires:
/// `balance_heuristic(a, b) + balance_heuristic(b, a) == 1.0` for any `a, b > 0`.
#[must_use]
pub(super) fn balance_heuristic(pdf_a: f32, pdf_b: f32) -> f32 {
    let denom = pdf_a + pdf_b;
    if denom > 1e-12 { pdf_a / denom } else { 0.0 }
}

/// A stable orthonormal basis `(t, b)` perpendicular to unit vector `n` -- the same
/// branch-minimal construction `birefringence::stable_orthonormal_basis` uses, kept as
/// its own local copy rather than exposing that private helper across modules.
pub(in crate::optics::raytracer) fn frosted_orthonormal_basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let t = (a - n * n.dot(a)).normalize_or_zero();
    let b = n.cross(t);
    (t, b)
}
