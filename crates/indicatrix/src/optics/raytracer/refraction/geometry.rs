//! Per-bounce refractive-index/incidence-angle geometry: [`BounceRefractionGeometry`]
//! and the functions that build it from a bounce's wave normal, facet normal, and
//! cached per-sample wavelength data.

use super::context::{RayMaterialContext, RayWavelengthCache};
use crate::optics::{
    birefringence::{BiaxialIndicatrix, BirefringenceParams},
    raytracer::{NUM_CHANNELS, uniaxial_fresnel::UniaxialFrame},
};
use glam::Vec3;

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
        // rather than a constant-offset-only `n_o_hero_seed +
        // material.birefringence_delta`.
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

pub(in crate::optics::raytracer) fn compute_bounce_refraction_geometry(
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
