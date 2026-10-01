//! Per-ray/per-sample/per-bounce context types shared by every bounce-dispatch function
//! in this module tree.
//!
//! Plus [`narrow_compat`], the MIS-family narrowing step those functions share at an
//! interior dispersive event.

use super::{DIRECTION_MATCH_COS_TOL, geometry::BounceRefractionGeometry};
use crate::optics::{
    birefringence::{AbsorptionTensor3, BiaxialIndicatrix},
    materials::GemMaterial,
    polarization::StokesVector,
    raytracer::{NUM_CHANNELS, environment::EnvironmentSource, uniaxial_fresnel::UniaxialFrame},
    studio_rig::StudioRig,
};
use glam::Vec3;

/// Per-ray context that stays fixed across every bounce of `trace_spectral_ray`'s main
/// loop: the material, the ray's 8 hero-wavelength comb, which slot drives the shared
/// geometric path, the optical c-axis, and whether this material is anisotropic at
/// all. Bundled into one struct to keep [`super::geometry::compute_bounce_refraction_geometry`]'s
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
/// via [`super::wavelength_cache::build_ray_wavelength_cache`] and read back every bounce
/// instead of recomputed. Kept as its own struct rather than new fields on
/// [`RayMaterialContext`]: that struct is built via a bare struct literal at several
/// `renderer::gpu::transport_check` Tier 2 GPU self-test call sites that must keep
/// compiling unchanged, and it cannot derive `Default` (its `&'a GemMaterial` field
/// isn't `Default`).
pub(crate) struct RayWavelengthCache {
    /// `material.dispersion.evaluate(lambdas[k])` per channel. See
    /// [`super::geometry::per_channel_uniaxial_indices`] for the per-bounce `n_eff_ch`
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

/// Per-trace context for exit-event spectral splitting -- see `refraction`'s top-of-file
/// "Exit-event spectral splitting" doc comment for the estimator this supports. The
/// fixed parts (`plane_soa`, `environment`, `studio_rig`, `enabled`) are bundled the
/// same way [`RayMaterialContext`]/[`RayWavelengthCache`] are; the two per-trace
/// accumulators (`split_radiance`, `compat`) are threaded as `&mut` through every
/// bounce-dispatch function exactly like `stokes`/`path_pdf` are. `radiance` itself
/// stays outside (threaded separately, as it always was).
pub(in crate::optics::raytracer) struct ExitSplitCtx<'a> {
    /// The same intersection arena `trace_spectral_ray_inner`'s own bounce loop already
    /// built once per trace -- reused here for [`super::exit_split::try_split_exit_channel`]'s
    /// bounded "does channel k's own exit ray re-enter the gem" probe, never rebuilt.
    pub(in crate::optics::raytracer) plane_soa: &'a crate::simd::PlanesSoA32,
    pub(in crate::optics::raytracer) environment: EnvironmentSource<'a>,
    /// Built once per trace by `trace_spectral_ray_inner` and shared: the exit-split
    /// probes here and `accumulate_miss_radiance`'s escape lookup both borrow this one
    /// instance, since the rig depends only on the light pose, which is constant across
    /// an entire ray. `None` for [`EnvironmentSource::HdrMap`], which
    /// `sample_environment_channel` ignores.
    pub(in crate::optics::raytracer) studio_rig: Option<StudioRig>,
    /// Unit direction from the stone towards the eye -- the reverse of the pixel's
    /// primary ray -- for the lit lighting models' head shadow; see
    /// `environment::sample_studio_environment_observed`.
    pub(in crate::optics::raytracer) observer: Vec3,
    /// A staging accumulator, separate from `trace_spectral_ray_inner`'s own
    /// `radiance`: every split channel's contribution lands here first, and
    /// `trace_spectral_ray_inner` folds it into `radiance` only if the shared/hero path
    /// itself ultimately terminates via [`PathTermination::Escaped`]. This mirrors the
    /// all-or-nothing way `accumulate_miss_radiance` already works for a plain
    /// matching channel: if the shared path instead terminates via Russian roulette,
    /// scatter absorption, or `max_bounces`, `accumulate_miss_radiance` is never called
    /// and every channel's `radiance` stays `0.0` regardless of how many exit events it
    /// survived. Without this staging + conditional-commit step, a split channel would
    /// keep contributions in exactly the cases a plain matching channel would have lost
    /// them -- an asymmetry `transport::exit_splitting_tests` caught empirically
    /// (splitting-on/off z-scores growing rather than shrinking with sample count).
    pub(in crate::optics::raytracer) split_radiance: &'a mut [f32; NUM_CHANNELS],
    /// `false` reproduces pre-splitting chromatic-termination behaviour bit-for-bit at
    /// every exit event; `true` (the production default from every public entry point)
    /// enables splitting. Mirrors `RayMaterialContext`'s own
    /// `enable_internal_mode_coupling` precedent: threaded only as far as
    /// `trace_spectral_ray_inner`'s own parameter, never exposed publicly.
    pub(in crate::optics::raytracer) enabled: bool,
    /// Pairwise spectral-compatibility mask -- bit `j` of `compat[c]` is set while
    /// channels `c` and `j` have refracted within `DIRECTION_MATCH_COS_TOL` of each
    /// other at every interior dispersive event so far (all bits set before the first
    /// one; `compat[c]` always keeps its own bit). `compat[c]` is channel c's MIS
    /// *family*: the set of techniques under which channel c would have stayed alive on
    /// this same geometric path, and therefore the set its balance-heuristic weight
    /// must be normalised over -- see this module's top-of-file doc comment. Narrowed
    /// by [`narrow_compat`]; read by `color::integrate_channels_to_xyz_families`. Only
    /// meaningful while `enabled`.
    pub(in crate::optics::raytracer) compat: [u8; NUM_CHANNELS],
    /// The balance-heuristic MIS weight [`super::exit_split::try_split_exit_channel`]
    /// must apply to every channel it adds into `split_radiance` THIS bounce -- `1.0`
    /// (the default outside a transmit-out event) reproduces the pre-MIS-weighted
    /// behaviour exactly. Set by `transport::bounce::dispatch_bounce` right before it
    /// dispatches a bounce that MIGHT turn out to be a transmit-out-of-gem event with a
    /// live incoming NEE carry (`pre_bounce_inside_gem && incoming_light_mis.is_some()`,
    /// mirroring the hero's own escape weight at `transport::inner`'s
    /// `phase_pdf_for_mis_this_check` site: `balance_heuristic(phase_pdf,
    /// environment_nee_pdf(environment, interior_dir))`), and reset to `1.0` right after
    /// that dispatch returns. A dispatch that turns out to reflect (or to transmit
    /// without a live carry) never reads this field for a nonzero contribution, since
    /// `try_split_exit_channel` is only reachable from the transmit/chromatic-termination
    /// branch in the first place -- so setting it speculatively before the branch
    /// decision is made is harmless.
    pub(in crate::optics::raytracer) split_mis_weight: f32,
}

const _: () = assert!(
    NUM_CHANNELS <= 8,
    "ExitSplitCtx::compat packs one bit per channel into a u8"
);

/// Bundles the three read-only context references (fixed per trace, per sample, and
/// per bounce respectively) every isotropic/biaxial bounce-dispatch function in this
/// module needs, instead of each of those functions taking the flat `ctx, cache, geo`
/// parameter triple individually. Purely a signature-level grouping: every
/// field is the exact same reference the caller already had.
pub(in crate::optics::raytracer) struct BounceContext<'m, 'b> {
    pub(in crate::optics::raytracer) ctx: &'b RayMaterialContext<'m>,
    pub(in crate::optics::raytracer) cache: &'b RayWavelengthCache,
    pub(in crate::optics::raytracer) geo: &'b BounceRefractionGeometry,
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
pub(in crate::optics::raytracer) struct BounceRay {
    pub(in crate::optics::raytracer) k_hat: Vec3,
    pub(in crate::optics::raytracer) normal: Vec3,
}

/// The mutable per-channel Stokes/path-pdf accumulators every bounce-dispatch function
/// reads and writes -- bundled so a `&mut` of this one struct replaces threading both
/// arrays as separate parameters. Always passed as `&mut BounceState<'_>` (never by
/// value) so a caller can reborrow it across a per-channel loop's repeated calls.
pub(in crate::optics::raytracer) struct BounceState<'a> {
    pub(in crate::optics::raytracer) stokes: &'a mut [StokesVector; NUM_CHANNELS],
    pub(in crate::optics::raytracer) path_pdf: &'a mut [f32; NUM_CHANNELS],
}

/// The per-bounce exit-event pair: [`ExitSplitCtx`]'s per-trace accumulator plus this
/// bounce's own hit point, needed together by [`super::exit_split::try_split_exit_channel`]'s
/// bounded re-entry probe. Always passed as `&mut ExitEvent<'_, '_>` for the same
/// reborrowing reason as [`BounceState`].
pub(in crate::optics::raytracer) struct ExitEvent<'a, 'b> {
    pub(in crate::optics::raytracer) exit: &'a mut ExitSplitCtx<'b>,
    pub(in crate::optics::raytracer) hit_point: Vec3,
}

/// This bounce's RNG identity -- the same `(rng_seed, bounce)` pair every stochastic
/// branch decision in the bounce loop hashes against its own stream salt.
#[derive(Clone, Copy)]
pub(in crate::optics::raytracer) struct RngDraw {
    pub(in crate::optics::raytracer) rng_seed: u32,
    pub(in crate::optics::raytracer) bounce: u32,
}

/// [`super::dispatch::apply_partial_fresnel_bounce`]'s own per-bounce path-mode state:
/// the current Stokes plane-of-incidence frame plus the two mode flags (`inside_gem`,
/// `is_extraordinary`) that decide which branch every downstream helper takes.
#[derive(Clone, Copy)]
pub(in crate::optics::raytracer) struct PathModeState {
    pub(in crate::optics::raytracer) current_plane_normal: Vec3,
    pub(in crate::optics::raytracer) inside_gem: bool,
    pub(in crate::optics::raytracer) is_extraordinary: bool,
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
