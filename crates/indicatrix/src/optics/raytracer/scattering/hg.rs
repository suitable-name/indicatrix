//! Homogeneous Henyey-Greenstein volumetric scattering.
//!
//! The per-bounce extinction/scatter estimator ([`maybe_scatter_or_extinguish`]), its
//! bounce-loop wrapper ([`try_scatter_step`]), and the phase function and its importance
//! sampler. The scattering-point next-event-estimation contribution the wrapper calls
//! lives in [`super::hg_nee`].

use super::{
    super::{
        NUM_CHANNELS,
        absorption::channel_absorption_alphas_assigned,
        camera::{FacetFinish, Ray},
        color::{integrate_channels_to_xyz, integrate_channels_to_xyz_families},
        environment::EnvironmentSource,
        refraction::{RayMaterialContext, RayWavelengthCache},
        sampling::{DISTANCE_SAMPLE_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM, hash_u32},
        transport::apply_russian_roulette,
        zoned::{ZoneAlphas, ZonedCache, local_extinction, optical_depths},
    },
    NeeContext, frosted_orthonormal_basis,
    hg_nee::{ShadowAlphas, nee_contribution_hg_scatter},
};
use crate::optics::{materials::GemMaterial, polarization::StokesVector};
use glam::Vec3;

/// Homogeneous Henyey-Greenstein volumetric scattering, decided/sampled once for the
/// shared hero-driven geometric path and reweighted per channel -- the same structural
/// pattern [`apply_partial_fresnel_bounce`]/[`apply_refract_channel`] use for every
/// other stochastic decision in this module tree.
///
/// `alphas` is the caller's precomputed [`channel_absorption_alphas_assigned`] result
/// (computed once in `try_scatter_step` below) rather than derived internally, so
/// `shaders/transport_physics.wgsl`'s WGSL translation can share this exact signature.
///
/// # The estimator, hazard by hazard
///
/// 1. **Extinction, not absorption.** `sigma_t = sigma_a + sigma_s` (`sigma_a` is this
///    channel's chromatic [`channel_absorption_alphas_assigned`] value, unchanged from
///    [`apply_absorption`]; `sigma_s` is the material's achromatic
///    [`GemMaterial::scattering_sigma_s`]). A scattering event REDIRECTS the path
///    rather than destroying its energy -- loss happens only through `sigma_a`.
/// 2. **Every stochastic decision divides by the SAME value that drove it.** One
///    `[0, 1)` random (`DISTANCE_SAMPLE_STREAM`) inverts the HERO channel's own
///    exponential CDF (`sigma_t_hero`) into a free-path distance `t_free`. Both
///    branches divide by the density that draw has under the hero's own technique
///    (`pdf_hero` scattering; `survive_hero` surviving) -- never a per-channel value.
///    Each companion channel k's own physical weight (`tr_k`/`survive_k`, from k's own
///    `sigma_t_k`) sits in the numerator, and `path_pdf[k]` is updated with k's own
///    technique density for the final `spectral_mis_weight` combination. For
///    `k == hero_idx` both branches collapse to `sigma_s / sigma_t_hero` (the
///    single-scattering albedo) and exactly `1.0` -- the standard unbounded-free-path-
///    sampling identities. Genuinely different estimator SHAPE than `apply_absorption`'s
///    deterministic `exp(-alpha*path_len)` (unbiased only in expectation), which is why
///    this is gated behind `scattering_sigma_s > 0.0` rather than relied on to reduce
///    to the old path algebraically at `sigma_s -> 0`.
/// 3. **HG phase/pdf cancellation.** [`sample_henyey_greenstein_direction`]
///    importance-samples exactly the HG phase function's own normalized distribution,
///    so `phase / pdf == 1.0` identically -- mirroring `cosine_weighted_hemisphere`'s
///    cancellation for the frosted BSDF.
/// 4. **Achromatic direction and distance.** Exactly one `t_free` and one scattered
///    direction (from the hero's `sigma_t_hero` and the material's achromatic
///    `scattering_g`) are sampled and reused for every channel, so -- unlike the
///    polished path's `direction_matches` guard -- there is no chromatic-termination
///    check: there is only ever one direction to match.
/// 5. **New, decorrelated RNG streams** ([`DISTANCE_SAMPLE_STREAM`],
///    [`PHASE_DIR_U_STREAM`], [`PHASE_DIR_V_STREAM`]), mirrored bit-for-bit in
///    `shaders/transport_physics.wgsl`.
///
/// # Depolarization
///
/// Like [`apply_frosted_bounce`](super::apply_frosted_bounce), every channel's Stokes
/// vector collapses to `StokesVector::unpolarized` at a scatter event: incoherent
/// scattering off an inclusion's internal structure scrambles the phase relationship
/// polarization depends on.
///
/// # Return value
///
/// `Some((t_free, new_dir))` if a scatter event fired strictly before `hit_t` (caller
/// must redirect `current_ray` from `t_free` along its OLD direction, not advance all
/// the way to the facet); `None` if the path survived to the facet boundary
/// (`stokes`/`path_pdf` already carry that survival's extinction weight -- the caller
/// must NOT also call [`apply_absorption`] for this same segment).
///
/// `pub(crate)`: `renderer::gpu::transport_check`'s Tier 2 ULP check calls this real
/// function directly, comparing against the WGSL translation. Visibility only.
#[expect(
    clippy::too_many_arguments,
    reason = "argument list mirrors transport_physics.wgsl's own maybe_scatter_or_extinguish \
              one-for-one; bundling into a struct would break that direct correspondence"
)]
pub(crate) fn maybe_scatter_or_extinguish(
    alphas: &[f32; NUM_CHANNELS],
    sigma_s_model: f32,
    g: f32,
    hero_idx: usize,
    ray_dir: Vec3,
    hit_t: f32,
    path_scale: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Option<(f32, Vec3)> {
    // `sigma_s_model` is per MODEL unit and independent of the stone's size
    // (`GemMaterial::scattering_sigma_s`); `alphas` are per absorption-length unit. Express the
    // scattering coefficient in absorption-length units too, so the extinction below stays one
    // consistent medium: `sigma_s * (hit_t * path_scale) == sigma_s_model * hit_t`.
    // `path_scale == 1.0` makes the division an exact no-op.
    let sigma_s = sigma_s_model / path_scale;
    let sigma_t_hero = alphas[hero_idx] + sigma_s;

    let dist_rand =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ DISTANCE_SAMPLE_STREAM)) as f32) / 4_294_967_295.0;
    // Unbounded exponential free-path sample (see hazard 2 above). `one_minus_u` is
    // reused as `pdf_hero` below (`exp(-sigma_t_hero * t_free) == one_minus_u`
    // algebraically) rather than recomputing the exponential. `t_free` is in
    // ABSORPTION-LENGTH units (same as `alphas`/`sigma_s`), converted back below.
    let one_minus_u = (1.0 - dist_rand).max(1e-7);
    let t_free = -(one_minus_u.ln()) / sigma_t_hero;

    // `hit_t` arrives in MODEL units; scale to absorption-length units (see
    // `GemMaterial::absorption_path_scale`). `path_scale == 1.0` (every built-in) makes
    // this an exact no-op, so `hit_t_scaled` is bit-identical to plain `hit_t`.
    let hit_t_scaled = hit_t * path_scale;

    if t_free < hit_t_scaled {
        let pdf_hero = sigma_t_hero * one_minus_u;
        // Vectorized Beer-Lambert; exp_f32x8 is a few-ULP polynomial exponential, NOT
        // bit-identical to f32::exp -- see apply_absorption's comment / src/simd.rs.
        let mut tr_args = [0f32; NUM_CHANNELS];
        for k in 0..NUM_CHANNELS {
            tr_args[k] = -(alphas[k] + sigma_s) * t_free;
        }
        let tr = crate::simd::exp_f32x8(tr_args);
        for k in 0..NUM_CHANNELS {
            let sigma_t_k = alphas[k] + sigma_s;
            let tr_k = tr[k];
            let weight = tr_k * sigma_s / pdf_hero;
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity() * weight);
            path_pdf[k] *= sigma_t_k * tr_k;
        }
        let u1 =
            (hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_U_STREAM)) as f32) / 4_294_967_295.0;
        let u2 =
            (hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_V_STREAM)) as f32) / 4_294_967_295.0;
        let new_dir = sample_henyey_greenstein_direction(u1, u2, g, ray_dir);
        // Convert back to MODEL units (`ray_dir` is a unit vector in model space) before
        // the caller advances `current_ray.origin` along it. `path_scale == 1.0` makes
        // this division an exact no-op; the estimator's weights are unaffected since
        // they're computed entirely in absorption-length units above.
        let t_free_model = t_free / path_scale;
        Some((t_free_model, new_dir))
    } else {
        // Vectorized Beer-Lambert (see the scatter branch above). `survive_hero` is
        // read from this same batched result at `hero_idx` so the hero lane's ratio
        // always comes from the same exponential its companions used.
        let mut survive_args = [0f32; NUM_CHANNELS];
        for k in 0..NUM_CHANNELS {
            survive_args[k] = -(alphas[k] + sigma_s) * hit_t_scaled;
        }
        let survive = crate::simd::exp_f32x8(survive_args);
        let survive_hero = survive[hero_idx];
        for k in 0..NUM_CHANNELS {
            let survive_k = survive[k];
            stokes[k] = stokes[k].scale(survive_k / survive_hero.max(1e-30));
            path_pdf[k] *= survive_k;
        }
        None
    }
}

/// The inputs of [`maybe_scatter_or_extinguish_zoned`] that are not the per-channel state:
/// bundled to keep its argument count down.
pub(in super::super) struct ZonedScatterParams {
    /// `GemMaterial::scattering_sigma_s`, per MODEL unit.
    pub(in super::super) sigma_s_model: f32,
    /// `GemMaterial::scattering_g`.
    pub(in super::super) g: f32,
    pub(in super::super) hero_idx: usize,
    /// The segment's ray (origin and unit direction, model units).
    pub(in super::super) ray: Ray,
    /// Distance to the facet just hit, model units.
    pub(in super::super) hit_t: f32,
    /// `GemMaterial::absorption_path_scale`: millimetres per model unit for a zoned material.
    pub(in super::super) path_scale: f32,
    pub(in super::super) rng_seed: u32,
    pub(in super::super) bounce: u32,
}

/// [`maybe_scatter_or_extinguish`] for a zoned medium: the same estimator
/// with the position-dependent extinction `sigma_t(x) = alpha_z(x) + sigma_s`.
///
/// Along the segment the hero optical depth `tau_h(t)` is piecewise linear and increasing, so
/// the free-flight sample inverts it: the draw `u` gives the target `-ln(1 - u)`, a scatter
/// happens before the facet exactly when `tau_h(hit_t)` exceeds it, and `t_free` is the root
/// of `tau_h(t) = target` (bisection on the kernel's cumulative lengths, 48 halvings at most).
/// Every weight is the homogeneous one with `tau_k(t)` in place of `sigma_t_k * t` and the
/// local `sigma_t_k(x)` at the scatter point in place of `sigma_t_k`:
///
/// * scatter: `weight_k = tr_k * sigma_s / pdf_hero` with `tr_k = exp(-tau_k(t_free))`,
///   `pdf_hero = sigma_t_hero(x) * exp(-tau_h(t_free))` (the same `one_minus_u`), and
///   `path_pdf[k] *= sigma_t_k(x) * tr_k`;
/// * survive: `survive_k = exp(-tau_k(hit_t))`, normalised by the hero's, `path_pdf[k] *= survive_k`.
///
/// For a single zone covering the segment this reduces to [`maybe_scatter_or_extinguish`] up to
/// the bisection's rounding. `zone_alphas` are the per-zone per-channel coefficients of
/// `ZonedCache::zone_alphas`. The caller disables scattering-point NEE for a zoned medium (its
/// shadow-ray transmittance would need the zone lengths of the shadow probe), which keeps the
/// phase-sampled continuation at full weight and the estimator unbiased.
pub(in super::super) fn maybe_scatter_or_extinguish_zoned(
    zoned: &ZonedCache,
    zone_alphas: &ZoneAlphas,
    params: &ZonedScatterParams,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Option<(f32, Vec3)> {
    let ZonedScatterParams {
        sigma_s_model,
        g,
        hero_idx,
        ray,
        hit_t,
        path_scale,
        rng_seed,
        bounce,
    } = *params;
    // Per absorption-length unit, like the homogeneous estimator.
    let sigma_s = sigma_s_model / path_scale;
    let depth_to = |t: f32| -> [f32; NUM_CHANNELS] {
        let lengths = zoned.lengths_mm(ray.origin, ray.dir, 0.0, t, path_scale);
        optical_depths(zone_alphas, &lengths, sigma_s)
    };

    let dist_rand =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ DISTANCE_SAMPLE_STREAM)) as f32) / 4_294_967_295.0;
    let one_minus_u = (1.0 - dist_rand).max(1e-7);
    let target = -(one_minus_u.ln());

    let depth_full = depth_to(hit_t);
    if target < depth_full[hero_idx] {
        // Invert the hero's piecewise-linear optical depth.
        let (mut lo, mut hi) = (0.0f32, hit_t);
        for _ in 0..48 {
            let mid = f32::midpoint(lo, hi);
            if depth_to(mid)[hero_idx] < target {
                lo = mid;
            } else {
                hi = mid;
            }
            if hi - lo <= f32::EPSILON * hi {
                break;
            }
        }
        let t_free = f32::midpoint(lo, hi);

        let sigma_t_local =
            local_extinction(zoned, zone_alphas, ray, t_free, hit_t, path_scale, sigma_s);
        let pdf_hero = sigma_t_local[hero_idx] * one_minus_u;
        let depth_free = depth_to(t_free);
        let mut tr_args = [0f32; NUM_CHANNELS];
        for (a, d) in tr_args.iter_mut().zip(&depth_free) {
            *a = -d;
        }
        let tr = crate::simd::exp_f32x8(tr_args);
        for k in 0..NUM_CHANNELS {
            let weight = tr[k] * sigma_s / pdf_hero;
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity() * weight);
            path_pdf[k] *= sigma_t_local[k] * tr[k];
        }
        let u1 =
            (hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_U_STREAM)) as f32) / 4_294_967_295.0;
        let u2 =
            (hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_V_STREAM)) as f32) / 4_294_967_295.0;
        let new_dir = sample_henyey_greenstein_direction(u1, u2, g, ray.dir);
        Some((t_free, new_dir))
    } else {
        let mut survive_args = [0f32; NUM_CHANNELS];
        for (a, d) in survive_args.iter_mut().zip(&depth_full) {
            *a = -d;
        }
        let survive = crate::simd::exp_f32x8(survive_args);
        let survive_hero = survive[hero_idx];
        for k in 0..NUM_CHANNELS {
            stokes[k] = stokes[k].scale(survive[k] / survive_hero.max(1e-30));
            path_pdf[k] *= survive[k];
        }
        None
    }
}

/// The segment [`try_scatter_step`] tests for a scatter event: the ray, the distance to the
/// facet it hits and the RNG stream identity.
#[derive(Clone, Copy)]
struct ScatterSegment {
    ray: Ray,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
}

/// The scatter-or-survive decision of [`try_scatter_step`]: the position-dependent estimator
/// for a zoned medium (`zoned` is its cache and per-zone alphas), the homogeneous one
/// otherwise.
fn scatter_or_survive(
    zoned: Option<(&ZonedCache, &ZoneAlphas)>,
    alphas: &[f32; NUM_CHANNELS],
    material: &GemMaterial,
    hero_idx: usize,
    segment: ScatterSegment,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Option<(f32, Vec3)> {
    let ScatterSegment {
        ray,
        hit_t,
        rng_seed,
        bounce,
    } = segment;
    let Some((zoned_cache, zone_alphas)) = zoned else {
        return maybe_scatter_or_extinguish(
            alphas,
            material.scattering_sigma_s,
            material.scattering_g,
            hero_idx,
            ray.dir,
            hit_t,
            material.absorption_path_scale,
            rng_seed,
            bounce,
            stokes,
            path_pdf,
        );
    };
    maybe_scatter_or_extinguish_zoned(
        zoned_cache,
        zone_alphas,
        &ZonedScatterParams {
            sigma_s_model: material.scattering_sigma_s,
            g: material.scattering_g,
            hero_idx,
            ray,
            hit_t,
            path_scale: material.absorption_path_scale,
            rng_seed,
            bounce,
        },
        stokes,
        path_pdf,
    )
}

/// What [`try_scatter_step`] found, and what `trace_spectral_ray_inner`'s bounce loop
/// should do about it.
///
/// `pub(in super::super)` (raytracer + descendants), not `pub(super)`: `transport`'s
/// bounce loop (a sibling of `scattering`, not a descendant of it) consumes this
/// directly -- the same scope this type had before the split, when it lived straight
/// in `scattering.rs` (parent: `raytracer`).
pub(in super::super) enum ScatterStepOutcome {
    /// `material.scattering_sigma_s <= 0.0` -- skipped entirely (no RNG draw, no
    /// extinction applied). Caller falls through to the plain facet-dispatch code,
    /// including its own `apply_absorption` call.
    NotApplicable,
    /// No scatter event fired: the path survived to the facet boundary, and
    /// `maybe_scatter_or_extinguish` already applied this segment's extinction weight.
    /// Caller falls through to facet-dispatch but must NOT also call `apply_absorption`.
    ReachedBoundary,
    /// A scatter event fired and the path survived Russian roulette (or wasn't yet
    /// eligible). Caller should `continue` the bounce loop.
    ///
    /// Carries the Henyey-Greenstein phase function's own value at the sampled
    /// continuation direction, paired with that continuation direction itself --
    /// `Some` only when [`NeeContext::enabled`], `None` otherwise (including every
    /// trace with NEE disabled). The caller stashes this as the
    /// pending complementary MIS weight, consumed by whichever LATER bounce-loop
    /// iteration first either escapes directly or dispatches a transmit-out event at
    /// the (necessarily convex) polyhedron's exit facet -- see `transport::dispatch_bounce`'s
    /// doc comment for why the carry must survive that intervening polished-exit
    /// refraction rather than being read only the very next iteration: a scattering
    /// point is strictly interior, so its very next iteration always hits the exit
    /// facet first, never escapes directly there. Once an eventual direct escape
    /// consumes it, that escape is weighted by the balance heuristic against
    /// [`nee_contribution_hg_scatter`]'s own light-sampling technique (evaluated at the
    /// carried INTERIOR direction, the same measure NEE sampled in), rather than
    /// double-counting the same direct-lighting contribution both ways. If a bounce
    /// happens instead while the carry is live -- a reflect, a TIR, another scatter
    /// event, or any hit while still inside the gem -- the pending weight is dropped:
    /// that path has no competing NEE sample left to weigh itself against, so it
    /// counts at full weight.
    ScatteredAndSurvived(Option<(f32, Vec3)>),
    /// A scatter event fired and Russian roulette terminated the path. Caller should
    /// `break` the bounce loop.
    ScatteredAndTerminated,
}

/// Attempts a Henyey-Greenstein scattering event on the segment from `current_ray`'s
/// current origin to the facet just hit (`hit_t` away), mutating
/// `current_ray`/`stokes`/`path_pdf` in place and returning what happened -- extracted
/// out of `trace_spectral_ray_inner`'s bounce loop to keep that function under
/// clippy's line-count lint.
///
/// Gated on `material.scattering_sigma_s > 0.0` -- `<= 0.0` returns
/// [`ScatterStepOutcome::NotApplicable`] immediately, before drawing from any new RNG
/// stream. See [`maybe_scatter_or_extinguish`] for the estimator itself.
///
/// Applies Russian roulette (via the same [`apply_russian_roulette`] the polished/
/// frosted facet-dispatch path calls at the end of every bounce, same `bounce > 4`
/// gate) when a scatter event fires, since it otherwise never reaches the bounce
/// loop's own trailing call.
///
/// `current_k` is the wave normal `k`, while [`maybe_scatter_or_extinguish`]'s
/// `ray_dir` argument stays `current_ray.dir` (`S`) -- different roles. A fired
/// scatter event collapses `k == S` going forward (`*current_k = new_dir`): scattering
/// already depolarizes the Stokes vector, so there's no wave-normal-vs-Poynting
/// distinction left to carry past it.
///
/// `is_extraordinary` names which eigenmode this path was assigned to at its most recent
/// air->crystal entry -- see [`channel_absorption_alphas_assigned`]'s doc comment. Does
/// not need `prev_plane_normal`: the assigned-mode absorption this calls reads no
/// Stokes-frame azimuth at all.
#[expect(
    clippy::too_many_arguments,
    reason = "bundles the fixed-for-the-trace contexts (mat_ctx, cache, nee), this \
              bounce's own state, the RNG stream identity, and the per-ray state a \
              scatter event can mutate -- the same shape dispatch_bounce's reason \
              explains in transport.rs; `lambdas`/`nee_xyz` are the HDR-map NEE \
              contribution's inputs/output, threaded through rather than bundled \
              into `nee` since they vary in a way `NeeContext` (fixed for the whole \
              trace) deliberately does not; `enable_exit_splitting`/`compat` are this \
              moment's own family-integration inputs (see `nee_xyz`'s own doc comment); \
              `facet_finishes` (the per-facet surface finish) is threaded the same way \
              rather than added to `NeeContext` so the GPU Tier 2 harness's \
              `NeeContext` literals stay independent of it"
)]
pub(in super::super) fn try_scatter_step(
    mat_ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    material: &GemMaterial,
    is_extraordinary: bool,
    current_ray: &mut Ray,
    current_k: &mut Vec3,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    split_radiance: &mut [f32; NUM_CHANNELS],
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    // The final XYZ output's own NEE accumulator (`trace_spectral_ray_inner`'s
    // `nee_xyz`) -- see F-09b / this function's own "NEE spectral weighting" doc note
    // below for why a scattering-point NEE deposit is integrated to XYZ HERE, using
    // THIS moment's own `path_pdf`/`compat`, rather than folded into the shared
    // `radiance` array the way it was before (which let it drift, wrongly, onto
    // whatever `path_pdf` the REST of the path happened to end up with).
    nee_xyz: &mut Vec3,
    // `trace_spectral_ray_inner`'s own `enable_exit_splitting` parameter and this
    // moment's `exit_split_ctx.compat` snapshot -- see `nee_xyz`'s doc comment above.
    enable_exit_splitting: bool,
    compat: [u8; NUM_CHANNELS],
    facet_finishes: &[FacetFinish],
) -> ScatterStepOutcome {
    if material.scattering_sigma_s <= 0.0 {
        return ScatterStepOutcome::NotApplicable;
    }
    let alphas = channel_absorption_alphas_assigned(mat_ctx, cache, *current_k, is_extraordinary);
    // A zoned medium: the position-dependent estimator, and no scattering-point NEE (the shadow
    // ray's transmittance would need its own zone lengths); with `nee.enabled` false the
    // phase-sampled continuation keeps full weight, exactly as for an analytic sun.
    let zoned_alphas = cache
        .zoned
        .as_ref()
        .map(|zoned| zoned.zone_alphas(mat_ctx, cache, *current_k, is_extraordinary));
    let nee = if zoned_alphas.is_some() {
        NeeContext {
            enabled: false,
            ..nee
        }
    } else {
        nee
    };
    let scatter = scatter_or_survive(
        cache.zoned.as_ref().zip(zoned_alphas.as_ref()),
        &alphas,
        material,
        mat_ctx.hero_idx,
        ScatterSegment {
            ray: *current_ray,
            hit_t,
            rng_seed,
            bounce,
        },
        stokes,
        path_pdf,
    );
    let Some((t_free, new_dir)) = scatter else {
        return ScatterStepOutcome::ReachedBoundary;
    };
    let old_dir = current_ray.dir;
    let scatter_point = current_ray.origin + t_free * old_dir;

    // Direct light sample from the scattering point, MIS-weighted against
    // the phase-sampled continuation below. A no-op (no RNG draw, no accumulator touch)
    // whenever `!nee.enabled`.
    //
    // # NEE spectral weighting (F-09b)
    //
    // This deposit is a complete, self-contained direct-lighting sample: it must be
    // combined into XYZ using `path_pdf`/`compat` AS THEY STAND RIGHT NOW (this
    // scattering event's own per-channel technique densities), never the FINAL
    // `path_pdf`/`compat` the rest of the path -- unrelated further bounces this same
    // sample has no bearing on -- happens to end up with. Depositing into a fresh local
    // buffer and integrating it immediately, added into the caller's own running
    // `nee_xyz` (unconditionally, exactly like the old shared-`radiance` deposit was
    // always summed regardless of how the loop eventually terminated -- Russian
    // roulette's `apply_russian_roulette` rescales `stokes`/`split_radiance` only,
    // never `radiance`/`nee_xyz`), fixes this without changing that unconditional
    // inclusion.
    let mut nee_deposit = [0.0f32; NUM_CHANNELS];
    // The shadow ray's own extinction. `alphas` above belongs to the segment that led
    // HERE (evaluated at `*current_k`, the direction the ray arrived along), which is not
    // the direction the shadow ray travels: an anisotropic material's assigned-mode
    // coefficients are re-evaluated at the shadow direction inside, exactly as the next
    // loop iteration re-evaluates them at `new_dir` for the phase-sampled continuation
    // (see `ShadowAlphas`). An isotropic material's coefficients do not depend on
    // direction, so its array is reused as it is and such a trace stays bit-identical.
    let shadow_alphas = if mat_ctx.is_anisotropic {
        ShadowAlphas::AssignedMode {
            ctx: mat_ctx,
            cache,
            is_extraordinary,
        }
    } else {
        ShadowAlphas::Fixed(&alphas)
    };
    nee_contribution_hg_scatter(
        nee,
        lambdas,
        cache.n_o_ch[mat_ctx.hero_idx],
        scatter_point,
        old_dir,
        material.scattering_g,
        rng_seed,
        bounce,
        stokes,
        &mut nee_deposit,
        shadow_alphas,
        material.scattering_sigma_s,
        material.absorption_path_scale,
        facet_finishes,
    );
    *nee_xyz += if enable_exit_splitting {
        integrate_channels_to_xyz_families(
            &nee_deposit,
            lambdas,
            path_pdf,
            mat_ctx.hero_idx,
            compat,
        )
    } else {
        integrate_channels_to_xyz(&nee_deposit, lambdas, path_pdf, mat_ctx.hero_idx)
    };

    current_ray.origin = scatter_point;
    current_ray.dir = new_dir;
    *current_k = new_dir;
    // The phase-sampled continuation's own technique density at the direction it just
    // drew -- see `ScatterStepOutcome::ScatteredAndSurvived`'s doc comment for how the
    // caller uses this. `None` (not merely `0.0`) whenever NEE is off, so the caller can
    // tell "no competing technique" apart from "competing technique has zero density
    // here" (the latter would legitimately give the escape branch full weight too, but
    // via `balance_heuristic`'s own degenerate-input handling, not by skipping it).
    let phase_pdf_for_mis =
        (nee.enabled && matches!(nee.environment, EnvironmentSource::HdrMap(_))).then(|| {
            (
                henyey_greenstein_phase(new_dir.dot(old_dir), material.scattering_g),
                new_dir,
            )
        });
    // `split_radiance` rides along on this same survival rescale, exactly like the
    // bounce loop's own trailing Russian-roulette call -- see `apply_russian_roulette`.
    if bounce > 4 && !apply_russian_roulette(bounce, rng_seed, stokes, split_radiance) {
        return ScatterStepOutcome::ScatteredAndTerminated;
    }
    ScatterStepOutcome::ScatteredAndSurvived(phase_pdf_for_mis)
}

/// The Henyey-Greenstein phase function, normalized to integrate to `1.0` over the
/// full sphere (4*pi steradians).
///
/// `cos_theta` is the cosine of the angle between the SCATTERED direction and the ray's
/// ORIGINAL propagation direction (not "look back toward the source") -- so `g > 0`
/// means forward scattering, matching
/// [`crate::optics::materials::GemMaterial::scattering_g`]. `g == 0.0` reduces to the
/// isotropic constant `1 / (4*pi)`.
///
/// Not called by [`maybe_scatter_or_extinguish`]'s own estimator (hazard 3:
/// [`sample_henyey_greenstein_direction`] importance-samples this exact distribution,
/// so `phase / pdf` cancels to `1.0` and is never evaluated at a real scattering event).
///
/// IS called by [`nee_contribution_hg_scatter`] (`hg_nee.rs`): NEE evaluates the phase
/// function at the LIGHT-sampled direction (not the phase-sampled one), where no such
/// cancellation applies, plus at the phase-sampled continuation's own direction for the
/// complementary MIS weight -- see that function's doc comment. Also exposed so a Tier 2
/// GPU self-test can pin `phase / pdf == 1.0` at a phase-sampled direction directly.
#[must_use]
pub(crate) fn henyey_greenstein_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    // `base^1.5` as `base * sqrt(base)`, not `powf(1.5)`: the WGSL twin cannot get an
    // accurate `pow` (GPUs lower it to exp2/log2, which amplifies error with the
    // magnitude of `base`), whereas `sqrt` is near-exact everywhere.
    let base = (2.0 * g).mul_add(-cos_theta, 1.0 + g2).max(1e-6);
    let denom = base * base.sqrt();
    (1.0 - g2) / (4.0 * std::f32::consts::PI * denom)
}

/// Importance-samples a direction from the Henyey-Greenstein phase function about
/// `forward` (the ray's current propagation direction), from two independent uniform
/// `[0, 1)` randoms.
///
/// # Derivation
///
/// The marginal density in `mu = cos_theta` is `p(mu) = (1 - g^2) / (2 * (1 + g^2 -
/// 2*g*mu)^1.5)`. Inverting its CDF (under [`henyey_greenstein_phase`]'s sign
/// convention) gives, for `g != 0`, `mu = (1 + g^2 - ((1 - g^2) / (1 - g +
/// 2*g*u1))^2) / (2*g)`, with the isotropic special case `mu = 1 - 2*u1` near `g == 0`
/// (the general formula divides by `g` twice and is unstable there). `phi` is uniform
/// (no azimuthal dependence); the direction reuses [`frosted_orthonormal_basis`], the
/// same basis [`cosine_weighted_hemisphere`](super::cosine_weighted_hemisphere) uses,
/// for a line-for-line WGSL translation.
///
/// Because this samples exactly [`henyey_greenstein_phase`]'s own distribution,
/// `henyey_greenstein_phase(dot(result, forward), g) / pdf(result) == 1.0` for every
/// `(u1, u2, g)` -- see [`maybe_scatter_or_extinguish`] hazard 3.
#[must_use]
pub(crate) fn sample_henyey_greenstein_direction(u1: f32, u2: f32, g: f32, forward: Vec3) -> Vec3 {
    let cos_theta = if g.abs() < 1e-3 {
        2.0f32.mul_add(-u1, 1.0)
    } else {
        let one_minus_g2 = g.mul_add(-g, 1.0);
        let denom = (2.0 * g).mul_add(u1, 1.0 - g);
        let sq = one_minus_g2 / denom;
        sq.mul_add(-sq, g.mul_add(g, 1.0)) / (2.0 * g)
    };
    let cos_theta = cos_theta.clamp(-1.0, 1.0);
    let sin_theta = f32::mul_add(cos_theta, -cos_theta, 1.0).max(0.0).sqrt();
    let phi = 2.0 * std::f32::consts::PI * u2;
    let (sin_p, cos_p) = phi.sin_cos();
    let (t, b) = frosted_orthonormal_basis(forward);
    (t * (sin_theta * cos_p) + b * (sin_theta * sin_p) + forward * cos_theta).normalize_or_zero()
}
