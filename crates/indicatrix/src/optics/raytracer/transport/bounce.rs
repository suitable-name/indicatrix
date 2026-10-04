//! Per-bounce facet dispatch and Russian-roulette/mode-coupling machinery.
//!
//! Accumulating environment radiance for an escaped ray, dispatching one bounce's
//! TIR/partial-reflect/refract/frosted event, the survival-weighted Russian-roulette
//! termination draw, and the stochastic o<->e (uniaxial) / mode-A<->mode-B (biaxial)
//! re-coupling at an internal reflection.

use super::super::{
    NUM_CHANNELS,
    camera::{FacetFinish, Ray},
    environment::{environment_nee_pdf, sample_environment_channels},
    refraction::{
        BounceContext, BounceRay, BounceRefractionGeometry, BounceState, ExitEvent, ExitSplitCtx,
        PathModeState, RayMaterialContext, RayWavelengthCache, RngDraw,
        apply_partial_fresnel_bounce, apply_tir_bounce, entry_eigenmode_selection,
        poynting_dir_for_mode,
    },
    sampling::{MODE_COUPLING_STREAM, RUSSIAN_ROULETTE_STREAM, hash_u32},
    scattering::{NeeContext, apply_frosted_bounce, balance_heuristic},
};
use crate::optics::polarization::StokesVector;
use glam::Vec3;

/// Accumulates the environment radiance for a ray that exited or missed the gemstone
/// entirely, terminating the bounce loop.
///
/// The environment, the `StudioRig` and the observer come from the per-trace `exit`
/// context, which builds the rig once from `light_yaw`/`light_pitch` -- neither varies per
/// spectral channel or per bounce, and `StudioRig::new` (~40 sin/cos plus 16 vector
/// normalizes) is far too costly to repeat for every escape. All `NUM_CHANNELS` channels
/// look along the same `ray_dir`, so the direction's wavelength-independent lighting is
/// evaluated once and each channel only applies its own spectral power; the per-channel
/// values are bit-identical to looking each channel up on its own.
///
/// `exit.observer` is the unit direction from the stone towards the eye, for the lit
/// lighting models' head shadow (see `environment::sample_studio_environment_observed`).
///
/// `mis_weight` scales every channel's contribution uniformly -- `1.0`
/// (exactly, so `stokes[k].intensity() * 1.0` is bit-identical to the bare value)
/// reproduces this function's behaviour with NEE absent precisely; every call site with
/// NEE disabled passes `1.0`. A value `< 1.0` is the
/// balance-heuristic weight for a phase/BSDF-sampled continuation directly following an
/// NEE-eligible scattering event -- see `trace_spectral_ray_inner`'s own call site.
pub(super) fn accumulate_miss_radiance(
    exit: &ExitSplitCtx<'_>,
    ray_dir: Vec3,
    lambdas: &[f32; NUM_CHANNELS],
    stokes: &[StokesVector; NUM_CHANNELS],
    mis_weight: f32,
    radiance: &mut [f32; NUM_CHANNELS],
) {
    let env_spectral = sample_environment_channels(
        exit.environment,
        ray_dir,
        lambdas,
        exit.studio_rig.as_ref(),
        exit.observer,
    );
    for k in 0..NUM_CHANNELS {
        // `StokesVector::intensity` clamps `I` to >= 0 before it reaches the
        // environment sample, matching `spectral_transport.wgsl`'s equivalent clamp at
        // its miss/environment-lookup site -- negative `I` is unphysical either side.
        radiance[k] = (stokes[k].intensity() * mis_weight).mul_add(env_spectral[k], radiance[k]);
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
pub(super) fn dispatch_bounce(
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
    // `*inside_gem` read directly below (both here and in `was_internal_reflection`
    // further down) is this bounce's PRE-event value -- nothing before the mutation at
    // `*inside_gem = new_inside_gem` further down touches it. `dispatch_polished_bounce`
    // reads the identical pre-event value back out of its own `mode_state.inside_gem`
    // (built from `*inside_gem` right here, below) rather than a redundant second
    // named binding.
    let (new_k, new_s, new_inside_gem, is_extraordinary_update, exact_p_o, bounce_light_mis) =
        if finish == FacetFinish::Frosted {
            let (new_dir, new_inside_gem, is_extraordinary_update, frosted_light_mis) =
                apply_frosted_bounce(
                    ctx,
                    geo,
                    normal,
                    hit_point,
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
            dispatch_polished_bounce(
                &bctx,
                ray,
                mode_state,
                rng,
                &mut state,
                &mut exit_event,
                (nee, incoming_light_mis),
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

/// [`dispatch_polished_bounce`]'s own return type -- named purely so clippy's
/// `type_complexity` lint doesn't trip on the six-tuple; mirrors
/// `refraction::dispatch::FresnelBounceResult`'s identical precedent for
/// `apply_partial_fresnel_bounce`'s own five-tuple, extended with the
/// `bounce_light_mis` carry.
type PolishedBounceResult = (
    Vec3,
    Vec3,
    bool,
    Option<bool>,
    Option<f32>,
    Option<(f32, Vec3)>,
);

/// The non-frosted, non-forced-TIR arm of [`dispatch_bounce`] -- the general
/// TIR/partial-reflect/refract dispatch (`apply_partial_fresnel_bounce`, covering both
/// the scalar-Fresnel and closed-form uniaxial paths). Extracted purely to keep
/// `dispatch_bounce` itself under clippy's function-length lint; behavior is unchanged,
/// this is a direct extraction of that function's own former `else` branch, with every
/// argument bundled into the SAME context structs `apply_partial_fresnel_bounce` itself
/// already takes (`bctx`/`ray`/`mode_state`/`rng`/`state`/`exit_event`, built by the
/// caller exactly as it built them inline before), plus one tuple for the two values
/// that function doesn't need (`nee`, for `split_mis_weight`'s own environment lookup,
/// and the live `incoming_light_mis` carry) -- keeping the argument count under
/// clippy's `too_many_arguments` limit without a new `#[expect]`.
///
/// `mode_state.inside_gem` is this bounce's PRE-event `inside_gem` (`dispatch_bounce`'s
/// own `pre_bounce_inside_gem`, read here via `mode_state` instead of a redundant extra
/// parameter -- `mode_state` is passed by value, unread again by the caller, so no
/// aliasing concern). Returns the same five-tuple `apply_partial_fresnel_bounce` does,
/// plus the transmit-out `bounce_light_mis` carry (`None` for every other outcome).
fn dispatch_polished_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
    nee_and_carry: (NeeContext<'_>, Option<(f32, Vec3)>),
) -> PolishedBounceResult {
    let (nee, incoming_light_mis) = nee_and_carry;
    let pre_bounce_inside_gem = mode_state.inside_gem;
    // This dispatch is the ONLY place a transmit-out-of-gem event with a live
    // incoming NEE carry can reach `try_split_exit_channel` (see
    // `ExitSplitCtx::split_mis_weight`'s doc comment): whether the hero itself
    // ends up transmitting out (vs. reflecting) is decided inside
    // `apply_partial_fresnel_bounce`/`apply_uniaxial_internal_bounce` below, but
    // `try_split_exit_channel` is reachable at all only from their own
    // transmit/chromatic-termination branch, and that branch always transmits OUT
    // when `pre_bounce_inside_gem` is true (entering_anisotropic, the only source
    // of an entry-transmit `Some` update, requires `!inside_gem`) -- so computing
    // the weight speculatively from `pre_bounce_inside_gem` here, before the
    // reflect/transmit coin flip happens, is exact, not an approximation.
    // Mirrors the hero's own escape weight at `transport::inner`'s
    // `phase_pdf_for_mis_this_check` site exactly: same balance heuristic, same
    // `environment_nee_pdf` at the carried INTERIOR direction.
    exit_event.exit.split_mis_weight = if pre_bounce_inside_gem {
        incoming_light_mis.map_or(1.0, |(phase_pdf, interior_dir)| {
            balance_heuristic(
                phase_pdf,
                environment_nee_pdf(nee.environment, interior_dir),
            )
        })
    } else {
        1.0
    };
    let (k_prime, s_prime, new_inside_gem, is_extraordinary_update, exact_p_o) =
        apply_partial_fresnel_bounce(bctx, ray, mode_state, rng, state, exit_event);
    // Reset immediately after dispatch -- `split_mis_weight` must never leak into
    // a later bounce's own (unrelated) split contributions.
    exit_event.exit.split_mis_weight = 1.0;
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
///
/// `pub(in super::super)` (raytracer + descendants), not `pub(super)`: `scattering`'s
/// `try_scatter_step` (a sibling of `transport`, not a descendant of it) calls this
/// directly for its own trailing Russian-roulette draw -- the same scope this function
/// had before the split, when it lived straight in `transport.rs` (parent: `raytracer`).
pub(in super::super) fn apply_russian_roulette(
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
/// moissanite) show. This function models that conversion as a fresh draw of which
/// mode's index governs the path from here: with the exact closed-form share when the
/// caller has one, the polarization-weighted `entry_eigenmode_selection` share for a
/// uniaxial reflection without one, and a blanket 50/50 for biaxial materials. It is not
/// a per-bounce probability derived from the actual angle between the old and new
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
/// accumulator is touched, no `1/p` weight is owed and there is no energy-fraction
/// constraint like the entry split's. The selection probability is therefore a
/// modelling choice that sets the o/e mixture along the path, not an importance-sampling
/// density that an estimator weight would correct. It uses the exact closed-form
/// `internal_solve` Poynting-weighted split `R_o/(R_o+R_e)` when the caller has one
/// (`exact_p_o: Some(..)`, from the same `internal_solve` call `apply_tir_bounce`
/// already ran for this bounce's reflected energy), which is the hero channel's share:
/// the path carries one mode label, so the companion channels use the hero's share even
/// though their own shares differ slightly (`docs/physics.md` section 2.4). Without a
/// closed-form share it falls back to the polarization-weighted
/// `entry_eigenmode_selection` heuristic, and biaxial materials keep the blanket 50/50
/// (`BiaxialIndicatrix` has no uniaxial closed-form solve to draw an exact split from).
///
/// `path_pdf` is left untouched rather than multiplied by `0.5` per draw: across the
/// long internal-bounce trains a TIR-heavy pavilion produces (tens to ~100 reflections
/// is not unusual), `0.5^n` underflows to `0.0` well before `n` reaches 100, which
/// would trip `spectral_mis_weight`'s "should not happen" guard.
///
/// Returns the freshly-selected `is_extraordinary`; the caller applies it only when the
/// internal reflection it just dispatched happened while `inside_gem && is_anisotropic`.
///
/// `pub(super)`: `transport::tests::mode_coupling_tests` exercises this directly (an A/B
/// mechanism-level check against the exact closed-form split), rather than only through
/// the full bounce loop.
#[expect(
    clippy::too_many_arguments,
    reason = "thin re-labeling draw: ctx/is_biaxial/current_plane_normal/new_k/stokes \
              are this function's own inputs (see its doc comment), exact_p_o is the \
              closed-form override of the same probability, plus the RNG stream identity"
)]
pub(super) fn apply_internal_mode_coupling(
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
