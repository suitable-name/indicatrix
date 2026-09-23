//! Henyey-Greenstein volumetric scattering and the frosted-facet BSDF.
//!
//! [`maybe_scatter_or_extinguish`]'s per-bounce extinction/scatter estimator, the HG
//! phase function and its importance sampler, and [`apply_frosted_bounce`]'s diffuse
//! reflect/transmit dispatch.

use super::{
    NUM_CHANNELS,
    absorption::channel_absorption_alphas_assigned,
    camera::{FacetFinish, Ray},
    environment::{EnvironmentSource, sample_environment_for_nee},
    intersect::intersect_polyhedron_soa,
    refraction::{BounceRefractionGeometry, RayMaterialContext, RayWavelengthCache},
    sampling::{
        BIREFRINGENT_SPLIT_STREAM, DISTANCE_SAMPLE_STREAM, FRESNEL_BRANCH_STREAM,
        FROSTED_DIR_U_STREAM, FROSTED_DIR_V_STREAM, FROSTED_NEE_ENV_DIR_U_STREAM,
        FROSTED_NEE_ENV_DIR_V_STREAM, NEE_ENV_DIR_U_STREAM, NEE_ENV_DIR_V_STREAM,
        PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM, hash_u32,
    },
    transport::apply_russian_roulette,
};
use crate::optics::{materials::GemMaterial, polarization::StokesVector};
use glam::Vec3;

/// Bundles the per-trace next-event-estimation inputs
/// [`nee_contribution_hg_scatter`] and [`nee_contribution_frosted_exterior`] need -- the
/// environment (only [`EnvironmentSource::HdrMap`] has an importance distribution to
/// sample; see [`sample_environment_for_nee`]), the intersection arena a shadow ray from
/// an interior scattering point probes against (used by the HG path only --
/// [`nee_contribution_frosted_exterior`] never needs it, see that function's doc
/// comment), and whether NEE is switched on for this trace at all.
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
    /// `optics::raytracer::transport::trace_spectral_ray_inner`'s own `enable_nee`
    /// parameter -- `true` only at every PUBLIC entry point when `environment` is
    /// `HdrMap` (never `Studio`: the analytic rig has no importance distribution to draw
    /// from), or when this crate's own tests force it explicitly for an on/off A-B
    /// comparison (mirroring `ExitSplitCtx::enabled`'s identical precedent).
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
/// Like [`apply_frosted_bounce`], every channel's Stokes vector collapses to
/// `StokesVector::unpolarized` at a scatter event: incoherent scattering off an
/// inclusion's internal structure scrambles the phase relationship polarization
/// depends on.
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
    sigma_s: f32,
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

/// What [`try_scatter_step`] found, and what `trace_spectral_ray_inner`'s bounce loop
/// should do about it.
pub(super) enum ScatterStepOutcome {
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
              explains in transport.rs; `lambdas`/`radiance` are the HDR-map NEE \
              contribution's inputs/output, threaded through rather than bundled \
              into `nee` since they vary in a way `NeeContext` (fixed for the whole \
              trace) deliberately does not; `facet_finishes` (the per-facet surface \
              finish) is threaded the same way rather than added to `NeeContext` so \
              the GPU Tier 2 harness's `NeeContext` literals stay independent of it"
)]
pub(super) fn try_scatter_step(
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
    radiance: &mut [f32; NUM_CHANNELS],
    facet_finishes: &[FacetFinish],
) -> ScatterStepOutcome {
    if material.scattering_sigma_s <= 0.0 {
        return ScatterStepOutcome::NotApplicable;
    }
    let alphas = channel_absorption_alphas_assigned(mat_ctx, cache, *current_k, is_extraordinary);
    let Some((t_free, new_dir)) = maybe_scatter_or_extinguish(
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
    ) else {
        return ScatterStepOutcome::ReachedBoundary;
    };
    let old_dir = current_ray.dir;
    let scatter_point = current_ray.origin + t_free * old_dir;

    // Direct light sample from the scattering point, MIS-weighted against
    // the phase-sampled continuation below. A no-op (no RNG draw, no accumulator touch)
    // whenever `!nee.enabled`.
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
        radiance,
        &alphas,
        material.scattering_sigma_s,
        material.absorption_path_scale,
        facet_finishes,
    );

    current_ray.origin = scatter_point;
    current_ray.dir = new_dir;
    *current_k = new_dir;
    // The phase-sampled continuation's own technique density at the direction it just
    // drew -- see `ScatterStepOutcome::ScatteredAndSurvived`'s doc comment for how the
    // caller uses this. `None` (not merely `0.0`) whenever NEE is off, so the caller can
    // tell "no competing technique" apart from "competing technique has zero density
    // here" (the latter would legitimately give the escape branch full weight too, but
    // via `balance_heuristic`'s own degenerate-input handling, not by skipping it).
    let phase_pdf_for_mis = nee.enabled.then(|| {
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
/// [`nee_contribution_frosted_exterior`] already handles NEE for a frosted exit's own
/// diffusely-sampled surface point, so this is a genuine division of labour, not a
/// dropped case).
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
/// the identical per-channel `exp(-(alphas[k]+sigma_s)*hit.t*path_scale)` transmittance,
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
    if !nee.enabled {
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
    let Some(hit) = intersect_polyhedron_soa(probe, nee.plane_soa) else {
        // No exit facet found -- shouldn't happen for a point genuinely inside a closed
        // convex solid, but a degenerate/open mesh must not panic or fabricate energy.
        return;
    };
    // A frosted exit facet has no well-defined specular Fresnel/refraction
    // for this shadow ray to use -- see this function's own doc comment for why that is
    // a division of labour with `nee_contribution_frosted_exterior`, not a dropped case.
    if facet_finishes
        .get(hit.facet_idx)
        .copied()
        .unwrap_or_default()
        == FacetFinish::Frosted
    {
        return;
    }

    // Scalar (hero-index, isotropic-approximation) exit Fresnel transmittance -- see
    // this function's own "Simplification" doc section. `hit.normal` is the exit
    // facet's own OUTWARD normal (`intersect_polyhedron`'s convention: an interior
    // origin's far/exit intersection returns the plane's own stored normal, unflipped),
    // and the shadow ray travels outward through it, so `cos_i = dot(sample.dir,
    // hit.normal)` (no negation) is this interface's cosine of incidence -- the same
    // sign convention `compute_channel_transmission`'s own `n1 -> n2` exit formula uses.
    let cos_i = sample.dir.dot(hit.normal).clamp(0.0, 1.0);
    let sin2_t = (n_inside_hero * n_inside_hero * cos_i.mul_add(-cos_i, 1.0)).min(1.0);
    if sin2_t >= 1.0 {
        // Total internal reflection along this direction -- see this function's own doc
        // comment for why this is a legitimate zero, not a bias.
        return;
    }
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s = n_inside_hero.mul_add(cos_i, -cos_t) / n_inside_hero.mul_add(cos_i, cos_t);
    let r_p = n_inside_hero.mul_add(-cos_t, cos_i) / n_inside_hero.mul_add(cos_t, cos_i);
    let r_unpol = (0.5 * r_p.mul_add(r_p, r_s * r_s)).clamp(0.0, 1.0);
    let t_unpol = 1.0 - r_unpol;

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
        - n_inside_hero.mul_add(cos_i, -cos_t) * hit.normal)
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
    let hit_t_scaled = hit.t * absorption_path_scale;
    let mut trans_args = [0f32; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        trans_args[k] = -(alphas[k] + sigma_s) * hit_t_scaled;
    }
    let transmittance = crate::simd::exp_f32x8(trans_args);

    let common = t_unpol * phase_val * mis_weight / sample.pdf;
    for k in 0..NUM_CHANNELS {
        let env_k = crate::renderer::env_map::rgb_to_spectral_radiance(env_rgb, lambdas[k]);
        radiance[k] =
            (stokes[k].intensity() * transmittance[k] * common * env_k).mul_add(1.0, radiance[k]);
    }
}

/// A stable orthonormal basis `(t, b)` perpendicular to unit vector `n` -- the same
/// branch-minimal construction `birefringence::stable_orthonormal_basis` uses, kept as
/// its own local copy rather than exposing that private helper across modules.
fn frosted_orthonormal_basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let t = (a - n * n.dot(a)).normalize_or_zero();
    let b = n.cross(t);
    (t, b)
}

/// Cosine-weighted hemisphere direction about `n`, from two independent uniform
/// `[0, 1)` randoms (Malley's method -- polar mapping, not the concentric-disk variant,
/// for a direct WGSL trig-call translation). See `apply_frosted_bounce` for why its
/// call sites need no explicit pdf division.
///
/// `pub(crate)`: `renderer::gpu::transport_check`'s Tier 2 ULP check compares this real
/// function against `shaders/transport_physics.wgsl`'s translation. Visibility only.
pub(crate) fn cosine_weighted_hemisphere(u1: f32, u2: f32, n: Vec3) -> Vec3 {
    let r = u1.sqrt();
    let theta = 2.0 * std::f32::consts::PI * u2;
    let (sin_t, cos_t) = theta.sin_cos();
    let (t, b) = frosted_orthonormal_basis(n);
    (t * (r * cos_t) + b * (r * sin_t) + n * (1.0 - u1).max(0.0).sqrt()).normalize_or_zero()
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
/// same basis [`cosine_weighted_hemisphere`] uses, for a line-for-line WGSL translation.
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

/// Next-event estimation for the two [`apply_frosted_bounce`] outcomes whose
/// sampled hemisphere lands on the EXTERIOR side of the gem -- see that function's own
/// doc comment ("Sign convention for NEE eligibility") for exactly which two branches
/// those are and why.
///
/// # Why this needs no shadow ray, unlike [`nee_contribution_hg_scatter`]
///
/// [`nee_contribution_hg_scatter`]'s scattering point sits somewhere in the INTERIOR of
/// the (necessarily convex) polyhedron, so it must trace a shadow ray to find which exit
/// facet the light-sampled direction would leave through, and pay that facet's own
/// Fresnel transmittance. This function's bounce point, by contrast, is already ON the
/// polyhedron's own surface, and `ext_normal` is that facet's true (mathematical)
/// outward-facing normal for this specific outcome. A convex solid's own surface point,
/// moving into the outward half-space about its own true normal, never re-enters the
/// solid for any positive distance -- so the environment is unconditionally visible
/// along any `dir` with `dir.dot(ext_normal) > 0`, with no occlusion test and no second
/// Fresnel interface to cross (the interface AT this point was already paid for by the
/// caller's `r_unpol`/`t_unpol` branch-selection division before `stokes` reached here).
///
/// # Simplification, mirroring `nee_contribution_hg_scatter`
///
/// One broadband cosine-weighted-hemisphere ("Lambertian") BSDF density
/// (`cos_light / pi`) stands in for the true frosted BSDF's competing technique here,
/// exactly like [`apply_frosted_bounce`] itself already uses one broadband `r_unpol`/
/// `t_unpol` split for every channel (see that function's own "The model, honestly"
/// section) -- consistent with, not an additional approximation on top of, the rest of
/// this BSDF's treatment.
///
/// Consumes exactly one new 2D RNG draw ([`FROSTED_NEE_ENV_DIR_U_STREAM`]/
/// [`FROSTED_NEE_ENV_DIR_V_STREAM`]) whenever `nee.enabled` -- callers gate the call
/// itself on eligibility (see [`apply_frosted_bounce`]), so a bounce that lands on the
/// interior side, or any bounce with NEE disabled, never touches this stream at all.
///
/// `stokes` must already carry this branch's own throughput (post `/r_unpol` or
/// `/t_unpol` rescale, exactly like [`nee_contribution_hg_scatter`]'s identical
/// precondition on its own `stokes` argument).
pub(crate) fn nee_contribution_frosted_exterior(
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    ext_normal: Vec3,
    rng_seed: u32,
    bounce: u32,
    stokes: &[StokesVector; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
) {
    if !nee.enabled {
        return;
    }
    let u0 = (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM)) as f32)
        / 4_294_967_295.0;
    let u1 = (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM)) as f32)
        / 4_294_967_295.0;
    let Some(sample) = sample_environment_for_nee(nee.environment, u0, u1) else {
        return;
    };

    // `EnvironmentMap::sample` draws from the FULL sphere, not this bounce's own
    // hemisphere -- roughly half its draws land behind the surface (`cos_light <= 0.0`)
    // and must be rejected outright, not folded in as negative energy. Not a bias: the
    // BSDF-sampled continuation (`new_dir`, always in the correct hemisphere by
    // construction) still gets its own full chance to reach the environment there.
    let cos_light = sample.dir.dot(ext_normal);
    if cos_light <= 0.0 {
        return;
    }

    // The competing (BSDF) technique's own density at this SAME light-sampled
    // direction -- the balance heuristic's other half. See this function's own
    // "Simplification" doc section for why a plain Lambertian `cos / pi` (rather than
    // the frosted BSDF's actual `r_unpol`/`t_unpol`-scaled value) is the right density
    // to compete the light sample against.
    let brdf_pdf = cos_light / std::f32::consts::PI;
    let mis_weight = balance_heuristic(sample.pdf, brdf_pdf);
    if mis_weight <= 0.0 {
        return;
    }

    let common = brdf_pdf * mis_weight / sample.pdf;
    for k in 0..NUM_CHANNELS {
        let env_k = crate::renderer::env_map::rgb_to_spectral_radiance(sample.rgb, lambdas[k]);
        radiance[k] = (stokes[k].intensity() * common * env_k).mul_add(1.0, radiance[k]);
    }
}

/// The replacement for the specular TIR/partial-reflect/refract dispatch at a
/// `FacetFinish::Frosted` facet.
///
/// # The model, honestly
///
/// A real bruted/ground surface's reflectance doesn't carry the polished interface's
/// sharp per-wavelength Fresnel structure -- roughness averages over local micro-facet
/// angles, washing out the fine structure -- so this uses ONE broadband reflect/
/// transmit fraction (`r_unpol`, hero channel only) for every spectral channel, unlike
/// the polished path's per-channel `r_unpol_k`. Every channel shares both the sampled
/// direction (roughness scattering isn't meaningfully dispersive) and the reflect/
/// transmit split probability (a modeling simplification).
///
/// Direction is drawn from the cosine-weighted hemisphere about the correct macroscopic
/// normal (`normal` reflect, `-normal` transmit) -- standard importance sampling for a
/// Lambertian BRDF/BTDF, so `f * cos(theta) / pdf` is the constant albedo (`1.0`,
/// already spent on `r_unpol`/`1 - r_unpol`) -- folded into the `1.0 / r_unpol` (or
/// `t_unpol`) throughput scale below, with no separate pdf division needed.
///
/// Depolarizes every channel (`StokesVector::unpolarized`): multiple internal
/// micro-scattering events scramble the coherent phase relationship polarization
/// depends on.
///
/// # Sign convention for NEE eligibility
///
/// `normal` arrives here ALREADY resolved by the caller (`transport::apply_interior_segment`)
/// to the facet's outward normal flipped to face the current medium: unflipped (the raw,
/// truly outward-pointing facet normal) when `!inside_gem` (this bounce is happening at a
/// ray currently in air, about to enter or bounce off the gem from outside), and flipped
/// to point INWARD when `inside_gem` (this bounce is happening at a ray currently inside
/// the crystal, hitting a facet from the inside). Given that:
///
/// - **`!inside_gem`, reflect branch** (hemisphere about `normal`, i.e. the true outward
///   normal): the ray was already outside and bounces back outside. `inside_gem` is
///   unchanged (`false`) -- EXTERIOR side. NEE-eligible.
/// - **`!inside_gem`, transmit branch** (hemisphere about `-normal`, i.e. pointing
///   inward): the ray enters the crystal for the first time through this frosted facet.
///   `inside_gem` flips to `true` -- INTERIOR side. Not handled here: a light sample
///   drawn into the interior hemisphere would need to find where THIS path eventually
///   exits the solid and pay that exit facet's own Fresnel transmittance, exactly the
///   shadow-ray search [`nee_contribution_hg_scatter`] already does from an interior
///   scattering point. Deliberately not implemented (documented gap, not an oversight):
///   entry-facet frosted transmission is the less common case in practice (a frosted
///   girdle scattering light back OUT to the camera -- the exterior-transmit case below
///   -- is the one every existing frosted-girdle test/scene exercises), and adding it
///   would double this function's shadow-ray-tracing surface for comparatively little
///   variance-reduction benefit. `pending_light_mis` is `None` for this branch, so a
///   phase-sampled continuation that (rarely, since it heads inward) still escapes
///   directly next bounce keeps its full weight rather than being MIS-weighted
///   against a light sample this function never drew.
/// - **`inside_gem`, forced-reflect (TIR) branch** (hemisphere about `normal`, i.e.
///   ALREADY flipped inward): TIR is only reachable with `inside_gem == true` (`n1 > n2`
///   for the hero channel -- same premise `dispatch_bounce`'s polished-path doc comment
///   states), so this hemisphere stays interior. `inside_gem` unchanged (`true`) --
///   INTERIOR side. Not NEE-eligible (an internal reflection has no direct environment
///   visibility at this point).
/// - **`inside_gem`, reflect branch** (hemisphere about `normal`, flipped inward): an
///   internal reflection off a frosted facet, staying inside. INTERIOR side. Not
///   NEE-eligible, for the same reason as the TIR branch above.
/// - **`inside_gem`, transmit branch** (hemisphere about `-normal`, i.e. the true
///   outward normal once the inward flip is undone): the ray exits the crystal to the
///   environment through this frosted facet -- the frosted-girdle scene every existing
///   test exercises. `inside_gem` flips to `false` -- EXTERIOR side. NEE-eligible.
///
/// In short: NEE fires exactly when this bounce's own `inside_gem` input and its
/// resulting output disagree in the "becoming exterior" direction reflect can never
/// take (`!inside_gem` reflect) or transmit does take (`inside_gem` transmit) -- both
/// cases share the property that the branch's own hemisphere normal (`normal` for the
/// first, `-normal` for the second) is the TRUE, un-flipped outward-facing normal.
///
/// # Estimator composition -- why this needs no chromatic termination
///
/// The polished path drops a companion channel to exactly zero when its specular
/// direction misses the hero-driven direction, because a delta BSDF has zero density
/// elsewhere. Neither premise holds here: direction is shared and the BSDF is a smooth
/// hemisphere, so `path_pdf[k] *= r_unpol` (or `t_unpol`) uses the same factor for
/// every channel.
///
/// Reuses `FRESNEL_BRANCH_STREAM` for the reflect-vs-transmit branch (divides by its
/// own selection probability, disjoint energy). `BIREFRINGENT_SPLIT_STREAM` on an
/// air->crystal entry picks which ~0.5 energy SHARE this path becomes, so it is NOT
/// divided by its own 0.5 (same reasoning as `apply_refract_bounce`). Mode
/// re-coupling is applied by the caller.
///
/// # Return value
///
/// The fourth tuple element is the `pending_light_mis` carry, mirroring
/// [`ScatterStepOutcome::ScatteredAndSurvived`]'s identical carry for the
/// Henyey-Greenstein path: `Some(cos(theta_new) / pi)` -- the cosine-weighted-hemisphere
/// BSDF's own density at the just-sampled continuation direction `new_dir` -- whenever
/// `nee.enabled` and this outcome is NEE-eligible (see above), `None` otherwise
/// (including every trace with NEE disabled). The caller stashes this exactly like the
/// HG path's own carry: consumed only by the VERY NEXT bounce-loop iteration's escape
/// check, and only if that iteration's ray escapes directly.
// `pub(crate)`: `renderer::gpu::transport_check`'s Tier 2 ULP check compares this real
// function against `shaders/transport_physics.wgsl`'s translation. Visibility only.
#[expect(
    clippy::too_many_arguments,
    reason = "bundles ctx/geo, the same contexts apply_partial_fresnel_bounce and \
              apply_refract_bounce use in refraction.rs; nee/lambdas/radiance are \
              the HDR-map NEE inputs/output, mirroring try_scatter_step's identical \
              addition for the Henyey-Greenstein path; the rest matches \
              transport_physics.wgsl's own apply_frosted_bounce parameter-for-parameter"
)]
pub(crate) fn apply_frosted_bounce(
    ctx: &RayMaterialContext,
    geo: &BounceRefractionGeometry,
    normal: Vec3,
    inside_gem: bool,
    is_extraordinary: bool,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
) -> (Vec3, bool, Option<bool>, Option<f32>) {
    let u1 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_U_STREAM)) as f32) / 4_294_967_295.0;
    let u2 =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_DIR_V_STREAM)) as f32) / 4_294_967_295.0;

    if geo.sin2_t > 1.0 {
        // Forced reflect (TIR), probability 1 -- no draw, no pdf division, mirroring
        // `apply_tir_bounce` for the polished path. Always interior -- see this
        // function's "Sign convention for NEE eligibility" doc section -- so no NEE.
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for s in &mut *stokes {
            *s = StokesVector::unpolarized(s.intensity());
        }
        return (new_dir, inside_gem, None, None);
    }

    let cos_t = (1.0 - geo.sin2_t).max(0.0).sqrt();
    let r_s = f32::mul_add(geo.n2, -cos_t, geo.n1 * geo.cos_i)
        / f32::mul_add(geo.n2, cos_t, geo.n1 * geo.cos_i);
    let r_p = f32::mul_add(geo.n1, -cos_t, geo.n2 * geo.cos_i)
        / f32::mul_add(geo.n1, cos_t, geo.n2 * geo.cos_i);
    let r_unpol = (0.5 * r_p.mul_add(r_p, r_s * r_s)).clamp(1e-4, 1.0 - 1e-4);
    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_unpol {
        let new_dir = cosine_weighted_hemisphere(u1, u2, normal);
        for k in 0..NUM_CHANNELS {
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity() / r_unpol);
            path_pdf[k] *= r_unpol;
        }
        // Exterior-side NEE only when this reflect happened while already
        // outside the gem -- see "Sign convention for NEE eligibility" above.
        let phase_pdf_for_mis = if nee.enabled && !inside_gem {
            nee_contribution_frosted_exterior(
                nee, lambdas, normal, rng_seed, bounce, stokes, radiance,
            );
            Some(new_dir.dot(normal) / std::f32::consts::PI)
        } else {
            None
        };
        (new_dir, inside_gem, None, phase_pdf_for_mis)
    } else {
        let new_dir = cosine_weighted_hemisphere(u1, u2, -normal);
        let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
        // Mode SELECTION is still a stochastic 50/50 draw -- no throughput weighting
        // accompanies it (a `split_pdf` divisor would estimate energy no interface can
        // deliver; see `apply_refract_bounce` in `refraction.rs`).
        let use_extraordinary = if entering_anisotropic {
            let split_rand = (hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
                as f32)
                / 4_294_967_295.0;
            split_rand < 0.5
        } else {
            is_extraordinary
        };
        let t_unpol = 1.0 - r_unpol;
        for k in 0..NUM_CHANNELS {
            // No `/ split_pdf` here (see `apply_refract_channel` in `refraction.rs`):
            // the selected mode already carries only its ~0.5 energy share, so dividing
            // by the 0.5 selection probability on top would double-count it.
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity() / t_unpol);
            // No `* split_pdf` either -- `spectral_mis_weight` is scale-invariant under
            // multiplying every channel's `path_pdf` by the same uniform factor, so this
            // was a no-op on the actual MIS weight, just one risking underflow.
            path_pdf[k] *= t_unpol;
        }
        let ext_normal = -normal;
        // Exterior-side NEE only when this transmit happened while inside
        // the gem (exiting to the environment) -- see "Sign convention for NEE
        // eligibility" above.
        let phase_pdf_for_mis = if nee.enabled && inside_gem {
            nee_contribution_frosted_exterior(
                nee, lambdas, ext_normal, rng_seed, bounce, stokes, radiance,
            );
            Some(new_dir.dot(ext_normal) / std::f32::consts::PI)
        } else {
            None
        };
        (
            new_dir,
            !inside_gem,
            entering_anisotropic.then_some(use_extraordinary),
            phase_pdf_for_mis,
        )
    }
}

/// Tests for [`henyey_greenstein_phase`], [`sample_henyey_greenstein_direction`],
/// [`maybe_scatter_or_extinguish`], and the `scattering_sigma_s` gate in
/// `trace_spectral_ray_inner`'s bounce loop.
#[cfg(test)]
mod scattering_tests {
    use super::{
        super::{
            absorption::channel_absorption_alphas,
            camera::Camera,
            color::cie_1931_cmf,
            environment::{EnvironmentSource, LightingPreset, environment_nee_pdf},
            transport::trace_spectral_ray,
        },
        *,
    };
    use crate::{
        geometry::cuts::StandardGemCuts,
        renderer::env_map::{EnvironmentMap, rgb_to_spectral_radiance},
    };

    /// [`henyey_greenstein_phase`] must integrate to `1.0` over the full sphere for any
    /// `g`, via dense quadrature over `cos_theta` (no azimuthal dependence, so the
    /// solid-angle integral reduces to `2*pi * integral_{-1}^{1} phase(mu) dmu`).
    #[test]
    fn hg_phase_integrates_to_one_over_the_sphere() {
        for g in [-0.8f32, -0.3, 0.0, 0.3, 0.7, 0.95] {
            let steps = 200_000u32;
            let dmu = 2.0 / f64::from(steps);
            let mut integral = 0.0f64;
            for i in 0..steps {
                let mu = f64::mul_add(f64::from(i) + 0.5, dmu, -1.0);
                integral = f64::mul_add(
                    f64::from(henyey_greenstein_phase(mu as f32, g)),
                    dmu,
                    integral,
                );
            }
            integral *= 2.0 * std::f64::consts::PI;
            assert!(
                (integral - 1.0).abs() < 1e-3,
                "HG phase function should integrate to 1.0 over the sphere for g={g}, got {integral}"
            );
        }
    }

    /// `henyey_greenstein_phase(1.0, g)` (exact forward direction) must strictly
    /// increase with `g` for `g > 0` -- pins the sign convention ("positive g
    /// forward-scatters") against the phase function itself, independent of the sampler.
    #[test]
    fn hg_phase_is_more_forward_peaked_for_larger_positive_g() {
        let p0 = henyey_greenstein_phase(1.0, 0.0);
        let p5 = henyey_greenstein_phase(1.0, 0.5);
        let p9 = henyey_greenstein_phase(1.0, 0.9);
        assert!(
            p0 < p5 && p5 < p9,
            "forward-direction phase value should strictly increase with g: p(g=0)={p0}, \
             p(g=0.5)={p5}, p(g=0.9)={p9}"
        );
    }

    /// The decisive check for hazard 3: [`sample_henyey_greenstein_direction`]'s mean
    /// `cos_theta` must converge to `g` exactly -- the closed-form mean of the HG
    /// distribution. A sign error, swapped `u1`/`u2`, or wrong CDF inversion would
    /// converge to the wrong mean and be caught here.
    #[test]
    fn hg_sampling_mean_cos_theta_converges_to_g() {
        let forward = Vec3::new(0.2, 0.6, 0.77).normalize();
        for g in [-0.7f32, -0.2, 0.0, 0.4, 0.85] {
            let trials = 400_000u32;
            let mut sum = 0.0f64;
            for i in 0..trials {
                let u1 = (hash_u32(i ^ 0x1111_1111) as f32) / 4_294_967_295.0;
                let u2 = (hash_u32(i ^ 0x2222_2222) as f32) / 4_294_967_295.0;
                let dir = sample_henyey_greenstein_direction(u1, u2, g, forward);
                sum += f64::from(dir.dot(forward));
            }
            let mean = (sum / f64::from(trials)) as f32;
            assert!(
                (mean - g).abs() < 0.01,
                "mean cos_theta for g={g} should converge to g itself, got {mean} over {trials} trials"
            );
        }
    }

    /// Sampled directions must stay unit-length and finite across the full `g` range,
    /// including at the isotropic branch threshold.
    #[test]
    fn hg_sampling_always_produces_finite_unit_directions() {
        let forward = Vec3::new(-0.3, 0.1, 0.94).normalize();
        for g in [-0.999f32, -0.5, -1e-3, 0.0, 1e-3, 0.5, 0.999] {
            for i in 0..2000u32 {
                let u1 = (hash_u32(i ^ 0x3333_3333) as f32) / 4_294_967_295.0;
                let u2 = (hash_u32(i ^ 0x4444_4444) as f32) / 4_294_967_295.0;
                let dir = sample_henyey_greenstein_direction(u1, u2, g, forward);
                assert!(
                    dir.is_finite(),
                    "non-finite direction for g={g}, i={i}: {dir:?}"
                );
                assert!(
                    (dir.length() - 1.0).abs() < 1e-4,
                    "non-unit direction for g={g}, i={i}: {dir:?} (len={})",
                    dir.length()
                );
            }
        }
    }

    fn round_brilliant_colourless_scattering_material(sigma_s: f32, g: f32) -> GemMaterial {
        // Colourless, non-dispersive, cubic -- isolates the scattering estimator from
        // the chromatic absorption/dispersion machinery.
        GemMaterial::new_custom("scattering furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0])
            .with_scattering(sigma_s, g)
    }

    /// Non-negotiable regression guard: a material with `scattering_sigma_s == 0.0`
    /// reached via `GemMaterial`'s plain default must trace bit-identically to the
    /// same material with scattering explicitly set to `(0.0, g)` for several `g` --
    /// proving the `scattering_sigma_s > 0.0` gate disables the new code path, since
    /// the estimator is not guaranteed to reduce to the old one algebraically at
    /// `sigma_s=0` (see [`maybe_scatter_or_extinguish`] hazard 2).
    #[test]
    fn default_off_scattering_is_bit_identical_regardless_of_g() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material_default = GemMaterial::ruby();
        assert!(
            material_default.scattering_sigma_s <= 0.0,
            "test premise: Ruby's own built-in scattering_sigma_s must be 0.0"
        );
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let env = || LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

        for g in [-0.6f32, 0.0, 0.4, 0.95] {
            let material_explicit_zero = material_default.clone().with_scattering(0.0, g);
            for iy in 0..6usize {
                for ix in 0..6usize {
                    let ray = camera.generate_ray(ix as f32, iy as f32, 6.0, 6.0, 0.5, 0.5);
                    for s in 0..4u32 {
                        let pixel_id = (iy as u32) * 6 + (ix as u32);
                        let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x1357_9BDF));
                        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                        let a = trace_spectral_ray(
                            ray,
                            &planes,
                            &material_default,
                            10,
                            env(),
                            seed,
                            hero_rand,
                            None,
                        );
                        let b = trace_spectral_ray(
                            ray,
                            &planes,
                            &material_explicit_zero,
                            10,
                            env(),
                            seed,
                            hero_rand,
                            None,
                        );
                        assert_eq!(
                            a.to_array(),
                            b.to_array(),
                            "sigma_s=0.0 (default) vs sigma_s=0.0 (explicit, g={g}) must be \
                             BIT identical at pixel ({ix},{iy}) sample {s}"
                        );
                    }
                }
            }
        }
    }

    /// The decisive check: a lossless scattering medium (`sigma_a == 0`, `sigma_s > 0`)
    /// immersed in a spatially uniform environment must still render at exactly that
    /// environment's own radiance -- scattering redirects energy, it must not create
    /// or destroy it. Mirrors `tests/raytracer_tests.rs`'s
    /// `frosted_girdle_white_furnace_energy_conservation_still_holds`.
    #[test]
    fn lossless_scattering_white_furnace_energy_conservation_holds() {
        const L0: f32 = 2.5;
        const SAMPLES_PER_PIXEL: u32 = 96;
        const GRID: usize = 12;
        const TOLERANCE: f32 = 0.08; // generous: CPU-only unit test sample budget

        let planes = StandardGemCuts::standard_round_brilliant();
        let material = round_brilliant_colourless_scattering_material(1.2, 0.4);
        let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let mut sum = Vec3::ZERO;
        let mut count = 0u32;
        for iy in 0..GRID {
            for ix in 0..GRID {
                let ray =
                    camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
                for s in 0..SAMPLES_PER_PIXEL {
                    let pixel_id = (iy as u32) * (GRID as u32) + (ix as u32);
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x5CA1_AB1E));
                    sum += trace_spectral_ray(
                        ray,
                        &planes,
                        &material,
                        16,
                        EnvironmentSource::HdrMap(&env_map),
                        seed,
                        (hash_u32(seed) as f32) / 4_294_967_295.0,
                        None,
                    );
                    count += 1;
                }
            }
        }
        let mean = sum / count as f32;

        let mut target = Vec3::ZERO;
        for step in 0..=(780 - 380) {
            let lambda = 380.0f32 + step as f32;
            let spec = rgb_to_spectral_radiance([L0, L0, L0], lambda);
            target += cie_1931_cmf(lambda) * spec;
        }
        target /= 106.856;

        let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
        let (ex, ey, ez) = (
            rel_err(mean.x, target.x),
            rel_err(mean.y, target.y),
            rel_err(mean.z, target.z),
        );
        println!(
            "[lossless-scattering furnace] mean={mean:?} target={target:?} rel_err=({ex:.4}, {ey:.4}, {ez:.4}) over {count} samples"
        );
        assert!(
            ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
            "a lossless scattering medium should still converge to the uniform \
             environment's own radiance (mean={mean:?}, target={target:?}, \
             rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
        );
    }

    /// The scattering estimator must actually redistribute energy directionally, not
    /// just pass an energy-conservation check by coincidence: a strongly forward-biased
    /// scattering medium should measurably change a transmissive scene's face-up
    /// appearance relative to `sigma_s = 0`.
    #[test]
    fn scattering_measurably_changes_face_up_appearance() {
        const SAMPLES_PER_PIXEL: u32 = 96;
        const GRID: usize = 14;

        let planes = StandardGemCuts::standard_round_brilliant();
        let clear = GemMaterial::diamond();
        assert!(clear.scattering_sigma_s <= 0.0);
        let hazy = clear.clone().with_scattering(1.5, 0.3);
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let env = || LightingPreset::RingLights.studio(1.0, 0.85, 0.95);

        let mut sum_clear = Vec3::ZERO;
        let mut sum_hazy = Vec3::ZERO;
        let mut count = 0u32;
        for iy in 0..GRID {
            for ix in 0..GRID {
                let ray =
                    camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
                for s in 0..SAMPLES_PER_PIXEL {
                    let pixel_id = (iy as u32) * (GRID as u32) + (ix as u32);
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x0BAD_F00D));
                    let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                    sum_clear +=
                        trace_spectral_ray(ray, &planes, &clear, 12, env(), seed, hero_rand, None);
                    sum_hazy +=
                        trace_spectral_ray(ray, &planes, &hazy, 12, env(), seed, hero_rand, None);
                    count += 1;
                }
            }
        }
        let mean_clear = sum_clear / count as f32;
        let mean_hazy = sum_hazy / count as f32;
        let delta_y = (mean_hazy.y - mean_clear.y).abs();
        let relative_change = delta_y / mean_clear.y.max(1e-6);
        println!(
            "[scattering face-up] clear Y={:.5} hazy Y={:.5} delta_y={:.5} ({:.2}%) over {count} samples",
            mean_clear.y,
            mean_hazy.y,
            delta_y,
            100.0 * relative_change
        );
        assert!(
            relative_change > 0.01,
            "a forward-scattering inclusion medium should measurably change face-up \
             brightness (>1%), not render identically to a clear stone -- got {:.4}% \
             (clear Y={:.5}, hazy Y={:.5})",
            100.0 * relative_change,
            mean_clear.y,
            mean_hazy.y
        );
    }

    /// [`maybe_scatter_or_extinguish`]'s hero channel must reproduce the textbook
    /// single-scattering-albedo/unity-survival identities (hazard 2) exactly, checked
    /// directly rather than only inferred from the furnace test above.
    #[test]
    fn hero_channel_weight_matches_albedo_and_unity_survival_identities() {
        let material = round_brilliant_colourless_scattering_material(0.8, 0.0);
        let alphas = [0.0f32; NUM_CHANNELS]; // sigma_a == 0 for this material, every channel
        let sigma_t_hero = material.scattering_sigma_s; // sigma_a == 0 for this material
        let albedo = material.scattering_sigma_s / sigma_t_hero; // == 1.0 (lossless)

        let mut scatter_trials = 0u32;
        let mut survive_trials = 0u32;
        for seed in 0..4000u32 {
            let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
            let mut path_pdf = [1.0f32; NUM_CHANNELS];
            let hit_t = 1.0f32; // fixed boundary distance
            let outcome = maybe_scatter_or_extinguish(
                &alphas,
                material.scattering_sigma_s,
                material.scattering_g,
                0,
                Vec3::Z,
                hit_t,
                1.0,
                seed,
                0,
                &mut stokes,
                &mut path_pdf,
            );
            if outcome.is_some() {
                scatter_trials += 1;
                assert!(
                    (stokes[0].intensity() - albedo).abs() < 1e-4,
                    "hero channel's scatter-branch weight should equal the single-scattering \
                     albedo sigma_s/sigma_t = {albedo} exactly (lossless), got {}",
                    stokes[0].intensity()
                );
            } else {
                survive_trials += 1;
                assert!(
                    (stokes[0].intensity() - 1.0).abs() < 1e-5,
                    "hero channel's no-scatter-branch weight should be exactly 1.0 \
                     (unity-survival identity), got {}",
                    stokes[0].intensity()
                );
            }
        }
        assert!(
            scatter_trials > 100 && survive_trials > 100,
            "test premise: both branches should fire many times at sigma_t*hit_t = {} \
             (scatter={scatter_trials}, survive={survive_trials})",
            sigma_t_hero * 1.0
        );
    }

    /// A companion (non-hero) channel's weight, in both branches, against an
    /// independently-derived closed form -- not merely re-checking the implementation
    /// against itself. Uses two channels with deliberately different chromatic
    /// absorption (Tourmaline's o-ray vs. e-ray at 550nm/430nm) so the hero-vs-companion
    /// divergence actually exercises the chromatic term, unlike a colourless material
    /// where `sigma_t_k == sigma_t_hero` for every k would hide a bug that swaps the
    /// shared hero-pdf denominator for a per-channel one.
    ///
    /// Closed forms (independently re-derived, not copied from the implementation):
    /// - no-scatter branch: `weight_k = exp(-(sigma_t_k - sigma_t_hero) * hit_t)`
    /// - scatter branch: `weight_k = (sigma_s / sigma_t_hero) * exp(-(sigma_t_k -
    ///   sigma_t_hero) * t_free)`, recovered by re-deriving `t_free` from the SAME
    ///   `dist_rand` the function drew (`t_free = -ln(1 - dist_rand) / sigma_t_hero`)
    ///   using a fixed `rng_seed`/`bounce` so the test can predict the exact draw.
    #[test]
    fn companion_channel_weight_matches_independently_derived_closed_form() {
        let material = GemMaterial::by_name("Tourmaline")
            .expect("\"Tourmaline\" must be a built-in material")
            .with_scattering(0.8, 0.0);
        let lambda_hero = 550.0f32;
        let lambda_companion = 430.0f32; // strongly dichroic vs. 550nm, see module doc comment
        let mat_ctx = RayMaterialContext {
            material: &material,
            lambdas: [
                lambda_hero,
                lambda_companion,
                lambda_hero,
                lambda_hero,
                lambda_hero,
                lambda_hero,
                lambda_hero,
                lambda_hero,
            ],
            hero_idx: 0,
            c_axis: material.c_axis,
            is_anisotropic: true,
            enable_internal_mode_coupling: true,
        };
        let cache = super::super::refraction::build_ray_wavelength_cache(&mat_ctx);
        let ray_dir = Vec3::new(0.3, 0.9, 0.3).normalize();
        let sigma_s = material.scattering_sigma_s;

        // Uses the real `channel_absorption_alphas` for alpha_hero/alpha_companion --
        // legitimate here since this test checks the downstream weight formula, not
        // pleochroic absorption itself.
        let stokes_probe = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let alphas =
            channel_absorption_alphas(&mat_ctx, &cache, Vec3::ZERO, ray_dir, &stokes_probe);
        assert!(
            (alphas[0] - alphas[1]).abs() > 1e-4,
            "test premise: hero (550nm) and companion (430nm) must have genuinely \
             different alpha (got {} vs {})",
            alphas[0],
            alphas[1]
        );
        let sigma_t_hero = alphas[0] + sigma_s;
        let sigma_t_companion = alphas[1] + sigma_s;

        let mut no_scatter_checked = false;
        let mut scatter_checked = false;
        for seed in 0..3000u32 {
            let hit_t = 0.9f32;
            let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
            let mut path_pdf = [1.0f32; NUM_CHANNELS];
            let outcome = maybe_scatter_or_extinguish(
                &alphas,
                sigma_s,
                material.scattering_g,
                0,
                ray_dir,
                hit_t,
                1.0,
                seed,
                0,
                &mut stokes,
                &mut path_pdf,
            );
            let dist_rand =
                (hash_u32(seed ^ hash_u32(DISTANCE_SAMPLE_STREAM)) as f32) / 4_294_967_295.0;
            let one_minus_u = (1.0 - dist_rand).max(1e-7);
            let t_free = -(one_minus_u.ln()) / sigma_t_hero;

            if outcome.is_some() {
                if t_free >= hit_t {
                    continue; // outcome disagreed with our recomputed t_free; skip (guard only)
                }
                scatter_checked = true;
                let expected =
                    (sigma_s / sigma_t_hero) * (-(sigma_t_companion - sigma_t_hero) * t_free).exp();
                let actual = stokes[1].intensity();
                assert!(
                    (actual - expected).abs() < 1e-3 * expected.abs().max(1.0),
                    "scatter-branch companion weight mismatch at seed={seed}: expected \
                     {expected} (closed form), got {actual}"
                );
            } else {
                no_scatter_checked = true;
                let expected = (-(sigma_t_companion - sigma_t_hero) * hit_t).exp();
                let actual = stokes[1].intensity();
                assert!(
                    (actual - expected).abs() < 1e-3 * expected.abs().max(1.0),
                    "no-scatter-branch companion weight mismatch at seed={seed}: expected \
                     {expected} (closed form), got {actual}"
                );
            }
        }
        assert!(
            no_scatter_checked && scatter_checked,
            "test premise: both branches should be exercised and checked \
             (no_scatter_checked={no_scatter_checked}, scatter_checked={scatter_checked})"
        );
    }

    /// Negative control: if a companion channel's `path_pdf` update used the hero's own
    /// technique density instead of its own, `spectral_mis_weight` would silently
    /// collapse toward `N` instead of reflecting genuine per-channel agreement. Pins
    /// the correct behaviour: two channels with different absorption must end up with
    /// different `path_pdf` values after a scatter event.
    #[test]
    fn scatter_event_path_pdf_is_genuinely_per_channel_when_absorption_is_chromatic() {
        // Real built-in Tourmaline (strongly dichroic o-ray vs. e-ray absorption),
        // opted into scattering.
        let material = GemMaterial::by_name("Tourmaline")
            .expect("\"Tourmaline\" must be a built-in material")
            .with_scattering(0.8, 0.0);
        let mat_ctx = RayMaterialContext {
            material: &material,
            lambdas: [550.0, 430.0, 550.0, 550.0, 550.0, 550.0, 550.0, 550.0],
            hero_idx: 0,
            c_axis: material.c_axis,
            is_anisotropic: true,
            enable_internal_mode_coupling: true,
        };
        let cache = super::super::refraction::build_ray_wavelength_cache(&mat_ctx);
        let ray_dir = Vec3::new(0.3, 0.9, 0.3).normalize();
        let probe_stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let alphas =
            channel_absorption_alphas(&mat_ctx, &cache, Vec3::ZERO, ray_dir, &probe_stokes);
        let mut found_scatter = false;
        for seed in 0..2000u32 {
            let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
            let mut path_pdf = [1.0f32; NUM_CHANNELS];
            let outcome = maybe_scatter_or_extinguish(
                &alphas,
                material.scattering_sigma_s,
                material.scattering_g,
                0,
                ray_dir,
                1.0,
                1.0,
                seed,
                0,
                &mut stokes,
                &mut path_pdf,
            );
            if outcome.is_some() {
                found_scatter = true;
                assert!(
                    (path_pdf[0] - path_pdf[1]).abs() > 1e-6,
                    "channel 0 (550nm) and channel 1 (430nm, strongly dichroic) must get \
                     DIFFERENT path_pdf values from a scatter event when the material's \
                     absorption is genuinely chromatic -- got path_pdf[0]={}, path_pdf[1]={}",
                    path_pdf[0],
                    path_pdf[1]
                );
            }
        }
        assert!(
            found_scatter,
            "test premise: at least one trial should hit the scatter branch"
        );
    }

    /// Veach's balance heuristic is a partition of unity -- for any two
    /// positive technique densities, `balance_heuristic(a, b) + balance_heuristic(b, a)
    /// == 1.0` exactly (the two divide the same `a + b` denominator). This is the
    /// property that makes `nee_contribution_hg_scatter`'s direct sample and
    /// `trace_spectral_ray_inner`'s complementary escape-branch weight sum to the true
    /// expected value rather than double-counting or losing energy.
    #[test]
    fn balance_heuristic_weights_sum_to_one() {
        let pairs = [
            (1.0f32, 1.0f32),
            (0.2, 5.0),
            (1e-3, 1e3),
            (12.5, 0.001),
            (7.0, 7.0),
        ];
        for (a, b) in pairs {
            let wa = balance_heuristic(a, b);
            let wb = balance_heuristic(b, a);
            assert!(
                (wa + wb - 1.0).abs() < 1e-5,
                "balance_heuristic({a}, {b}) + balance_heuristic({b}, {a}) should be 1.0, \
                 got {wa} + {wb} = {}",
                wa + wb
            );
        }
    }

    /// Degenerate input (both densities non-positive) must return `0.0`, not `NaN` from
    /// a `0.0 / 0.0` division -- the case a fully-occluded shadow ray or an all-black
    /// environment row can legitimately produce.
    #[test]
    fn balance_heuristic_handles_degenerate_zero_densities() {
        assert_eq!(balance_heuristic(0.0, 0.0), 0.0);
        assert!(!balance_heuristic(0.0, 0.0).is_nan());
    }

    /// [`sample_environment_for_nee`]/[`environment_nee_pdf`] must both decline for
    /// [`EnvironmentSource::Studio`] -- NEE is HDR-map-only (see [`NeeContext`]'s doc
    /// comment): the analytic rig has no importance distribution to draw a light sample
    /// from.
    #[test]
    fn nee_sampling_is_a_no_op_for_the_studio_rig() {
        let studio = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
        assert!(sample_environment_for_nee(studio, 0.3, 0.7).is_none());
        assert_eq!(environment_nee_pdf(studio, Vec3::Y), 0.0);
    }

    // The decisive NEE-on-vs-off unbiasedness measurement lives in `transport.rs`'s own
    // `#[cfg(test)] mod nee_tests` instead of here: it needs `trace_spectral_ray_inner`,
    // which is private to that module (this file has no access to it, unlike
    // `trace_spectral_ray` -- the public wrapper -- which cannot express an explicit
    // NEE-on/off A/B override).
}
