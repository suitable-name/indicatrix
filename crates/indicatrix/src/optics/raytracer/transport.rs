//! The spectral transport loop: [`trace_spectral_ray`]/[`trace_spectral_ray_with_finish`]
//! (public entries).
//!
//! [`trace_spectral_ray_with_finish_soa`] is the same, with the `PlanesSoA32`
//! intersection arena built once by the caller instead of per sample.
//! [`trace_spectral_ray_inner`] is their shared bounce loop, plus the per-bounce facet
//! dispatch and Russian-roulette/mode-coupling machinery they drive.
//!
//! [`trace_spectral_ray_inner`] builds the per-trace [`ExitSplitCtx`] and threads it
//! alongside `radiance`/`path_pdf` into [`dispatch_bounce`] -- see `refraction`'s
//! "Exit-event spectral splitting" doc comment for the estimator. `enable_exit_splitting`
//! is hardcoded `true` at every public entry point; only this file's `#[cfg(test)]`
//! module exercises it as an explicit A/B parameter (see `exit_splitting_tests`).

use super::{
    NUM_CHANNELS,
    absorption::{apply_absorption, rotate_stokes_to_plane_of_incidence},
    camera::{FacetFinish, HitRecord, Ray},
    color::{
        apply_von_kries_white_balance, integrate_channels_to_xyz,
        integrate_channels_to_xyz_families,
    },
    environment::{
        EnvironmentSource, environment_nee_pdf, environment_white_balance,
        sample_environment_channel,
    },
    intersect::{build_plane_soa, intersect_polyhedron_soa, shading_normal_near_edge},
    refraction::{
        BounceContext, BounceRay, BounceRefractionGeometry, BounceState, ExitEvent, ExitSplitCtx,
        PathModeState, RayMaterialContext, RayWavelengthCache, RngDraw,
        apply_partial_fresnel_bounce, apply_tir_bounce, build_ray_wavelength_cache,
        compute_bounce_refraction_geometry, entry_eigenmode_selection, poynting_dir_for_mode,
    },
    sampling::{MODE_COUPLING_STREAM, RUSSIAN_ROULETTE_STREAM, hash_u32},
    scattering::{
        NeeContext, ScatterStepOutcome, apply_frosted_bounce, balance_heuristic, try_scatter_step,
    },
};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::{CrystalSystem, GemMaterial},
        polarization::StokesVector,
    },
};
use glam::Vec3;

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

/// Accumulates the environment radiance for a ray that exited or missed the gemstone
/// entirely, terminating the bounce loop.
///
/// `StudioRig::new` (~40 sin/cos plus 16 vector normalizes) is constant across an
/// entire ray -- `light_yaw`/`light_pitch` don't vary per spectral channel -- so it is
/// built once here, before the `NUM_CHANNELS` loop, rather than once per channel
/// (measured: 204ns per `StudioRig::new`, x8 redundant rebuilds per ray ~= 1373ns/ray).
///
/// `observer` is the unit direction from the stone towards the eye, for the lit
/// lighting models' head shadow (see `environment::sample_studio_environment_observed`).
///
/// `mis_weight` scales every channel's contribution uniformly -- `1.0`
/// (exactly, so `stokes[k].intensity() * 1.0` is bit-identical to the bare value)
/// reproduces this function's behaviour with NEE absent precisely; every call site with
/// NEE disabled passes `1.0`. A value `< 1.0` is the
/// balance-heuristic weight for a phase/BSDF-sampled continuation directly following an
/// NEE-eligible scattering event -- see `trace_spectral_ray_inner`'s own call site.
fn accumulate_miss_radiance(
    environment: EnvironmentSource<'_>,
    ray_dir: Vec3,
    observer: Vec3,
    lambdas: &[f32; NUM_CHANNELS],
    stokes: &[StokesVector; NUM_CHANNELS],
    mis_weight: f32,
    radiance: &mut [f32; NUM_CHANNELS],
) {
    let studio_rig = match environment {
        EnvironmentSource::Studio {
            light_yaw,
            light_pitch,
            ..
        } => Some(crate::optics::studio_rig::StudioRig::new(
            light_yaw,
            light_pitch,
        )),
        EnvironmentSource::HdrMap(_) => None,
    };
    for k in 0..NUM_CHANNELS {
        let env_spectral = sample_environment_channel(
            environment,
            ray_dir,
            lambdas[k],
            studio_rig.as_ref(),
            observer,
        );
        // `StokesVector::intensity` clamps `I` to >= 0 before it reaches the
        // environment sample, matching `spectral_transport.wgsl`'s equivalent clamp at
        // its miss/environment-lookup site -- negative `I` is unphysical either side.
        radiance[k] = (stokes[k].intensity() * mis_weight).mul_add(env_spectral, radiance[k]);
    }
}

/// Dispatches one bounce's TIR / partial-reflect / refract event -- or, for a
/// `FacetFinish::Frosted` facet, `apply_frosted_bounce` instead -- then applies
/// internal mode re-coupling if that event was an internal reflection. Extracted out
/// of the bounce loop purely for clippy's function-length lint; behavior is unchanged.
///
/// `was_internal_reflection` mirrors the polished-reflect condition
/// (`inside_gem && is_extraordinary_update.is_none() && new_inside_gem == inside_gem`,
/// evaluated pre-bounce): it is unconditionally `true` for a TIR bounce, since TIR is
/// only reachable with `inside_gem == true` (`n1 > n2` for the hero channel -- see
/// `compute_bounce_refraction_geometry`), and `false` for any transmit/refract event,
/// since those always flip `inside_gem`.
///
/// `current_k` is the wave normal `k` (see `refraction`'s "wave normal vs Poynting
/// direction" design note), read here as the incoming `k` and updated in place to the
/// outgoing `k'` alongside `current_ray.dir` (`S'`). A `Frosted` bounce collapses
/// `k' == S'`: a diffuse bounce depolarizes like a Henyey-Greenstein scatter (see
/// `try_scatter_step`'s identical treatment), leaving no coherent wave-normal-vs-
/// Poynting distinction to track past it.
///
/// Returns the `pending_light_mis` carry -- `Some` only when the `Frosted`
/// branch dispatched an NEE-eligible outcome (see `apply_frosted_bounce`'s "Sign
/// convention for NEE eligibility" doc section), `None` for every TIR/polished dispatch
/// and every non-eligible frosted one. The caller stashes this in `pending_light_mis`
/// exactly like `try_scatter_step`'s identical HG-path carry.
#[expect(
    clippy::too_many_arguments,
    reason = "bundles every value fixed for the trace (ctx) or the bounce (cache, geo) \
              into context structs; the rest are this bounce's scalar inputs, the RNG \
              stream identity, and the per-ray state a bounce can mutate (stokes, \
              path_pdf, current_ray, current_k, inside_gem, is_extraordinary) -- \
              collapsing those into one struct would hide which fields each bounce kind \
              touches behind an opaque &mut; `exit` is the per-trace `ExitSplitCtx` \
              (see its doc comment), used only by the apply_partial_fresnel_bounce \
              branch -- the frosted/TIR branches never reach an exit event; `nee`/ \
              `lambdas`/`radiance` are the HDR-map NEE inputs/output, used only by \
              the frosted branch, mirroring try_scatter_step's identical addition for \
              the Henyey-Greenstein path; `incoming_light_mis` is the light-sample MIS \
              carry, \
              threaded through so the partial-Fresnel arm's transmit-out case can return \
              it unchanged rather than silently dropping it"
)]
fn dispatch_bounce(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    geo: &BounceRefractionGeometry,
    hit_point: Vec3,
    normal: Vec3,
    current_plane_normal: Vec3,
    finish: FacetFinish,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    exit: &mut ExitSplitCtx<'_>,
    current_ray: &mut Ray,
    current_k: &mut Vec3,
    inside_gem: &mut bool,
    is_extraordinary: &mut bool,
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
    incoming_light_mis: Option<(f32, Vec3)>,
) -> Option<(f32, Vec3)> {
    // Pre-bounce `inside_gem`, captured before any branch below mutates it -- both the
    // `was_internal_reflection` computation further down and the transmit-out
    // carry-through need the value from BEFORE this bounce, not after.
    let pre_bounce_inside_gem = *inside_gem;
    let (new_k, new_s, new_inside_gem, is_extraordinary_update, exact_p_o, bounce_light_mis) =
        if finish == FacetFinish::Frosted {
            let (new_dir, new_inside_gem, is_extraordinary_update, frosted_light_mis) =
                apply_frosted_bounce(
                    ctx,
                    geo,
                    normal,
                    *inside_gem,
                    *is_extraordinary,
                    rng_seed,
                    bounce,
                    stokes,
                    path_pdf,
                    nee,
                    lambdas,
                    radiance,
                );
            (
                new_dir,
                new_dir,
                new_inside_gem,
                is_extraordinary_update,
                None,
                // The frosted arm's own carry pairs with `new_dir`: unlike the polished
                // exit below, a frosted transmit's `new_dir` is already the true
                // exterior propagation direction (no further refraction happens between
                // here and a possible direct escape next iteration), so pairing the
                // carry with `new_dir` is bit-identical to reading `current_ray.dir`
                // (equal to this same `new_dir`) at escape time. The
                // INCOMING carry is deliberately dropped here, superseded by this
                // bounce's own fresh one (or `None`).
                frosted_light_mis.map(|phase_pdf| (phase_pdf, new_dir)),
            )
        } else if geo.sin2_t > 1.0 {
            let (k_prime, exact_p_o) = apply_tir_bounce(
                ctx,
                geo,
                *is_extraordinary,
                *current_k,
                normal,
                stokes,
                path_pdf,
            );
            let s_prime = poynting_dir_for_mode(ctx, geo, k_prime, *inside_gem, *is_extraordinary);
            // TIR is always an internal reflection (still inside afterward) -- the
            // incoming carry has no competing NEE sample to weigh itself against here,
            // so it is dropped, exactly like every other non-transmit-out dispatch.
            (k_prime, s_prime, *inside_gem, None, exact_p_o, None)
        } else {
            let bctx = BounceContext { ctx, cache, geo };
            let ray = BounceRay {
                k_hat: *current_k,
                normal,
            };
            let mode_state = PathModeState {
                current_plane_normal,
                inside_gem: *inside_gem,
                is_extraordinary: *is_extraordinary,
            };
            let rng = RngDraw { rng_seed, bounce };
            let mut state = BounceState { stokes, path_pdf };
            let mut exit_event = ExitEvent { exit, hit_point };
            let (k_prime, s_prime, new_inside_gem, is_extraordinary_update, exact_p_o) =
                apply_partial_fresnel_bounce(
                    &bctx,
                    ray,
                    mode_state,
                    rng,
                    &mut state,
                    &mut exit_event,
                );
            // A scatter event's pending complementary-MIS carry must survive
            // the polished exit refraction rather than being dropped here. Reachable
            // only for the genuine transmit-out case -- `pre_bounce_inside_gem` was
            // true, the outcome flipped to exterior, and `is_extraordinary_update` is
            // `None` (guaranteed here since `entering_anisotropic` requires
            // `!inside_gem`, so it can never be the transmit branch's own entry-mode
            // update when the ray started inside) -- never for a reflect (which never
            // changes `inside_gem`) or an air->crystal entry transmit (`inside_gem` was
            // already `false`, so no carry could have been pending for it anyway).
            let transmit_out_of_gem =
                pre_bounce_inside_gem && !new_inside_gem && is_extraordinary_update.is_none();
            let bounce_light_mis = if transmit_out_of_gem {
                incoming_light_mis
            } else {
                None
            };
            (
                k_prime,
                s_prime,
                new_inside_gem,
                is_extraordinary_update,
                exact_p_o,
                bounce_light_mis,
            )
        };
    let was_internal_reflection =
        *inside_gem && is_extraordinary_update.is_none() && new_inside_gem == *inside_gem;
    current_ray.dir = new_s;
    current_ray.origin = hit_point + new_s * 1e-4;
    *current_k = new_k;
    *inside_gem = new_inside_gem;
    if let Some(updated) = is_extraordinary_update {
        *is_extraordinary = updated;
    }
    // `current_k` (updated above) and `stokes` are this bounce's post-event state --
    // what the internal re-coupling draw needs to weight itself by polarization
    // relative to the new wave normal's eigenbasis (see
    // `maybe_apply_internal_mode_coupling`'s doc comment). `exact_p_o` is the
    // closed-form `R_o/(R_o+R_e)` split already computed for a uniaxial internal
    // reflection (`None` for a frosted bounce, biaxial material, or transmit/exit
    // event, in which case the fallback heuristic below is used instead).
    maybe_apply_internal_mode_coupling(
        ctx,
        geo.is_biaxial,
        current_plane_normal,
        *current_k,
        stokes,
        was_internal_reflection,
        exact_p_o,
        rng_seed,
        bounce,
        is_extraordinary,
    );
    bounce_light_mis
}

/// Russian Roulette termination with weighted survival. A hard cutoff on dim paths
/// biases the estimator dark, worst on long internal bounce trains inside high-index
/// stones. Instead, survive with probability `q` (the path's max spectral throughput,
/// clamped) and on survival divide every Stokes vector by `q` so the estimator stays
/// unbiased (`E[survive] * (1/q) = 1`). Returns `false` if the path should terminate,
/// `true` if it survives (Stokes vectors rescaled in place).
///
/// `split_radiance` rides the same `1/q` rescale as `stokes`: a channel that split off
/// at an earlier bounce (`bounce > 4`) already folded its contribution into
/// `exit.split_radiance[k]`, and that contribution is committed into `radiance` only if
/// the hero path survives every remaining roulette draw -- so it must be rescaled
/// exactly like every other quantity riding the same path, or it understates itself by
/// a factor of `q` per surviving draw past the split.
pub(super) fn apply_russian_roulette(
    bounce: u32,
    rng_seed: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    split_radiance: &mut [f32; NUM_CHANNELS],
) -> bool {
    let max_intensity = stokes.iter().fold(0.0f32, |a, s| a.max(s.intensity()));
    let q = max_intensity.clamp(0.05, 1.0);
    let rr_rand =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM)) as f32) / 4_294_967_295.0;
    if rr_rand > q {
        return false;
    }
    for s in &mut *stokes {
        *s = s.scale(1.0 / q);
    }
    for r in &mut *split_radiance {
        *r /= q;
    }
    true
}

/// Stochastic o<->e (uniaxial) / mode-A<->mode-B (biaxial) re-coupling at an internal
/// reflection inside an anisotropic crystal.
///
/// # What this models
///
/// A path's eigenmode is assigned once at the air->crystal entry. Reusing that mode's
/// index unconditionally at every subsequent internal bounce via an isotropic Fresnel
/// calculation would be wrong: a uniaxial crystal's o/e eigenbasis is defined
/// relative to the LOCAL wave-normal direction at each facet, so a differently
/// oriented facet has a different eigenbasis than the one the path last bounced off:
/// real light partially converts between the two labels at every internal bounce, a
/// genuine contributor to the "doubling" high-birefringence stones (zircon,
/// moissanite) show. This function models that conversion as a fresh, unconditional
/// 50/50 coin flip of which mode's index governs the path from here, rather than a
/// per-bounce probability derived from the actual angle between the old and new
/// eigenbases (that would need genuine anisotropic Fresnel coefficients at an
/// anisotropic interface -- out of scope). This lets a many-internal-bounce path
/// (the pavilion TIR trains responsible for brilliance) sample a genuine mix of
/// o-like/e-like propagation instead of freezing to its entry mode; it does not buy a
/// physically-derived conversion fraction per bounce or coherent superposition.
///
/// # Decision record: a relabeling, not a split -- no `1/p` division needed
///
/// At the entry split, one incident ray becomes two physically distinct rays each
/// carrying ~0.5 the energy, and the selected mode's Fresnel transmittance is
/// evaluated against the full incident Stokes vector -- so its 0.5 selection
/// probability must match each mode's true energy share for the unscaled sample to
/// stay unbiased. Here nothing new is created: the path still has exactly one Stokes
/// vector and one `path_pdf`, and this function only re-rolls which eigenmode LABEL
/// governs the refractive index for the next bounce, touching neither. Since neither
/// accumulator is touched, this draw's selection probability cannot introduce bias no
/// matter what it is -- unlike the entry split, there is no energy-fraction constraint
/// to violate here. That means the relabeling probability can safely use the exact
/// closed-form `internal_solve` Poynting-weighted split `R_o/(R_o+R_e)` when the caller
/// has one (`exact_p_o: Some(..)`, from the same `internal_solve` call
/// `apply_tir_bounce` already ran for this bounce's reflected energy) instead of the
/// blanket 50/50 -- it can only improve which label the next bounce uses. Biaxial
/// materials keep the blanket 50/50 (`BiaxialIndicatrix` has no uniaxial closed-form
/// solve to draw an exact split from).
///
/// `path_pdf` is left untouched rather than multiplied by `0.5` per draw: across the
/// long internal-bounce trains a TIR-heavy pavilion produces (tens to ~100 reflections
/// is not unusual), `0.5^n` underflows to `0.0` well before `n` reaches 100, which
/// would trip `spectral_mis_weight`'s "should not happen" guard.
///
/// Returns the freshly-selected `is_extraordinary`; the caller applies it only when the
/// internal reflection it just dispatched happened while `inside_gem && is_anisotropic`.
#[expect(
    clippy::too_many_arguments,
    reason = "thin re-labeling draw: ctx/is_biaxial/current_plane_normal/new_k/stokes \
              are this function's own inputs (see its doc comment), exact_p_o is the \
              closed-form override of the same probability, plus the RNG stream identity"
)]
fn apply_internal_mode_coupling(
    ctx: &RayMaterialContext,
    is_biaxial: bool,
    current_plane_normal: Vec3,
    new_k: Vec3,
    stokes: &[StokesVector; NUM_CHANNELS],
    exact_p_o: Option<f32>,
    rng_seed: u32,
    bounce: u32,
) -> bool {
    let p_o = exact_p_o.unwrap_or_else(|| {
        if is_biaxial {
            None
        } else {
            entry_eigenmode_selection(
                ctx.c_axis,
                current_plane_normal,
                new_k,
                stokes[ctx.hero_idx],
            )
        }
        .map_or(0.5, |(p, ..)| p)
    });
    let split_rand =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ MODE_COUPLING_STREAM)) as f32) / 4_294_967_295.0;
    split_rand >= p_o
}

/// Applies [`apply_internal_mode_coupling`] in place on `is_extraordinary` exactly when
/// the bounce dispatch that just ran was an internal reflection inside an anisotropic
/// crystal; a no-op otherwise. `was_internal_reflection` is `true` for a TIR bounce
/// (only reachable from inside the gem -- see `compute_bounce_refraction_geometry`),
/// and `is_extraordinary_update.is_none() && new_inside_gem == inside_gem_before_bounce`
/// for `apply_partial_fresnel_bounce`'s outcome (its reflect arm returns `None` and
/// never changes `inside_gem`; its refract arm always flips `inside_gem` and returns
/// `Some`).
#[expect(
    clippy::too_many_arguments,
    reason = "thin dispatcher for apply_internal_mode_coupling's own inputs plus the \
              was_internal_reflection gate and RNG stream identity"
)]
fn maybe_apply_internal_mode_coupling(
    ctx: &RayMaterialContext,
    is_biaxial: bool,
    current_plane_normal: Vec3,
    new_k: Vec3,
    stokes: &[StokesVector; NUM_CHANNELS],
    was_internal_reflection: bool,
    exact_p_o: Option<f32>,
    rng_seed: u32,
    bounce: u32,
    is_extraordinary: &mut bool,
) {
    if ctx.enable_internal_mode_coupling && ctx.is_anisotropic && was_internal_reflection {
        *is_extraordinary = apply_internal_mode_coupling(
            ctx,
            is_biaxial,
            current_plane_normal,
            new_k,
            stokes,
            exact_p_o,
            rng_seed,
            bounce,
        );
    }
}

/// Why a traced path's bounce loop stopped, for the `bounce_cost` benchmark harness
/// (`examples/bounce_cost.rs`). Maps 1:1 onto [`trace_spectral_ray_inner`]'s
/// break/fall-through sites:
/// - [`Self::Escaped`]: the ray missed every facet (or exited the polyhedron) and
///   [`accumulate_miss_radiance`] sampled the environment.
/// - [`Self::ScatterAbsorbed`]: [`try_scatter_step`] returned
///   [`ScatterStepOutcome::ScatteredAndTerminated`] -- only reachable for a material
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
    // bool parameter exists only so this file's `#[cfg(test)]` module can A/B it (see
    // `mode_coupling_tests::internal_mode_coupling_changes_zircon_render`). Girdle
    // finish: `&[]` for `facet_finishes` looks up `FacetFinish::default() ==
    // Polished` for every facet, reproducing all-polished behaviour.
    //
    // A single traced ray is not a hot per-sample batch loop, so the `PlanesSoA32`
    // arena is built fresh here -- see [`trace_spectral_ray_with_finish_soa`] for the
    // entry point a caller tracing many samples against the same `planes` should use.
    let plane_soa = build_plane_soa(planes);
    trace_spectral_ray_inner(
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
    trace_spectral_ray_inner(
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
    let radiance = trace_spectral_ray_inner(
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

/// Records the primary ray's first hit for the denoiser's guide buffers. Only bounce 0
/// writes: at that point `current_ray` is still the camera ray, and the traced path is
/// always driven by the hero channel, so this is unambiguously the hero's own first hit.
fn capture_primary_hit(
    primary_hit_out: &mut Option<&mut Option<HitRecord>>,
    bounce: u32,
    hit: Option<HitRecord>,
) {
    if bounce == 0
        && let Some(slot) = primary_hit_out.as_deref_mut()
    {
        *slot = hit;
    }
}

/// The facet's finish, with `Polished` for any index `facet_finishes` doesn't cover.
fn facet_finish_for(facet_finishes: &[FacetFinish], facet_idx: usize) -> FacetFinish {
    facet_finishes.get(facet_idx).copied().unwrap_or_default()
}

/// Interior-side handling of one facet hit: flips the geometric normal to face the
/// interior ray, then applies the segment's plain Beer-Lambert absorption. A no-op
/// while the ray is outside the gem.
///
/// A scattering-active material's extinction for this segment is already applied by
/// `try_scatter_step`'s no-scatter branch -- calling `apply_absorption` here too would
/// double-charge absorption. `scattering_sigma_s <= 0.0` is the non-scattering case,
/// where this is the only absorption application.
///
/// `ray_ctx` bundles `(&RayMaterialContext, &RayWavelengthCache)` to keep this
/// function's argument count within clippy's `too_many_arguments` limit.
///
/// `k_hat` is the WAVE NORMAL `k` (`current_k` in the caller), not the Poynting
/// direction `S` -- `apply_absorption`'s assigned-mode E-field derives from `k`; `hit_t`
/// (path length) stays geometric. See `refraction`'s design note, rule 6.
///
/// `is_extraordinary` names which eigenmode this path was assigned to at its most recent
/// air->crystal entry -- see `absorption::channel_absorption_alphas_assigned`'s doc
/// comment.
fn apply_interior_segment(
    ray_ctx: (&RayMaterialContext, &RayWavelengthCache),
    k_hat: Vec3,
    is_extraordinary: bool,
    hit_t: f32,
    inside_gem: bool,
    normal: &mut Vec3,
    stokes: &mut [StokesVector; NUM_CHANNELS],
) {
    let (ctx, cache) = ray_ctx;
    if !inside_gem {
        return;
    }
    *normal = -*normal;
    if ctx.material.scattering_sigma_s <= 0.0 {
        apply_absorption(ctx, cache, k_hat, is_extraordinary, hit_t, stokes);
    }
}

/// Builds the per-sample [`RayMaterialContext`].
fn build_ray_material_context(
    material: &GemMaterial,
    lambdas: [f32; NUM_CHANNELS],
    hero_idx: usize,
    enable_internal_mode_coupling: bool,
) -> RayMaterialContext<'_> {
    // Per-material optical c-axis for anisotropic birefringence.
    let c_axis = material.c_axis;
    let is_anisotropic = material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4;
    RayMaterialContext {
        material,
        lambdas,
        hero_idx,
        c_axis,
        is_anisotropic,
        enable_internal_mode_coupling,
    }
}

/// Bundles [`build_ray_material_context`] and [`build_ray_wavelength_cache`] into one
/// call -- see [`RayWavelengthCache`]'s doc comment for what it caches.
fn build_ray_context(
    material: &GemMaterial,
    lambdas: [f32; NUM_CHANNELS],
    hero_idx: usize,
    enable_internal_mode_coupling: bool,
) -> (RayMaterialContext<'_>, RayWavelengthCache) {
    let mat_ctx =
        build_ray_material_context(material, lambdas, hero_idx, enable_internal_mode_coupling);
    let wavelength_cache = build_ray_wavelength_cache(&mat_ctx);
    (mat_ctx, wavelength_cache)
}

/// Records why/where the bounce loop stopped into the caller's `termination_out` slot;
/// a no-op if it is `None`.
fn record_termination(
    termination_out: &mut Option<&mut (u32, PathTermination)>,
    bounce: u32,
    reason: PathTermination,
) {
    if let Some(out) = termination_out.as_deref_mut() {
        *out = (bounce, reason);
    }
}

/// The shared body of [`trace_spectral_ray`]/[`trace_spectral_ray_with_finish`] -- see
/// those functions' doc comments for the parameter list. `enable_internal_mode_coupling
/// = false` reproduces the pre-existing behaviour (mode fixed at entry for the whole
/// interior traversal); only this file's tests use `false`.
///
/// `plane_soa` is the caller-supplied [`crate::simd::PlanesSoA32`] arena
/// `intersect_polyhedron_soa` scans, built once by the caller rather than once per
/// sample. `planes` is still taken separately since `shading_normal_near_edge` needs
/// the per-facet plane records themselves, not just the `SoA` arena.
#[expect(
    clippy::too_many_arguments,
    reason = "the shared bounce loop every public entry point delegates to -- see \
              trace_spectral_ray's own reason, plus the debug/instrumentation output \
              hooks (primary_hit_out, termination_out) and the caller-supplied \
              plane_soa arena alongside the planes slice it was built from"
)]
#[expect(
    clippy::too_many_lines,
    reason = "already at the pedantic line-length threshold; the bounce_cost harness's \
              termination bookkeeping pushes it a few lines over -- see \
              record_termination's doc comment for why that extraction, not further \
              splitting this already heavily-decomposed loop, is the right amount of \
              surgery"
)]
fn trace_spectral_ray_inner(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    mut primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    // `true` only at every public entry point when `environment` is `HdrMap`
    // (the procedural studio rig has no importance distribution to draw NEE samples
    // from, and its own sampling already matches its structure -- see
    // `NeeContext::enabled`'s doc comment), or when this file's own tests force it
    // explicitly for an on/off A-B comparison, mirroring `enable_exit_splitting`'s
    // identical precedent.
    enable_nee: bool,
    mut termination_out: Option<&mut (u32, PathTermination)>,
) -> Vec3 {
    // Hero is drawn over the full visible range [380, 780) with wraparound, so a hero
    // draw `h` and `h + channel_width` generate the same 8-member comb, cyclically
    // rotated -- each member is equally likely to be drawn as hero, which is the
    // premise spectral MIS (`spectral_mis_weight` below) requires: every wavelength
    // must be reachable as the hero channel at positive, uniform probability. See
    // `wrapped_hero_wavelengths`'s doc comment for the formula.
    let lambdas: [f32; NUM_CHANNELS] = wrapped_hero_wavelengths(hero_rand);

    let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
    let mut radiance = [0.0f32; NUM_CHANNELS];

    // `hero_idx` names which slot of `lambdas` (and every other per-channel array
    // below) holds the wavelength driving the shared geometric path. Provably 0 for
    // every invocation under this construction (`lambdas[0] == lambda_hero`
    // identically), but threaded explicitly so the code documents which channel plays
    // the hero role.
    let hero_idx: usize = 0;

    // Per-channel running density of "technique k (channel k as hero) would have
    // generated this exact realized path" -- see the TIR/reflect/refract branches
    // below and `spectral_mis_weight`'s doc comment. Starts at 1.0 for every channel.
    let mut path_pdf = [1.0f32; NUM_CHANNELS];

    let mut current_ray = initial_ray;
    // Unit direction from the stone back towards the eye: the lit lighting models darken
    // exit directions inside the head-shadow cone around it (see
    // `environment::sample_studio_environment_observed`). Per pixel rather than the
    // camera axis, so the cone is centred exactly where this pixel's eye sits.
    let observer = -initial_ray.dir;
    // The wave normal `k`, tracked alongside `current_ray.dir` (the Poynting/energy
    // direction `S`) -- see `refraction`'s "wave normal vs Poynting direction" note.
    // Starts equal to the initial ray direction (k == S in isotropic air).
    let mut current_k = initial_ray.dir;
    let mut inside_gem = false;
    // `None` until the first plane of incidence is recorded, so no spurious frame
    // rotation applies at the very first surface.
    let mut prev_plane_normal: Option<Vec3> = None;
    // Which eigenmode the ray inside the crystal was stochastically assigned at its
    // most recent air->crystal entry. Meaningless while `!inside_gem`; carried across
    // internal bounces so a path keeps using its entry index until it exits.
    //
    // For a UNIAXIAL material this is the ordinary/extraordinary split. For a BIAXIAL
    // material (Alexandrite, Topaz, Tanzanite) there is no "ordinary" ray -- both
    // eigenmodes are direction-dependent and both walk off -- so this flag is a plain
    // two-valued mode selector: `false` selects mode A (faster, lower-index root of
    // `BiaxialIndicatrix::wave_indices`), `true` selects mode B (slower, higher-index).
    let mut is_extraordinary = false;

    // Fixed across every bounce below -- see `RayMaterialContext`/`RayWavelengthCache`.
    let (mat_ctx, wavelength_cache) =
        build_ray_context(material, lambdas, hero_idx, enable_internal_mode_coupling);

    // Precomputed once per trace, like `accumulate_miss_radiance`'s own `studio_rig`.
    let exit_split_studio_rig = match environment {
        EnvironmentSource::Studio {
            light_yaw,
            light_pitch,
            ..
        } => Some(crate::optics::studio_rig::StudioRig::new(
            light_yaw,
            light_pitch,
        )),
        EnvironmentSource::HdrMap(_) => None,
    };
    // Exit-event splitting's own staging accumulator -- see
    // `ExitSplitCtx::split_radiance`'s doc comment for why a split channel's
    // contribution lands here first, folded into `radiance` only if the shared/hero
    // path terminates via `PathTermination::Escaped`.
    let mut split_radiance = [0.0f32; NUM_CHANNELS];
    // Per-trace context for exit-event spectral splitting -- see `refraction`'s
    // "Exit-event spectral splitting" doc comment.
    let mut exit_split_ctx = ExitSplitCtx {
        plane_soa,
        environment,
        studio_rig: exit_split_studio_rig,
        observer,
        split_radiance: &mut split_radiance,
        enabled: enable_exit_splitting,
        compat: [u8::MAX; NUM_CHANNELS],
    };
    // Set alongside `record_termination(.., PathTermination::Escaped)` below -- the
    // only point `radiance` is ever populated from a real environment sample. See
    // `ExitSplitCtx::split_radiance`'s doc comment for why `split_radiance` must only
    // be committed when this ends up `true`.
    let mut path_escaped = false;

    // Next-event estimation's per-trace context (fixed for the whole trace,
    // like `exit_split_ctx` above) plus the one piece of state that carries from a
    // scatter event to its VERY NEXT loop iteration -- see `ScatterStepOutcome::
    // ScatteredAndSurvived`'s doc comment for exactly what this represents and why it is
    // consumed (via `.take()`) every single iteration regardless of outcome.
    let nee_ctx = NeeContext {
        environment,
        plane_soa,
        enabled: enable_nee,
    };
    let mut pending_light_mis: Option<(f32, Vec3)> = None;

    for bounce in 0..max_bounces {
        // Consumed unconditionally every iteration: meaningful only if THIS iteration's
        // ray turns out to have escaped directly (see below); otherwise silently
        // dropped, which is correct -- a phase-sampled continuation that instead hits
        // more geometry has no competing NEE sample to weigh itself against.
        let phase_pdf_for_mis_this_check = pending_light_mis.take();
        let hit = intersect_polyhedron_soa(current_ray, plane_soa);

        // Denoiser wiring: the primary ray's first-hit depth/normal/facet index feed
        // the A-Trous denoiser's guide buffers (see `renderer::denoise`). Captured
        // only at bounce 0, which is unambiguously the hero channel's own first hit.
        capture_primary_hit(&mut primary_hit_out, bounce, hit);

        let Some(hit_rec) = hit else {
            // The camera ray sees the backdrop card, if the scene has one -- only the
            // stone's own light reaches the environment lookup below.
            if bounce == 0
                && super::environment::fill_backdrop(environment, &lambdas, &mut radiance)
            {
                path_escaped = true;
                record_termination(&mut termination_out, bounce, PathTermination::Escaped);
                break;
            }
            // Ray exited or missed the gemstone -> sample the environment source.
            // If an earlier bounce left a pending NEE-eligible carry still live (a
            // scatter event whose continuation just now escaped directly, possibly
            // after first transmitting out through a polished exit facet -- see
            // `dispatch_bounce`'s doc comment for why the carry must survive that
            // intervening refraction), this escape is the SAME light-sampling
            // technique's competing (BSDF/phase-sampled) continuation -- weight it by
            // the balance heuristic so the two techniques' contributions sum to the
            // true value rather than double-counting. Evaluated at the carried INTERIOR
            // direction, the same measure `nee_contribution_hg_scatter`
            // sampled in -- NOT `current_ray.dir`, which by the time a transmit-out
            // carry reaches here is the refracted EXTERIOR direction instead.
            // `phase_pdf_for_mis_this_check.map_or(1.0, ..)` reproduces the full-weight
            // behaviour exactly whenever no carry is live (including every trace with
            // NEE disabled, where this is always `None`).
            let mis_weight =
                phase_pdf_for_mis_this_check.map_or(1.0, |(phase_pdf, interior_dir)| {
                    let light_pdf = environment_nee_pdf(environment, interior_dir);
                    balance_heuristic(phase_pdf, light_pdf)
                });
            accumulate_miss_radiance(
                environment,
                current_ray.dir,
                observer,
                &lambdas,
                &stokes,
                mis_weight,
                &mut radiance,
            );
            path_escaped = true;
            record_termination(&mut termination_out, bounce, PathTermination::Escaped);
            break;
        };
        // Attempt a Henyey-Greenstein scattering event along this segment before
        // processing the facet -- see `try_scatter_step`'s doc comment.
        if inside_gem {
            match try_scatter_step(
                &mat_ctx,
                &wavelength_cache,
                material,
                is_extraordinary,
                &mut current_ray,
                &mut current_k,
                hit_rec.t,
                rng_seed,
                bounce,
                &mut stokes,
                &mut path_pdf,
                exit_split_ctx.split_radiance,
                nee_ctx,
                &lambdas,
                &mut radiance,
                facet_finishes,
            ) {
                ScatterStepOutcome::NotApplicable | ScatterStepOutcome::ReachedBoundary => {}
                ScatterStepOutcome::ScatteredAndSurvived(phase_pdf_for_mis) => {
                    // Scattered Stokes vectors are already depolarized, so the previous
                    // plane of incidence is no longer meaningful -- reset it so the
                    // next facet hit applies no spurious frame rotation.
                    prev_plane_normal = None;
                    // Consumed by the VERY NEXT iteration's escape check
                    // above, and only there -- see that check's own doc comment.
                    pending_light_mis = phase_pdf_for_mis;
                    continue;
                }
                ScatterStepOutcome::ScatteredAndTerminated => {
                    record_termination(
                        &mut termination_out,
                        bounce,
                        PathTermination::ScatterAbsorbed,
                    );
                    break;
                }
            }
        }

        let hit_point = current_ray.origin + hit_rec.t * current_ray.dir;
        // Facet edge rounding: see `shading_normal_near_edge`'s doc comment.
        let mut normal = shading_normal_near_edge(
            planes,
            hit_point,
            hit_rec.facet_idx,
            hit_rec.normal,
            material.edge_rounding_radius,
        );

        // See `rotate_stokes_to_plane_of_incidence`'s doc comment. The Stokes
        // plane-of-incidence frame is defined by the wave normal `k`, not `S`.
        let current_plane_normal =
            rotate_stokes_to_plane_of_incidence(current_k, normal, prev_plane_normal, &mut stokes);
        prev_plane_normal = Some(current_plane_normal);

        // See `apply_interior_segment`'s doc comment. Assigned-mode absorption's
        // E-field direction uses `k`; `hit_rec.t` stays geometric/`S`-based.
        apply_interior_segment(
            (&mat_ctx, &wavelength_cache),
            current_k,
            is_extraordinary,
            hit_rec.t,
            inside_gem,
            &mut normal,
            &mut stokes,
        );

        // See `compute_bounce_refraction_geometry`'s doc comment. Index lookups,
        // angles, and every downstream Snell/Fresnel evaluation use the wave normal
        // `k`, not `S`.
        let geo = compute_bounce_refraction_geometry(
            &mat_ctx,
            &wavelength_cache,
            normal,
            current_k,
            inside_gem,
            is_extraordinary,
        );

        // Girdle finish: `Polished` (default) takes the pre-existing dispatch;
        // `Frosted` takes `apply_frosted_bounce` instead.
        let finish = facet_finish_for(facet_finishes, hit_rec.facet_idx);
        // `dispatch_bounce`'s return is `Some` for an NEE-eligible frosted-facet
        // outcome (see `apply_frosted_bounce`'s doc comment), OR for a
        // polished transmit-out event that had a live incoming carry
        // (`phase_pdf_for_mis_this_check`, this same iteration's `.take()` result from
        // above) to pass through -- `None` for every other dispatch.
        pending_light_mis = dispatch_bounce(
            &mat_ctx,
            &wavelength_cache,
            &geo,
            hit_point,
            normal,
            current_plane_normal,
            finish,
            rng_seed,
            bounce,
            &mut stokes,
            &mut path_pdf,
            &mut exit_split_ctx,
            &mut current_ray,
            &mut current_k,
            &mut inside_gem,
            &mut is_extraordinary,
            nee_ctx,
            &lambdas,
            &mut radiance,
            phase_pdf_for_mis_this_check,
        );

        // See `apply_russian_roulette`'s doc comment. Reborrowed through
        // `exit_split_ctx.split_radiance` since `exit_split_ctx` already holds that
        // local borrowed mutably for the whole loop.
        if bounce > 4
            && !apply_russian_roulette(bounce, rng_seed, &mut stokes, exit_split_ctx.split_radiance)
        {
            record_termination(
                &mut termination_out,
                bounce,
                PathTermination::RussianRoulette,
            );
            break;
        }
    }

    // Each channel's MIS family, final after the last interior dispersive event.
    let compat = exit_split_ctx.compat;

    // Commit staged exit-split contributions into `radiance` only if the shared/hero
    // path itself reached its own environment lookup -- see
    // `ExitSplitCtx::split_radiance`'s doc comment for why this all-or-nothing gate is
    // required. Every channel is then integrated under the one shared
    // `spectral_mis_weight`.
    if path_escaped {
        for k in 0..NUM_CHANNELS {
            radiance[k] += split_radiance[k];
        }
    }

    // See `integrate_channels_to_xyz`'s doc comment. With splitting enabled every
    // channel is weighted over its own family instead -- see
    // `integrate_channels_to_xyz_families`'s doc comment.
    let xyz = if enable_exit_splitting {
        integrate_channels_to_xyz_families(&radiance, &lambdas, &path_pdf, hero_idx, compat)
    } else {
        integrate_channels_to_xyz(&radiance, &lambdas, &path_pdf, hero_idx)
    };

    // Von Kries white-balance (diagonalised in Bradford LMS, not raw XYZ -- see
    // `compute_illuminant_white_balance`'s doc comment) so the chosen illuminant
    // itself renders as neutral white. Only the analytic `Studio` rig has a
    // single well-defined illuminant colour temperature to neutralize against -- see
    // `environment_white_balance`'s own doc comment, which already documents the
    // `HdrMap` no-op. The transform is skipped entirely for `HdrMap` rather than run at
    // that documented-no-op `Vec3::ONE` scale, because the round trip is not
    // quite the identity in f32 (the two published Bradford matrices are not exact
    // inverses, `max|B*A - I| ~= 5.2e-7`): running it anyway would make a hybrid CPU/GPU
    // HDR frame -- the WGSL twin gates this same transform on `params.env_mode == 1u`
    // (`Studio`), never running it for `HdrMap` -- sum CPU and GPU tiles that disagree
    // systematically at exactly this floor. Skipping the transform entirely for
    // `HdrMap`, matching the WGSL gate, is exact instead of merely close.
    //
    // `.max(Vec3::ZERO)` clamps the result the same way `StokesVector::intensity`/
    // `cie_1931_cmf(_x8)` already clamp every other radiance quantity to non-negative.
    // Pre-white-balance `xyz` is provably non-negative, but the Bradford LMS
    // chromatic-adaptation matrices have negative off-diagonal entries, so the
    // transform does not itself preserve non-negativity for a sufficiently
    // saturated/spectrally-narrow input -- a pre-existing property of that transform.
    // Exit-event splitting can make more previously-terminated companion channels
    // survive to shift some inputs into that regime, so the clamp matters more now,
    // but it is the same physical floor already applied elsewhere in this file. Kept
    // unconditionally (including for `HdrMap`, which now skips the transform above it)
    // since it is a physical floor on `xyz` itself, not a byproduct of white balance.
    match environment {
        EnvironmentSource::Studio { .. } => {
            apply_von_kries_white_balance(xyz, environment_white_balance(environment))
        }
        EnvironmentSource::HdrMap(_) => xyz,
    }
    .max(Vec3::ZERO)
}

/// O<->e mode re-coupling at internal reflections.
#[cfg(test)]
mod mode_coupling_tests {
    use super::{
        super::{
            environment::LightingPreset,
            uniaxial_fresnel::{self, UniaxialFrame},
        },
        *,
    };
    use crate::geometry::cuts::StandardGemCuts;

    /// Builds the same `(k_hat, normal, frame)` shape `uniaxial_fresnel`'s own tests
    /// use (`normal = -Z`, incidence in the XZ plane) so a `UniaxialFrame` can be built
    /// directly from an incidence angle and optic axis, without a full bounce/geometry
    /// setup.
    fn frame_for(ang_deg: f32, c_axis: Vec3) -> (Vec3, Vec3, UniaxialFrame) {
        let theta = ang_deg.to_radians();
        let cos_i = theta.cos();
        let sin_i = theta.sin();
        let k_hat = Vec3::new(sin_i, 0.0, cos_i);
        let normal = Vec3::new(0.0, 0.0, -1.0);
        (
            k_hat,
            normal,
            UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i),
        )
    }

    /// `apply_internal_mode_coupling`'s relabeling probability uses the exact
    /// closed-form `uniaxial_fresnel::internal_solve` Poynting-weighted split
    /// `p_o = R_o / (R_o + R_e)` when the caller supplies one, rather than the
    /// polarization-projection heuristic.
    ///
    /// Mechanism-level check: for a moderate, non-degenerate uniaxial internal-
    /// reflection geometry (25 degrees incidence, c-axis tilted off both the facet
    /// normal and the incidence plane), compute `p_o` directly from `internal_solve`'s
    /// own `R_o`/`R_e`, then check the empirical draw frequency over many seeds
    /// reproduces that value (not a hard-coded 0.5) within binomial sampling error.
    #[test]
    fn internal_mode_coupling_draw_matches_exact_closed_form_split() {
        let lambdas = [589.3f32; NUM_CHANNELS];
        let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
        let ctx = build_ray_material_context(&zircon, lambdas, 0, true);
        let n_o = zircon.dispersion.evaluate(589.3);
        let n_e = zircon.extraordinary_index_at(589.3, n_o);

        let c_axis = Vec3::new(0.4, 0.5, 0.767_2).normalize();
        let (_, _, frame) = frame_for(25.0, c_axis);
        let sol = uniaxial_fresnel::internal_solve(n_o, n_o, n_e, c_axis, &frame, true);
        let ro_pow = sol.r_o.norm_sqr() * sol.flux_ro;
        let re_pow = sol.r_e.norm_sqr() * sol.flux_re;
        let p_o_exact = ro_pow / (ro_pow + re_pow).max(1e-12);

        let stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let mut extraordinary_count = 0u32;
        let trials = 20_000u32;
        for seed in 0..trials {
            if apply_internal_mode_coupling(
                &ctx,
                false,
                Vec3::X,
                Vec3::NEG_Z,
                &stokes,
                Some(p_o_exact),
                seed,
                3,
            ) {
                extraordinary_count += 1;
            }
        }
        let frac = f64::from(extraordinary_count) / f64::from(trials);
        let expected = f64::from(1.0 - p_o_exact);
        // 4-sigma binomial tolerance plus a floor for f32-vs-f64 conversion noise.
        let tol = 4.0f64.mul_add(
            (expected * (1.0 - expected) / f64::from(trials)).sqrt(),
            0.005,
        );
        assert!(
            (frac - expected).abs() < tol,
            "draw frequency should match the exact closed-form split p_o_exact={p_o_exact} \
             (expected extraordinary frac={expected}), got {frac} over {trials} trials \
             (tol={tol})"
        );
    }

    /// The exact split's real payoff over the old heuristic: at an incidence angle
    /// between the ordinary and extraordinary modes' own critical angles, one mode is
    /// in genuine TIR while the other only partially reflects -- the sharpest
    /// asymmetry `R_o`/`R_e` can produce, which the old heuristic (which never looked
    /// at either mode's reflectance) could not represent. Found by an angle sweep
    /// rather than a hand-derived critical angle.
    #[test]
    fn internal_mode_coupling_draw_is_strongly_biased_near_a_modes_own_critical_angle() {
        let lambdas = [589.3f32; NUM_CHANNELS];
        let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
        let ctx = build_ray_material_context(&zircon, lambdas, 0, true);
        let n_o = zircon.dispersion.evaluate(589.3);
        let n_e = zircon.extraordinary_index_at(589.3, n_o);
        let c_axis = Vec3::new(0.4, 0.5, 0.767_2).normalize();

        let mut best_angle = 0.0f32;
        let mut best_p_o = 0.5f32;
        let mut best_skew = 0.0f32;
        let steps: u16 = 4000;
        for i in 0..=steps {
            let ang = 20.0f32.mul_add(f32::from(i) / f32::from(steps), 20.0);
            let (_, _, frame) = frame_for(ang, c_axis);
            let sol = uniaxial_fresnel::internal_solve(n_o, n_o, n_e, c_axis, &frame, true);
            let ro_pow = sol.r_o.norm_sqr() * sol.flux_ro;
            let re_pow = sol.r_e.norm_sqr() * sol.flux_re;
            let p_o = ro_pow / (ro_pow + re_pow).max(1e-12);
            let skew = (p_o - 0.5).abs();
            if skew > best_skew {
                best_skew = skew;
                best_angle = ang;
                best_p_o = p_o;
            }
        }
        assert!(
            best_skew > 0.45,
            "test premise: sweep should find a strongly asymmetric angle between the \
             two modes' own critical angles -- best found: angle={best_angle} \
             p_o={best_p_o} skew={best_skew}"
        );

        let stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let mut extraordinary_count = 0u32;
        let trials = 20_000u32;
        for seed in 0..trials {
            if apply_internal_mode_coupling(
                &ctx,
                false,
                Vec3::X,
                Vec3::NEG_Z,
                &stokes,
                Some(best_p_o),
                seed,
                7,
            ) {
                extraordinary_count += 1;
            }
        }
        let frac = f64::from(extraordinary_count) / f64::from(trials);
        let expected = f64::from(1.0 - best_p_o);
        let tol = 4.0f64.mul_add(
            (expected * (1.0 - expected) / f64::from(trials)).sqrt(),
            0.005,
        );
        assert!(
            (frac - expected).abs() < tol,
            "at angle={best_angle} (p_o_exact={best_p_o}), draw frequency should match \
             expected={expected}, got {frac} over {trials} trials (tol={tol})"
        );
    }

    /// Runs `samples` trials of `trace_spectral_ray_inner` at `ray`/`planes`/
    /// `plane_soa`/`material`/`env`, seeded `seed_base + i`, once with
    /// `enable_internal_mode_coupling` forced on and once forced off (same seed both
    /// times), returning `(sum_with, sum_without)`. Split out of
    /// `internal_mode_coupling_changes_zircon_render` purely to keep that test under
    /// clippy's function-length lint -- both call sites there share this exact A/B
    /// sweep, once for Zircon and once for the Diamond null check.
    fn sum_with_and_without_mode_coupling(
        material: &GemMaterial,
        ray: Ray,
        planes: &[GpuFacetPlane],
        plane_soa: &crate::simd::PlanesSoA32,
        env: EnvironmentSource<'_>,
        samples: u32,
        seed_base: u32,
    ) -> (Vec3, Vec3) {
        let mut sum_with = Vec3::ZERO;
        let mut sum_without = Vec3::ZERO;
        for i in 0..samples {
            let seed = seed_base + i;
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            sum_with += trace_spectral_ray_inner(
                ray,
                planes,
                plane_soa,
                &[],
                material,
                12,
                env,
                seed,
                hero_rand,
                None,
                true,
                true,
                false,
                None,
            );
            sum_without += trace_spectral_ray_inner(
                ray,
                planes,
                plane_soa,
                &[],
                material,
                12,
                env,
                seed,
                hero_rand,
                None,
                false,
                true,
                false,
                None,
            );
        }
        (sum_with, sum_without)
    }

    /// The decisive measurement: render the same real Zircon material (the largest
    /// `birefringence_delta` in the built-in catalogue) at the same ray and seeds,
    /// averaged over many samples, with internal mode coupling forced on vs. off. Any
    /// nonzero, reproducible difference is caused by exactly one thing: whether the
    /// eigenmode governing internal TIR/reflect bounces is re-rolled after entry.
    #[test]
    fn internal_mode_coupling_changes_zircon_render() {
        let planes = StandardGemCuts::standard_round_brilliant();
        // Built once, reused across every call below -- `planes` is fixed for this test.
        let plane_soa = build_plane_soa(&planes);
        let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
        assert!(
            zircon.birefringence_delta.abs() > 0.01,
            "test assumes Zircon is strongly birefringent"
        );

        // Oblique ray (not aligned with c_axis == Y), 12 max_bounces -- produces real
        // internal TIR trains.
        let ray = Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
        };
        let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let samples = 256u32;

        let (sum_with, sum_without) = sum_with_and_without_mode_coupling(
            &zircon, ray, &planes, &plane_soa, env, samples, 1000,
        );
        let mean_with = sum_with / samples as f32;
        let mean_without = sum_without / samples as f32;
        let diff = (mean_with - mean_without).length();
        assert!(
            diff > 1e-4,
            "internal mode coupling should measurably change Zircon's rendered XYZ \
             (mean_with={mean_with:?}, mean_without={mean_without:?}, diff={diff})"
        );

        // Null check: the identical comparison for a CUBIC (isotropic) material must
        // show exactly zero difference, proving this is the `is_anisotropic` gate.
        let diamond = GemMaterial::by_name("Diamond").expect("Diamond must be a built-in material");
        let (sum_with_diamond, sum_without_diamond) = sum_with_and_without_mode_coupling(
            &diamond, ray, &planes, &plane_soa, env, samples, 1000,
        );
        assert_eq!(
            sum_with_diamond, sum_without_diamond,
            "Diamond (cubic, is_anisotropic == false) must be bit-identical with the flag \
             either way -- the mechanism must not leak into isotropic materials"
        );
    }
}

/// Empirical unbiasedness check plus a variance-ratio measurement, via
/// `trace_spectral_ray_inner`'s own `enable_exit_splitting` A/B switch (the same
/// pattern `mode_coupling_tests` uses for `enable_internal_mode_coupling`). Two
/// independent sample sets (disjoint seed ranges) so the two-sample z-test below is
/// the standard unpaired form.
#[cfg(test)]
mod exit_splitting_tests {
    use super::*;
    use crate::{geometry::cuts::StandardGemCuts, optics::LightingPreset};

    /// Mean and (population) variance of `trace_spectral_ray_inner`'s XYZ output over
    /// `samples` independent draws. `f64` accumulation for `sumsq` avoids catastrophic
    /// cancellation in `E[X^2] - E[X]^2` at these sample counts.
    struct RenderStats {
        mean: Vec3,
        var: Vec3,
        n: f64,
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "test-only helper bundling exactly the inputs trace_spectral_ray_inner \
                  needs plus this sweep's sample count/seed base/on-off switch"
    )]
    fn render_stats(
        material: &GemMaterial,
        ray: Ray,
        planes: &[GpuFacetPlane],
        plane_soa: &crate::simd::PlanesSoA32,
        env: EnvironmentSource<'_>,
        samples: u32,
        seed_base: u32,
        enable_exit_splitting: bool,
    ) -> RenderStats {
        let mut sum = Vec3::ZERO;
        let mut sumsq_x = 0.0f64;
        let mut sumsq_y = 0.0f64;
        let mut sumsq_z = 0.0f64;
        for i in 0..samples {
            let seed = seed_base.wrapping_add(i);
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            let xyz = trace_spectral_ray_inner(
                ray,
                planes,
                plane_soa,
                &[],
                material,
                12,
                env,
                seed,
                hero_rand,
                None,
                true,
                enable_exit_splitting,
                false,
                None,
            );
            sum += xyz;
            let (xd, yd, zd) = (f64::from(xyz.x), f64::from(xyz.y), f64::from(xyz.z));
            sumsq_x = xd.mul_add(xd, sumsq_x);
            sumsq_y = yd.mul_add(yd, sumsq_y);
            sumsq_z = zd.mul_add(zd, sumsq_z);
        }
        let n = f64::from(samples);
        let mean = sum / samples as f32;
        let (mx, my, mz) = (f64::from(mean.x), f64::from(mean.y), f64::from(mean.z));
        let var = Vec3::new(
            mx.mul_add(-mx, sumsq_x / n).max(0.0) as f32,
            my.mul_add(-my, sumsq_y / n).max(0.0) as f32,
            mz.mul_add(-mz, sumsq_z / n).max(0.0) as f32,
        );
        RenderStats { mean, var, n }
    }

    /// Two-sample z-score per XYZ component: `(mean_a - mean_b) / sqrt(var_a/n_a +
    /// var_b/n_b)`. Matches the pooling `renderer::gpu::estimator_check`'s
    /// image-comparison harness uses -- a magnitude of a few units is ordinary
    /// sampling noise at these trial counts, not evidence of bias.
    fn z_scores(a: &RenderStats, b: &RenderStats) -> Vec3 {
        let se =
            |va: f32, na: f64, vb: f32, nb: f64| (f64::from(va) / na + f64::from(vb) / nb).sqrt();
        let sx = se(a.var.x, a.n, b.var.x, b.n).max(1e-20);
        let sy = se(a.var.y, a.n, b.var.y, b.n).max(1e-20);
        let sz = se(a.var.z, a.n, b.var.z, b.n).max(1e-20);
        Vec3::new(
            (f64::from(a.mean.x - b.mean.x) / sx) as f32,
            (f64::from(a.mean.y - b.mean.y) / sy) as f32,
            (f64::from(a.mean.z - b.mean.z) / sz) as f32,
        )
    }

    /// Regression guard for a bounce-budget truncation asymmetry (see
    /// [`super::ExitSplitCtx::split_radiance`]'s doc comment): a split channel resolves
    /// inline (one bounded intersect test) while the shared/hero path needs a whole
    /// extra loop iteration to discover its own miss, so at a low `max_bounces` cap
    /// splitting-on/off means diverged sharply (up to 2.6x for Synthetic Moissanite at
    /// `max_bounces = 4`) before `split_radiance` staging + conditional commit fixed
    /// it. Pins the on/off ratio close to 1 across a range of caps.
    #[test]
    fn exit_splitting_respects_bounce_budget_truncation() {
        const SAMPLES: u32 = 6_000;
        let planes = StandardGemCuts::standard_round_brilliant();
        let plane_soa = build_plane_soa(&planes);
        let ray = Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
        };
        let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let material = GemMaterial::by_name("Synthetic Moissanite")
            .expect("Synthetic Moissanite must be a built-in material");
        for max_bounces in [2u32, 4, 6, 8, 12] {
            let mut sum_on = Vec3::ZERO;
            let mut sum_off = Vec3::ZERO;
            for i in 0..SAMPLES {
                let seed_on = 10_000 + i;
                let hero_rand_on = (hash_u32(seed_on) as f32) / 4_294_967_295.0;
                sum_on += trace_spectral_ray_inner(
                    ray,
                    &planes,
                    &plane_soa,
                    &[],
                    &material,
                    max_bounces,
                    env,
                    seed_on,
                    hero_rand_on,
                    None,
                    true,
                    true,
                    false,
                    None,
                );
                let seed_off = 90_000 + i;
                let hero_rand_off = (hash_u32(seed_off) as f32) / 4_294_967_295.0;
                sum_off += trace_spectral_ray_inner(
                    ray,
                    &planes,
                    &plane_soa,
                    &[],
                    &material,
                    max_bounces,
                    env,
                    seed_off,
                    hero_rand_off,
                    None,
                    true,
                    false,
                    false,
                    None,
                );
            }
            let mean_on = sum_on / SAMPLES as f32;
            let mean_off = sum_off / SAMPLES as f32;
            let ratio_x = mean_on.x / mean_off.x;
            println!(
                "max_bounces={max_bounces}: mean_on={mean_on:?} mean_off={mean_off:?} ratio_x={ratio_x:.4}"
            );
            assert!(
                (0.97..1.03).contains(&ratio_x),
                "max_bounces={max_bounces}: splitting-on/off diverged sharply \
                 (ratio_x={ratio_x:.4}) -- likely a bounce-budget truncation \
                 regression (mean_on={mean_on:?}, mean_off={mean_off:?})"
            );
        }
    }

    #[test]
    fn exit_splitting_is_unbiased_and_reduces_variance() {
        const SAMPLES: u32 = 16_384;
        let planes = StandardGemCuts::standard_round_brilliant();
        let plane_soa = build_plane_soa(&planes);
        let ray = Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
        };
        let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);

        for name in ["Zircon", "Rutile", "Synthetic Moissanite"] {
            let material = GemMaterial::by_name(name)
                .unwrap_or_else(|| panic!("{name} must be a built-in material"));
            let on = render_stats(
                &material, ray, &planes, &plane_soa, env, SAMPLES, 10_000, true,
            );
            let off = render_stats(
                &material, ray, &planes, &plane_soa, env, SAMPLES, 90_000, false,
            );
            let z = z_scores(&on, &off);
            let var_ratio = Vec3::new(
                f64::from(off.var.x / on.var.x.max(1e-12)) as f32,
                f64::from(off.var.y / on.var.y.max(1e-12)) as f32,
                f64::from(off.var.z / on.var.z.max(1e-12)) as f32,
            );
            println!(
                "P6 unbiasedness/variance -- {name}: mean_on={:?} mean_off={:?} z={z:?} \
                 var_on={:?} var_off={:?} var_ratio(off/on)={var_ratio:?}",
                on.mean, off.mean, on.var, off.var
            );
            assert!(
                z.x.abs() < 3.5 && z.y.abs() < 3.5 && z.z.abs() < 3.5,
                "{name}: exit-event splitting changed the estimator's expectation \
                 (z={z:?}, mean_on={:?}, mean_off={:?}) -- should be unbiased",
                on.mean,
                off.mean
            );
        }
    }
}

/// Next-event-estimation unbiasedness, via `trace_spectral_ray_inner`'s own
/// `enable_nee` A/B switch (the same pattern `exit_splitting_tests`/`mode_coupling_tests`
/// use for their own on/off flags).
#[cfg(test)]
mod nee_tests {
    use super::*;
    use crate::geometry::cuts::StandardGemCuts;

    /// The decisive end-to-end measurement: on a uniform ("furnace-like") HDR map,
    /// NEE-on and NEE-off must converge to the SAME mean radiance -- MIS is a
    /// variance-reduction technique, not a change in expectation. A biased
    /// complementary-weight formula (double-counting direct light both ways, or a
    /// dropped `pending_light_mis` reset letting it leak into an unrelated later
    /// escape) would show up here as a systematic mean shift, not merely extra noise.
    #[test]
    fn nee_on_and_off_converge_to_the_same_mean_on_a_uniform_hdr_map() {
        const L0: f32 = 2.0;
        const SAMPLES: u32 = 20_000;
        const TOLERANCE: f32 = 0.05;
        let planes = StandardGemCuts::standard_round_brilliant();
        let plane_soa = build_plane_soa(&planes);
        // Lossless scattering (sigma_a == 0) so every scatter event is genuinely
        // NEE-eligible and the comparison isolates the NEE/MIS machinery itself, not
        // absorption.
        let material = GemMaterial::new_custom("G7 furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0])
            .with_scattering(1.0, 0.2);
        let env_map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [L0, L0, L0]);
        let environment = EnvironmentSource::HdrMap(&env_map);
        let ray = Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.1, -1.0, 0.05).normalize(),
        };

        let mut sum_on = Vec3::ZERO;
        let mut sum_off = Vec3::ZERO;
        for i in 0..SAMPLES {
            let seed = 42_000 + i;
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            sum_on += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                12,
                environment,
                seed,
                hero_rand,
                None,
                true,
                true,
                true,
                None,
            );
            sum_off += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                12,
                environment,
                seed,
                hero_rand,
                None,
                true,
                true,
                false,
                None,
            );
        }
        let mean_on = sum_on / SAMPLES as f32;
        let mean_off = sum_off / SAMPLES as f32;
        let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
        let (ex, ey, ez) = (
            rel_err(mean_on.x, mean_off.x),
            rel_err(mean_on.y, mean_off.y),
            rel_err(mean_on.z, mean_off.z),
        );
        println!(
            "[G7 NEE furnace] mean_on={mean_on:?} mean_off={mean_off:?} \
             rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
        );
        assert!(
            ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
            "NEE-on and NEE-off should converge to the same mean radiance on a uniform \
             HDR map (mean_on={mean_on:?}, mean_off={mean_off:?}, rel_err=({ex}, {ey}, \
             {ez}), tolerance={TOLERANCE})"
        );
    }

    /// The uniform-map furnace test above cannot catch NEE looking up the
    /// environment at the wrong direction -- `rgb_to_spectral_radiance` gives the same
    /// answer everywhere on a uniform map regardless of which direction is sampled. A
    /// genuinely directional (two-hemisphere: bright `+Y` half, black `-Y` half) map
    /// makes that bug visible: if [`nee_contribution_hg_scatter`] looked up
    /// the environment at the UNREFRACTED interior light-sample direction instead of the
    /// true refracted exterior one, NEE-on would read a systematically different mean
    /// than NEE-off (whose phase-sampled continuation always escapes along the genuinely
    /// refracted direction).
    #[test]
    fn nee_on_and_off_agree_on_a_two_hemisphere_hdr_map() {
        const L0: f32 = 3.0;
        const SAMPLES: u32 = 40_000;
        const TOLERANCE: f32 = 0.05;
        const WIDTH: usize = 8;
        const HEIGHT: usize = 4;

        let planes = StandardGemCuts::standard_round_brilliant();
        let plane_soa = build_plane_soa(&planes);
        // Lossless scattering, same as the uniform-map furnace test above, so this
        // isolates the NEE lookup-direction check here from the separately-tested
        // medium transmittance.
        let material =
            GemMaterial::new_custom("2c two-hemisphere probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0])
                .with_scattering(1.0, 0.2);

        // Row `0` is `v = 0` (north pole, `+Y`); row `HEIGHT - 1` is `v = 1` (south
        // pole, `-Y`) -- see `EnvironmentMap`'s own struct doc comment. The upper half
        // of the rows (`+Y` hemisphere) is bright, the lower half (`-Y` hemisphere)
        // stays black.
        let mut pixels = vec![[0.0f32, 0.0, 0.0]; WIDTH * HEIGHT];
        for row in 0..HEIGHT / 2 {
            for col in 0..WIDTH {
                pixels[row * WIDTH + col] = [L0, L0, L0];
            }
        }
        let env_map = crate::renderer::env_map::EnvironmentMap::from_rgb(WIDTH, HEIGHT, pixels)
            .expect("WIDTH * HEIGHT pixels for a WIDTH x HEIGHT map");
        let environment = EnvironmentSource::HdrMap(&env_map);
        let ray = Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.1, -1.0, 0.05).normalize(),
        };

        let mut sum_on = Vec3::ZERO;
        let mut sum_off = Vec3::ZERO;
        for i in 0..SAMPLES {
            let seed = 77_000 + i;
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            sum_on += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                12,
                environment,
                seed,
                hero_rand,
                None,
                true,
                true,
                true,
                None,
            );
            sum_off += trace_spectral_ray_inner(
                ray,
                &planes,
                &plane_soa,
                &[],
                &material,
                12,
                environment,
                seed,
                hero_rand,
                None,
                true,
                true,
                false,
                None,
            );
        }
        let mean_on = sum_on / SAMPLES as f32;
        let mean_off = sum_off / SAMPLES as f32;
        let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
        let (ex, ey, ez) = (
            rel_err(mean_on.x, mean_off.x),
            rel_err(mean_on.y, mean_off.y),
            rel_err(mean_on.z, mean_off.z),
        );
        println!(
            "[2c two-hemisphere NEE] mean_on={mean_on:?} mean_off={mean_off:?} \
             rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
        );
        assert!(
            ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
            "NEE-on and NEE-off should agree within tolerance on a directional \
             (two-hemisphere) HDR map, not only a uniform one (mean_on={mean_on:?}, \
             mean_off={mean_off:?}, rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
        );
    }

    /// The same decisive NEE-on-vs-off unbiasedness measurement as
    /// [`nee_on_and_off_converge_to_the_same_mean_on_a_uniform_hdr_map`] above, but for a
    /// FROSTED-GIRDLE scene instead of a Henyey-Greenstein scattering medium -- isolates
    /// `apply_frosted_bounce`'s own NEE contribution
    /// (`scattering::nee_contribution_frosted_exterior`) and its `pending_light_mis`
    /// carry from the HG path's identical-shaped machinery, which this scene's material
    /// (`scattering_sigma_s == 0.0`, the `GemMaterial::new_custom` default) never
    /// exercises at all. A camera-grid sweep (mirroring
    /// `frosted_girdle_white_furnace_energy_conservation_still_holds` in
    /// `tests/raytracer_tests.rs`) rather than one fixed ray, so both the entry-facet-
    /// reflect and exit-facet-transmit NEE-eligible branches (see
    /// `apply_frosted_bounce`'s "Sign convention for NEE eligibility" doc section) get
    /// exercised across many incidence angles.
    #[test]
    fn frosted_girdle_nee_on_and_off_converge_to_the_same_mean_on_a_uniform_hdr_map() {
        use super::super::camera::Camera;

        const L0: f32 = 2.0;
        const SAMPLES_PER_PIXEL: u32 = 48;
        const GRID: usize = 10;
        const TOLERANCE: f32 = 0.06;

        let planes = StandardGemCuts::standard_round_brilliant();
        let plane_soa = build_plane_soa(&planes);
        let finishes = crate::geometry::girdle_facet_finishes(&planes);
        // Colourless, non-dispersive, no volumetric scattering -- isolates the
        // frosted-facet NEE machinery from both chromatic absorption and the HG
        // scattering NEE path.
        let material =
            GemMaterial::new_custom("G8 frosted furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0]);
        let env_map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [L0, L0, L0]);
        let environment = EnvironmentSource::HdrMap(&env_map);

        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let mut sum_on = Vec3::ZERO;
        let mut sum_off = Vec3::ZERO;
        let mut count = 0u32;
        for iy in 0..GRID {
            for ix in 0..GRID {
                let ray =
                    camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
                for s in 0..SAMPLES_PER_PIXEL {
                    let pixel_id = (iy as u32) * (GRID as u32) + (ix as u32);
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x6667_8899));
                    let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                    sum_on += trace_spectral_ray_inner(
                        ray,
                        &planes,
                        &plane_soa,
                        &finishes,
                        &material,
                        12,
                        environment,
                        seed,
                        hero_rand,
                        None,
                        true,
                        true,
                        true,
                        None,
                    );
                    sum_off += trace_spectral_ray_inner(
                        ray,
                        &planes,
                        &plane_soa,
                        &finishes,
                        &material,
                        12,
                        environment,
                        seed,
                        hero_rand,
                        None,
                        true,
                        true,
                        false,
                        None,
                    );
                    count += 1;
                }
            }
        }
        let mean_on = sum_on / count as f32;
        let mean_off = sum_off / count as f32;
        let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
        let (ex, ey, ez) = (
            rel_err(mean_on.x, mean_off.x),
            rel_err(mean_on.y, mean_off.y),
            rel_err(mean_on.z, mean_off.z),
        );
        println!(
            "[G8 frosted-girdle NEE furnace] mean_on={mean_on:?} mean_off={mean_off:?} \
             rel_err=({ex:.4}, {ey:.4}, {ez:.4})"
        );
        assert!(
            ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
            "frosted-girdle NEE-on and NEE-off should converge to the same mean radiance \
             on a uniform HDR map (mean_on={mean_on:?}, mean_off={mean_off:?}, \
             rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
        );
    }
}

/// Plane-parallel uniaxial slab, e-mode forced. Drives
/// `compute_bounce_refraction_geometry`/`apply_partial_fresnel_bounce` directly
/// (bypassing the full stochastic bounce loop) at exactly two facets -- the slab's
/// entry and exit faces -- so the resulting `k`/`S` at each step can be inspected.
#[cfg(test)]
mod p2_wave_normal_tests {
    use super::{super::intersect::intersect_polyhedron, *};
    use crate::optics::materials::CrystalSystem;

    /// Two real slab faces (`+-Y`, `y` in `[-HALF_THICKNESS, HALF_THICKNESS]`) plus
    /// four far blank planes so `intersect_polyhedron` sees a bounded polyhedron --
    /// the measured walk-off displacement never reaches them.
    const HALF_THICKNESS: f32 = 1.0;
    fn slab_planes() -> Vec<GpuFacetPlane> {
        vec![
            GpuFacetPlane::new(Vec3::Y, -HALF_THICKNESS),
            GpuFacetPlane::new(Vec3::NEG_Y, -HALF_THICKNESS),
            GpuFacetPlane::new(Vec3::X, -1000.0),
            GpuFacetPlane::new(Vec3::NEG_X, -1000.0),
            GpuFacetPlane::new(Vec3::Z, -1000.0),
            GpuFacetPlane::new(Vec3::NEG_Z, -1000.0),
        ]
    }

    /// Drives the entry facet (bounce 0) and exit facet (bounce 1) directly via
    /// [`compute_bounce_refraction_geometry`]/[`apply_partial_fresnel_bounce`],
    /// searching `rng_seed` for the first value that selects the extraordinary
    /// eigenmode at entry and transmits at both facets. Returns `(entry_k, entry_s,
    /// exit_k, exit_s, lateral_displacement)`.
    ///
    /// # Panics
    ///
    /// Panics if no seed in `0..SEED_SEARCH_LIMIT` satisfies every condition.
    #[expect(
        clippy::too_many_lines,
        reason = "the fixed (enabled: false) ExitSplitCtx both \
                  apply_partial_fresnel_bounce calls require; the search loop body \
                  itself is unchanged"
    )]
    fn trace_forced_extraordinary_slab(
        material: &GemMaterial,
        incident_origin: Vec3,
        incident_dir: Vec3,
    ) -> (Vec3, Vec3, Vec3, Vec3, f32) {
        const SEED_SEARCH_LIMIT: u32 = 20_000;
        let planes = slab_planes();
        let lambdas = [589.3f32; NUM_CHANNELS]; // sodium D line, every channel (direction-only test)
        let mat_ctx = build_ray_material_context(material, lambdas, 0, false);
        let cache = build_ray_wavelength_cache(&mat_ctx);

        // The entry hit point depends only on the fixed incident ray/geometry, not on
        // `seed` -- computed once, outside the search loop.
        let entry_hit = intersect_polyhedron(
            Ray {
                origin: incident_origin,
                dir: incident_dir,
            },
            &planes,
        )
        .expect("incident ray must hit the slab's top face");
        let entry_point = incident_origin + entry_hit.t * incident_dir;

        // This test's assertions only read k1/s1/k2/s2 (direction, unaffected by
        // splitting) -- `enabled: false` keeps every other side effect out of the way,
        // avoiding any dependence on this synthetic slab's `EnvironmentSource` choice.
        let plane_soa = build_plane_soa(&planes);
        let mut split_radiance = [0.0f32; NUM_CHANNELS];
        let mut exit_ctx = ExitSplitCtx {
            plane_soa: &plane_soa,
            environment: EnvironmentSource::Studio {
                preset: crate::optics::raytracer::environment::LightingPreset::RingLights,
                exposure: 1.0,
                light_yaw: 0.0,
                light_pitch: 0.85,
                backdrop: 0.0,
            },
            studio_rig: None,
            observer: Vec3::ZERO,
            split_radiance: &mut split_radiance,
            enabled: false,
            compat: [u8::MAX; NUM_CHANNELS],
        };

        for seed in 0..SEED_SEARCH_LIMIT {
            let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
            let mut path_pdf = [1.0f32; NUM_CHANNELS];

            // Entry facet: normal +Y (top of the slab), current_k == current_ray.dir ==
            // incident_dir (air, isotropic -- k == S trivially before any interface).
            let entry_normal = Vec3::Y;
            let geo0 = compute_bounce_refraction_geometry(
                &mat_ctx,
                &cache,
                entry_normal,
                incident_dir,
                false,
                false,
            );
            if geo0.sin2_t > 1.0 {
                continue; // TIR is geometrically unreachable entering from air (n1==1), but stay defensive.
            }
            // `stokes` is unpolarized here, so `entry_eigenmode_selection` returns
            // `None` regardless -- computed properly anyway for clarity.
            let entry_plane_normal = incident_dir.cross(entry_normal).normalize_or_zero();
            let bctx0 = BounceContext {
                ctx: &mat_ctx,
                cache: &cache,
                geo: &geo0,
            };
            let mut state0 = BounceState {
                stokes: &mut stokes,
                path_pdf: &mut path_pdf,
            };
            let mut exit_event0 = ExitEvent {
                exit: &mut exit_ctx,
                hit_point: entry_point,
            };
            let (k1, s1, inside_gem_1, is_extraordinary_update, _) = apply_partial_fresnel_bounce(
                &bctx0,
                BounceRay {
                    k_hat: incident_dir,
                    normal: entry_normal,
                },
                PathModeState {
                    current_plane_normal: entry_plane_normal,
                    inside_gem: false,
                    is_extraordinary: false,
                },
                RngDraw {
                    rng_seed: seed,
                    bounce: 0,
                },
                &mut state0,
                &mut exit_event0,
            );
            let entered_extraordinary = is_extraordinary_update == Some(true);
            if !inside_gem_1 || !entered_extraordinary {
                continue; // reflected off the top face, or entered the ORDINARY mode -- keep searching.
            }

            // Advance geometrically along S (k1/s1 as computed) to the exit facet.
            let post_entry_origin = entry_point + s1 * 1e-4;
            let exit_ray = Ray {
                origin: post_entry_origin,
                dir: s1,
            };
            let Some(exit_hit) = intersect_polyhedron(exit_ray, &planes) else {
                continue;
            };
            let exit_point = post_entry_origin + exit_hit.t * s1;
            // `intersect_polyhedron`'s hit normal is outward-facing; flip it to match
            // `apply_interior_segment`'s convention.
            let exit_normal = -exit_hit.normal;

            let geo1 =
                compute_bounce_refraction_geometry(&mat_ctx, &cache, exit_normal, k1, true, true);
            if geo1.sin2_t > 1.0 {
                continue; // Would TIR back into the slab -- keep searching for a seed that transmits.
            }
            // Exiting the crystal (`inside_gem == true`), so `entering_anisotropic`
            // is false at this call regardless -- this value is never consulted.
            let exit_plane_normal = k1.cross(exit_normal).normalize_or_zero();
            let bctx1 = BounceContext {
                ctx: &mat_ctx,
                cache: &cache,
                geo: &geo1,
            };
            let mut state1 = BounceState {
                stokes: &mut stokes,
                path_pdf: &mut path_pdf,
            };
            let mut exit_event1 = ExitEvent {
                exit: &mut exit_ctx,
                hit_point: exit_point,
            };
            let (k2, s2, inside_gem_2, _, _) = apply_partial_fresnel_bounce(
                &bctx1,
                BounceRay {
                    k_hat: k1,
                    normal: exit_normal,
                },
                PathModeState {
                    current_plane_normal: exit_plane_normal,
                    inside_gem: true,
                    is_extraordinary: true,
                },
                RngDraw {
                    rng_seed: seed,
                    bounce: 1,
                },
                &mut state1,
                &mut exit_event1,
            );
            if inside_gem_2 {
                continue; // Reflected back into the slab instead of transmitting out -- keep searching.
            }

            // Lateral displacement: perpendicular distance from exit_point to the
            // infinite line through entry_point along incident_dir.
            let to_exit = exit_point - entry_point;
            let along = to_exit.dot(incident_dir);
            let perp = to_exit - along * incident_dir;
            let lateral_displacement = perp.length();

            return (k1, s1, k2, s2, lateral_displacement);
        }
        panic!(
            "no seed in 0..{SEED_SEARCH_LIMIT} entered the extraordinary mode and \
             transmitted cleanly through both slab faces -- test premise violated"
        );
    }

    /// The decisive correctness check: for a plane-parallel uniaxial slab with a
    /// tilted c-axis, the extraordinary ray's exit wave normal `k` (and `S == k` once
    /// back in air) must come out parallel to the incident ray within 1e-5 -- the
    /// classical "parallel slab" Snell's-law result applied twice to the same `k` (see
    /// `refraction.rs`'s design note, rule 4). The exit point must also be laterally
    /// displaced by a nonzero amount -- the walk-off did something real, it just
    /// didn't change the outgoing direction.
    #[test]
    fn plane_parallel_uniaxial_slab_extraordinary_ray_exits_parallel_and_displaced() {
        let mut material = GemMaterial::by_name("Zircon")
            .expect("\"Zircon\" is a built-in uniaxial material in GemMaterial::all_materials()");
        assert_eq!(material.crystal_system, CrystalSystem::Tetragonal);
        assert!(
            material.birefringence_delta.abs() > 0.01,
            "test premise: Zircon must be strongly birefringent"
        );
        // c-axis deliberately tilted away from BOTH the slab normal (Y) and the
        // incidence plane (XY) -- genuine 3D walk-off, not a coincidental in-plane one.
        material.c_axis = Vec3::new(0.3, 0.8, 0.5).normalize();

        let incident_origin = Vec3::new(0.0, 5.0, 0.0);
        // ~16.7 degrees off normal incidence -- comfortably sub-critical for n~1.9, and
        // oblique enough that Snell's law genuinely bends k (unlike normal incidence,
        // where k passes straight through regardless of index and the k/S distinction
        // could never show up in the exit direction at all).
        let incident_dir = Vec3::new(0.3, -1.0, 0.0).normalize();

        let (entry_k, entry_s, exit_k, exit_s, lateral_displacement) =
            trace_forced_extraordinary_slab(&material, incident_origin, incident_dir);

        // Sanity: the extraordinary mode's walk-off must have actually fired (entry_k
        // != entry_s) -- otherwise this test would be silently checking the degenerate
        // ordinary-mode-equivalent case instead of what it claims to.
        assert!(
            (entry_k - entry_s).length() > 1e-4,
            "test premise: the extraordinary mode's walk-off should visibly separate k \
             from S at entry (entry_k={entry_k:?}, entry_s={entry_s:?})"
        );

        let cos_parallel_k = exit_k.dot(incident_dir).clamp(-1.0, 1.0);
        let cos_parallel_s = exit_s.dot(incident_dir).clamp(-1.0, 1.0);
        assert!(
            (1.0 - cos_parallel_k).abs() < 1e-5,
            "exit wave normal k must be parallel to the incident ray within 1e-5 \
             (exit_k={exit_k:?}, incident_dir={incident_dir:?}, 1-cos={})",
            1.0 - cos_parallel_k
        );
        // Once back in air, S == k exactly (isotropic medium) -- both must agree.
        assert!(
            (1.0 - cos_parallel_s).abs() < 1e-5,
            "exit Poynting direction S (== k in air) must be parallel to the incident ray \
             within 1e-5 (exit_s={exit_s:?}, incident_dir={incident_dir:?}, 1-cos={})",
            1.0 - cos_parallel_s
        );
        assert!(
            (exit_k - exit_s).length() < 1e-6,
            "S must equal k exactly once back in isotropic air (exit_k={exit_k:?}, \
             exit_s={exit_s:?})"
        );
        assert!(
            lateral_displacement > 1e-4,
            "the exit point must be laterally displaced from the straight-through path \
             by a nonzero amount (the walk-off's real, physical effect) -- got {lateral_displacement}"
        );
    }
}
