//! Homogeneous Henyey-Greenstein volumetric scattering.
//!
//! The per-bounce extinction/scatter estimator ([`maybe_scatter_or_extinguish`]), its
//! bounce-loop wrapper ([`try_scatter_step`]), the phase function and its importance
//! sampler, and the scattering-point next-event-estimation contribution.

#[cfg(feature = "zoning")]
use super::super::zoned::{ZoneAlphas, ZonedCache, local_extinction, optical_depths};
use super::{
    super::{
        NUM_CHANNELS,
        absorption::channel_absorption_alphas_assigned,
        camera::{FacetFinish, Ray},
        color::{integrate_channels_to_xyz, integrate_channels_to_xyz_families},
        environment::{EnvironmentSource, sample_environment_for_nee},
        intersect_stone::intersect_stone_soa,
        refraction::{RayMaterialContext, RayWavelengthCache},
        sampling::{
            DISTANCE_SAMPLE_STREAM, NEE_ENV_DIR_U_STREAM, NEE_ENV_DIR_V_STREAM, PHASE_DIR_U_STREAM,
            PHASE_DIR_V_STREAM, hash_u32,
        },
        transport::apply_russian_roulette,
    },
    HG_NEE_MAX_CROSSINGS, NeeContext, balance_heuristic, frosted_orthonormal_basis,
};
use crate::optics::{
    materials::GemMaterial, polarization::StokesVector, raytracer::camera::HitRecord,
};
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
#[cfg(feature = "zoning")]
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

/// [`maybe_scatter_or_extinguish`] for a zoned medium (`zoning` feature): the same estimator
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
#[cfg(feature = "zoning")]
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
#[cfg(feature = "zoning")]
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
#[cfg(feature = "zoning")]
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
    #[cfg(feature = "zoning")]
    let zoned_alphas = cache
        .zoned
        .as_ref()
        .map(|zoned| zoned.zone_alphas(mat_ctx, cache, *current_k, is_extraordinary));
    #[cfg(feature = "zoning")]
    let nee = if zoned_alphas.is_some() {
        NeeContext {
            enabled: false,
            ..nee
        }
    } else {
        nee
    };
    #[cfg(feature = "zoning")]
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
    #[cfg(not(feature = "zoning"))]
    let scatter = maybe_scatter_or_extinguish(
        &alphas,
        material.scattering_sigma_s,
        material.scattering_g,
        mat_ctx.hero_idx,
        current_ray.dir,
        hit_t,
        material.absorption_path_scale,
        rng_seed,
        bounce,
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
        &alphas,
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

/// Next-event estimation at a Henyey-Greenstein scattering event.
///
/// Draws one direction from the environment's own importance distribution
/// ([`sample_environment_for_nee`]), traces a shadow ray from the scattering point to the
/// (necessarily convex) polyhedron's exit facet, applies a scalar Fresnel transmittance
/// there, refracts the sampled direction through that exit facet to look up
/// the environment radiance where the light-sampled ray actually leaves along, applies
/// the medium's own transmittance over the shadow ray's interior path length, and adds
/// the balance-heuristic-weighted contribution directly into `radiance`.
/// A no-op whenever `!nee.enabled`, whenever the environment has no importance
/// distribution to sample (`Studio`), whenever the sampled direction is totally
/// internally reflected at the exit facet (cannot reach the environment at all along that
/// direction -- not a bias: the phase-sampled continuation still has its own chance to
/// escape through a different exit, at full weight, since no NEE sample competes with it
/// there), or whenever the exit facet is [`FacetFinish::Frosted`] (a frosted
/// exit has no well-defined specular Fresnel/refraction for this shadow ray to use --
/// [`nee_contribution_frosted_exterior`](super::nee_contribution_frosted_exterior)
/// already handles NEE for a frosted exit's own diffusely-sampled surface point, so this
/// is a genuine division of labour, not a dropped case).
///
/// Consumes exactly one new 2D RNG draw
/// ([`NEE_ENV_DIR_U_STREAM`]/[`NEE_ENV_DIR_V_STREAM`]) -- a stream pair no existing draw
/// uses, so a trace with NEE disabled never even hashes against it and stays
/// bit-identical to one traced with NEE support absent altogether.
///
/// # Simplification: one achromatic exit transmittance
///
/// Like [`maybe_scatter_or_extinguish`] itself (hazard 4: one achromatic direction/
/// distance shared by every channel), this uses ONE scalar Fresnel transmittance --
/// computed from the HERO channel's own dispersion index (`n_inside_hero`) via the plain
/// isotropic Fresnel formula -- shared by every channel's own spectral radiance at the
/// sampled direction, rather than each channel's own (dispersion-shifted) exit index. A
/// real gem's dispersion at a diffuse scattering event is a second-order effect next to
/// the achromatic phase-function/free-path sampling already in place; a future revision
/// could add per-channel exit indices the way [`compute_channel_transmission`]
/// (`refraction.rs`) already does for the polished exit path.
///
/// # Medium transmittance
///
/// `alphas`/`sigma_s`/`absorption_path_scale` are the SAME quantities
/// [`maybe_scatter_or_extinguish`]'s own survive branch uses (`alphas` is the caller's
/// precomputed [`channel_absorption_alphas_assigned`] result, `sigma_s` is
/// [`GemMaterial::scattering_sigma_s`]), and `hit.t` (the shadow ray's own distance to
/// the exit facet) plays the same role that function's `hit_t` does -- so this applies
/// the identical per-channel `exp(-(alphas[k]+sigma_s/path_scale)*hit.t*path_scale)` transmittance
/// (`sigma_s` is per model unit and does not scale with the stone's size; `alphas` do),
/// via the same [`crate::simd::exp_f32x8`] idiom, that a phase-sampled continuation
/// reaching the same boundary would have paid.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors try_scatter_step's own argument shape -- context, this bounce's own \
              geometric/RNG state, and the per-channel accumulators an NEE contribution \
              reads/writes; see that function's identical justification"
)]
pub(crate) fn nee_contribution_hg_scatter(
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    n_inside_hero: f32,
    scatter_point: Vec3,
    scatter_dir_in: Vec3,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &[StokesVector; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
    alphas: &[f32; NUM_CHANNELS],
    sigma_s: f32,
    absorption_path_scale: f32,
    facet_finishes: &[FacetFinish],
) {
    // HDR maps only. The analytic sun's light sample is an EXTERIOR direction, but this
    // estimator treats the sample as an INTERIOR direction and refracts it through the exit
    // facet before the radiance lookup; for a disc 0.27 degrees wide that lookup would almost
    // never land on the sun (a biased deposit). A proper sun estimator would have to invert
    // the exit refraction, so a scattering medium under `DaylightSun` gets no NEE here and its
    // phase-sampled continuation keeps full weight (see `phase_pdf_for_mis` below).
    if !nee.enabled || !matches!(nee.environment, EnvironmentSource::HdrMap(_)) {
        return;
    }
    let u0 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM)) as f32) / 4_294_967_295.0;
    let u1 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM)) as f32) / 4_294_967_295.0;
    let Some(sample) = sample_environment_for_nee(nee.environment, u0, u1) else {
        return;
    };

    // Shadow ray from the scattering point toward the sampled direction, offset by a
    // small epsilon along it -- mirrors `try_split_exit_channel`'s identical `1e-4`
    // offset (`refraction.rs`) to clear the origin point itself before probing.
    let probe = Ray {
        origin: scatter_point + sample.dir * 1e-4,
        dir: sample.dir,
    };
    let Some(first) = intersect_stone_soa(probe, nee.plane_soa, nee.plane_soa.len(), nee.tools)
    else {
        // No exit facet found -- shouldn't happen for a point genuinely inside a closed
        // solid, but a degenerate/open mesh must not panic or fabricate energy.
        return;
    };
    // A convex stone has exactly one crossing: the exit. A concave one may cross out into
    // a cavity and back in before the final exit, so the probe walks its boundaries.
    let Some(exit) = (if nee.tools.is_empty() {
        single_exit(first, sample.dir, n_inside_hero, facet_finishes)
    } else {
        walk_exits(nee, probe, first, n_inside_hero, facet_finishes)
    }) else {
        return;
    };
    let ExitProbe {
        normal: exit_normal,
        t_material: hit_t,
        cos_i,
        cos_t,
        t_unpol,
    } = exit;

    // The competing (phase-function) technique's own density at this SAME
    // light-sampled direction -- the balance heuristic's other half.
    let phase_cos = sample.dir.dot(scatter_dir_in);
    let phase_val = henyey_greenstein_phase(phase_cos, g);
    let mis_weight = balance_heuristic(sample.pdf, phase_val);
    if mis_weight <= 0.0 {
        return;
    }

    // The exterior direction this light sample actually leaves along --
    // Snell's law at the exit facet, `sample.dir` playing the incident-direction role
    // and (the inward-flipped) `-hit.normal` the surface normal, mirroring
    // `refraction.rs`'s own `eta*k_hat + (eta*cos_i - cos_t)*normal` vector form (see
    // this function's doc comment for the sign-convention correspondence). `sample.pdf`
    // itself stays in the INTERIOR (pre-refraction) measure `sample_environment_for_nee`
    // sampled in -- only the radiance LOOKUP moves to the refracted direction.
    let refracted_dir = (n_inside_hero * sample.dir
        - n_inside_hero.mul_add(cos_i, -cos_t) * exit_normal)
        .normalize_or_zero();
    let EnvironmentSource::HdrMap(env_map) = nee.environment else {
        // `sample` is `Some` only for `HdrMap` (see `sample_environment_for_nee`), so
        // this is unreachable in practice -- a defensive `return`, not a real branch.
        return;
    };
    let env_rgb = env_map.radiance_rgb(refracted_dir);

    // The medium transmittance a phase-sampled continuation reaching this
    // same boundary would have paid -- see this function's own "Medium transmittance"
    // doc section. Vectorized Beer-Lambert via the same `exp_f32x8` idiom
    // `maybe_scatter_or_extinguish`'s survive branch uses, so the CPU stays
    // self-consistent between the two estimators.
    let hit_t_scaled = hit_t * absorption_path_scale;
    // `sigma_s` is per model unit (size-independent): same conversion as
    // `maybe_scatter_or_extinguish`'s `sigma_s_model`.
    let sigma_s_abs = sigma_s / absorption_path_scale;
    let mut trans_args = [0f32; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        trans_args[k] = -(alphas[k] + sigma_s_abs) * hit_t_scaled;
    }
    let transmittance = crate::simd::exp_f32x8(trans_args);

    let common = t_unpol * phase_val * mis_weight / sample.pdf;
    for k in 0..NUM_CHANNELS {
        let env_k = crate::renderer::env_map::rgb_to_spectral_radiance(env_rgb, lambdas[k]);
        radiance[k] =
            (stokes[k].intensity() * transmittance[k] * common * env_k).mul_add(1.0, radiance[k]);
    }
}

/// The shadow probe's resolved exit: the surface the sample direction finally leaves
/// through, and the chain's total Fresnel transmittance.
struct ExitProbe {
    /// Outward normal of the last exit facet, which refracts the direction the
    /// environment is looked up along.
    normal: Vec3,
    /// Total path length inside the material: the sum of every in-material segment of
    /// the walk (just the distance to the exit on a convex stone). Air gaps across a
    /// cavity carry no absorption.
    t_material: f32,
    /// Cosine of incidence at the last exit.
    cos_i: f32,
    /// Cosine of the refracted angle at the last exit.
    cos_t: f32,
    /// Product of the unpolarised Fresnel transmittance at every exit crossed.
    t_unpol: f32,
}

/// Unpolarised exit Fresnel transmittance and refracted-angle cosine for a ray leaving
/// an interior of index `n_inside` at `cos_i`, or `None` on total internal reflection.
///
/// One scalar hero-index transmittance, as the doc comment of
/// [`nee_contribution_hg_scatter`] describes.
fn exit_transmittance(n_inside: f32, cos_i: f32) -> Option<(f32, f32)> {
    let sin2_t = (n_inside * n_inside * cos_i.mul_add(-cos_i, 1.0)).min(1.0);
    if sin2_t >= 1.0 {
        // Total internal reflection along this direction -- see the NEE doc comment for
        // why this is a legitimate zero, not a bias.
        return None;
    }
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s = n_inside.mul_add(cos_i, -cos_t) / n_inside.mul_add(cos_i, cos_t);
    let r_p = n_inside.mul_add(-cos_t, cos_i) / n_inside.mul_add(cos_t, cos_i);
    let r_unpol = (0.5 * r_p.mul_add(r_p, r_s * r_s)).clamp(0.0, 1.0);
    Some((1.0 - r_unpol, cos_t))
}

/// Whether the facet `hit` is on has a frosted finish; tool facets are always polished.
fn hit_is_frosted(facet_finishes: &[FacetFinish], hit: HitRecord) -> bool {
    facet_finishes
        .get(hit.facet_idx)
        .copied()
        .unwrap_or_default()
        == FacetFinish::Frosted
}

/// The convex-stone probe result: the single crossing is the exit. This is the
/// pre-concave logic, kept as its own function so a planar trace stays bit-identical.
///
/// A frosted exit facet has no well-defined specular Fresnel/refraction for this shadow
/// ray to use -- see [`nee_contribution_hg_scatter`]'s doc comment for why that is a
/// division of labour with `nee_contribution_frosted_exterior`, not a dropped case.
fn single_exit(
    hit: HitRecord,
    dir: Vec3,
    n_inside: f32,
    facet_finishes: &[FacetFinish],
) -> Option<ExitProbe> {
    if hit_is_frosted(facet_finishes, hit) {
        return None;
    }
    // `hit.normal` is the exit facet's own OUTWARD normal (`intersect_polyhedron`'s
    // convention: an interior origin's far/exit intersection returns the plane's own
    // stored normal, unflipped), and the shadow ray travels outward through it, so
    // `cos_i = dot(dir, hit.normal)` (no negation) is this interface's cosine of
    // incidence -- the same sign convention `compute_channel_transmission`'s own
    // `n1 -> n2` exit formula uses.
    let cos_i = dir.dot(hit.normal).clamp(0.0, 1.0);
    let (t_unpol, cos_t) = exit_transmittance(n_inside, cos_i)?;
    Some(ExitProbe {
        normal: hit.normal,
        t_material: hit.t,
        cos_i,
        cos_t,
        t_unpol,
    })
}

/// The concave-stone probe: follows the sample direction through up to
/// [`HG_NEE_MAX_CROSSINGS`] boundaries, multiplying in the Fresnel transmittance at each
/// exit and at each re-entry through a cavity's far wall (the same unpolarised Fresnel
/// formula, evaluated at the interior-side cosine of the unbent direction, so the two
/// interfaces of one air gap are both paid), and summing the length of every in-material
/// segment for the Beer-Lambert term. Drops the sample
/// above the cap, on TIR or a frosted exit at any crossing, and when the walk ends on an
/// entry (a ray that never leaves a closed stone is degenerate geometry).
///
/// The walk keeps the interior direction through the air gaps rather than refracting it
/// at each interface: v1 bends the direction only once, at the last exit, where the
/// environment is looked up. That matches the single-exit estimator exactly for a stone
/// with no cavity in the way and is an approximation, noted in `physics.md`, otherwise.
fn walk_exits(
    nee: NeeContext<'_>,
    probe: Ray,
    first: HitRecord,
    n_inside: f32,
    facet_finishes: &[FacetFinish],
) -> Option<ExitProbe> {
    let dir = probe.dir;
    let mut ray = probe;
    let mut hit = first;
    let mut crossings = 1;
    let mut chain = 1.0f32;
    let mut material_len = 0.0f32;
    // Whether the segment ending at the current hit starts just past a re-entry, whose
    // origin offset lies inside the material.
    let mut after_entry = false;
    let mut last: Option<(Vec3, f32, f32)>;
    loop {
        if hit_is_frosted(facet_finishes, hit) {
            return None;
        }
        let cos = dir.dot(hit.normal);
        if cos > 0.0 {
            // The segment ending at an exit is in-material.
            material_len += if after_entry { hit.t + 1e-4 } else { hit.t };
            let cos_i = cos.clamp(0.0, 1.0);
            let (t, cos_t) = exit_transmittance(n_inside, cos_i)?;
            chain *= t;
            last = Some((hit.normal, cos_i, cos_t));
            after_entry = false;
        } else {
            // Re-entry through a far cavity wall: the air-gap segment ending here is
            // not absorbing, but the interface transmits only `T`.
            let (t, _) = exit_transmittance(n_inside, (-cos).clamp(0.0, 1.0))?;
            chain *= t;
            last = None;
            after_entry = true;
        }
        ray = Ray {
            origin: ray.origin + hit.t * dir + dir * 1e-4,
            dir,
        };
        let Some(next) = intersect_stone_soa(ray, nee.plane_soa, nee.plane_soa.len(), nee.tools)
        else {
            break;
        };
        crossings += 1;
        if crossings > HG_NEE_MAX_CROSSINGS {
            return None;
        }
        hit = next;
    }
    let (normal, cos_i, cos_t) = last?;
    Some(ExitProbe {
        normal,
        t_material: material_len,
        cos_i,
        cos_t,
        t_unpol: chain,
    })
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
/// IS called by [`nee_contribution_hg_scatter`]: NEE evaluates the phase
/// function at the LIGHT-sampled direction (not the phase-sampled one), where no such
/// cancellation applies, plus at the phase-sampled continuation's own direction for the
/// complementary MIS weight -- see that function's doc comment. Also exposed so a Tier 2
/// GPU self-test can pin `phase / pdf == 1.0` at a phase-sampled direction directly.
#[must_use]
pub(crate) fn henyey_greenstein_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denom = (2.0 * g).mul_add(-cos_theta, 1.0 + g2).max(1e-6).powf(1.5);
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

#[cfg(test)]
mod walk_tests {
    use super::*;
    use crate::{
        geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
        optics::raytracer::{build_plane_soa, environment::LightingPreset},
    };

    /// A shadow ray that exits through a groove wall, crosses the cavity and re-enters
    /// the far wall must pay Beer-Lambert over BOTH in-material segments and the
    /// Fresnel transmittance at all three interfaces (exit, re-entry, final exit).
    #[test]
    fn through_groove_probe_matches_analytic_segments_and_interfaces() {
        // Unit cube, half-extent 0.5, with a Z-axis through-groove of radius 0.1.
        let planes: Vec<GpuFacetPlane> = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .map(|n| GpuFacetPlane::new(n, -0.5))
        .collect();
        let soa = build_plane_soa(&planes);
        let tools = [ToolPrimitive::cylinder(Vec3::ZERO, Vec3::Z, 0.1, 2.0)];
        let env = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
        let nee = NeeContext {
            environment: env,
            plane_soa: &soa,
            tools: &tools,
            enabled: true,
        };
        let n = 1.76f32;
        let start = Vec3::new(-0.4, 0.05, 0.0);
        let probe = Ray {
            origin: start + Vec3::X * 1e-4,
            dir: Vec3::X,
        };
        let first = intersect_stone_soa(probe, &soa, planes.len(), &tools).expect("groove wall");
        let exit = walk_exits(nee, probe, first, n, &[]).expect("probe escapes");

        // Walls of the groove at this height: x = -+sqrt(0.1^2 - 0.05^2).
        let half_chord = 0.05f32.mul_add(-0.05, 0.1f32 * 0.1).sqrt();
        let seg_a = (-half_chord) - (-0.4);
        let seg_b = 0.5 - half_chord;
        assert!(
            (exit.t_material - (seg_a + seg_b)).abs() < 1e-3,
            "material path {} vs {}",
            exit.t_material,
            seg_a + seg_b
        );
        // Interface transmittances from the unpolarised Fresnel formula at each
        // crossing's own cosine (the groove walls are tilted, the cube face is normal).
        let cos_wall = (0.05f32 / 0.1).mul_add(-(0.05f32 / 0.1), 1.0).sqrt();
        let t_wall = exit_transmittance(n, cos_wall).expect("no TIR").0;
        let t_face = exit_transmittance(n, 1.0).expect("no TIR").0;
        let analytic = t_wall * t_wall * t_face;
        assert!(
            (exit.t_unpol - analytic).abs() < 1e-4,
            "chain {} vs {}",
            exit.t_unpol,
            analytic
        );
        // And the absorption the caller multiplies in is strictly larger than the old
        // first-segment-only value.
        assert!(exit.t_material > first.t + 0.4);
    }
}
