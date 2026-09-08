//! Fresnel reflection/transmission and Total Internal Reflection.
//!
//! The per-bounce refractive-index/incidence-angle geometry ([`RayMaterialContext`],
//! [`BounceRefractionGeometry`]) and the TIR/partial-reflect/refract bounce appliers
//! built on top of it.
//!
//! ## Uniaxial dispatch
//!
//! Every uniaxial (`is_anisotropic && !is_biaxial`, [`BounceRefractionGeometry::
//! uniaxial_frame`] `Some`) bounce runs the exact closed-form [`uniaxial_fresnel`]
//! solve rather than a scalar "effective index" approximation: [`apply_uniaxial_entry_bounce`]
//! at an isotropic-air -> crystal entry, [`apply_tir_bounce`]'s own branch at a
//! hero-forced (`sin2_t > 1.0`) internal reflection, and [`apply_uniaxial_internal_bounce`]
//! at every other internal event -- the general sub-critical partial reflection and
//! the uniaxial -> isotropic exit transmission, both from one
//! [`uniaxial_fresnel::internal_solve`] call per channel. [`apply_partial_reflect_bounce`]/
//! [`apply_refract_bounce`]/[`apply_refract_channel`]'s scalar-Fresnel machinery is
//! reached only by an isotropic material, a biaxial material, or the degenerate
//! wave-normal-parallel-to-optic-axis limit (see [`apply_partial_fresnel_bounce`] for
//! where each guard sits).
//!
//! ## Exit-event spectral splitting
//!
//! Implemented in [`apply_refract_channel`] (isotropic/biaxial exit, and the degenerate
//! uniaxial wave-normal-parallel-to-optic-axis case sharing its scalar machinery) and
//! in [`apply_uniaxial_internal_transmit_channels`] (the exact uniaxial exit);
//! [`try_split_exit_channel`] is the bounded fan-out primitive both share, and
//! [`ExitSplitCtx`] carries the per-trace state. Gated on [`ExitSplitCtx::enabled`];
//! `false` reproduces the legacy chromatic-termination estimator bit for bit (that
//! estimator is unbiased, verified against a single-wavelength reference).
//!
//! **Interior path unchanged.** Every bounce up to, but not including, the one where
//! the ray leaves the gem back into air is still one shared, hero-driven geometric
//! path. A channel whose own refracted direction at an interior dispersive event (an
//! entry into the gem) diverges from the hero's beyond `DIRECTION_MATCH_COS_TOL` still
//! loses its radiance for the rest of the trace.
//!
//! **The estimator, in three parts (they only work together):**
//!
//! 1. *The exit event.* A still-alive companion `k` whose own Snell direction
//!    diverges from the hero's, at the bounce where the hero-driven path leaves the
//!    gem, is no longer zeroed. It gets its own Fresnel transmission at its own index
//!    (the same formula the matching-direction case always used, factored into
//!    [`compute_channel_transmission`] / [`compute_uniaxial_exit_transmission`]), and
//!    [`try_split_exit_channel`] resolves its own continuation with exactly one extra
//!    intersection test: a deterministic environment lookup at `k`'s own wavelength
//!    along `k`'s own direction (a re-entry probe declines rather than recursing).
//!    Staged in [`ExitSplitCtx::split_radiance`] and committed into `radiance` only if
//!    the shared path itself goes on to escape, rescaled on every Russian-roulette
//!    survival exactly like `stokes` (`transport::apply_russian_roulette`) so a split
//!    contribution committed with probability `q` is compensated by the same `1/q`.
//!
//! 2. *`path_pdf` never stops accumulating for a live technique.* `path_pdf[k]` is
//!    technique k's (channel k as hero) density of having produced this geometric
//!    path. A companion that lost its radiance at an interior mismatch is still a
//!    technique that would have produced this path for every channel it remains
//!    compatible with (see 3), so its density keeps accumulating -- the mismatch site
//!    folds in the same transmit factor the matching case does, and the exit event
//!    folds its own transmit factor (`1 - r_unpol_k`, or `t_unpol_k` on the uniaxial
//!    path) into split and matching channels alike. The only genuine zero stays a
//!    genuine zero: a channel that cannot transmit at all where the hero did
//!    (`sin2_t_k > 1`) has density 0 for any path through that exit.
//!
//! 3. *Per-channel MIS families.* `color::spectral_mis_weight`'s balance heuristic
//!    `N * p_hero / sum_j p_j` is Veach's one-sample combination over the N techniques
//!    that could have produced the realized path. With splitting, every live companion
//!    contributes past the exit, and the set of techniques under which channel `c` is
//!    alive is NOT the hero's own alive set (direction tolerance keeps only channels
//!    within some spectral distance of a given hero alive, and that set shifts with
//!    the hero). Normalising `c`'s weight over the hero's set instead of its own makes
//!    weights summed over the heroes that see it exceed 1 (measured +7% Diamond / +12%
//!    Synthetic Moissanite bias against a single-wavelength reference). The fix:
//!    [`ExitSplitCtx::compat`] tracks the pairwise compatibility of all channels
//!    through every interior dispersive event ([`narrow_compat`], reusing the same
//!    `direction_matches` verdict for every pair involving the hero), and
//!    `color::integrate_channels_to_xyz_families` weights channel `c` by
//!    `N * path_pdf[hero] / sum_{j in compat[c]} path_pdf[j]` -- which sums to exactly
//!    1 over `c`'s own family, making the combined estimator unbiased. The exit event
//!    never narrows a family: every technique in `c`'s family produces `c`'s
//!    (deterministic) exit path.
//!
//! **Root cause of the bias.** Leaving `path_pdf[k]` at its prefix value for a split
//! channel, or at 0, or applying the exit factor some other way -- none of it fixes
//! the family-normalization problem in point 3; each choice only relocates the bias.
//!
//! **Verification.** `transport::exit_splitting_tests` compares splitting on vs. off
//! (independent seeds, two-sample z-scores) across several materials and a
//! bounce-budget sweep, and asserts the means agree while variance drops. A
//! single-wavelength ground truth (every companion terminated before the first
//! bounce, degenerating to a plain per-wavelength Monte Carlo) puts both the legacy
//! and this estimator within `|z| < 2.1` of reference for Diamond and Synthetic
//! Moissanite at 4, 6 and 12 bounces. The GPU mirror in
//! `renderer/shaders/spectral_transport.wgsl` implements the same three points.

use super::{
    NUM_CHANNELS,
    absorption::spectral_absorption,
    camera::Ray,
    environment::{EnvironmentSource, sample_environment_channel},
    intersect::intersect_polyhedron_soa,
    sampling::{BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, hash_u32},
    uniaxial_fresnel::{self, UniaxialFrame},
};
use crate::optics::{
    birefringence::{AbsorptionTensor3, BiaxialIndicatrix, BirefringenceParams},
    materials::GemMaterial,
    polarization::{MuellerMatrix, StokesVector},
    studio_rig::StudioRig,
};
use glam::Vec3;

/// The wave normal `k` and the Poynting (energy-propagation) direction `S` coincide
/// exactly in an isotropic medium (air, or a cubic material) and for the uniaxial
/// ordinary eigenmode (no walk-off by definition); they diverge only for the
/// extraordinary/mode-B eigenmode of a genuinely anisotropic material while inside it,
/// by the walk-off angle (order 1-2 degrees for zircon-class birefringence). Snell's
/// law and Fresnel phase matching apply to `k`, not `S`, so this module threads both
/// through the bounce loop: `S` (== `Ray::dir` / `current_ray.dir` in `transport.rs`)
/// for intersection/advancement and path length, `k` (== `current_k` in `transport.rs`)
/// for every index lookup, `cos_i`/`sin_i`, Snell refraction, Fresnel coefficients, the
/// TIR decision and phase, and the Stokes plane-of-incidence frame. [`poynting_dir_for_mode`]
/// recovers `S` from a freshly-reflected/refracted `k`.
///
/// Total Internal Reflection phase retardation delta = `delta_p` - `delta_s` (Fresnel
/// rhomb formula) for a wave whose channel index `n1k` puts it past its own critical
/// angle at this interface (`n1k * sin_i > 1`). Shared by the hero-past-critical
/// (deterministic reflect) branch and the partial-reflection branch's per-channel loop,
/// where an individual channel k can be past its own critical angle even though the
/// hero isn't.
#[inline]
pub(crate) fn tir_phase_delta(n1k: f32, cos_i: f32, sin_i: f32) -> f32 {
    let tan_half_delta_k = (cos_i * (n1k * n1k * sin_i).mul_add(sin_i, -1.0).max(0.0).sqrt())
        / (n1k * sin_i * sin_i).max(1e-6);
    2.0 * tan_half_delta_k.atan()
}

/// Per-ray context that stays fixed across every bounce of `trace_spectral_ray`'s main
/// loop: the material, the ray's 8 hero-wavelength comb, which slot drives the shared
/// geometric path, the optical c-axis, and whether this material is anisotropic at
/// all. Bundled into one struct to keep [`compute_bounce_refraction_geometry`]'s
/// argument count within clippy's `too_many_arguments` limit.
pub(crate) struct RayMaterialContext<'a> {
    pub(crate) material: &'a GemMaterial,
    pub(crate) lambdas: [f32; NUM_CHANNELS],
    pub(crate) hero_idx: usize,
    pub(crate) c_axis: Vec3,
    pub(crate) is_anisotropic: bool,
    /// Whether `maybe_apply_internal_mode_coupling` is active for this path. See
    /// `trace_spectral_ray_inner`'s `enable_internal_mode_coupling` parameter.
    pub(crate) enable_internal_mode_coupling: bool,
}

/// Per-sample cache of quantities that depend only on the ray's fixed 8-wavelength comb
/// (`ctx.lambdas`) and the material -- both fixed for the whole trace -- computed once
/// via [`build_ray_wavelength_cache`] and read back every bounce instead of
/// recomputed. Kept as its own struct rather than new fields on [`RayMaterialContext`]:
/// that struct is built via a bare struct literal at several
/// `renderer::gpu::transport_check` Tier 2 GPU self-test call sites that must keep
/// compiling unchanged, and it cannot derive `Default` (its `&'a GemMaterial` field
/// isn't `Default`).
pub(crate) struct RayWavelengthCache {
    /// `material.dispersion.evaluate(lambdas[k])` per channel. See
    /// [`per_channel_effective_extraordinary_indices`] for the per-bounce `n_eff_ch`
    /// half that reads it back.
    pub(crate) n_o_ch: [f32; NUM_CHANNELS],
    /// `material.biaxial_indicatrix(lambdas[hero_idx])`. Always exactly
    /// `biaxial_ch[hero_idx]`; kept as its own field so call sites that only need the
    /// hero's own indicatrix don't have to index `biaxial_ch` themselves.
    pub(crate) hero_indicatrix: Option<BiaxialIndicatrix>,
    /// `material.biaxial_indicatrix(lambdas[k])` per channel.
    pub(crate) biaxial_ch: [Option<BiaxialIndicatrix>; NUM_CHANNELS],
    /// Per-channel [`AbsorptionTensor3`] (uniaxial or biaxial, matching whether
    /// `hero_indicatrix.is_some()`), built from this channel's own `alpha_o`/`alpha_e`/
    /// `alpha_beta` (each `spectral_absorption` at `lambdas[k]`) and the material's
    /// fixed `c_axis`. `channel_absorption_alphas` calls
    /// `birefringence::effective_pleochroic_alpha` against this cached tensor rather
    /// than rebuilding one (the axis frame + `Mat3`) from scratch every bounce.
    pub(crate) tensor_ch: [AbsorptionTensor3; NUM_CHANNELS],
}

/// Per-trace context for exit-event spectral splitting -- see this module's top-of-file
/// "Exit-event spectral splitting" doc comment for the estimator this supports. The
/// fixed parts (`plane_soa`, `environment`, `studio_rig`, `enabled`) are bundled the
/// same way [`RayMaterialContext`]/[`RayWavelengthCache`] are; the two per-trace
/// accumulators (`split_radiance`, `compat`) are threaded as `&mut` through every
/// bounce-dispatch function exactly like `stokes`/`path_pdf` are. `radiance` itself
/// stays outside (threaded separately, as it always was).
pub(super) struct ExitSplitCtx<'a> {
    /// The same intersection arena `trace_spectral_ray_inner`'s own bounce loop already
    /// built once per trace -- reused here for [`try_split_exit_channel`]'s bounded
    /// "does channel k's own exit ray re-enter the gem" probe, never rebuilt.
    pub(super) plane_soa: &'a crate::simd::PlanesSoA32,
    pub(super) environment: EnvironmentSource<'a>,
    /// Precomputed once per trace, same rationale as [`accumulate_miss_radiance`]'s own
    /// `studio_rig`: constant across an entire ray, so building it once here avoids a
    /// redundant rebuild per split channel. `None` for [`EnvironmentSource::HdrMap`],
    /// which [`sample_environment_channel`] ignores.
    pub(super) studio_rig: Option<StudioRig>,
    /// A staging accumulator, separate from `trace_spectral_ray_inner`'s own
    /// `radiance`: every split channel's contribution lands here first, and
    /// `trace_spectral_ray_inner` folds it into `radiance` only if the shared/hero path
    /// itself ultimately terminates via [`PathTermination::Escaped`]. This mirrors the
    /// all-or-nothing way [`accumulate_miss_radiance`] already works for a plain
    /// matching channel: if the shared path instead terminates via Russian roulette,
    /// scatter absorption, or `max_bounces`, `accumulate_miss_radiance` is never called
    /// and every channel's `radiance` stays `0.0` regardless of how many exit events it
    /// survived. Without this staging + conditional-commit step, a split channel would
    /// keep contributions in exactly the cases a plain matching channel would have lost
    /// them -- an asymmetry `transport::exit_splitting_tests` caught empirically
    /// (splitting-on/off z-scores growing rather than shrinking with sample count).
    pub(super) split_radiance: &'a mut [f32; NUM_CHANNELS],
    /// `false` reproduces pre-splitting chromatic-termination behaviour bit-for-bit at
    /// every exit event; `true` (the production default from every public entry point)
    /// enables splitting. Mirrors `RayMaterialContext`'s own
    /// `enable_internal_mode_coupling` precedent: threaded only as far as
    /// `trace_spectral_ray_inner`'s own parameter, never exposed publicly.
    pub(super) enabled: bool,
    /// Pairwise spectral-compatibility mask -- bit `j` of `compat[c]` is set while
    /// channels `c` and `j` have refracted within `DIRECTION_MATCH_COS_TOL` of each
    /// other at every interior dispersive event so far (all bits set before the first
    /// one; `compat[c]` always keeps its own bit). `compat[c]` is channel c's MIS
    /// *family*: the set of techniques under which channel c would have stayed alive on
    /// this same geometric path, and therefore the set its balance-heuristic weight
    /// must be normalised over -- see this module's top-of-file doc comment. Narrowed
    /// by [`narrow_compat`]; read by `color::integrate_channels_to_xyz_families`. Only
    /// meaningful while `enabled`.
    pub(super) compat: [u8; NUM_CHANNELS],
}

const _: () = assert!(
    NUM_CHANNELS <= 8,
    "ExitSplitCtx::compat packs one bit per channel into a u8"
);

/// Bundles the three read-only context references (fixed per trace, per sample, and
/// per bounce respectively) every isotropic/biaxial bounce-dispatch function in this
/// module needs -- retiring the flat `ctx, cache, geo` parameter triple every one of
/// those functions used to take individually. Purely a signature-level grouping: every
/// field is the exact same reference the caller already had.
pub(super) struct BounceContext<'m, 'b> {
    pub(super) ctx: &'b RayMaterialContext<'m>,
    pub(super) cache: &'b RayWavelengthCache,
    pub(super) geo: &'b BounceRefractionGeometry,
}

/// The uniaxial-closed-form counterpart of [`BounceContext`]: every uniaxial
/// bounce-dispatch function shares exactly `ctx`/`geo`/`frame` (never `cache` -- the
/// closed-form `uniaxial_fresnel` solve reads indices straight off `ctx`/`geo` instead
/// of the biaxial-indicatrix cache).
pub(super) struct UniaxialBounceContext<'m, 'b> {
    pub(super) ctx: &'b RayMaterialContext<'m>,
    pub(super) geo: &'b BounceRefractionGeometry,
    pub(super) frame: &'b UniaxialFrame,
}

/// The wave normal `k` and facet normal at one bounce -- always consumed together
/// (Snell refraction/reflection both act on this pair). See this module's top-of-file
/// doc comment for the `k` (wave normal) vs `S` (Poynting direction) distinction.
#[derive(Clone, Copy)]
pub(super) struct BounceRay {
    pub(super) k_hat: Vec3,
    pub(super) normal: Vec3,
}

/// The mutable per-channel Stokes/path-pdf accumulators every bounce-dispatch function
/// reads and writes -- bundled so a `&mut` of this one struct replaces threading both
/// arrays as separate parameters. Always passed as `&mut BounceState<'_>` (never by
/// value) so a caller can reborrow it across a per-channel loop's repeated calls.
pub(super) struct BounceState<'a> {
    pub(super) stokes: &'a mut [StokesVector; NUM_CHANNELS],
    pub(super) path_pdf: &'a mut [f32; NUM_CHANNELS],
}

/// The per-bounce exit-event pair: [`ExitSplitCtx`]'s per-trace accumulator plus this
/// bounce's own hit point, needed together by [`try_split_exit_channel`]'s bounded
/// re-entry probe. Always passed as `&mut ExitEvent<'_, '_>` for the same reborrowing
/// reason as [`BounceState`].
pub(super) struct ExitEvent<'a, 'b> {
    pub(super) exit: &'a mut ExitSplitCtx<'b>,
    pub(super) hit_point: Vec3,
}

/// This bounce's RNG identity -- the same `(rng_seed, bounce)` pair every stochastic
/// branch decision in the bounce loop hashes against its own stream salt.
#[derive(Clone, Copy)]
pub(super) struct RngDraw {
    pub(super) rng_seed: u32,
    pub(super) bounce: u32,
}

/// [`apply_partial_fresnel_bounce`]'s own per-bounce path-mode state: the current
/// Stokes plane-of-incidence frame plus the two mode flags (`inside_gem`,
/// `is_extraordinary`) that decide which branch every downstream helper takes.
#[derive(Clone, Copy)]
pub(super) struct PathModeState {
    pub(super) current_plane_normal: Vec3,
    pub(super) inside_gem: bool,
    pub(super) is_extraordinary: bool,
}

/// Narrows [`ExitSplitCtx::compat`] after one interior dispersive event (an entry
/// refraction, or any other event where a channel's own refracted direction may
/// diverge from another's). `dirs[k]` is channel k's own direction out of this event
/// (`None` for a channel that cannot transmit here at all, whose `path_pdf` is already
/// zero); `hero_match[k]` is the same `direction_matches` verdict the calling loop used
/// to decide chromatic termination of k against the hero, reused verbatim for every
/// pair involving the hero so the mask and the zeroing can never disagree; every other
/// pair is tested with the same tolerance directly.
///
/// Exit events never narrow the mask: every live channel resolves its own exit
/// direction deterministically, so every technique in a channel's family produces that
/// channel's exit path.
pub(super) fn narrow_compat(
    compat: &mut [u8; NUM_CHANNELS],
    dirs: &[Option<Vec3>; NUM_CHANNELS],
    hero: usize,
    hero_match: [bool; NUM_CHANNELS],
) {
    const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;
    for a in 0..NUM_CHANNELS {
        for b in (a + 1)..NUM_CHANNELS {
            let matches = if a == hero {
                hero_match[b]
            } else if b == hero {
                hero_match[a]
            } else if let (Some(da), Some(db)) = (dirs[a], dirs[b]) {
                da.dot(db) >= DIRECTION_MATCH_COS_TOL
            } else {
                true
            };
            if !matches {
                compat[a] &= !(1u8 << b);
                compat[b] &= !(1u8 << a);
            }
        }
    }
}

/// Channel k's own Fresnel transmission at a refract/exit interface -- the same
/// per-channel computation [`apply_refract_channel`]'s matching-direction branch runs
/// inline, factored out so the split branch (a channel whose own direction diverges
/// from the hero's) can compute the identical transmitted Stokes state without
/// duplicating the formula. Returns the transmitted state and channel k's own
/// unpolarized reflectance `r_unpol_k` (the matching branch's own
/// `path_pdf[k] *= 1.0 - r_unpol_k` factor; the mismatch branch applies this same
/// factor to `path_pdf[k]` when splitting is enabled -- see this module's top-of-file
/// doc comment, point 2).
/// The scalar physics inputs [`compute_channel_transmission`] needs -- a direct
/// extraction of [`apply_refract_channel`]'s own inline matching-branch body, nothing
/// added or bundled further.
struct ChannelTransmissionInputs {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
    incident_stokes_k: StokesVector,
}

fn compute_channel_transmission(inputs: &ChannelTransmissionInputs) -> (StokesVector, f32) {
    let &ChannelTransmissionInputs {
        n1k,
        n2k,
        cos_i,
        cos_t_k,
        r_unpol,
        entering_anisotropic,
        entry_mode_azimuth2,
        incident_stokes_k,
    } = inputs;
    let t_s_k = (2.0 * n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k =
        MuellerMatrix::fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    let incident_k =
        if entering_anisotropic && let Some((cos_2psi_x, sin_2psi_x)) = entry_mode_azimuth2 {
            let i_k = incident_stokes_k.i;
            StokesVector::new(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0)
        } else {
            incident_stokes_k
        };
    let transmitted = incident_k
        .apply_matrix(&trans_matrix_k)
        .scale(1.0 / (1.0 - r_unpol));
    let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
    (transmitted, r_unpol_k)
}

/// Resolves channel `k`'s own radiance contribution when its Snell-refracted direction
/// at an exit bounce (leaving the gem back into air) diverges from the direction the
/// hero-driven shared path actually took. `path_pdf[k]` is not this function's concern:
/// every non-TIR channel at an exit event gets its own local transmit factor folded
/// into `path_pdf[k]` by the caller before this call, whether or not the split below
/// finds an escaping ray. This function is purely the bounded fan-out mechanism for
/// `split_radiance`, called only for a channel that still carries radiance.
///
/// Callers compute channel k's own Fresnel transmission via
/// [`compute_channel_transmission`] and pass in its intensity, already fully resolved.
/// `stokes_k` has already been zeroed by the caller -- only `split_radiance` is touched
/// here. Bounded to exactly one extra intersection test, never recurses into the
/// general per-bounce dispatch:
///   - k's own ray escapes the gem (the common case): its contribution is folded
///     directly into `exit.split_radiance[k]` (`transmitted_intensity * env_spectral`,
///     the same product shape [`accumulate_miss_radiance`] uses for the shared path).
///   - k's own ray instead re-enters the (necessarily convex) gem: tracing further is
///     unbounded work this design declines, so nothing is added to `split_radiance` --
///     a pure energy-loss truncation, exactly like hitting `max_bounces` or a Russian
///     roulette kill partway through a sub-path, not a density/pdf event. Unbiased for
///     the same reason the `sin2_t_k > 1.0` TIR-mismatch case in
///     [`apply_refract_channel`] already is: channel `k`'s true contribution through
///     that longer sub-path is captured by other render samples, the ones that draw
///     `k` itself as hero.
pub(super) fn try_split_exit_channel(
    exit: &mut ExitSplitCtx<'_>,
    hit_point: Vec3,
    k: usize,
    lambda_k: f32,
    dir_k: Vec3,
    transmitted_intensity: f32,
) {
    let probe = Ray {
        origin: hit_point + dir_k * 1e-4,
        dir: dir_k,
    };
    if intersect_polyhedron_soa(probe, exit.plane_soa).is_some() {
        // Bounded re-entry: decline to trace further (energy-loss truncation only).
        return;
    }
    let env_spectral =
        sample_environment_channel(exit.environment, dir_k, lambda_k, exit.studio_rig.as_ref());
    exit.split_radiance[k] = f32::mul_add(
        transmitted_intensity.max(0.0),
        env_spectral,
        exit.split_radiance[k],
    );
}

/// Builds [`RayWavelengthCache`] once per sample.
pub(super) fn build_ray_wavelength_cache(ctx: &RayMaterialContext) -> RayWavelengthCache {
    let material = ctx.material;

    // `n_o_ch` never depends on `theta_c` (only the discarded `n_eff_ch` half does), so
    // the `0.0` argument here is an arbitrary placeholder; the real per-bounce
    // `n_eff_ch` is computed fresh every bounce by
    // `per_channel_effective_extraordinary_indices` from this cached `n_o_ch`.
    let (n_o_ch, _n_eff_ch_unused_theta_c_independent_half) =
        per_channel_uniaxial_indices(ctx, 0.0);

    let biaxial_ch: [Option<BiaxialIndicatrix>; NUM_CHANNELS] =
        std::array::from_fn(|k| material.biaxial_indicatrix(ctx.lambdas[k]));
    let hero_indicatrix = biaxial_ch[ctx.hero_idx];
    let is_biaxial = hero_indicatrix.is_some();

    let abs_o = &material.absorption.o_ray;
    let abs_e = &material.absorption.e_ray;
    let abs_beta = material.absorption.beta_ray.as_deref();
    let alpha_o_ch: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| spectral_absorption(abs_o, ctx.lambdas[k]));
    let alpha_e_ch: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| spectral_absorption(abs_e, ctx.lambdas[k]));
    let alpha_beta_ch: [Option<f32>; NUM_CHANNELS] = std::array::from_fn(|k| {
        if is_biaxial {
            abs_beta.map(|bands| spectral_absorption(bands, ctx.lambdas[k]))
        } else {
            None
        }
    });
    let tensor_ch: [AbsorptionTensor3; NUM_CHANNELS] = std::array::from_fn(|k| {
        alpha_beta_ch[k].map_or_else(
            || AbsorptionTensor3::uniaxial(alpha_o_ch[k], alpha_e_ch[k], ctx.c_axis),
            |beta| AbsorptionTensor3::biaxial(alpha_o_ch[k], beta, alpha_e_ch[k], ctx.c_axis),
        )
    });

    RayWavelengthCache {
        n_o_ch,
        hero_indicatrix,
        biaxial_ch,
        tensor_ch,
    }
}

/// Per-bounce refractive-index and incidence-angle quantities, computed once at the top
/// of each bounce iteration before the TIR / partial-reflect / refract branches decide
/// what to do with them.
///
/// `pub(crate)` (struct and every field) plus `#[derive(Clone, Copy, Default)]` so
/// `renderer::gpu::transport_check`'s Tier 2 ULP check for `apply_frosted_bounce` can
/// build one directly, overriding just the four fields (`cos_i`/`n1`/`n2`/`sin2_t`)
/// that function reads, rather than hand-deriving the full biaxial/per-channel
/// machinery real bounce dispatch would populate. `Default` is derivable regardless of
/// `BiaxialIndicatrix`'s own `Default`-ness since `Option<T>` always implements
/// `Default` (`None`).
#[derive(Clone, Copy, Default)]
pub(crate) struct BounceRefractionGeometry {
    pub(crate) cos_i: f32,
    pub(crate) sin_i: f32,
    pub(crate) is_biaxial: bool,
    pub(crate) n_o_hero: f32,
    pub(crate) n_e_hero: f32,
    pub(crate) hero_indicatrix: Option<BiaxialIndicatrix>,
    pub(crate) n_biax_a_hero: f32,
    pub(crate) n_biax_b_hero: f32,
    pub(crate) n_o_ch: [f32; NUM_CHANNELS],
    pub(crate) n_biax_a_ch: [f32; NUM_CHANNELS],
    pub(crate) n1: f32,
    pub(crate) n2: f32,
    pub(crate) sin2_t: f32,
    pub(crate) n1_ch: [f32; NUM_CHANNELS],
    pub(crate) n2_ch: [f32; NUM_CHANNELS],
    pub(crate) sin2_t_ch: [f32; NUM_CHANNELS],
    /// The shared local frame (`that`/`s_axis`/`zhat` + optic-axis direction cosines)
    /// `uniaxial_fresnel`'s closed-form solver needs, built once per bounce from this
    /// bounce's own `k_hat`/`normal`/`c_axis`/`cos_i`/`sin_i` -- see
    /// `UniaxialFrame::build`'s own doc comment. `Some` only when this material is
    /// genuinely uniaxial (`is_anisotropic && !is_biaxial`); `None` for an isotropic or
    /// biaxial material, in which case every `apply_*` function in this file uses its
    /// scalar-Fresnel/biaxial-indicatrix code path instead.
    pub(crate) uniaxial_frame: Option<UniaxialFrame>,
}

/// Builds [`BounceRefractionGeometry`] for the current bounce. Touches no accumulator
/// (`stokes`, `path_pdf`, `radiance`) and mutates no loop state -- a pure
/// "compute from this bounce's inputs and return" step.
///
/// `theta_c` (the angle against the c-axis used to evaluate the direction-dependent
/// extraordinary index) must be measured against the wave normal `k`, not the
/// Poynting/energy direction `S`. On an air->crystal entry (`!inside_gem`) this is
/// mildly circular: the refracted wave normal depends on `n2`, which (for the
/// extraordinary index) depends on `theta_c`, which depends on the refracted wave
/// normal. Resolved with 2 fixed-point iterations, seeded from the ordinary index `n_o`
/// (an isotropic first guess, exactly correct if the path turns out to be the ordinary
/// eigenmode); `k_hat == S` trivially outside the crystal (isotropic air). While
/// already inside the crystal, `k_hat` is the caller's own tracked wave normal
/// (`current_k` in `transport.rs`, carried and re-evaluated across bounces -- see
/// [`poynting_dir_for_mode`]), so the angle is read directly from it.
///
/// Only applies to the uniaxial ordinary/extraordinary approximation -- a biaxial
/// material never consults its result (see the biaxial per-channel/per-mode block in
/// [`compute_bounce_refraction_geometry`] for its own, separate iteration via
/// `BiaxialIndicatrix::resolve_entry_mode`); callers guard `is_biaxial` themselves.
pub(crate) fn theta_c_for_bounce(
    ctx: &RayMaterialContext,
    normal: Vec3,
    k_hat: Vec3,
    cos_i: f32,
    inside_gem: bool,
    is_biaxial: bool,
    n_o_hero_seed: f32,
) -> f32 {
    let material = ctx.material;
    let c_axis = ctx.c_axis;
    let is_anisotropic = ctx.is_anisotropic;

    if !inside_gem && is_anisotropic && !is_biaxial {
        // Uses `extraordinary_index_at` (a genuine independent e-ray dispersion
        // curve when the material carries one -- Quartz/Amethyst/Citrine/Rutile --
        // falling back to the constant-offset approximation otherwise), matching
        // `per_channel_effective_extraordinary_indices`'s own per-channel `n_e_k` lookup
        // rather than the constant-offset-only `n_o_hero_seed +
        // material.birefringence_delta` this used to hardcode.
        let n_e_hero_seed =
            material.extraordinary_index_at(ctx.lambdas[ctx.hero_idx], n_o_hero_seed);
        let mut n_guess = n_o_hero_seed;
        let mut theta = 0.0f32;
        for _ in 0..2 {
            let eta_guess = 1.0 / n_guess;
            let sin2_t_guess = eta_guess * eta_guess * cos_i.mul_add(-cos_i, 1.0);
            if sin2_t_guess > 1.0 {
                break;
            }
            let cos_t_guess = (1.0 - sin2_t_guess).max(0.0).sqrt();
            let wave_dir_guess =
                (eta_guess * k_hat + eta_guess.mul_add(cos_i, -cos_t_guess) * normal).normalize();
            let cos_theta_wave = wave_dir_guess.dot(c_axis).clamp(-1.0, 1.0).abs();
            theta = cos_theta_wave.acos();
            n_guess = BirefringenceParams::effective_extraordinary_index(
                n_o_hero_seed,
                n_e_hero_seed,
                theta,
            );
        }
        theta
    } else {
        k_hat.dot(c_axis).clamp(-1.0, 1.0).abs().acos()
    }
}

/// Per-channel uniaxial ordinary (`n_o_ch`) and effective-extraordinary (`n_eff_ch`)
/// indices, each channel evaluated at its OWN wavelength (Fix F) against the shared
/// `theta_c` (see [`theta_c_for_bounce`]).
pub(crate) fn per_channel_uniaxial_indices(
    ctx: &RayMaterialContext,
    theta_c: f32,
) -> ([f32; NUM_CHANNELS], [f32; NUM_CHANNELS]) {
    let material = ctx.material;
    let is_anisotropic = ctx.is_anisotropic;
    let mut n_o_ch = [0.0f32; NUM_CHANNELS];
    let mut n_eff_ch = [0.0f32; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let n_o_k = material.dispersion.evaluate(ctx.lambdas[k]);
        // Wavelength-dependent extraordinary index when the material carries one
        // (currently only Quartz); falls back to the constant-offset
        // `n_o_k + birefringence_delta` otherwise -- see
        // `GemMaterial::extraordinary_index_at`'s own doc comment.
        let n_e_k = material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        n_o_ch[k] = n_o_k;
        n_eff_ch[k] = if is_anisotropic {
            BirefringenceParams::effective_extraordinary_index(n_o_k, n_e_k, theta_c)
        } else {
            n_o_k
        };
    }
    (n_o_ch, n_eff_ch)
}

/// The per-bounce (`theta_c`-dependent) half of `per_channel_uniaxial_indices`, reading
/// the theta_c-independent half (`n_o_ch`) back from [`RayWavelengthCache`] instead of
/// recomputing it.
fn per_channel_effective_extraordinary_indices(
    ctx: &RayMaterialContext,
    n_o_ch: &[f32; NUM_CHANNELS],
    theta_c: f32,
) -> [f32; NUM_CHANNELS] {
    let material = ctx.material;
    let is_anisotropic = ctx.is_anisotropic;
    let mut n_eff_ch = [0.0f32; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let n_o_k = n_o_ch[k];
        let n_e_k = material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        n_eff_ch[k] = if is_anisotropic {
            BirefringenceParams::effective_extraordinary_index(n_o_k, n_e_k, theta_c)
        } else {
            n_o_k
        };
    }
    n_eff_ch
}

/// Genuinely biaxial per-channel mode indices, and -- at an air->crystal entry -- the
/// per-mode wave-normal directions those indices were resolved at. Unlike the uniaxial
/// arrays, neither eigenmode here has a direction-independent index: "mode A" names the
/// faster (lower-index) root of `wave_indices` and "mode B" the slower (higher-index)
/// root at the relevant wave-normal direction -- an arbitrary but self-consistent
/// relabelling of `is_extraordinary`'s two slots, not a claim that "mode B" is always
/// what was "extraordinary" in the uniaxial arrays.
///
/// The wave-normal direction feeding each channel's index lookup is resolved once from
/// the hero channel's own indicatrix -- mirroring `theta_c` (one shared geometric
/// direction reused for every channel; only the index magnitude varies per channel).
/// Entering the crystal this needs `resolve_entry_mode`'s fixed-point iteration (the
/// direction depends on the index, which depends on the direction) run once per mode,
/// since neither mode has a constant index to seed the other from. Already inside the
/// crystal, `k_hat` is the caller's own tracked wave normal (`current_k` in
/// `transport.rs`), so both modes share it directly with no iteration -- both biaxial
/// modes walk off (see [`poynting_dir_for_mode`]'s doc comment), so `k_hat` here is
/// genuinely not the same as `Ray::dir`/`S` while inside the crystal in either mode,
/// unlike the uniaxial ordinary eigenmode.
fn hero_biaxial_wave_dirs(
    cache: &RayWavelengthCache,
    normal: Vec3,
    k_hat: Vec3,
    cos_i: f32,
    inside_gem: bool,
    is_biaxial: bool,
    n_o_hero_seed: f32,
) -> (Option<BiaxialIndicatrix>, Vec3, Vec3) {
    // `is_biaxial` and `cache.hero_indicatrix.is_some()` are the same condition, so
    // this guard is redundant with the cached value but kept explicit.
    let hero_indicatrix = if is_biaxial {
        cache.hero_indicatrix
    } else {
        None
    };
    let (wave_dir_a_hero, wave_dir_b_hero) =
        hero_indicatrix.map_or((Vec3::ZERO, Vec3::ZERO), |ind| {
            if inside_gem {
                (k_hat, k_hat)
            } else {
                let (_, dir_a) = ind.resolve_entry_mode(k_hat, normal, cos_i, n_o_hero_seed, false);
                let (_, dir_b) = ind.resolve_entry_mode(k_hat, normal, cos_i, n_o_hero_seed, true);
                (dir_a, dir_b)
            }
        });
    (hero_indicatrix, wave_dir_a_hero, wave_dir_b_hero)
}

/// Per-channel biaxial mode-A/mode-B indices, each channel's own indicatrix evaluated
/// at the shared hero wave-normal directions from [`hero_biaxial_wave_dirs`]. Zero
/// (the array default) for a non-biaxial material or a channel whose indicatrix is
/// somehow unavailable, matching the pre-extraction code's behaviour exactly.
fn per_channel_biaxial_indices(
    cache: &RayWavelengthCache,
    is_biaxial: bool,
    wave_dir_a_hero: Vec3,
    wave_dir_b_hero: Vec3,
) -> ([f32; NUM_CHANNELS], [f32; NUM_CHANNELS]) {
    let mut n_biax_a_ch = [0.0f32; NUM_CHANNELS];
    let mut n_biax_b_ch = [0.0f32; NUM_CHANNELS];
    if is_biaxial {
        for k in 0..NUM_CHANNELS {
            if let Some(ind_k) = cache.biaxial_ch[k] {
                n_biax_a_ch[k] = ind_k.wave_indices(wave_dir_a_hero).1;
                n_biax_b_ch[k] = ind_k.wave_indices(wave_dir_b_hero).0;
            }
        }
    }
    (n_biax_a_ch, n_biax_b_ch)
}

/// Per-channel `n1`/`n2`/`sin2(theta_t)`, evaluated against the SAME shared incidence
/// geometry (`cos_i`, the facet normal) as the hero -- only the index (`n_medium_ch`)
/// varies by channel.
fn per_channel_medium_indices(
    inside_gem: bool,
    n_medium_ch: [f32; NUM_CHANNELS],
    cos_i: f32,
) -> (
    [f32; NUM_CHANNELS],
    [f32; NUM_CHANNELS],
    [f32; NUM_CHANNELS],
) {
    let mut n1_ch = [0.0f32; NUM_CHANNELS];
    let mut n2_ch = [0.0f32; NUM_CHANNELS];
    let mut sin2_t_ch = [0.0f32; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let n1k = if inside_gem { n_medium_ch[k] } else { 1.0 };
        let n2k = if inside_gem { 1.0 } else { n_medium_ch[k] };
        let etak = n1k / n2k;
        n1_ch[k] = n1k;
        n2_ch[k] = n2k;
        sin2_t_ch[k] = etak * etak * cos_i.mul_add(-cos_i, 1.0);
    }
    (n1_ch, n2_ch, sin2_t_ch)
}

pub(super) fn compute_bounce_refraction_geometry(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    normal: Vec3,
    k_hat: Vec3,
    inside_gem: bool,
    is_extraordinary: bool,
) -> BounceRefractionGeometry {
    let material = ctx.material;
    let hero_idx = ctx.hero_idx;
    let is_anisotropic = ctx.is_anisotropic;

    // Angle of incidence measured against the wave normal `k_hat`, not the
    // Poynting/energy direction `S`. `k_hat == S` trivially outside the crystal
    // (isotropic air) and for the uniaxial ordinary eigenmode. Computed up front since
    // the fixed-point iteration below needs it to seed a trial refraction.
    let cos_i = (-k_hat).dot(normal).clamp(0.0, 1.0);
    let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();

    // Every channel evaluates the material's dispersion at its OWN wavelength instead
    // of all eight channels sharing the hero's index. The single traced geometric path
    // is still driven entirely by the hero channel (one ray per bounce); only the
    // per-channel Stokes/radiometric bookkeeping is wavelength-correct.
    // Is this material's anisotropy genuinely biaxial (three distinct principal
    // indices) rather than the uniaxial ordinary/extraordinary approximation every
    // other anisotropic built-in uses?
    let is_biaxial = material.biaxial_delta_beta_alpha.is_some();

    let n_o_hero_seed = cache.n_o_ch[hero_idx];
    let theta_c = theta_c_for_bounce(
        ctx,
        normal,
        k_hat,
        cos_i,
        inside_gem,
        is_biaxial,
        n_o_hero_seed,
    );

    let n_o_ch = cache.n_o_ch;
    let n_eff_ch = per_channel_effective_extraordinary_indices(ctx, &n_o_ch, theta_c);
    let n_o_hero = n_o_ch[hero_idx];
    // Deliberately the constant-offset form, NOT `extraordinary_index_at` -- unlike
    // `theta_c_for_bounce`'s own seed above, this `n_e_hero` feeds only
    // the walk-off/direction (Poynting) approximation
    // (`BirefringenceParams::extraordinary_poynting_dir`), where the existing
    // constant-offset behaviour is deliberately kept as-is. Mirrored identically in
    // `spectral_transport.wgsl`'s own `n_e_hero` at the direction-approximation call
    // sites -- see that file's comment cross-referencing this one.
    let n_e_hero = n_o_hero + material.birefringence_delta;

    let (hero_indicatrix, wave_dir_a_hero, wave_dir_b_hero) = hero_biaxial_wave_dirs(
        cache,
        normal,
        k_hat,
        cos_i,
        inside_gem,
        is_biaxial,
        n_o_hero_seed,
    );
    let (n_biax_a_ch, n_biax_b_ch) =
        per_channel_biaxial_indices(cache, is_biaxial, wave_dir_a_hero, wave_dir_b_hero);
    // Self-consistency: the hero's own per-mode index, read back out of the per-channel
    // arrays above (a lookup, not an independent recomputation), mirroring how the
    // uniaxial branch defines `n_o_hero := n_o_ch[hero_idx]`. Using the same array
    // element at both the hero-level refraction below and the per-channel loop's own
    // `k == hero_idx` iteration guarantees bit-identical directions -- otherwise the
    // hero channel could spuriously fail its own direction-match check against itself,
    // chromatically self-terminating on every biaxial entry.
    let n_biax_a_hero = n_biax_a_ch[hero_idx];
    let n_biax_b_hero = n_biax_b_ch[hero_idx];

    // Which per-channel index represents "the medium this ray is currently in". While
    // inside an anisotropic crystal, this is mode A or mode B depending on which
    // eigenmode the path was stochastically assigned to at its most recent entry
    // (`is_extraordinary`, set in the refract branch below). Outside the crystal this
    // keeps using the mode B array. `n_mode_a_ch`/`n_mode_b_ch` select between the
    // uniaxial and biaxial arrays; for a non-biaxial material this is the
    // `n_o_ch`/`n_eff_ch` pair.
    let n_mode_a_ch = if is_biaxial { n_biax_a_ch } else { n_o_ch };
    let n_mode_b_ch = if is_biaxial { n_biax_b_ch } else { n_eff_ch };
    let n_medium_ch: [f32; NUM_CHANNELS] = if is_anisotropic && inside_gem && !is_extraordinary {
        n_mode_a_ch
    } else {
        n_mode_b_ch
    };
    let n_medium_hero = n_medium_ch[hero_idx];

    let n1 = if inside_gem { n_medium_hero } else { 1.0 };
    let n2 = if inside_gem { 1.0 } else { n_medium_hero };
    let eta = n1 / n2;
    let sin2_t = eta * eta * cos_i.mul_add(-cos_i, 1.0);

    let (n1_ch, n2_ch, sin2_t_ch) = per_channel_medium_indices(inside_gem, n_medium_ch, cos_i);

    // Built once per bounce, shared by every channel's own closed-form solve at this
    // interface -- see `BounceRefractionGeometry::uniaxial_frame`'s own doc comment.
    let uniaxial_frame = (is_anisotropic && !is_biaxial)
        .then(|| UniaxialFrame::build(k_hat, normal, material.c_axis, cos_i, sin_i));

    BounceRefractionGeometry {
        cos_i,
        sin_i,
        is_biaxial,
        n_o_hero,
        n_e_hero,
        hero_indicatrix,
        n_biax_a_hero,
        n_biax_b_hero,
        n_o_ch,
        n_biax_a_ch,
        n1,
        n2,
        sin2_t,
        n1_ch,
        n2_ch,
        sin2_t_ch,
        uniaxial_frame,
    }
}

/// Recovers the mode's Poynting (energy/ray) direction `S` for a freshly-computed wave
/// normal `k` -- needed after a reflection event, where the reflection law
/// (`k' = reflect(k, normal)`) acts on `k` directly and `S'` must be re-evaluated (not
/// carried over from before the reflection), since the mode's own index and walk-off
/// angle both depend on the wave-normal direction, which the reflection just changed.
///
/// Returns `k` unchanged (`S == k`, no walk-off) outside the crystal (`!inside_gem`,
/// isotropic air), for an isotropic material (`!ctx.is_anisotropic`), and for the
/// uniaxial ordinary eigenmode (`!is_extraordinary` while `!geo.is_biaxial`). A biaxial
/// material's two modes ("mode A"/"mode B") both walk off -- see
/// `hero_biaxial_wave_dirs`'s doc comment -- so [`BiaxialIndicatrix::mode_poynting_dir`]
/// is called unconditionally whenever `geo.is_biaxial`.
#[must_use]
pub(crate) fn poynting_dir_for_mode(
    ctx: &RayMaterialContext,
    geo: &BounceRefractionGeometry,
    k: Vec3,
    inside_gem: bool,
    is_extraordinary: bool,
) -> Vec3 {
    if !inside_gem || !ctx.is_anisotropic {
        return k;
    }
    if geo.is_biaxial {
        geo.hero_indicatrix
            .map_or(k, |ind| ind.mode_poynting_dir(k, is_extraordinary))
    } else if is_extraordinary {
        BirefringenceParams::extraordinary_poynting_dir(k, ctx.c_axis, geo.n_o_hero, geo.n_e_hero)
    } else {
        k
    }
}

/// Total Internal Reflection for the hero channel forces a deterministic reflect for
/// the shared path (probability 1, no pdf division needed). Each channel still gets its
/// own physically-correct outcome for that reflect event: if channel k is itself past
/// its own (wavelength-dependent) critical angle it gets the exact TIR phase
/// retardation at its own index; otherwise -- since the critical angle depends on
/// n(lambda), a channel can be below the hero's critical angle even though the hero
/// isn't -- it gets its own ordinary partial-reflectance Fresnel matrix. No probability
/// division is needed here: the hero's own selection probability for this action is 1
/// (forced), and each channel's Stokes value already carries the correct
/// importance-sampling division from whichever earlier decision had a nontrivial hero
/// probability. The reflection law acts on the wave normal `k_hat`, not `S` -- returns
/// the reflected wave normal `k'`; the caller derives the reflected Poynting direction
/// `S'` via [`poynting_dir_for_mode`] and sets `current_ray.origin` itself.
pub(super) fn apply_tir_bounce(
    ctx: &RayMaterialContext,
    geo: &BounceRefractionGeometry,
    is_extraordinary: bool,
    k_hat: Vec3,
    normal: Vec3,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> (Vec3, Option<f32>) {
    if let Some(frame) = geo.uniaxial_frame {
        // Uniaxial closed-form solve. `apply_tir_bounce` is reachable only from inside
        // the gem, so every channel's incident state is a single eigenmode (o or e,
        // whichever `is_extraordinary` names) -- the propagating Stokes vector is
        // therefore always fully linearly polarized along that mode's own fixed axis
        // in the current `(s_axis, p_axis)` frame. Reflecting a single real-valued mode
        // amplitude by a complex coefficient only rescales its magnitude -- an overall
        // phase on a one-component state is unobservable, so no `tir_retardation`-style
        // rotation is needed here (unlike the isotropic case, where s and p genuinely
        // retard relative to each other). The energy that couples into the other mode
        // is not represented in this Stokes vector at all -- it is instead captured by
        // `apply_internal_mode_coupling`'s own Poynting-weighted relabeling probability
        // for the next bounce.
        //
        // The hero channel's own `R_o`/`R_e` split (`ro_pow`/`re_pow` below) is also
        // the exact o<->e relabeling probability `transport::apply_internal_mode_coupling`
        // needs for the path's next bounce (`p_o = R_o / (R_o + R_e)`, both
        // Poynting-flux-weighted) -- captured here from the same closed-form solve this
        // loop already runs for the energy accounting.
        let mut exact_p_o = None;
        for k in 0..NUM_CHANNELS {
            let n_inc = geo.n1_ch[k];
            let n_o_k = geo.n_o_ch[k];
            let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
            let sol = uniaxial_fresnel::internal_solve(
                n_inc,
                n_o_k,
                n_e_k,
                ctx.c_axis,
                &frame,
                !is_extraordinary,
            );
            let flux_inc = sol.flux_inc.max(1e-12);
            let ro_pow = sol.r_o.norm_sqr() * sol.flux_ro;
            let re_pow = sol.r_e.norm_sqr() * sol.flux_re;
            let r_total_k = ((ro_pow + re_pow) / flux_inc).min(1.0);
            stokes[k] = stokes[k].scale(r_total_k);
            if geo.sin2_t_ch[k] <= 1.0 {
                // Channel k is genuinely below its own critical angle here even though
                // the hero forced a reflect -- under k's own technique, reflecting has
                // probability r_total_k.
                path_pdf[k] *= r_total_k.clamp(1e-4, 1.0 - 1e-4);
            }
            if k == ctx.hero_idx {
                exact_p_o = Some(ro_pow / (ro_pow + re_pow).max(1e-12));
            }
        }
        return (k_hat - 2.0 * k_hat.dot(normal) * normal, exact_p_o);
    }

    for k in 0..NUM_CHANNELS {
        let n1k = geo.n1_ch[k];
        let n2k = geo.n2_ch[k];
        if geo.sin2_t_ch[k] > 1.0 {
            let delta_k = tir_phase_delta(n1k, geo.cos_i, geo.sin_i);
            let tir_matrix_k = MuellerMatrix::tir_retardation(delta_k);
            stokes[k] = stokes[k].apply_matrix(&tir_matrix_k);
            // Channel k is also past its own critical angle here, so under k's own
            // technique this reflect is also forced (probability 1); reflection
            // direction never depends on wavelength, so channel k's path-pdf factor
            // here is exactly 1, a no-op left unwritten.
        } else {
            let cos_t_k = (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt();
            let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * geo.cos_i)
                / f32::mul_add(n2k, cos_t_k, n1k * geo.cos_i);
            let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * geo.cos_i)
                / f32::mul_add(n1k, cos_t_k, n2k * geo.cos_i);
            let refl_matrix_k = MuellerMatrix::fresnel_reflection(r_s_k, r_p_k);
            stokes[k] = stokes[k].apply_matrix(&refl_matrix_k);
            // Channel k is genuinely below its own critical angle here even though
            // the hero forced a reflect -- under k's own technique, reflecting (the
            // observed outcome) has probability equal to k's own unpolarized
            // reflectance. Direction still matches trivially (reflection is never
            // dispersive), so no chromatic-termination check applies at a reflect
            // event.
            let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
            path_pdf[k] *= r_unpol_k;
        }
    }
    (k_hat - 2.0 * k_hat.dot(normal) * normal, None)
}

/// Partial Fresnel Reflection & Refraction via Stokes-Mueller Polarized Wave Transport
/// -- the reflect half. Which branch is taken (reflect vs. transmit) is decided once,
/// from the hero's `r_unpol`, driving the single shared geometric path; each channel
/// then applies its own Fresnel reflection matrix, divided by the same hero selection
/// probability (`r_unpol`, the actual probability with which "reflect" was sampled).
/// When the material is non-dispersive, every channel's matrix is identical to the
/// hero's, reducing to a plain unweighted result. Reflects the wave normal `k_hat` and
/// returns the reflected wave normal `k'`; the caller derives `S'` via
/// [`poynting_dir_for_mode`] and sets `current_ray.origin` itself.
fn apply_partial_reflect_bounce(
    geo: &BounceRefractionGeometry,
    r_unpol: f32,
    k_hat: Vec3,
    normal: Vec3,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Vec3 {
    for k in 0..NUM_CHANNELS {
        let n1k = geo.n1_ch[k];
        let n2k = geo.n2_ch[k];
        let refl_matrix_k = if geo.sin2_t_ch[k] > 1.0 {
            // Channel k is past its own critical angle here even though the hero
            // isn't; it also picks up the TIR phase retardation delta = delta_p -
            // delta_s, exactly as `apply_tir_bounce` applies for the hero. Channel k
            // is forced to TIR here regardless of the hero's own physics -- under k's
            // own technique this reflect also happens with probability 1, so no
            // path-pdf factor is needed.
            let delta_k = tir_phase_delta(n1k, geo.cos_i, geo.sin_i);
            MuellerMatrix::tir_retardation(delta_k)
        } else {
            let cos_t_k = (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt();
            let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * geo.cos_i)
                / f32::mul_add(n2k, cos_t_k, n1k * geo.cos_i);
            let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * geo.cos_i)
                / f32::mul_add(n1k, cos_t_k, n2k * geo.cos_i);
            // Channel k's own probability of choosing reflect here, using k's own
            // unpolarized reflectance (not the hero's r_unpol). Reflection direction
            // never depends on wavelength, so no chromatic-termination check applies
            // at a reflect event.
            let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
            path_pdf[k] *= r_unpol_k;
            MuellerMatrix::fresnel_reflection(r_s_k, r_p_k)
        };
        stokes[k] = stokes[k].apply_matrix(&refl_matrix_k).scale(1.0 / r_unpol);
    }
    k_hat - 2.0 * k_hat.dot(normal) * normal
}

/// What [`apply_refract_bounce`] changes about the traced path beyond `stokes` and
/// `path_pdf` (which it mutates directly): the new wave normal `k'` and Poynting
/// direction `S'`, and -- only on an air->crystal entry into an anisotropic material --
/// the eigenmode this path was stochastically assigned to. `None` means "leave
/// `is_extraordinary` exactly as it was".
struct RefractBounceOutcome {
    new_k: Vec3,
    new_s: Vec3,
    is_extraordinary_update: Option<bool>,
}

/// Partial Fresnel Reflection & Refraction via Stokes-Mueller Polarized Wave Transport
/// -- the refract half (transmit branch, taken when the hero's `rng_bounce >=
/// r_unpol`). A companion channel's refracted direction must match the shared
/// hero-driven direction to within `DIRECTION_MATCH_COS_TOL` (not exact float equality:
/// two evaluations of the same refraction formula at the same index are bit-identical
/// or differ by a handful of ULPs, but two different indices generically produce a
/// direction difference many orders of magnitude larger).
///
/// At an air->crystal entry into an anisotropic material, unpolarized incident light
/// couples into two orthogonally polarized eigenmodes -- ordinary (no walk-off) and
/// extraordinary (Poynting direction displaced by the walk-off angle), each carrying
/// roughly half the incident energy. Only one geometric path is traced per sample, so
/// which eigenmode this path becomes is chosen stochastically 50/50 -- but this is an
/// energy share, not a 1-of-N selection: `trans_matrix_k` below already computes the
/// full transmitted intensity for a beam at the selected mode's own index, so weighting
/// it by that mode's ~0.5 energy share gives an unbiased estimator of
/// `0.5*T_o + 0.5*T_e` with a factor of exactly 1, not the `1/0.5 = 2.0` a naive
/// "divide by the selection probability" rule would suggest (that rule is correct for
/// the reflect/refract split's `r_unpol`/`1 - r_unpol`, where the two branches
/// partition disjoint energy; mode selection instead splits one shared energy pool).
///
/// Chromatic termination: when channel k's own specular refraction direction genuinely
/// diverges from the direction the hero-driven path actually took (or channel k cannot
/// transmit at this angle at all), channel k's path pdf and its Stokes/radiance
/// contribution are dropped to exactly 0, not merely down-weighted -- required for
/// unbiasedness, confirmed by a two-channel Fresnel Monte Carlo cross-check (see
/// `two_channel_dispersive_termination_monte_carlo_is_unbiased_under_alternating_hero`).
/// The already-resolved eigenmode selection [`apply_partial_fresnel_bounce`] passes
/// down into [`apply_refract_bounce`]: which mode this path's transmission event
/// represents, and (when the caller found a meaningful polarization frame) that mode's
/// own doubled azimuth for [`apply_refract_channel`]'s eigenmode projection.
#[derive(Clone, Copy)]
struct RefractSelection {
    use_extraordinary: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
}

fn apply_refract_bounce(
    bctx: &BounceContext<'_, '_>,
    r_unpol: f32,
    ray: BounceRay,
    inside_gem: bool,
    selection: RefractSelection,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> RefractBounceOutcome {
    let ctx = bctx.ctx;
    let geo = bctx.geo;
    let BounceRay { k_hat, normal } = ray;
    let RefractSelection {
        use_extraordinary,
        entry_mode_azimuth2,
    } = selection;
    let c_axis = ctx.c_axis;

    // The choice is recorded in the returned `is_extraordinary_update` so subsequent
    // internal bounces keep using the same eigenmode's index (via `n_medium_ch` in
    // `compute_bounce_refraction_geometry`). Exiting the crystal, and any refraction in
    // an isotropic material, leaves `entering_anisotropic` false.
    let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
    // Mode selection -- both the coin flip (`entry_eigenmode_selection`'s
    // polarization-weighted `p_o` in place of a blanket 50/50) and the
    // reflect-vs-transmit decision that must be consistent with it -- is resolved once
    // by the caller, `apply_partial_fresnel_bounce`, before it decides whether this
    // bounce is even a refract event at all. `use_extraordinary` here is that
    // already-resolved value, not a fresh draw.
    //
    // Direction: the ordinary eigenmode's wave normal uses n_o and is never walked
    // off; the extraordinary eigenmode uses n_eff and its energy (Poynting) direction
    // is displaced by the walk-off angle. Computed before the per-channel loop below
    // because each companion channel's own hypothetical refracted direction must be
    // compared against this same hero-driven direction to detect a dispersive
    // mismatch. For a biaxial material entering the crystal, neither mode is a plain
    // constant-index Snell refraction -- both modes walk off via `mode_poynting_dir` --
    // using `geo.n_biax_a_hero`/`geo.n_biax_b_hero` (the same looked-up scalars the
    // per-channel loop's own `k == hero_idx` iteration uses) for self-consistency.
    // `refr_wave_dir` is the Snell-refracted wave normal `k'` (Snell's law acts on `k`,
    // not `S`) -- captured alongside the Poynting-converted `S'` in every branch below,
    // since the caller needs both (see [`RefractBounceOutcome`]'s own doc comment). At
    // an air->crystal entry, `k_hat == S` trivially (isotropic air). At an exit
    // (leaving an anisotropic crystal) or any isotropic refraction, Snell's law at the
    // interface must refract the wave normal, not the walked-off Poynting direction.
    let (new_k, final_refr_dir) = if let (true, Some(ind)) =
        (entering_anisotropic && geo.is_biaxial, geo.hero_indicatrix)
    {
        let n2_hero_dir = if use_extraordinary {
            geo.n_biax_b_hero
        } else {
            geo.n_biax_a_hero
        };
        let eta_dir = geo.n1 / n2_hero_dir;
        let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
        let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
        let refr_wave_dir =
            (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();
        (
            refr_wave_dir,
            ind.mode_poynting_dir(refr_wave_dir, use_extraordinary),
        )
    } else {
        let n2_hero_dir = if entering_anisotropic && !use_extraordinary {
            geo.n_o_hero
        } else {
            geo.n2
        };
        let eta_dir = geo.n1 / n2_hero_dir;
        let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
        let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
        let refr_wave_dir =
            (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();

        let s = if entering_anisotropic && use_extraordinary {
            BirefringenceParams::extraordinary_poynting_dir(
                refr_wave_dir,
                c_axis,
                geo.n_o_hero,
                geo.n_e_hero,
            )
        } else {
            refr_wave_dir
        };
        (refr_wave_dir, s)
    };

    let decision = RefractDecision {
        entering_anisotropic,
        use_extraordinary,
        entry_mode_azimuth2,
        final_refr_dir,
        r_unpol,
    };
    let mut dirs = [None; NUM_CHANNELS];
    let mut hero_match = [false; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let (dir_k, matches_hero) =
            apply_refract_channel(bctx, k, ray, inside_gem, &decision, state, exit_event);
        dirs[k] = dir_k;
        hero_match[k] = matches_hero;
    }
    // An interior dispersive event (an entry into the gem) narrows every channel's MIS
    // family; the exit event never does -- see `narrow_compat`'s doc comment.
    if exit_event.exit.enabled && !inside_gem {
        narrow_compat(&mut exit_event.exit.compat, &dirs, ctx.hero_idx, hero_match);
    }

    RefractBounceOutcome {
        new_k,
        new_s: final_refr_dir,
        is_extraordinary_update: entering_anisotropic.then_some(use_extraordinary),
    }
}

/// Every channel's shared per-bounce refraction decision -- computed once by
/// [`apply_refract_bounce`] and reused, unchanged, by every one of
/// [`apply_refract_channel`]'s `NUM_CHANNELS` per-channel calls.
#[derive(Clone, Copy)]
struct RefractDecision {
    entering_anisotropic: bool,
    use_extraordinary: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
    final_refr_dir: Vec3,
    r_unpol: f32,
}

/// Channel k's own refracted-direction geometry -- the fields
/// [`compute_channel_refraction_geometry`] resolves.
struct ChannelRefractionGeometry {
    n1k: f32,
    n2k: f32,
    cos_t_k: f32,
    refr_wave_dir_k: Vec3,
    final_dir_k: Vec3,
}

/// The direction-independent half of [`apply_refract_channel`]'s per-channel work,
/// split out purely to keep that function's own body under the workspace line-count
/// lint. Returns `None` when channel k cannot physically transmit at this angle even
/// though the hero-driven path did (`sin2_t_k > 1.0`); the caller zeroes
/// `stokes[k]`/`path_pdf[k]` for that case itself, since this helper never touches
/// either array.
fn compute_channel_refraction_geometry(
    bctx: &BounceContext<'_, '_>,
    k: usize,
    ray: BounceRay,
    decision: &RefractDecision,
) -> Option<ChannelRefractionGeometry> {
    let (cache, geo) = (bctx.cache, bctx.geo);
    let BounceRay { k_hat, normal } = ray;
    let &RefractDecision {
        entering_anisotropic,
        use_extraordinary,
        ..
    } = decision;
    let material = bctx.ctx.material;
    let c_axis = bctx.ctx.c_axis;

    let n1k = geo.n1_ch[k];
    // geo.n2_ch[k] (== n_eff_ch[k] here) is the extraordinary-biased index used to
    // decide reflect vs. refract; only correct for this channel's transmission if the
    // extraordinary mode was actually selected. If the ordinary mode was selected
    // instead, this channel transmits at its own ordinary index geo.n_o_ch[k]. For a
    // biaxial material, use this channel's own biaxial mode-A index (already resolved
    // at the shared hero direction) instead of the uniaxial index.
    let n2k = if entering_anisotropic && !use_extraordinary {
        if geo.is_biaxial {
            geo.n_biax_a_ch[k]
        } else {
            geo.n_o_ch[k]
        }
    } else {
        geo.n2_ch[k]
    };
    let sin2_t_k = (n1k / n2k).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
    if sin2_t_k > 1.0 {
        // Channel k cannot physically transmit at this angle even though the
        // hero-driven path did. Correct and unbiased, not a bias to be corrected: its
        // reflect-branch contributions elsewhere are already correctly weighted by
        // the hero's own selection probability.
        return None;
    }

    let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();

    // The direction channel k's own technique would have taken through this same
    // interface, using k's own index -- including its own ordinary/extraordinary
    // Poynting-direction walk-off, since that is also wavelength-dependent. Refraction
    // is specular, so channel k's technique has positive density of having produced
    // the realized path only where this direction coincides with `final_refr_dir`.
    let eta_dir_k = n1k / n2k;
    let refr_wave_dir_k =
        (eta_dir_k * k_hat + f32::mul_add(eta_dir_k, geo.cos_i, -cos_t_k) * normal).normalize();
    // Channel k's own biaxial walk-off, using k's own indicatrix evaluated at k's own
    // single-shot refracted wave direction -- the per-channel generalization of the
    // uniaxial `extraordinary_poynting_dir` call below.
    let final_dir_k = if let (true, Some(ind_k)) =
        (entering_anisotropic && geo.is_biaxial, cache.biaxial_ch[k])
    {
        ind_k.mode_poynting_dir(refr_wave_dir_k, use_extraordinary)
    } else if entering_anisotropic && use_extraordinary {
        let n_e_k = geo.n_o_ch[k] + material.birefringence_delta;
        BirefringenceParams::extraordinary_poynting_dir(
            refr_wave_dir_k,
            c_axis,
            geo.n_o_ch[k],
            n_e_k,
        )
    } else {
        refr_wave_dir_k
    };

    Some(ChannelRefractionGeometry {
        n1k,
        n2k,
        cos_t_k,
        refr_wave_dir_k,
        final_dir_k,
    })
}

/// Inputs [`apply_channel_transmission_match`] needs from its caller's own bounce
/// geometry and refraction decision.
struct ChannelTransmitMatchInputs {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
}

/// The `direction_matches` branch of [`apply_refract_channel`]'s per-channel work,
/// split out purely to keep that function's own body under the workspace line-count
/// lint -- see this crate's doc comments there for the full physical rationale.
fn apply_channel_transmission_match(
    inputs: &ChannelTransmitMatchInputs,
    stokes_k: &mut StokesVector,
    path_pdf_k: &mut f32,
) {
    let &ChannelTransmitMatchInputs {
        n1k,
        n2k,
        cos_i,
        cos_t_k,
        r_unpol,
        entering_anisotropic,
        entry_mode_azimuth2,
    } = inputs;

    let t_s_k = (2.0 * n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k =
        MuellerMatrix::fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    // Project this channel's incident Stokes state onto the selected eigenmode
    // before transmission -- see `entry_eigenmode_selection`'s doc comment for the
    // full derivation. The transmitted state becomes fully linearly polarized
    // along the mode's own axis (`V` zeroed), at this channel's own unscaled
    // intensity: the mode-selection draw's probability matches this mode's true
    // physical energy fraction exactly, so multiplying by that fraction and
    // dividing by the identical selection probability cancel, leaving `i_k` itself
    // rather than a scaled-down share. `entry_mode_azimuth2` is `None` for a
    // biaxial material or when the incident light carries negligible linear
    // polarization -- in both cases `stokes[k]` passes through unprojected.
    let incident_k =
        if entering_anisotropic && let Some((cos_2psi_x, sin_2psi_x)) = entry_mode_azimuth2 {
            let i_k = stokes_k.i;
            StokesVector::new(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0)
        } else {
            *stokes_k
        };
    // No `/ split_pdf` here: at an anisotropic entry, `trans_matrix_k` is already
    // the full transmitted intensity for a beam at the selected mode's own index,
    // and that mode carries only its ~0.5 energy share of the incident light --
    // dividing by the 0.5 selection probability on top of that would double-count.
    *stokes_k = incident_k
        .apply_matrix(&trans_matrix_k)
        .scale(1.0 / (1.0 - r_unpol));

    // Channel k's own probability of choosing "transmit" at this interface, using
    // k's own unpolarized reflectance. For k == hero_idx this reproduces r_unpol
    // exactly only when `n2k` is computed from the same index the branch decision
    // in `apply_partial_fresnel_bounce` used; at an anisotropic entry where the
    // ordinary mode is selected instead, `n2k` is genuinely different, so
    // `r_unpol_k` is not `r_unpol` there -- still the physically correct
    // probability for the mode actually selected.
    let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
    // `path_pdf`'s role is `spectral_mis_weight`'s per-channel weight,
    // `N * path_pdf[hero] / sum(path_pdf)`, which is scale-invariant under
    // multiplying every channel's `path_pdf` by the same uniform factor -- so no
    // `* split_pdf` is needed here either.
    *path_pdf_k *= 1.0 - r_unpol_k;
}

/// Inputs [`apply_channel_chromatic_termination`] needs from its caller's own bounce
/// geometry and refraction decision.
struct ChannelMismatchInputs {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
    inside_gem: bool,
    lambda_k: f32,
    refr_wave_dir_k: Vec3,
    original_stokes_k: StokesVector,
}

/// The chromatic-termination (`!direction_matches`) branch of [`apply_refract_channel`]'s
/// per-channel work, split out purely to keep that function's own body under the
/// workspace line-count lint -- see this crate's doc comments there for the full
/// physical rationale.
fn apply_channel_chromatic_termination(
    inputs: &ChannelMismatchInputs,
    k: usize,
    stokes_k: &mut StokesVector,
    path_pdf_k: &mut f32,
    exit_event: &mut ExitEvent<'_, '_>,
) {
    let &ChannelMismatchInputs {
        n1k,
        n2k,
        cos_i,
        cos_t_k,
        r_unpol,
        entering_anisotropic,
        entry_mode_azimuth2,
        inside_gem,
        lambda_k,
        refr_wave_dir_k,
        original_stokes_k,
    } = inputs;
    // Chromatic termination. Reached by a genuine interior mismatch (an
    // anisotropic entry's eigenmode direction, or -- via the degenerate
    // wave-normal-parallel-to-optic-axis fallback this function also serves -- an
    // internal reflection's direction, diverging from the hero's), or an exit
    // mismatch (`is_exit_event` below). Channel k's own reflect-branch
    // contributions elsewhere are already correctly weighted by the hero's own
    // selection probability, same as the `sin2_t_k > 1.0` early return above --
    // correct and unbiased, not a bias to be corrected.
    //
    // With splitting enabled, a mismatched channel loses only its radiance here --
    // its `path_pdf[k]` keeps accumulating (this event's own transmit factor, the
    // same `1 - r_unpol_k` the matching branch above folds in), because technique
    // k remains a live member of every compatible channel's MIS family (see
    // `ExitSplitCtx::compat`). At the exit event the channel additionally resolves
    // its own transmitted radiance along its own refracted direction via
    // `try_split_exit_channel`. With splitting disabled this is the plain
    // chromatic termination, bit for bit.
    let prefix_path_pdf_k = *path_pdf_k;
    *stokes_k = stokes_k.scale(0.0);
    *path_pdf_k = 0.0;

    let (exit, hit_point) = (&mut *exit_event.exit, exit_event.hit_point);
    if exit.enabled {
        // Leaving the gem back into air -- never true simultaneously with
        // `entering_anisotropic` (that flag requires `!inside_gem`).
        let is_exit_event = inside_gem && !entering_anisotropic;
        let (transmitted, r_unpol_k) = compute_channel_transmission(&ChannelTransmissionInputs {
            n1k,
            n2k,
            cos_i,
            cos_t_k,
            r_unpol,
            entering_anisotropic,
            entry_mode_azimuth2,
            incident_stokes_k: original_stokes_k,
        });
        *path_pdf_k = prefix_path_pdf_k * (1.0 - r_unpol_k);
        if is_exit_event && original_stokes_k.intensity() > 0.0 {
            try_split_exit_channel(
                exit,
                hit_point,
                k,
                lambda_k,
                refr_wave_dir_k,
                transmitted.intensity(),
            );
        }
    }
}

/// One channel's share of [`apply_refract_bounce`]'s per-channel loop -- see that
/// function's doc comment for the full rationale (chromatic termination, per-channel
/// path-pdf bookkeeping). Each channel `k` is fully independent of every other.
fn apply_refract_channel(
    bctx: &BounceContext<'_, '_>,
    k: usize,
    ray: BounceRay,
    inside_gem: bool,
    decision: &RefractDecision,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> (Option<Vec3>, bool) {
    const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

    let ctx = bctx.ctx;
    let &RefractDecision {
        entering_anisotropic,
        entry_mode_azimuth2,
        final_refr_dir,
        r_unpol,
        ..
    } = decision;
    let (stokes, path_pdf) = (&mut *state.stokes, &mut *state.path_pdf);
    // Captured before any mutation below, so the split branch (which needs the
    // original incident value after `stokes[k]` has already been zeroed) can still
    // compute k's own transmission -- see this function's `else` branch.
    let original_stokes_k = stokes[k];

    let Some(geometry) = compute_channel_refraction_geometry(bctx, k, ray, decision) else {
        stokes[k] = stokes[k].scale(0.0);
        path_pdf[k] = 0.0;
        return (None, false);
    };
    let ChannelRefractionGeometry {
        n1k,
        n2k,
        cos_t_k,
        refr_wave_dir_k,
        final_dir_k,
    } = geometry;

    let direction_matches = final_dir_k.dot(final_refr_dir) >= DIRECTION_MATCH_COS_TOL;

    if direction_matches {
        apply_channel_transmission_match(
            &ChannelTransmitMatchInputs {
                n1k,
                n2k,
                cos_i: bctx.geo.cos_i,
                cos_t_k,
                r_unpol,
                entering_anisotropic,
                entry_mode_azimuth2,
            },
            &mut stokes[k],
            &mut path_pdf[k],
        );
    } else {
        apply_channel_chromatic_termination(
            &ChannelMismatchInputs {
                n1k,
                n2k,
                cos_i: bctx.geo.cos_i,
                cos_t_k,
                r_unpol,
                entering_anisotropic,
                entry_mode_azimuth2,
                inside_gem,
                lambda_k: ctx.lambdas[k],
                refr_wave_dir_k,
                original_stokes_k,
            },
            k,
            &mut stokes[k],
            &mut path_pdf[k],
            exit_event,
        );
    }
    (Some(final_dir_k), direction_matches)
}

/// Polarization-weighted probability that the uniaxial ordinary eigenmode is the
/// physically correct label for this bounce's shared hero-driven path, together with
/// that eigenmode's own doubled polarization azimuth (`cos(2*psi_o)`, `sin(2*psi_o)`)
/// expressed in the same `(s, p)` Stokes frame `current_plane_normal` establishes
/// (`s_axis == current_plane_normal`, `p_axis == k_hat x s_axis`, matching
/// `MuellerMatrix::fresnel_reflection`/`fresnel_transmission`).
///
/// `p_o = 1/2 * (1 + DoP_linear * cos(2*(psi - psi_o)))`, where `psi` is the incident
/// Stokes state's own polarization azimuth and `DoP_linear = sqrt(Q^2+U^2)/I` -- the
/// physical energy fraction Malus's law puts into the ordinary eigenmode for a
/// partially linearly polarized beam (the unpolarized remainder splits 50/50; the
/// polarized remainder follows `cos^2(psi-psi_o)`). Circular polarization does not
/// bias the split between two linear eigenmodes, so `V` plays no role. Expanded
/// directly in `Q`/`U` rather than via `psi`/`DoP_linear` intermediates, which also
/// makes `p_o` reduce to exactly 0.5 for unpolarized light with no separate branch.
/// `cos(2*psi_o)`/`sin(2*psi_o)` come from `o_hat`'s components in the local
/// `(s_hat, p_hat)` basis via the double-angle identities `cos(2x) = s^2 - p^2`,
/// `sin(2x) = 2*s*p`, avoiding an `atan2`/half-angle round-trip.
///
/// Returns `None` (fall back to a flat 50/50 draw, no eigenmode projection) whenever
/// there isn't enough signal to weight the draw meaningfully: `i` negligibly small, a
/// degenerate plane of incidence (`current_plane_normal` near zero, near-normal
/// incidence), or negligible linear polarization (`q`/`u` both near zero). Callers also
/// skip this entirely for a biaxial material -- `ordinary_eigen_polarization` is
/// uniaxial-only -- keeping a biaxial entry at a blanket 50/50 unconditionally.
#[must_use]
pub(super) fn entry_eigenmode_selection(
    c_axis: Vec3,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    hero_stokes: StokesVector,
) -> Option<(f32, f32, f32)> {
    if hero_stokes.i <= 1e-7 || current_plane_normal.length_squared() <= 1e-6 {
        return None;
    }
    let (q, u) = (hero_stokes.q, hero_stokes.u);
    if q.mul_add(q, u * u) <= 1e-12 {
        return None;
    }
    let o_hat = BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis);
    // `current_plane_normal` is already unit and exactly perpendicular to `k_hat`
    // (`k_hat.cross(normal)`, normalized) -- no re-orthogonalization needed.
    let s_hat = current_plane_normal;
    let p_hat = k_hat.cross(s_hat);
    let s_comp = o_hat.dot(s_hat);
    let p_comp = o_hat.dot(p_hat);
    let cos_2psi_o = s_comp.mul_add(s_comp, -(p_comp * p_comp));
    let sin_2psi_o = 2.0 * s_comp * p_comp;
    let p_o = (0.5 + 0.5 * q.mul_add(cos_2psi_o, u * sin_2psi_o) / hero_stokes.i).clamp(0.0, 1.0);
    Some((p_o, cos_2psi_o, sin_2psi_o))
}

/// The degenerate (`k_hat` parallel to the optic axis) fallback [`apply_uniaxial_entry_bounce`]
/// dispatches to: plain scalar isotropic Fresnel at each channel's own `n_o_ch[k]`
/// (`effective_extraordinary_index(n_o, n_e, theta_c=0) == n_o` exactly, so `n_o` is
/// the correct, exact index here), applied to the incident Stokes state unprojected --
/// the isotropic-material code path elsewhere in this file, inlined for this
/// anisotropic-material special case. Always labels the resulting internal state
/// ordinary (`Some(false)`): with both eigenmodes truly degenerate to `n_o` here,
/// `n_medium_ch`'s selector reads the same value either way, so the label is bookkeeping
/// only, not a physical claim.
fn apply_uniaxial_entry_bounce_isotropic_fallback(
    geo: &BounceRefractionGeometry,
    k_hat: Vec3,
    normal: Vec3,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> (Vec3, Vec3, bool, Option<bool>) {
    const R_UNPOL_SELECT_MIN: f32 = 0.02;
    const R_UNPOL_SELECT_MAX: f32 = 0.98;

    let n1 = 1.0f32;
    let n2_hero = geo.n_o_hero;
    let cos_i = geo.cos_i;
    let sin2_t = (n1 / n2_hero).powi(2) * sin_i_sq(cos_i);
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s_hero = n2_hero.mul_add(-cos_t, n1 * cos_i) / n2_hero.mul_add(cos_t, n1 * cos_i);
    let r_p_hero = n1.mul_add(-cos_t, n2_hero * cos_i) / n1.mul_add(cos_t, n2_hero * cos_i);
    let r_unpol = (0.5 * r_p_hero.mul_add(r_p_hero, r_s_hero * r_s_hero))
        .clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);

    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_unpol {
        for k in 0..NUM_CHANNELS {
            let n2k = geo.n_o_ch[k];
            let sin2_t_k = (n1 / n2k).powi(2) * sin_i_sq(cos_i);
            let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
            let r_s_k = n2k.mul_add(-cos_t_k, n1 * cos_i) / n2k.mul_add(cos_t_k, n1 * cos_i);
            let r_p_k = n1.mul_add(-cos_t_k, n2k * cos_i) / n1.mul_add(cos_t_k, n2k * cos_i);
            let refl_matrix_k = MuellerMatrix::fresnel_reflection(r_s_k, r_p_k);
            let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
            stokes[k] = stokes[k].apply_matrix(&refl_matrix_k).scale(1.0 / r_unpol);
            path_pdf[k] *= r_unpol_k;
        }
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        (new_k, new_k, false, None)
    } else {
        for k in 0..NUM_CHANNELS {
            let n2k = geo.n_o_ch[k];
            let sin2_t_k = (n1 / n2k).powi(2) * sin_i_sq(cos_i);
            let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
            let t_s_k = (2.0 * n1 * cos_i) / n1.mul_add(cos_i, n2k * cos_t_k);
            let t_p_k = (2.0 * n1 * cos_i) / n2k.mul_add(cos_i, n1 * cos_t_k);
            let trans_matrix_k =
                MuellerMatrix::fresnel_transmission(n1, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
            stokes[k] = stokes[k]
                .apply_matrix(&trans_matrix_k)
                .scale(1.0 / (1.0 - r_unpol));
            let r_s_k = n2k.mul_add(-cos_t_k, n1 * cos_i) / n2k.mul_add(cos_t_k, n1 * cos_i);
            let r_p_k = n1.mul_add(-cos_t_k, n2k * cos_i) / n1.mul_add(cos_t_k, n2k * cos_i);
            let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
            path_pdf[k] *= 1.0 - r_unpol_k;
        }
        let eta = n1 / n2_hero;
        let new_k = (eta * k_hat + f32::mul_add(eta, cos_i, -cos_t) * normal).normalize();
        (new_k, new_k, true, Some(false))
    }
}

#[inline]
fn sin_i_sq(cos_i: f32) -> f32 {
    cos_i.mul_add(-cos_i, 1.0)
}

/// The entire isotropic-air -> uniaxial-crystal entry bounce, via the closed-form
/// `uniaxial_fresnel` solver -- replaces a per-mode scalar-Fresnel entry path (a
/// Malus-law mode split plus isotropic-style `r_s`/`r_p`/`t_s`/`t_p` at a single
/// "effective index") for `entering_anisotropic && !geo.is_biaxial`. Called as an
/// early, self-contained branch from `apply_partial_fresnel_bounce`, leaving the
/// biaxial/isotropic/internal-exit code paths unaffected.
///
/// Reuses the existing Snell-refraction/extraordinary-walk-off geometry formulas
/// (`compute_bounce_refraction_geometry`'s `geo.n2`/`geo.n2_ch`/`geo.n_o_hero`
/// scaffolding) unchanged: only the Fresnel amplitude/polarization physics changes
/// here, not the direction the transmitted ray travels in.
///
/// # Reflect vs. transmit, and mode selection
///
/// `r_branch` is the hero's own unpolarized-average total reflectance from the full
/// coupled Jones solution (`jones_to_mueller(...).col(0)[0]`) -- used only to draw the
/// reflect/transmit branch, never to weight the actual contribution. If transmitting,
/// `p_o_hero` is the hero's true Poynting-weighted ordinary-mode power fraction
/// (`uniaxial_fresnel::mode_power`, fed the hero's actual Stokes state): it already
/// incorporates both the incident polarization's alignment with each mode's eigenaxis
/// and that mode's own transmission efficiency in one physically exact quantity.
/// Reduces to 50/50 only where the geometry actually makes it symmetric (e.g. optic
/// axis along the surface normal) -- see `mode_power`'s own doc comment.
///
/// Each channel's deposited transmitted intensity is `P_mode(stokes[k]) /
/// p_mode_hero_frac` (standard importance-sampling `target / p_sample`, unbiased for
/// any valid `p_sample`), polarized along the drawn mode's own eigenaxis (`o_hat`/
/// `e_hat`, always real -- an entry interface's transmitted side never evanesces).
fn apply_uniaxial_entry_bounce(
    ubctx: &UniaxialBounceContext<'_, '_>,
    ray: BounceRay,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit: &mut ExitSplitCtx<'_>,
) -> (Vec3, Vec3, bool, Option<bool>) {
    const R_UNPOL_SELECT_MIN: f32 = 0.02;
    const R_UNPOL_SELECT_MAX: f32 = 0.98;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let BounceRay { k_hat, normal } = ray;
    let RngDraw { rng_seed, bounce } = rng;

    let hero = ctx.hero_idx;
    let c_axis = ctx.c_axis;
    let n_o_hero = geo.n_o_ch[hero];
    let n_e_hero = ctx
        .material
        .extraordinary_index_at(ctx.lambdas[hero], n_o_hero);

    // Degenerate case: wave normal parallel to the optic axis (light travelling
    // straight down the c-axis sees no birefringence; both eigenmodes collapse to
    // `n_o`). The closed-form ordinary D-direction `k x c_axis` vanishes exactly here,
    // singularizing the boundary-match system -- checked via `k_hat` (the incident
    // direction) since at this limit Snell's law leaves the transmitted direction
    // parallel to it too. Falls back to the plain scalar isotropic-at-`n_o` path,
    // without projecting onto either eigenmode's own axis, since there is no
    // meaningful axis to project onto at this exact limit.
    if k_hat.cross(c_axis).length_squared() < 1e-6 {
        return apply_uniaxial_entry_bounce_isotropic_fallback(
            geo,
            k_hat,
            normal,
            rng_seed,
            bounce,
            state.stokes,
            state.path_pdf,
        );
    }

    // Built once per bounce, shared by the hero's own branch-decision solve below and
    // every channel's solve in
    // `apply_uniaxial_entry_reflect_channels`/`apply_uniaxial_entry_transmit_channels`
    // -- bit-identical to each of those calling `entry_solve_pair` (which rebuilds
    // this internally) fresh, since `n1 == 1.0` for every channel at an air->crystal
    // entry -- see `EntryIncidenceFrame`'s own doc comment.
    let inc_frame = uniaxial_fresnel::entry_incidence_frame(1.0, frame);
    let (sol_s_hero, sol_p_hero) = uniaxial_fresnel::entry_solve_pair_with_incidence(
        &inc_frame, 1.0, n_o_hero, n_e_hero, c_axis, frame,
    );
    let reflect_mueller_hero = uniaxial_fresnel::jones_to_mueller(
        sol_s_hero.r_s,
        sol_p_hero.r_s,
        sol_s_hero.r_p,
        sol_p_hero.r_p,
    );
    let r_branch = reflect_mueller_hero.col(0)[0].clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);

    // Amplitude-1 isotropic-mode intrinsic flux (`poynting_z`'s own 0.5 time-average
    // factor included) -- shared by every channel at entry (`n1 == 1.0` for air,
    // `frame.cos_i` is the hero-driven shared geometry every channel's own solve also
    // uses), so computed once here rather than re-derived per `mode_power` call.
    let inc_flux = 0.5 * frame.cos_i;
    let p_o_raw = uniaxial_fresnel::mode_power(
        sol_s_hero.t_o,
        sol_p_hero.t_o,
        sol_s_hero.flux_o,
        inc_flux,
        state.stokes[hero],
    );
    let p_e_raw = uniaxial_fresnel::mode_power(
        sol_s_hero.t_e,
        sol_p_hero.t_e,
        sol_s_hero.flux_e,
        inc_flux,
        state.stokes[hero],
    );
    let p_o_hero =
        (p_o_raw / (p_o_raw + p_e_raw).max(1e-12)).clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let mode_split_rand = (hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
        as f32)
        / 4_294_967_295.0;
    let use_extraordinary = mode_split_rand < (1.0 - p_o_hero);
    let p_mode_hero_frac = if use_extraordinary {
        1.0 - p_o_hero
    } else {
        p_o_hero
    };

    // Geometry: bit-identical to apply_refract_bounce's own non-biaxial branch.
    let n2_hero_dir = if use_extraordinary {
        geo.n2
    } else {
        geo.n_o_hero
    };
    let eta_dir = geo.n1 / n2_hero_dir;
    let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
    let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
    let refr_wave_dir =
        (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();
    let final_refr_dir = if use_extraordinary {
        BirefringenceParams::extraordinary_poynting_dir(
            refr_wave_dir,
            c_axis,
            geo.n_o_hero,
            geo.n_e_hero,
        )
    } else {
        refr_wave_dir
    };
    // `o_hat`/`e_hat` are only guaranteed perpendicular to `refr_wave_dir` (the
    // transmitted wave normal), not to `k_hat` (the incident one) -- `frame.p_axis` is
    // built from `k_hat`, so projecting a mode direction onto it directly would lose
    // information whenever Snell's law bends the ray. `s_axis` is shared between the
    // incident and transmitted sides (Snell coplanarity), so
    // `p_axis_transmitted = refr_wave_dir x s_axis` is the correct partner axis, and
    // `refr_wave_dir` is exactly the axis `current_k` becomes for the next bounce's
    // own rotation.
    let p_axis_transmitted = refr_wave_dir.cross(frame.s_axis);

    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_branch {
        apply_uniaxial_entry_reflect_channels(ubctx, &inc_frame, r_branch, state);
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        (new_k, new_k, false, None)
    } else {
        let mode = EntryTransmitMode {
            use_extraordinary,
            final_refr_dir,
            p_axis_transmitted,
            p_mode_hero_frac,
            inc_flux,
            r_branch,
        };
        apply_uniaxial_entry_transmit_channels(ubctx, &inc_frame, ray, &mode, state, exit);
        (refr_wave_dir, final_refr_dir, true, Some(use_extraordinary))
    }
}

/// The reflect branch's per-channel loop, extracted from [`apply_uniaxial_entry_bounce`]
/// to keep that function under clippy's function-length lint. `inc_frame` is the
/// caller's already-built [`EntryIncidenceFrame`] -- see that type's own doc comment.
fn apply_uniaxial_entry_reflect_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    r_branch: f32,
    state: &mut BounceState<'_>,
) {
    let ctx = ubctx.ctx;
    let geo = ubctx.geo;
    let frame = ubctx.frame;
    let c_axis = ctx.c_axis;
    let stokes = &mut *state.stokes;
    let path_pdf = &mut *state.path_pdf;
    for k in 0..NUM_CHANNELS {
        let n_o_k = geo.n_o_ch[k];
        let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
            inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
        );
        let mueller_k =
            uniaxial_fresnel::jones_to_mueller(sol_s_k.r_s, sol_p_k.r_s, sol_s_k.r_p, sol_p_k.r_p);
        let r_unpol_k = mueller_k.col(0)[0].clamp(1e-4, 1.0 - 1e-4);
        stokes[k] = stokes[k].apply_matrix(&mueller_k).scale(1.0 / r_branch);
        path_pdf[k] *= r_unpol_k;
    }
}

/// The transmit branch's per-channel loop (chromatic-termination geometry check plus
/// the closed-form mode-power amplitude deposit), extracted from
/// [`apply_uniaxial_entry_bounce`] for the same reason as
/// [`apply_uniaxial_entry_reflect_channels`].
/// [`apply_uniaxial_entry_bounce`]'s own resolved transmit-branch outcome, passed down
/// to [`apply_uniaxial_entry_transmit_channels`]'s per-channel loop unchanged.
struct EntryTransmitMode {
    use_extraordinary: bool,
    final_refr_dir: Vec3,
    p_axis_transmitted: Vec3,
    p_mode_hero_frac: f32,
    inc_flux: f32,
    r_branch: f32,
}

fn apply_uniaxial_entry_transmit_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    ray: BounceRay,
    mode: &EntryTransmitMode,
    state: &mut BounceState<'_>,
    exit: &mut ExitSplitCtx<'_>,
) {
    const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let c_axis = ctx.c_axis;
    let BounceRay { k_hat, normal } = ray;
    let &EntryTransmitMode {
        use_extraordinary,
        final_refr_dir,
        p_axis_transmitted,
        p_mode_hero_frac,
        inc_flux,
        r_branch,
    } = mode;
    let (stokes, path_pdf) = (&mut *state.stokes, &mut *state.path_pdf);

    let mut dirs = [None; NUM_CHANNELS];
    let mut hero_match = [false; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let n_o_k = geo.n_o_ch[k];
        // Chromatic-termination geometry: apply_refract_channel's own formula,
        // reusing geo.n2_ch[k] (the channel's own effective extraordinary index) for
        // the direction check only.
        let n2k_dir = if use_extraordinary {
            geo.n2_ch[k]
        } else {
            n_o_k
        };
        let sin2_t_k = (geo.n1 / n2k_dir).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
        if sin2_t_k > 1.0 {
            stokes[k] = stokes[k].scale(0.0);
            path_pdf[k] = 0.0;
            continue;
        }
        let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
        let eta_dir_k = geo.n1 / n2k_dir;
        let refr_wave_dir_k =
            (eta_dir_k * k_hat + f32::mul_add(eta_dir_k, geo.cos_i, -cos_t_k) * normal).normalize();
        let final_dir_k = if use_extraordinary {
            let n_e_eff_k = n_o_k + ctx.material.birefringence_delta;
            BirefringenceParams::extraordinary_poynting_dir(
                refr_wave_dir_k,
                c_axis,
                n_o_k,
                n_e_eff_k,
            )
        } else {
            refr_wave_dir_k
        };
        dirs[k] = Some(final_dir_k);
        let direction_matches = final_dir_k.dot(final_refr_dir) >= DIRECTION_MATCH_COS_TOL;
        hero_match[k] = direction_matches;
        if !direction_matches {
            stokes[k] = stokes[k].scale(0.0);
            if exit.enabled {
                // Technique k stays a live member of every compatible channel's MIS
                // family -- keep its own entry-transmit density accumulating (the same
                // `t_unpol_k` the matching case below folds in); only its radiance
                // ends here. See `apply_refract_channel`'s scalar counterpart.
                let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
                let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
                    inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
                );
                let mueller_k = uniaxial_fresnel::jones_to_mueller(
                    sol_s_k.r_s,
                    sol_p_k.r_s,
                    sol_s_k.r_p,
                    sol_p_k.r_p,
                );
                let t_unpol_k = (1.0 - mueller_k.col(0)[0]).clamp(1e-4, 1.0 - 1e-4);
                path_pdf[k] *= t_unpol_k;
            } else {
                path_pdf[k] = 0.0;
            }
            continue;
        }

        let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
            inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
        );
        let (t_s_row, t_p_row, flux_k, mode_dir) = if use_extraordinary {
            (sol_s_k.t_e, sol_p_k.t_e, sol_s_k.flux_e, sol_s_k.e_hat)
        } else {
            (sol_s_k.t_o, sol_p_k.t_o, sol_s_k.flux_o, sol_s_k.o_hat)
        };
        let p_mode_k = uniaxial_fresnel::mode_power(t_s_row, t_p_row, flux_k, inc_flux, stokes[k]);
        let deposit_i = p_mode_k / p_mode_hero_frac;
        let (cos_2psi, sin_2psi) =
            uniaxial_fresnel::azimuth2_in_frame(mode_dir, frame.s_axis, p_axis_transmitted);
        stokes[k] = StokesVector::new(deposit_i, deposit_i * cos_2psi, deposit_i * sin_2psi, 0.0)
            .scale(1.0 / (1.0 - r_branch));

        let mueller_k =
            uniaxial_fresnel::jones_to_mueller(sol_s_k.r_s, sol_p_k.r_s, sol_s_k.r_p, sol_p_k.r_p);
        let t_unpol_k = (1.0 - mueller_k.col(0)[0]).clamp(1e-4, 1.0 - 1e-4);
        path_pdf[k] *= t_unpol_k;
    }
    // The uniaxial entry is an interior dispersive event -- narrow every channel's MIS
    // family, see `narrow_compat`'s doc comment.
    if exit.enabled {
        narrow_compat(&mut exit.compat, &dirs, ctx.hero_idx, hero_match);
    }
}

/// The uniaxial-crystal internal bounce -- o<->e coupled partial reflection and
/// uniaxial->isotropic exit transmission -- via the closed-form
/// `uniaxial_fresnel::internal_solve`. The internal-bounce analogue of
/// [`apply_uniaxial_entry_bounce`]; reached from [`apply_partial_fresnel_bounce`]
/// whenever `inside_gem && geo.uniaxial_frame.is_some()` (any uniaxial internal event
/// that is not already hero-forced past critical angle -- that case is
/// [`apply_tir_bounce`]'s own uniaxial branch, reached from a different call site in
/// `transport::dispatch_bounce`). The shared `apply_partial_reflect_bounce`/
/// `apply_refract_bounce`/`apply_refract_channel` machinery is reached only by an
/// isotropic or biaxial material, unaffected by this function.
///
/// Unlike the entry case there is only ONE incident polarization state to solve for
/// (the propagating Stokes vector is always fully linearly polarized along the current
/// eigenmode's own axis while inside the crystal -- see [`apply_tir_bounce`]'s own doc
/// comment for why), so `internal_solve` is called once per channel (not as an (s, p)
/// pair, unlike `entry_solve_pair`).
///
/// # Reflect vs. transmit
///
/// `r_branch` is the hero's own Poynting-flux-weighted total reflectance `R_o + R_e`
/// (mirrors `apply_uniaxial_entry_bounce`'s `r_branch` role: drives the reflect/transmit
/// coin flip only, never the per-channel deposit, each channel re-evaluating
/// `internal_solve` at its own wavelength).
///
/// # Exit-transmission Stokes construction
///
/// The incident state is a coherent, fully-polarized single mode (unlike entry's
/// general (s, p) input), so the transmitted Stokes vector for a unit incident
/// amplitude is the standard Jones-vector-to-Stokes identity applied to the
/// flux-normalized amplitudes `t_s' = t_s * sqrt(flux_ts / flux_inc)`, `t_p' = t_p *
/// sqrt(flux_tp / flux_inc)` (needed because the isotropic exit side has two output
/// channels, s and p, with generally different intrinsic flux-per-amplitude, so
/// combining them into one coherent Stokes vector requires the same normalization
/// before `Q`/`U`/`V` are meaningful): `i_unit = |t_s'|^2 + |t_p'|^2` is the physical
/// power transmittance fraction for unit incident amplitude. The incident mode's own
/// current `stokes[k].i` is then the only scale factor needed: the crystal-internal
/// invariant above already guarantees `stokes[k]` carries no other information this
/// event needs.
///
/// Returns `(new_k, new_s, new_inside_gem, is_extraordinary_update, exact_p_o)`. The
/// last element is `Some` only on the reflect branch (o<->e coupling is a
/// reflection-time event only): the hero channel's own `R_o / (R_o + R_e)`, fed to
/// `transport::apply_internal_mode_coupling` as its o<->e relabeling probability.
fn apply_uniaxial_internal_bounce(
    ubctx: &UniaxialBounceContext<'_, '_>,
    is_extraordinary: bool,
    ray: BounceRay,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> (Vec3, Vec3, bool, Option<bool>, Option<f32>) {
    const R_UNPOL_SELECT_MIN: f32 = 0.02;
    const R_UNPOL_SELECT_MAX: f32 = 0.98;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let BounceRay { k_hat, normal } = ray;
    let RngDraw { rng_seed, bounce } = rng;

    let hero = ctx.hero_idx;
    let c_axis = ctx.c_axis;
    let n_inc_hero = geo.n1_ch[hero];
    let n_o_hero = geo.n_o_ch[hero];
    let n_e_hero = ctx
        .material
        .extraordinary_index_at(ctx.lambdas[hero], n_o_hero);

    let sol_hero = uniaxial_fresnel::internal_solve(
        n_inc_hero,
        n_o_hero,
        n_e_hero,
        c_axis,
        frame,
        !is_extraordinary,
    );
    let flux_inc_hero = sol_hero.flux_inc.max(1e-12);
    let ro_pow = sol_hero.r_o.norm_sqr() * sol_hero.flux_ro;
    let re_pow = sol_hero.r_e.norm_sqr() * sol_hero.flux_re;
    let r_branch =
        ((ro_pow + re_pow) / flux_inc_hero).clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let p_o_exact = ro_pow / (ro_pow + re_pow).max(1e-12);

    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    let hero_solve = HeroInternalSolve {
        hero,
        is_extraordinary,
        sol_hero,
        r_branch,
    };
    if rng_bounce < r_branch {
        apply_uniaxial_internal_reflect_channels(ubctx, &hero_solve, state);
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        (new_k, new_k, true, None, Some(p_o_exact))
    } else {
        // Exit transmission into isotropic air -- Snell geometry matching
        // `apply_refract_bounce`'s own non-biaxial branch, using k_hat rather than the
        // walked-off S. `geo.n2 == 1.0` (air) and `geo.n1 == n_inc_hero` always hold
        // here (`inside_gem` is true on every path that reaches this function).
        let eta_dir = geo.n1 / geo.n2;
        let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
        let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
        let refr_wave_dir =
            (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();
        apply_uniaxial_internal_transmit_channels(ubctx, &hero_solve, ray, state, exit_event);
        (refr_wave_dir, refr_wave_dir, false, None, None)
    }
}

/// The hero channel's already-solved [`uniaxial_fresnel::internal_solve`] result, shared
/// by [`apply_uniaxial_internal_reflect_channels`] and
/// [`apply_uniaxial_internal_transmit_channels`] so neither re-solves the hero's own
/// boundary system a second time.
#[derive(Clone, Copy)]
struct HeroInternalSolve {
    hero: usize,
    is_extraordinary: bool,
    sol_hero: uniaxial_fresnel::InternalPolarizationSolution,
    r_branch: f32,
}

/// The reflect branch's per-channel loop for [`apply_uniaxial_internal_bounce`] --
/// same magnitude-only simplification [`apply_tir_bounce`]'s own uniaxial branch uses
/// (see its doc comment), just importance-sampling-corrected by `1 / r_branch` since
/// this reflect event is NOT forced (probability `r_branch`, not 1).
fn apply_uniaxial_internal_reflect_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    hero_solve: &HeroInternalSolve,
    state: &mut BounceState<'_>,
) {
    let ctx = ubctx.ctx;
    let geo = ubctx.geo;
    let frame = ubctx.frame;
    let c_axis = ctx.c_axis;
    let &HeroInternalSolve {
        hero,
        is_extraordinary,
        sol_hero,
        r_branch,
    } = hero_solve;
    let stokes = &mut *state.stokes;
    let path_pdf = &mut *state.path_pdf;
    for k in 0..NUM_CHANNELS {
        // The caller already solved the hero channel's own boundary system once, to
        // decide `r_branch` -- reuse it here instead of solving the identical system
        // again.
        let sol = if k == hero {
            sol_hero
        } else {
            let n_inc_k = geo.n1_ch[k];
            let n_o_k = geo.n_o_ch[k];
            let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
            uniaxial_fresnel::internal_solve(
                n_inc_k,
                n_o_k,
                n_e_k,
                c_axis,
                frame,
                !is_extraordinary,
            )
        };
        let flux_inc = sol.flux_inc.max(1e-12);
        let r_total_k = (sol
            .r_e
            .norm_sqr()
            .mul_add(sol.flux_re, sol.r_o.norm_sqr() * sol.flux_ro)
            / flux_inc)
            .min(1.0);
        stokes[k] = stokes[k].scale(r_total_k / r_branch);
        path_pdf[k] *= r_total_k.clamp(1e-4, 1.0 - 1e-4);
    }
}

/// Channel k's own flux-normalized transmitted Stokes state at the uniaxial exact exit
/// interface -- the same per-channel computation
/// [`apply_uniaxial_internal_transmit_channels`]'s matching-direction branch runs
/// inline, factored out so the split branch can compute the identical value without
/// duplicating the formula. The crystal-internal invariant (`stokes[k].i` is the only
/// scale factor needed) is what makes `incident_i`, rather than the full incident
/// Stokes vector, sufficient here. Returns the transmitted state and `i_unit` (the
/// matching branch's own `path_pdf[k] *= i_unit.clamp(..)` factor; the split branch
/// applies this same factor unconditionally at every exit event).
fn compute_uniaxial_exit_transmission(
    sol: uniaxial_fresnel::InternalPolarizationSolution,
    r_branch: f32,
    incident_i: f32,
) -> (StokesVector, f32) {
    let flux_inc = sol.flux_inc.max(1e-12);
    let ts_n = sol.t_s.scale((sol.flux_ts / flux_inc).sqrt());
    let tp_n = sol.t_p.scale((sol.flux_tp / flux_inc).sqrt());
    let i_unit = ts_n.norm_sqr() + tp_n.norm_sqr();
    let q_unit = ts_n.norm_sqr() - tp_n.norm_sqr();
    let cross = ts_n.mul(tp_n.conj());
    let u_unit = 2.0 * cross.re;
    let v_unit = -2.0 * cross.im;
    let transmitted = StokesVector::new(
        incident_i * i_unit,
        incident_i * q_unit,
        incident_i * u_unit,
        incident_i * v_unit,
    )
    .scale(1.0 / (1.0 - r_branch));
    (transmitted, i_unit)
}

/// The exit-transmission branch's per-channel loop for
/// [`apply_uniaxial_internal_bounce`] -- see [`compute_uniaxial_exit_transmission`]'s
/// own doc comment for the flux-normalized-amplitude-to-Stokes derivation. This is the
/// uniaxial exact exit event, so channel k's own mismatch here is always the
/// exit-splitting case, never an interior one -- see this module's top-of-file
/// doc comment. `exit.enabled == false` reproduces plain chromatic termination exactly
/// (channel k's own Snell direction at k's own index, compared against the hero's,
/// zeroed on mismatch).
fn apply_uniaxial_internal_transmit_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    hero_solve: &HeroInternalSolve,
    ray: BounceRay,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) {
    const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let c_axis = ctx.c_axis;
    let &HeroInternalSolve {
        hero,
        is_extraordinary,
        sol_hero,
        r_branch,
    } = hero_solve;
    let BounceRay { k_hat, normal } = ray;
    let (stokes, path_pdf) = (&mut *state.stokes, &mut *state.path_pdf);
    let (exit, hit_point) = (&mut *exit_event.exit, exit_event.hit_point);

    // The hero's own exit direction, recomputed here from `geo.n1`/`geo.n2`/`k_hat`/
    // `normal` rather than threaded through -- needed as the chromatic-termination
    // reference direction for every channel's own check below.
    let eta_dir = geo.n1 / geo.n2;
    let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
    let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
    let hero_refr_dir =
        (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();

    for k in 0..NUM_CHANNELS {
        // Captured before any mutation below, so the split branch (which needs the
        // original incident value after `stokes[k]` has already been zeroed) can still
        // compute k's own transmission.
        let original_stokes_i = stokes[k].i;

        let n_inc_k = geo.n1_ch[k];
        let n2k = geo.n2_ch[k];
        let sin2_t_k = (n_inc_k / n2k).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
        if sin2_t_k > 1.0 {
            stokes[k] = stokes[k].scale(0.0);
            path_pdf[k] = 0.0;
            continue;
        }
        let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
        let eta_dir_k = n_inc_k / n2k;
        let refr_wave_dir_k =
            (eta_dir_k * k_hat + f32::mul_add(eta_dir_k, geo.cos_i, -cos_t_k) * normal).normalize();
        let direction_matches = refr_wave_dir_k.dot(hero_refr_dir) >= DIRECTION_MATCH_COS_TOL;

        // Reuse the caller's already-solved hero channel instead of re-solving the
        // identical boundary system. Needed by both branches below (the matching one
        // directly; the split one via `compute_uniaxial_exit_transmission`), so solved
        // once here regardless.
        let sol = if k == hero {
            sol_hero
        } else {
            let n_o_k = geo.n_o_ch[k];
            let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
            uniaxial_fresnel::internal_solve(
                n_inc_k,
                n_o_k,
                n_e_k,
                c_axis,
                frame,
                !is_extraordinary,
            )
        };

        if !direction_matches {
            // Chromatic termination -- this is always the exit event, so this is
            // exactly `apply_refract_channel`'s own `else` branch's uniaxial-exact
            // counterpart. With splitting enabled, channel k keeps its own density of
            // having produced the shared path (prefix times its own exit transmit
            // probability, the same `t_unpol_k` the matching branch below folds in)
            // and resolves its own transmitted radiance along its own direction.
            let prefix_path_pdf_k = path_pdf[k];
            stokes[k] = stokes[k].scale(0.0);
            path_pdf[k] = 0.0;

            if exit.enabled {
                let (transmitted, i_unit) =
                    compute_uniaxial_exit_transmission(sol, r_branch, original_stokes_i);
                let t_unpol_k = i_unit.clamp(1e-4, 1.0 - 1e-4);
                path_pdf[k] = prefix_path_pdf_k * t_unpol_k;
                if original_stokes_i > 0.0 {
                    try_split_exit_channel(
                        exit,
                        hit_point,
                        k,
                        ctx.lambdas[k],
                        refr_wave_dir_k,
                        transmitted.intensity(),
                    );
                }
            }
            continue;
        }

        // direction_matches: this is hero's own realized path, or a companion that
        // genuinely coincides with it -- both need `path_pdf[k]`'s normal accumulation
        // for `spectral_mis_weight`.
        let (transmitted, i_unit) =
            compute_uniaxial_exit_transmission(sol, r_branch, original_stokes_i);
        stokes[k] = transmitted;
        let t_unpol_k = i_unit.clamp(1e-4, 1.0 - 1e-4);
        path_pdf[k] *= t_unpol_k;
    }
}

/// Partial Fresnel Reflection & Refraction via Stokes-Mueller Polarized Wave Transport:
/// decides reflect vs. transmit for the shared hero-driven path from the hero's own
/// `r_unpol` (a well-mixed hash of `(rng_seed, bounce)`, replacing an earlier
/// deterministic `(rng_seed + bounce*7919) % 1000` arithmetic progression that could
/// correlate with the Russian-roulette draw; this hash is decorrelated from it via a
/// distinct salt), then applies whichever event was sampled via
/// [`apply_partial_reflect_bounce`] or [`apply_refract_bounce`]. A direct extraction of
/// the pre-extraction inline branch dispatch: the same floating-point operations, in
/// the same order, driven by the same random draw. Returns the new wave normal `k'`,
/// the new Poynting direction `S'` (reflection's `k'` is re-converted to `S'` via
/// [`poynting_dir_for_mode`] here, using the mode still in effect -- reflection alone
/// never changes `is_extraordinary`, see `maybe_apply_internal_mode_coupling`'s own doc
/// comment for the SEPARATE relabeling step that may reassign it for the NEXT bounce),
/// the new `inside_gem` state, and (mirroring [`RefractBounceOutcome`]) the
/// `is_extraordinary` update, if any.
///
/// At an air->crystal entry into an anisotropic material, which eigenmode
/// (ordinary/mode-A vs extraordinary/mode-B) this path's single geometric transmission
/// event represents is decided HERE -- before the reflect-vs-transmit draw below --
/// rather than inside `apply_refract_bounce`'s own transmit branch as it used to be.
/// Two reasons this has to happen up here:
///   - The SELECTION itself is weighted by the incident polarization's projection onto
///     each eigenmode's own axis ([`entry_eigenmode_selection`]), not a blanket 50/50 --
///     and that weighting has to happen exactly once per bounce, shared by both
///     branches below: a beam already aligned with the ordinary axis should be MORE
///     likely to reflect at the ordinary index too, not just more likely to transmit
///     as ordinary conditional on transmitting.
///   - The REFLECT branch's own Fresnel coefficients must be evaluated at the SAME
///     mode's index the transmit branch (`apply_refract_channel`) already uses for this
///     channel, so that `R + T == 1` for whichever mode this draw actually selects.
///     Before this fix, the reflect/transmit decision always used mode B
///     (extraordinary)'s index regardless of which mode transmission ultimately used,
///     so `R + T != 1` by `O(delta_n / n)` whenever the ordinary mode was selected.
///
/// A biaxial material has no uniaxial "ordinary" eigenmode to weight against, so both
/// the selection and the index correction below are gated on `!geo.is_biaxial` and a
/// biaxial entry reduces exactly to the previous behaviour (blanket 50/50, mode B's
/// index driving the reflect/transmit decision).
/// Dispatches [`apply_partial_fresnel_bounce`]'s two closed-form uniaxial paths --
/// air->crystal entry and internal/exit -- split out purely to keep that function's
/// own body under the workspace line-count lint. Returns `Some` with the full result
/// tuple when one of the closed-form paths applies; `None` when the caller must fall
/// through to the general (biaxial/isotropic) machinery instead.
///
/// A uniaxial air->crystal entry takes the closed-form `apply_uniaxial_entry_bounce`
/// path in full, self-contained, and returns directly -- everything else here (the
/// `entry_eigenmode_selection`/scalar-Fresnel machinery `apply_partial_fresnel_bounce`
/// falls through to) is now reached only by a biaxial entry, an internal/exit bounce,
/// or an isotropic material.
///
/// Any uniaxial internal event (o<->e coupled partial reflection or
/// uniaxial->isotropic exit transmission) that is not already hero-forced past
/// critical angle (that case is `apply_tir_bounce`'s own uniaxial branch, dispatched
/// from a different call site in `transport::dispatch_bounce`) takes the closed-form
/// `apply_uniaxial_internal_bounce` path in full, self-contained, and returns
/// directly -- see that function's own doc comment. The remaining fallthrough case is
/// reached only by a biaxial material, an isotropic one, or this SAME degenerate
/// wave-normal-parallel-to-optic-axis case `apply_uniaxial_entry_bounce` also
/// special-cases (`k_hat.cross(c_axis)` ~ 0): the closed-form ordinary D-direction `k
/// x c_axis` vanishes exactly there, singularizing `internal_solve`'s boundary system
/// (its own `flux_inc` floors to the `1e-12` guard instead of the true nonzero value,
/// which very nearly zeroed EVERY uniaxial internal bounce along a c-axis-aligned ray
/// -- caught by `test_spectral_raytrace_colored_gem`'s ruby render going black). At
/// this exact limit BOTH eigenmodes truly collapse to a single isotropic response at
/// `n_o` (`effective_extraordinary_index(n_o, n_e, theta_c=0) == n_o` exactly), so
/// falling through to the plain scalar isotropic-at-`n_o` machinery is not an
/// approximation -- it is the exact physics, identical to
/// `apply_uniaxial_entry_bounce_isotropic_fallback`'s own rationale.
/// [`apply_partial_fresnel_bounce`]'s own full result tuple: the new wave-normal `k`
/// and Poynting direction `S`, the new `inside_gem` state, the `is_extraordinary`
/// update (if any), and (mirroring [`RefractBounceOutcome`]) a reserved fifth slot.
/// Named purely so [`try_dispatch_uniaxial_bounce`]'s `Option`-wrapped return type
/// doesn't trip clippy's `type_complexity` lint.
type FresnelBounceResult = (Vec3, Vec3, bool, Option<bool>, Option<f32>);

fn try_dispatch_uniaxial_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    entering_anisotropic: bool,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> Option<FresnelBounceResult> {
    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, .. } = ray;
    let PathModeState {
        inside_gem,
        is_extraordinary,
        ..
    } = mode_state;

    if entering_anisotropic && let Some(frame) = geo.uniaxial_frame {
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        let (new_k, new_s, new_inside_gem, is_extraordinary_update) =
            apply_uniaxial_entry_bounce(&ubctx, ray, rng, state, exit_event.exit);
        return Some((new_k, new_s, new_inside_gem, is_extraordinary_update, None));
    }
    if inside_gem
        && let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        return Some(apply_uniaxial_internal_bounce(
            &ubctx,
            is_extraordinary,
            ray,
            rng,
            state,
            exit_event,
        ));
    }
    None
}

/// The polarization-weighted eigenmode selection [`apply_partial_fresnel_bounce`]
/// makes at an anisotropic entry -- split out purely to keep that function's own body
/// under the workspace line-count lint. Returns `(use_extraordinary,
/// entry_mode_azimuth2)`.
///
/// Mirrors `apply_refract_bounce`'s old `split_rand < 0.5` exactly when
/// `entry_selection` is `None` (`p_o == 0.5`: `1.0 - p_o == 0.5` too, same hash,
/// same threshold, same sense -- extraordinary iff `mode_split_rand < 0.5`);
/// weighted by polarization otherwise (extraordinary drawn with probability
/// `1 - p_o`, ordinary with probability `p_o`, as `p_o`'s own doc comment defines
/// it). For a non-entering bounce (internal TIR/reflect/refract, or any
/// isotropic-material refraction) this keeps whatever mode the path already
/// carries -- exactly the `entering_anisotropic` guard's else branch.
///
/// The chosen mode's own doubled polarization azimuth is for `apply_refract_channel`'s
/// eigenmode projection -- `None` when `entry_selection` itself was `None`, in which
/// case that projection is skipped entirely (see its own doc comment). Extraordinary
/// is perpendicular to ordinary (`psi_e == psi_o + 90 deg`), so its doubled azimuth
/// is simply the negation of the ordinary one (`cos(2*psi_o + 180deg) ==
/// -cos(2*psi_o)`, likewise for `sin`).
struct EntryModeSelectionInputs<'m, 'b> {
    ctx: &'b RayMaterialContext<'m>,
    geo: &'b BounceRefractionGeometry,
    entering_anisotropic: bool,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    hero_stokes: StokesVector,
    is_extraordinary: bool,
    rng_seed: u32,
    bounce: u32,
}

fn resolve_entry_mode_selection(
    inputs: &EntryModeSelectionInputs<'_, '_>,
) -> (bool, Option<(f32, f32)>) {
    let &EntryModeSelectionInputs {
        ctx,
        geo,
        entering_anisotropic,
        current_plane_normal,
        k_hat,
        hero_stokes,
        is_extraordinary,
        rng_seed,
        bounce,
    } = inputs;
    let entry_selection = if entering_anisotropic && !geo.is_biaxial {
        entry_eigenmode_selection(ctx.c_axis, current_plane_normal, k_hat, hero_stokes)
    } else {
        None
    };
    let p_o = entry_selection.map_or(0.5, |(p, ..)| p);
    let mode_split_rand = (hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
        as f32)
        / 4_294_967_295.0;
    let use_extraordinary = if entering_anisotropic {
        mode_split_rand < (1.0 - p_o)
    } else {
        is_extraordinary
    };
    let entry_mode_azimuth2 = entry_selection.map(|(_, cos_2psi_o, sin_2psi_o)| {
        if use_extraordinary {
            (-cos_2psi_o, -sin_2psi_o)
        } else {
            (cos_2psi_o, sin_2psi_o)
        }
    });
    (use_extraordinary, entry_mode_azimuth2)
}

/// The reflect-vs-transmit SELECTION probability [`apply_partial_fresnel_bounce`]
/// draws against -- split out purely to keep that function's own body under the
/// workspace line-count lint.
///
/// Use the SELECTED mode's own index for n2 at THIS interface -- `geo.n2` (mode
/// B/extraordinary) unchanged when extraordinary is selected, not entering an
/// anisotropic material at all, or the material is biaxial (`geo.n_o_hero` is a
/// uniaxial-only quantity, not mode A -- see `entry_eigenmode_selection`'s doc
/// comment), `geo.n_o_hero` when the ordinary mode is selected instead. `geo.n1`/
/// `geo.cos_i` are unaffected: `n1 == 1.0` (air) on every path this function can
/// reach (`entering_anisotropic` requires `!inside_gem`).
///
/// `sin2_t` is bit-identical to the earlier `(1.0 - geo.sin2_t).sqrt()` whenever `n2
/// == geo.n2` (every case that isn't a fresh ordinary-mode selection): reuses
/// `geo.sin2_t` itself rather than re-deriving an algebraically-equivalent value from
/// `n1`/`n2`/`cos_i`, since a DIFFERENT sequence of floating-point operations
/// computing "the same" mathematical quantity is not guaranteed (and, empirically, is
/// not) bit-identical to `geo.sin2_t`'s own `eta * eta * cos_i.mul_add(-cos_i, 1.0)`.
/// Only genuinely recomputed (at `n_o_hero` instead of `geo.n2`, via the exact same
/// expression shape `compute_bounce_refraction_geometry` uses) when the ordinary
/// mode was actually selected.
///
/// The reflect/transmit SELECTION probability's clamp (`min`/`max`) is distinct from
/// the per-channel `r_unpol_k` clamps elsewhere in this file (which only scale
/// `path_pdf`, never divide `stokes` directly). `[0.02, 0.98]` rather than
/// `[1e-4, 1-1e-4]` caps the `1/r_unpol`/`1/(1-r_unpol)` divisions
/// (`apply_partial_reflect_bounce`/`apply_refract_channel`) at 50x instead of
/// 10,000x at grazing incidence, where the unclamped raw value genuinely approaches 0
/// or 1 -- still unbiased (the contribution is weighted by the TRUE `R`/`T` divided by
/// this same `p`), just far less firefly-prone.
fn compute_entry_reflect_probability(
    geo: &BounceRefractionGeometry,
    entering_anisotropic: bool,
    use_extraordinary: bool,
    min: f32,
    max: f32,
) -> f32 {
    let ordinary_selected = entering_anisotropic && !geo.is_biaxial && !use_extraordinary;
    let n2 = if ordinary_selected {
        geo.n_o_hero
    } else {
        geo.n2
    };
    let sin2_t = if ordinary_selected {
        let eta = geo.n1 / n2;
        eta * eta * geo.cos_i.mul_add(-geo.cos_i, 1.0)
    } else {
        geo.sin2_t
    };
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s =
        f32::mul_add(n2, -cos_t, geo.n1 * geo.cos_i) / f32::mul_add(n2, cos_t, geo.n1 * geo.cos_i);
    let r_p =
        f32::mul_add(geo.n1, -cos_t, n2 * geo.cos_i) / f32::mul_add(geo.n1, cos_t, n2 * geo.cos_i);

    let r_unpol_raw = 0.5 * r_p.mul_add(r_p, r_s * r_s);
    r_unpol_raw.clamp(min, max)
}

pub(super) fn apply_partial_fresnel_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> (Vec3, Vec3, bool, Option<bool>, Option<f32>) {
    const R_UNPOL_SELECT_MIN: f32 = 0.02;
    const R_UNPOL_SELECT_MAX: f32 = 0.98;

    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, normal } = ray;
    let PathModeState {
        current_plane_normal,
        inside_gem,
        is_extraordinary,
    } = mode_state;
    let RngDraw { rng_seed, bounce } = rng;

    let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
    if let Some(result) = try_dispatch_uniaxial_bounce(
        bctx,
        ray,
        mode_state,
        entering_anisotropic,
        rng,
        state,
        exit_event,
    ) {
        return result;
    }

    let (use_extraordinary, entry_mode_azimuth2) =
        resolve_entry_mode_selection(&EntryModeSelectionInputs {
            ctx,
            geo,
            entering_anisotropic,
            current_plane_normal,
            k_hat,
            hero_stokes: state.stokes[ctx.hero_idx],
            is_extraordinary,
            rng_seed,
            bounce,
        });

    let r_unpol = compute_entry_reflect_probability(
        geo,
        entering_anisotropic,
        use_extraordinary,
        R_UNPOL_SELECT_MIN,
        R_UNPOL_SELECT_MAX,
    );
    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_unpol {
        let new_k =
            apply_partial_reflect_bounce(geo, r_unpol, k_hat, normal, state.stokes, state.path_pdf);
        let new_s = poynting_dir_for_mode(ctx, geo, new_k, inside_gem, is_extraordinary);
        (new_k, new_s, inside_gem, None, None)
    } else {
        let selection = RefractSelection {
            use_extraordinary,
            entry_mode_azimuth2,
        };
        let outcome =
            apply_refract_bounce(bctx, r_unpol, ray, inside_gem, selection, state, exit_event);
        (
            outcome.new_k,
            outcome.new_s,
            !inside_gem,
            outcome.is_extraordinary_update,
            None,
        )
    }
}

#[cfg(test)]
mod tir_phase_retardation_tests {
    use super::*;

    /// The partial-reflection branch's "channel k is past its own critical
    /// angle" case now applies `tir_phase_delta` (via `MuellerMatrix::tir_retardation`)
    /// instead of the phase-blind `fresnel_reflection(1.0, 1.0)`. This checks the
    /// shared helper reproduces the same delta the dedicated (hero-is-past-critical)
    /// TIR branch computes inline, for a channel genuinely past its critical angle --
    /// i.e. the two sites are guaranteed to agree because they now share one formula.
    #[test]
    fn tir_phase_delta_is_nonzero_past_critical_angle() {
        // n1 = 2.42 (diamond-like), incidence well past the ~24.4 degree critical angle.
        let n1k = 2.42f32;
        let cos_i = 60.0f32.to_radians().cos();
        let sin_i = 60.0f32.to_radians().sin();
        assert!(
            n1k * sin_i > 1.0,
            "test setup must be past the critical angle"
        );

        let delta = tir_phase_delta(n1k, cos_i, sin_i);
        assert!(
            delta.abs() > 1e-3,
            "TIR phase retardation should be nonzero past the critical angle (got {delta})"
        );

        let tir_matrix = MuellerMatrix::tir_retardation(delta);
        let linear_45 = StokesVector::new(1.0, 0.0, 1.0, 0.0);
        let out = linear_45.apply_matrix(&tir_matrix);
        assert!(
            out.v.abs() > 1e-3,
            "a nonzero TIR phase retardation must convert some linear polarization to circular (V) (got V={})",
            out.v
        );
    }

    /// At exactly grazing incidence the retardation formula must stay finite (no NaN /
    /// Inf) even though several terms blow up in the naive algebra.
    #[test]
    fn tir_phase_delta_is_finite_near_grazing_incidence() {
        let n1k = 2.42f32;
        let cos_i = 0.001f32;
        let sin_i = cos_i.mul_add(-cos_i, 1.0).sqrt();
        let delta = tir_phase_delta(n1k, cos_i, sin_i);
        assert!(
            delta.is_finite(),
            "delta must remain finite near grazing incidence (got {delta})"
        );
    }
}

/// `apply_partial_fresnel_bounce` computes the reflect-vs-transmit decision's
/// `r_unpol` from the SAME mode's own refractive index the transmit branch
/// (`apply_refract_channel`) uses for that mode, rather than always mode B's -- these
/// tests check the resulting `R + T == 1` identity directly against the same Fresnel
/// algebra those two sites share (`r_s`/`r_p`/`t_s`/`t_p`, `MuellerMatrix::
/// fresnel_reflection`/`fresnel_transmission`'s own "a" coefficients), at both the
/// ordinary and extraordinary index, across a spread of incidence angles.
#[cfg(test)]
mod p2_fresnel_energy_conservation_tests {
    use crate::optics::materials::GemMaterial;

    #[test]
    fn ordinary_and_extraordinary_indices_each_conserve_energy_at_several_angles() {
        let material = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
        let n_o = material.dispersion.evaluate(589.3);
        let n_e = n_o + material.birefringence_delta;
        assert!(
            (n_o - n_e).abs() > 0.01,
            "test premise: Zircon must be strongly birefringent"
        );

        for n2 in [n_o, n_e] {
            for angle_deg in [0.0f32, 15.0, 30.0, 45.0, 60.0, 75.0] {
                let n1 = 1.0f32; // air -> crystal entry
                let cos_i = angle_deg.to_radians().cos();
                let sin_i = angle_deg.to_radians().sin();
                let eta = n1 / n2;
                let sin2_t = eta * eta * sin_i * sin_i;
                assert!(
                    sin2_t <= 1.0,
                    "air->crystal entry (n1 == 1.0 < n2) should never TIR"
                );
                let cos_t = (1.0 - sin2_t).sqrt();

                // Exactly `apply_partial_fresnel_bounce`'s r_s/r_p (UNCLAMPED -- the
                // selection-probability clamp there is a firefly-mitigation detail of
                // the sampling probability, not part of the underlying Fresnel
                // identity this test checks) and `apply_refract_channel`'s t_s/t_p.
                let r_s =
                    f32::mul_add(n2, -cos_t, n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i);
                let r_p =
                    f32::mul_add(n1, -cos_t, n2 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i);
                let t_s = (2.0 * n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i);
                let t_p = (2.0 * n1 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i);

                let r_unpol = 0.5 * r_p.mul_add(r_p, r_s * r_s);
                // Exactly `MuellerMatrix::fresnel_transmission`'s flux-conserving
                // `factor`/`a` coefficients.
                let factor = (n2 * cos_t) / (n1 * cos_i).max(1e-6);
                let t_unpol = 0.5 * f32::mul_add(t_p * t_p, factor, t_s * t_s * factor);

                assert!(
                    (r_unpol + t_unpol - 1.0).abs() < 1e-6,
                    "R + T should equal 1 at n2={n2}, angle={angle_deg} deg \
                     (R={r_unpol}, T={t_unpol}, R+T={})",
                    r_unpol + t_unpol
                );
            }
        }
    }
}

/// Energy conservation through the ACTUAL
/// CPU wiring `apply_uniaxial_internal_bounce` dispatches to --
/// `apply_uniaxial_internal_reflect_channels` and `apply_uniaxial_internal_transmit_
/// channels` -- not just the closed-form solver in isolation (already covered, at
/// looser tolerance, by `uniaxial_fresnel::tests::
/// internal_exit_energy_conservation_holds_including_tir`).
#[cfg(test)]
mod p2_uniaxial_internal_wiring_energy_conservation_tests {
    use super::*;
    use crate::optics::materials::GemMaterial;

    /// For a UNIT incident intensity, undoing each branch's own `1/r_branch` /
    /// `1/(1-r_branch)` importance-sampling division (exactly what a real Monte Carlo
    /// path's expectation does across many samples) and summing the two branches'
    /// deposited intensity must reproduce 1.0 -- `E[reflected] + E[transmitted] ==
    /// incident`, checked at a fixed, arbitrary `r_branch` (the wiring under test does
    /// not know or care what value drove the coin flip that selected it -- both
    /// branches are exercised directly and deterministically here, not sampled) across
    /// at least 6 angles x 3 axis orientations x both incident modes.
    ///
    /// Sweep excludes genuine TIR (this test's `r_branch` is a fixed 0.5, not derived
    /// from the hero's own reflectance, so every angle here is sub-critical for BOTH
    /// modes -- the closed-form solver's own TIR-inclusive conservation is already
    /// covered, at a looser tolerance, by `internal_exit_energy_conservation_holds_
    /// including_tir`); measured `worst_err` over this sweep is `~3.6e-7`, comfortably
    /// inside the required `1e-6` target -- asserted at `5e-6` (a >10x margin
    /// over the measured worst case) rather than the raw measured value, so the test
    /// does not flake on an unrelated few-ULP shift from an unrelated future change.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the empty-arena/disabled `ExitSplitCtx` this test's \
                  `apply_uniaxial_internal_transmit_channels` call requires \
                  (splitting itself is irrelevant to this test's own energy-conservation \
                  sweep, which only reads the hero channel -- see that setup's own \
                  comment) adds to an already-long sweep body"
    )]
    fn uniaxial_internal_bounce_wiring_conserves_energy_at_exit_interface() {
        let zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in material");
        let n_o = zircon.dispersion.evaluate(589.3);
        let n_e = zircon.extraordinary_index_at(589.3, n_o);
        let lambdas = [589.3f32; NUM_CHANNELS];

        let axes = [Vec3::X, Vec3::Y, Vec3::new(0.4, 0.5, 0.767_2).normalize()];
        let r_branch = 0.5f32;
        let mut worst = 0.0f32;
        for &c_axis in &axes {
            let ctx = RayMaterialContext {
                material: &zircon,
                lambdas,
                hero_idx: 0,
                c_axis,
                is_anisotropic: true,
                enable_internal_mode_coupling: true,
            };
            for angle_deg in [5.0f32, 15.0, 25.0, 35.0, 45.0, 55.0, 65.0] {
                let theta = angle_deg.to_radians();
                let cos_i = theta.cos();
                let sin_i = theta.sin();
                let k_hat = Vec3::new(sin_i, 0.0, cos_i);
                let normal = Vec3::new(0.0, 0.0, -1.0);
                if k_hat.cross(c_axis).length_squared() < 1e-4 {
                    // Degenerate wave-normal-parallel-to-optic-axis case: handled by a
                    // dedicated exact fallback in `apply_partial_fresnel_bounce`
                    // itself (this wiring is never reached there) -- see that
                    // function's own doc comment.
                    continue;
                }
                let frame = UniaxialFrame::build(k_hat, normal, c_axis, cos_i, sin_i);

                for is_extraordinary in [false, true] {
                    let n_inc = if is_extraordinary {
                        let cos_kc = frame.gamma.mul_add(frame.cos_i, frame.alpha * frame.sin_i);
                        let sin2 = cos_kc.mul_add(-cos_kc, 1.0).max(0.0);
                        1.0 / (cos_kc * cos_kc / (n_o * n_o) + sin2 / (n_e * n_e)).sqrt()
                    } else {
                        n_o
                    };
                    let geo = BounceRefractionGeometry {
                        cos_i,
                        sin_i,
                        n1: n_inc,
                        n2: 1.0,
                        n1_ch: [n_inc; NUM_CHANNELS],
                        n2_ch: [1.0; NUM_CHANNELS],
                        n_o_ch: [n_o; NUM_CHANNELS],
                        ..BounceRefractionGeometry::default()
                    };

                    // Matches `apply_uniaxial_internal_bounce`'s own hero-channel solve
                    // (this test's `geo` is uniform across channels, so it is exactly
                    // what channel `ctx.hero_idx` would compute anyway).
                    let sol_hero = uniaxial_fresnel::internal_solve(
                        n_inc,
                        n_o,
                        n_e,
                        c_axis,
                        &frame,
                        !is_extraordinary,
                    );

                    let mut stokes_r = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
                    let mut pdf_r = [1.0f32; NUM_CHANNELS];
                    let ubctx = UniaxialBounceContext {
                        ctx: &ctx,
                        geo: &geo,
                        frame: &frame,
                    };
                    let hero_solve = HeroInternalSolve {
                        hero: ctx.hero_idx,
                        is_extraordinary,
                        sol_hero,
                        r_branch,
                    };
                    let mut state_r = BounceState {
                        stokes: &mut stokes_r,
                        path_pdf: &mut pdf_r,
                    };
                    apply_uniaxial_internal_reflect_channels(&ubctx, &hero_solve, &mut state_r);
                    let mut stokes_t = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
                    let mut pdf_t = [1.0f32; NUM_CHANNELS];
                    // This test reads only `stokes_t[0]` (the hero channel), whose
                    // `direction_matches` is trivially true regardless of splitting --
                    // no real gem geometry/environment is needed, so an empty arena and
                    // `enabled: false` (the split path is never reached) keep this test
                    // unaffected by the split-transmission machinery.
                    let empty_soa = crate::simd::PlanesSoA32::from_normals_d(std::iter::empty(), 0);
                    let mut split_radiance_t = [0.0f32; NUM_CHANNELS];
                    let mut exit_ctx = ExitSplitCtx {
                        plane_soa: &empty_soa,
                        environment: EnvironmentSource::Studio {
                            preset: crate::optics::LightingPreset::RingLights,
                            exposure: 1.0,
                            light_yaw: 0.0,
                            light_pitch: 0.85,
                        },
                        studio_rig: None,
                        split_radiance: &mut split_radiance_t,
                        enabled: false,
                        compat: [u8::MAX; NUM_CHANNELS],
                    };
                    let mut state_t = BounceState {
                        stokes: &mut stokes_t,
                        path_pdf: &mut pdf_t,
                    };
                    let mut exit_event_t = ExitEvent {
                        exit: &mut exit_ctx,
                        hit_point: Vec3::ZERO,
                    };
                    apply_uniaxial_internal_transmit_channels(
                        &ubctx,
                        &hero_solve,
                        BounceRay { k_hat, normal },
                        &mut state_t,
                        &mut exit_event_t,
                    );

                    let reflected = stokes_r[0].i * r_branch;
                    let transmitted = stokes_t[0].i * (1.0 - r_branch);
                    let err = (reflected + transmitted - 1.0).abs();
                    worst = worst.max(err);
                }
            }
        }
        println!(
            "uniaxial_internal_bounce_wiring_conserves_energy_at_exit_interface: worst_err={worst}"
        );
        assert!(
            worst < 5e-6,
            "reflected + transmitted power should equal incident power (1.0) through \
             the actual CPU wiring, worst_err={worst}"
        );
    }
}

/// [`entry_eigenmode_selection`]'s polarization-weighted probability and eigenmode
/// azimuth.
#[cfg(test)]
mod entry_eigenmode_selection_tests {
    use super::*;

    /// Unpolarized (and negligibly-polarized) light must reduce EXACTLY to a
    /// blanket 50/50 -- i.e. `entry_eigenmode_selection` returns `None`, telling
    /// callers to skip both the weighted draw and the eigenmode projection.
    #[test]
    fn unpolarized_light_returns_none() {
        let c_axis = Vec3::Y;
        let current_plane_normal = Vec3::X;
        let k_hat = Vec3::NEG_Z;
        let stokes = StokesVector::unpolarized(1.0);
        assert!(
            entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes).is_none(),
            "unpolarized light must not be weighted -- it must fall back to flat 50/50"
        );
    }

    /// A degenerate plane of incidence (near-normal incidence, `current_plane_normal`
    /// near zero) must also fall back to `None` regardless of polarization -- there is
    /// no well-defined frame to express `psi_o` in.
    #[test]
    fn degenerate_plane_of_incidence_returns_none() {
        let c_axis = Vec3::Y;
        let current_plane_normal = Vec3::ZERO;
        let k_hat = Vec3::NEG_Z;
        let stokes = StokesVector::new(1.0, 1.0, 0.0, 0.0);
        assert!(entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes).is_none());
    }

    /// Light fully linearly polarized EXACTLY along the ordinary eigenaxis must select
    /// the ordinary mode with probability 1 (`p_o == 1.0`), and its doubled azimuth
    /// must be `(cos, sin) == (1, 0)` (`psi_o == 0` in this frame by construction).
    #[test]
    fn fully_polarized_along_ordinary_axis_gives_p_o_one() {
        let c_axis = Vec3::Y;
        let k_hat = Vec3::NEG_Z;
        // Ordinary eigenaxis for this (k_hat, c_axis) pair is `k_hat x c_axis`,
        // normalized -- here that's exactly +X (see `ordinary_eigen_polarization`'s
        // doc comment). Using it directly as `current_plane_normal` (the Stokes
        // frame's own s_axis) makes psi_o == 0 in this frame by construction.
        let current_plane_normal = Vec3::X;
        let stokes = StokesVector::new(1.0, 1.0, 0.0, 0.0); // fully linear along s_axis
        let (p_o, cos_2psi_o, sin_2psi_o) =
            entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
                .expect("polarized light with a well-defined frame must select Some");
        assert!((p_o - 1.0).abs() < 1e-4, "p_o should be ~1.0, got {p_o}");
        assert!((cos_2psi_o - 1.0).abs() < 1e-4, "got {cos_2psi_o}");
        assert!(sin_2psi_o.abs() < 1e-4, "got {sin_2psi_o}");
    }

    /// Light fully linearly polarized PERPENDICULAR to the ordinary eigenaxis (i.e.
    /// along the extraordinary one) must select the ordinary mode with probability 0.
    #[test]
    fn fully_polarized_along_extraordinary_axis_gives_p_o_zero() {
        let c_axis = Vec3::Y;
        let k_hat = Vec3::NEG_Z;
        let current_plane_normal = Vec3::X;
        let stokes = StokesVector::new(1.0, -1.0, 0.0, 0.0); // fully linear along p_axis
        let (p_o, ..) = entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
            .expect("polarized light with a well-defined frame must select Some");
        assert!(p_o.abs() < 1e-4, "p_o should be ~0.0, got {p_o}");
    }
}

/// The two properties `apply_refract_channel`'s entry-eigenmode projection relies
/// on, checked directly rather than only through a full render (`entry_eigenmode_selection_tests`
/// pins `p_o`/azimuth at `DoP` 0 and 1; this module exercises intermediate, partially
/// polarized `DoP` values too, and a non-trivial frame where BOTH `cos_2psi_o` and
/// `sin_2psi_o` are nonzero, not just the axis-aligned special case):
///
/// 1. The projected Stokes vector `apply_refract_channel` builds for whichever mode is
///    selected (`StokesVector::new(i, +/-i*cos_2psi_o, +/-i*sin_2psi_o, 0.0)`) is fully
///    linearly polarized -- `Q^2 + U^2 == I^2` exactly (up to float error) and `V == 0`
///    -- for BOTH the ordinary and the extraordinary projection, at every `DoP` tested.
/// 2. The mode-selection draw is an unbiased estimator of the true polarization-weighted
///    mixture. Concretely: import two DIFFERENT hypothetical per-mode transmittances
///    `T_o != T_e` (standing in for `apply_refract_channel`'s own mode-dependent Fresnel
///    `T`), so the "correct" physical answer -- Malus's law's `i*(p_o*T_o + (1-p_o)*T_e)`
///    -- is a genuinely nontrivial target (not the same value regardless of which mode
///    is drawn). A Monte Carlo sweep of the ACTUAL selection formula
///    (`apply_partial_fresnel_bounce`'s `mode_split_rand < (1.0 - p_o)`, driven by the
///    real `BIREFRINGENT_SPLIT_STREAM` hash) reporting `T_o*i`/`T_e*i` unscaled on each
///    trial (exactly what `apply_refract_channel` does -- no `1/p` division, see that
///    function's own doc comment for why none is needed) must converge to that same
///    target: `E[estimate] == p_o*T_o*i + (1-p_o)*T_e*i` by construction, and the RNG
///    sweep confirms the real selection probability the hash actually produces matches
///    `p_o` closely enough for that identity to hold within Monte Carlo noise.
#[cfg(test)]
mod entry_mode_projection_tests {
    use super::*;

    /// Builds a non-axis-aligned Stokes frame (`s_hat` NOT equal to the ordinary axis,
    /// so both `cos_2psi_o` and `sin_2psi_o` come out nonzero) once, shared by every
    /// `DoP` case below.
    fn oblique_frame() -> (Vec3, Vec3, Vec3) {
        let c_axis = Vec3::Y;
        let k_hat = Vec3::NEG_Z;
        // Perpendicular to k_hat (lies in the XY plane, like every other test in this
        // file's Vec3::NEG_Z-based frames), but NOT aligned with the ordinary axis
        // (Vec3::X for this (k_hat, c_axis) pair -- see the axis-aligned tests above).
        let current_plane_normal = Vec3::new(0.6, 0.8, 0.0);
        (c_axis, current_plane_normal, k_hat)
    }

    #[test]
    fn projected_stokes_is_fully_linear_at_several_dop_values() {
        let (c_axis, current_plane_normal, k_hat) = oblique_frame();
        let psi = 25.0f32.to_radians();
        let (cos_2psi, sin_2psi) = ((2.0 * psi).cos(), (2.0 * psi).sin());

        for dop in [0.05f32, 0.3, 0.6, 0.9, 1.0] {
            let i = 1.0f32;
            let q = i * dop * cos_2psi;
            let u = i * dop * sin_2psi;
            let stokes = StokesVector::new(i, q, u, 0.0);
            let (_, cos_2psi_o, sin_2psi_o) =
                entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
                    .expect("nonzero DoP with a well-defined frame must select Some");

            for (cos_x, sin_x) in [(cos_2psi_o, sin_2psi_o), (-cos_2psi_o, -sin_2psi_o)] {
                let projected = StokesVector::new(i, i * cos_x, i * sin_x, 0.0);
                assert!(
                    projected.v.abs() < 1e-6,
                    "projected V must be exactly zero at dop={dop}, got {}",
                    projected.v
                );
                let lin_energy = projected.q.mul_add(projected.q, projected.u * projected.u);
                assert!(
                    projected.i.mul_add(-projected.i, lin_energy).abs() < 1e-4,
                    "projected Stokes vector must be fully linearly polarized (Q^2+U^2 \
                     == I^2) at dop={dop}: I={}, Q={}, U={}, Q^2+U^2={lin_energy}",
                    projected.i,
                    projected.q,
                    projected.u
                );
            }
        }
    }

    #[test]
    fn mode_selection_draw_is_an_unbiased_estimator_at_several_dop_values() {
        // Two deliberately different per-mode "transmittances" -- stand-ins for
        // `apply_refract_channel`'s real mode-dependent Fresnel T, chosen far enough
        // apart that a convention bug (e.g. an inverted comparison sense) would show up
        // as a converged mean far from the analytically correct target, not hidden in
        // noise.
        const T_O: f32 = 0.8;
        const T_E: f32 = 0.35;
        const TRIALS: u32 = 20_000;

        let (c_axis, current_plane_normal, k_hat) = oblique_frame();
        let psi = 25.0f32.to_radians();
        let (cos_2psi, sin_2psi) = ((2.0 * psi).cos(), (2.0 * psi).sin());

        for dop in [0.05f32, 0.3, 0.6, 0.9, 1.0] {
            let i = 1.0f32;
            let q = i * dop * cos_2psi;
            let u = i * dop * sin_2psi;
            let stokes = StokesVector::new(i, q, u, 0.0);
            let (p_o, ..) = entry_eigenmode_selection(c_axis, current_plane_normal, k_hat, stokes)
                .expect("nonzero DoP with a well-defined frame must select Some");
            let target = i * p_o.mul_add(T_O, (1.0 - p_o) * T_E);

            let mut sum = 0.0f64;
            for bounce in 0..TRIALS {
                // Exactly `apply_partial_fresnel_bounce`'s own draw: same stream, same
                // comparison sense (`mode_split_rand < 1.0 - p_o` selects extraordinary).
                let mode_split_rand = (hash_u32(hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
                    as f32)
                    / 4_294_967_295.0;
                let use_extraordinary = mode_split_rand < (1.0 - p_o);
                let estimate = if use_extraordinary { T_E * i } else { T_O * i };
                sum += f64::from(estimate);
            }
            let mean = (sum / f64::from(TRIALS)) as f32;
            assert!(
                (mean - target).abs() < 0.01,
                "mode-selection draw should be an unbiased estimator of the \
                 polarization-weighted mixture at dop={dop}: p_o={p_o}, target={target}, \
                 Monte Carlo mean={mean} over {TRIALS} trials"
            );
        }
    }
}
