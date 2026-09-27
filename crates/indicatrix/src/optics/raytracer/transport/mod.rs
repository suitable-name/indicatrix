//! The spectral transport loop: [`trace_spectral_ray`]/[`trace_spectral_ray_with_finish`]
//! (public entries).
//!
//! [`trace_spectral_ray_with_finish_soa`] is the same, with the `PlanesSoA32`
//! intersection arena built once by the caller instead of per sample.
//! [`trace_spectral_ray_inner`](inner::trace_spectral_ray_inner) is their shared bounce
//! loop, plus the per-bounce facet dispatch and Russian-roulette/mode-coupling
//! machinery they drive.
//!
//! [`trace_spectral_ray_inner`](inner::trace_spectral_ray_inner) builds the per-trace
//! `ExitSplitCtx` and threads it alongside `radiance`/`path_pdf` into
//! [`dispatch_bounce`](bounce::dispatch_bounce) -- see `refraction`'s "Exit-event
//! spectral splitting" doc comment for the estimator. `enable_exit_splitting` is
//! hardcoded `true` at every public entry point; only this module's `#[cfg(test)]`
//! tests exercise it as an explicit A/B parameter (see `tests::exit_splitting_tests`).
//!
//! Split from a single `transport.rs` into this module tree by seam: [`bounce`] holds
//! the per-bounce facet dispatch and Russian-roulette/mode-coupling machinery,
//! [`inner`] holds the shared bounce loop itself, and this file keeps the public entry
//! points and the module docs. Every path reachable as `transport::X` before the split
//! stays reachable at exactly that path via the re-export below.

use super::{
    camera::{FacetFinish, HitRecord, Ray},
    environment::EnvironmentSource,
    intersect::build_plane_soa,
};
use crate::{geometry::plane::GpuFacetPlane, optics::materials::GemMaterial};
use glam::Vec3;

mod bounce;
mod inner;
#[cfg(test)]
mod tests;

// bounce.rs -- `scattering::try_scatter_step`'s own trailing Russian-roulette call
// needs this too, so it is re-exported at `pub(super)` (raytracer + descendants),
// exactly the scope it had directly in `transport.rs` before the split.
pub(super) use bounce::apply_russian_roulette;

/// Builds the `N`-member wrapped hero-wavelength comb from a single `hero_rand`.
///
/// Per `GEMSTONE_RENDERING_BLUEPRINT.md`'s Algorithm 2, line 5: `wavelengths[i] = 380 +
/// fmod((lambda_hero - 380) + i*(400/N), 400)`, with `lambda_hero = 380 +
/// hero_rand*400` drawn over the full visible range (`hero_rand` in `[0, 1)`). Every
/// generated wavelength stays within `[380, 780]` and the hero always lands at index 0
/// by construction. Uses a const-generic array (`std::array::from_fn`) rather than a
/// `Vec`, so calling it from the hot per-sample path costs no heap allocation.
#[must_use]
pub fn wrapped_hero_wavelengths<const N: usize>(hero_rand: f32) -> [f32; N] {
    const SPECTRUM_SPAN: f32 = 780.0 - 380.0;
    let channel_width = SPECTRUM_SPAN / N as f32;
    let lambda_hero = hero_rand.mul_add(SPECTRUM_SPAN, 380.0);
    std::array::from_fn(|k| {
        let offset = (k as f32).mul_add(channel_width, lambda_hero - 380.0);
        380.0 + offset.rem_euclid(SPECTRUM_SPAN)
    })
}

/// Why a traced path's bounce loop stopped, for the `bounce_cost` benchmark harness
/// (`examples/bounce_cost.rs`). Maps 1:1 onto
/// [`trace_spectral_ray_inner`](inner::trace_spectral_ray_inner)'s break/fall-through
/// sites:
/// - [`Self::Escaped`]: the ray missed every facet (or exited the polyhedron) and the
///   environment was sampled.
/// - [`Self::ScatterAbsorbed`]: `try_scatter_step` returned
///   `ScatterStepOutcome::ScatteredAndTerminated` -- only reachable for a material
///   with nonzero `scattering_sigma_s`; plain Beer-Lambert absorption never terminates
///   a path on its own, it only dims `stokes` until [`apply_russian_roulette`]
///   eventually kills it (recorded as [`Self::RussianRoulette`], not this variant).
/// - [`Self::RussianRoulette`]: [`apply_russian_roulette`] drew a kill.
/// - [`Self::HitCap`]: the loop ran out of iterations first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathTermination {
    /// The ray missed the gemstone (or exited it) and the environment was sampled.
    Escaped,
    /// A Henyey-Greenstein scattering event extinguished the path (absorbed).
    ScatterAbsorbed,
    /// Russian roulette's stochastic kill fired.
    RussianRoulette,
    /// The loop ran out of bounces (`max_bounces`) before any of the above happened.
    HitCap,
}

/// Traces a single spectral ray through the 3D gemstone polyhedron with:
/// 1. 8-Channel Stratified Hero Wavelength Spectral Sampling (HWSS)
/// 2. Full 4D Stokes-Mueller Polarized Wave Tracking (with TIR phase shift and Brewster extinction)
/// 3. Birefringent Extraordinary Ray Walk-Off & Optical Doubling
/// 4. Directional Pleochroic Beer-Lambert Absorption Tensors
///
/// # `hero_rand`
///
/// An explicit `[0, 1)` parameter rather than derived internally from `rng_seed`, so
/// callers that want it STRATIFIED across a pixel's sample sequence (every production
/// call site) compute it via `low_discrepancy_base2` + `cranley_patterson_rotate` from
/// the sample and pixel index separately -- information a single opaque `rng_seed`
/// cannot carry. Callers that don't care about stratification can pass
/// `(hash_u32(rng_seed) as f32) / 4_294_967_295.0`. `rng_seed` itself still seeds every
/// per-bounce draw (Fresnel branch, Russian roulette, birefringent split) unchanged.
#[expect(
    clippy::too_many_arguments,
    reason = "the crate's public raytracing entry point: each parameter is one \
              independent input a caller supplies once per traced ray; wrapping it in \
              a context struct would only make every caller build a struct instead of \
              calling a function"
)]
#[must_use]
pub fn trace_spectral_ray(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
) -> Vec3 {
    // Internal o<->e mode re-coupling is always on for the public entry point; the
    // bool parameter exists only so this module's `#[cfg(test)]` tests can A/B it (see
    // `tests::mode_coupling_tests::internal_mode_coupling_changes_zircon_render`).
    // Girdle finish: `&[]` for `facet_finishes` looks up `FacetFinish::default() ==
    // Polished` for every facet, reproducing all-polished behaviour.
    //
    // A single traced ray is not a hot per-sample batch loop, so the `PlanesSoA32`
    // arena is built fresh here -- see [`trace_spectral_ray_with_finish_soa`] for the
    // entry point a caller tracing many samples against the same `planes` should use.
    let plane_soa = build_plane_soa(planes);
    inner::trace_spectral_ray_inner(
        initial_ray,
        planes,
        &plane_soa,
        &[],
        material,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        true,
        true,
        matches!(environment, EnvironmentSource::HdrMap(_)),
        None,
    )
}

/// [`trace_spectral_ray`] with an explicit per-facet `finish`.
///
/// See [`FacetFinish`]'s doc comment for why this is a separate function and its
/// CPU-only status. `facet_finishes[i]` is looked up for `planes[i]`; an index with no
/// entry (a shorter slice, or `&[]`) defaults to `FacetFinish::Polished`, so `&[]` here
/// is equivalent to calling `trace_spectral_ray`.
///
/// A thin wrapper that builds the `PlanesSoA32` arena fresh and delegates to
/// [`trace_spectral_ray_with_finish_soa`] -- see that function's doc comment for the
/// entry point a caller tracing many samples against the same `planes` should use.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray's own reason, plus the per-facet finish slice"
)]
#[must_use]
pub fn trace_spectral_ray_with_finish(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
) -> Vec3 {
    let plane_soa = build_plane_soa(planes);
    trace_spectral_ray_with_finish_soa(
        initial_ray,
        planes,
        &plane_soa,
        facet_finishes,
        material,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
    )
}

/// [`trace_spectral_ray_with_finish`] with the [`crate::simd::PlanesSoA32`] arena built
/// by the CALLER and passed in, rather than rebuilt from `planes` on every call.
///
/// For a batch/frame tracing many samples against the same `planes` (measured:
/// ~10-20% of per-sample cost on a 57-plane cut), build `plane_soa` via
/// [`build_plane_soa`] once outside the sample loop. `planes` must be the exact slice
/// `plane_soa` was built from -- this function does not cheaply verify that itself.
/// Results are bit-identical to [`trace_spectral_ray_with_finish`]'s
/// rebuild-every-call behaviour.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray's own reason, plus the per-facet finish slice and \
              the caller-supplied plane_soa arena alongside the planes slice it was \
              built from"
)]
#[must_use]
pub fn trace_spectral_ray_with_finish_soa(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
) -> Vec3 {
    inner::trace_spectral_ray_inner(
        initial_ray,
        planes,
        plane_soa,
        facet_finishes,
        material,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        true,
        true,
        matches!(environment, EnvironmentSource::HdrMap(_)),
        None,
    )
}

/// Instrumented variant of [`trace_spectral_ray_with_finish`] for the `bounce_cost`
/// benchmark harness.
///
/// Identical computation, plus how many bounces the path took and why it stopped --
/// see [`PathTermination`]'s doc comment.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray's own reason, plus the per-facet finish slice and \
              the bounce_cost harness's own instrumentation"
)]
#[must_use]
pub fn trace_spectral_ray_with_finish_instrumented(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
) -> (Vec3, u32, PathTermination) {
    let plane_soa = build_plane_soa(planes);
    let mut termination = (max_bounces, PathTermination::HitCap);
    let radiance = inner::trace_spectral_ray_inner(
        initial_ray,
        planes,
        &plane_soa,
        facet_finishes,
        material,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        None,
        true,
        true,
        matches!(environment, EnvironmentSource::HdrMap(_)),
        Some(&mut termination),
    );
    (radiance, termination.0, termination.1)
}
