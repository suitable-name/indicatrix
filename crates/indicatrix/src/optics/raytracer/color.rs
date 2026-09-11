//! Spectral-to-tristimulus colour conversion.
//!
//! The CIE 1931 colour-matching functions, the spectral-MIS combination weight
//! applied at XYZ integration, von Kries white balance, and the final XYZ -> sRGB
//! gamut/gamma mapping.

use super::{
    NUM_CHANNELS,
    environment::{LightingPreset, blackbody_spectrum},
};
use glam::Vec3;

/// Wyman, Sloan, Shirley (2013) multi-lobe analytic fit to the CIE 1931 2° Standard
/// Observer Color Matching Functions.
///
/// Delegates to [`crate::color::cie1931::cie_1931_cmf`] (the single source of truth for
/// the fit's constants); a thin `Vec3`-returning wrapper since the raytracer's hot paths
/// want a `glam::Vec3` rather than `[f32; 3]`.
#[must_use]
pub fn cie_1931_cmf(lambda_nm: f32) -> Vec3 {
    Vec3::from_array(crate::color::cie1931::cie_1931_cmf(lambda_nm))
}

/// Batched, `Vec3`-returning wrapper around
/// [`crate::color::cie1931::cie_1931_cmf_x8`] -- see that function's doc comment for
/// the deliberate ULP-level re-baseline versus 8 calls to [`cie_1931_cmf`] (constant-
/// folding the CMF call out of `integrate_channels_to_xyz` measurably cut tracer time,
/// hence this batched path).
#[must_use]
fn cie_1931_cmf_x8(lambdas: &[f32; NUM_CHANNELS]) -> [Vec3; NUM_CHANNELS] {
    crate::color::cie1931::cie_1931_cmf_x8(lambdas).map(Vec3::from_array)
}

/// Identity pass-through for a channel's already-unbiased radiance estimate.
///
/// # Decision record: a per-channel `own_pdf/sum_pdf * N` reweight is a category error
///
/// The 8 spectral channels are N different *integrands*, not N techniques for one
/// integrand. Veach's balance-heuristic estimate for technique `i`'s own integral is
/// `w_i * f_i / (c_i * p_i)`, and after combination `p_i` cancels -- a *different*
/// integrand's pdf never enters the weight. Multiplying `radiance[k]` by `own_pdf /
/// sum_pdf * N` double-counts the wrong channel's pdf as a combination weight; the
/// `two_channel_fresnel_monte_carlo_discriminates_correct_from_biased_weighting` test
/// below shows this biased by roughly +17% on a two-channel Fresnel analogue.
///
/// The correct combination (`spectral_mis_weight`, applied once as a single shared
/// scalar at final XYZ integration) uses `path_pdf[hero_idx]` -- the density of the
/// technique actually sampled -- never a companion channel's own pdf.
#[inline]
const fn mis_weighted_radiance(radiance: f32) -> f32 {
    radiance
}

/// The spectral-MIS balance-heuristic weight `N * p_hero(x) / sum_k p_k(x)`, where
/// `p_k(x)` is `path_pdf[k]` -- channel k's own running density of having produced the
/// exact realized path `x`, tracked incrementally through `trace_spectral_ray`'s
/// bounce loop.
///
/// This is Veach's balance heuristic for the "one-sample MIS" model, combining N
/// stochastic techniques -- "which channel's index drives the shared geometric path" --
/// each chosen with probability `1/N` (the wrapped hero construction makes this
/// uniform-1/N premise hold). Per Veach's proof this weight is valid for any integrand
/// multiplied by it, which is why the same scalar weight is applied uniformly to every
/// channel's radiance, rather than the rejected per-channel `p_k(x)/sum_pdf` form (see
/// `mis_weighted_radiance`).
///
/// A companion channel's `path_pdf[k]` (and its `stokes[k]`/`radiance[k]`) collapses
/// to exactly 0 the moment its own specular refraction direction diverges from the
/// direction the hero-driven path actually took. `sum_pdf` then degenerates toward
/// `path_pdf[hero_idx]` alone, pushing the weight up toward `N` -- concentrating that
/// sample's contribution onto the hero's own colour, the mechanism that produces
/// dispersion "fire" at the image level: different samples have different hero
/// wavelengths, so they concentrate onto different colours.
///
/// `sum_pdf` degenerates to exactly `NUM_CHANNELS * path_pdf[hero_idx]` whenever every
/// channel's technique agrees at every decision (a non-dispersive material), making
/// this collapse to exactly 1.0.
///
/// A chromatically-terminated channel's own radiance must also be zeroed, not merely
/// its `path_pdf` -- a variant that leaves it un-zeroed is measurably biased on the
/// two-channel analogue in
/// `two_channel_dispersive_termination_monte_carlo_is_unbiased_under_alternating_hero`
/// below, even though its `path_pdf` alone is correctly zeroed.
#[inline]
pub(crate) fn spectral_mis_weight(path_pdf: &[f32; 8], hero_idx: usize) -> f32 {
    let sum_pdf: f32 = path_pdf.iter().sum();
    if sum_pdf <= 1e-12 {
        // Should not happen in practice (the hero's own path_pdf factor is always
        // bounded away from 0 by the r_unpol clamps), but fall back to the safe,
        // already-proven-unbiased weight=1 rather than risk a NaN from 0/0.
        return 1.0;
    }
    (path_pdf.len() as f32) * path_pdf[hero_idx] / sum_pdf
}

/// Numerical Integration: Spectral Radiance -> CIE XYZ Tristimulus (normalized by the
/// integral of `y_bar` = 106.856). `radiance[k]` is already an unbiased per-channel
/// estimator on its own -- see [`mis_weighted_radiance`]'s doc comment for why a
/// per-channel reweighting on top would be a category error. A single shared scalar
/// MIS weight, computed from the fully-accumulated `path_pdf`, is layered on top
/// instead -- see [`spectral_mis_weight`]'s doc comment.
// `pub(crate)`, not private: `renderer::gpu::furnace_check`'s CPU-side reference
// estimator assembles itself from the SAME building blocks (`cie_1931_cmf`,
// `spectral_mis_weight`) the GPU port was translated from, rather than re-deriving the
// norm_factor/MIS-weight formula by hand in test code. Pure function, no side effects.
pub(crate) fn integrate_channels_to_xyz(
    radiance: &[f32; NUM_CHANNELS],
    lambdas: &[f32; NUM_CHANNELS],
    path_pdf: &[f32; NUM_CHANNELS],
    hero_idx: usize,
) -> Vec3 {
    let mis_weight = spectral_mis_weight(path_pdf, hero_idx);

    let mut xyz = Vec3::ZERO;
    let norm_factor = (400.0 / NUM_CHANNELS as f32) / 106.856;
    let cmfs = cie_1931_cmf_x8(lambdas);
    for k in 0..NUM_CHANNELS {
        let cmf = cmfs[k];
        let weighted_radiance = mis_weighted_radiance(radiance[k]) * mis_weight;
        xyz += cmf * (weighted_radiance * norm_factor);
    }
    xyz
}

/// The per-channel-family generalisation of [`integrate_channels_to_xyz`] for
/// exit-event spectral splitting. Three points:
///
/// 1. With splitting enabled every channel alive at the exit contributes its own
///    radiance, so each channel's balance-heuristic weight must be normalised over
///    exactly the hero choices under which THAT channel would have been alive on this
///    path -- its family, `compat[k]` (see `refraction::ExitSplitCtx::compat`), which
///    differs channel to channel since the direction tolerance keeping a companion
///    alive is a band around each hero, not one shared clique:
///    `weight_k = N * path_pdf[hero] / sum_{j in compat[k]} path_pdf[j]`.
/// 2. Summed over the heroes in channel k's family these weights add up to exactly 1,
///    which is what makes the combined estimator unbiased.
/// 3. Bias root cause of the naive alternative: a single shared weight normalised over
///    the HERO's family does not have that property once companions contribute
///    (measured +7% for Diamond, +12% for Synthetic Moissanite against a
///    single-wavelength reference).
///
/// `path_pdf` must keep accumulating for a chromatically-terminated channel too (its
/// radiance is zero, but its density still normalises other channels' weights).
/// Reduces to [`integrate_channels_to_xyz`] when every family is the full set.
pub(crate) fn integrate_channels_to_xyz_families(
    radiance: &[f32; NUM_CHANNELS],
    lambdas: &[f32; NUM_CHANNELS],
    path_pdf: &[f32; NUM_CHANNELS],
    hero_idx: usize,
    compat: [u8; NUM_CHANNELS],
) -> Vec3 {
    let mut xyz = Vec3::ZERO;
    let norm_factor = (400.0 / NUM_CHANNELS as f32) / 106.856;
    let cmfs = cie_1931_cmf_x8(lambdas);
    for k in 0..NUM_CHANNELS {
        let family_pdf: f32 = path_pdf
            .iter()
            .enumerate()
            .filter(|&(j, _)| compat[k] & (1u8 << j) != 0)
            .map(|(_, &p)| p)
            .sum();
        // Same "should not happen" fallback as `spectral_mis_weight`.
        let weight_k = if family_pdf <= 1e-12 {
            1.0
        } else {
            (NUM_CHANNELS as f32) * path_pdf[hero_idx] / family_pdf
        };
        let weighted_radiance = mis_weighted_radiance(radiance[k]) * weight_k;
        xyz += cmfs[k] * (weighted_radiance * norm_factor);
    }
    xyz
}

/// Colour temperature (Kelvin) associated with each named lighting preset. A thin
/// pass-through to [`LightingPreset::params`] -- the single source of truth both this
/// and `sample_studio_environment` read from, since the white balance must be derived
/// from the same illuminant that lit the scene.
// `pub(crate)`: the GPU white-balance self-test needs the exact same preset-to-
// temperature mapping `sample_studio_environment` derives its lighting from.
pub(crate) const fn illuminant_temperature_k(lighting_preset: LightingPreset) -> f32 {
    lighting_preset.params().temp_k
}

/// CIE Standard Illuminant D65 chromaticity -- matching
/// `color::space::ColorSpace::Srgb::white_point_xy()` (and every other D65-referenced
/// space this crate defines). This is the reference white
/// [`compute_illuminant_white_balance`] adapts every studio illuminant toward, since
/// `trace_spectral_ray`'s output reaches the screen through an sRGB-family encode step
/// built around this same white point -- adapting to it here is what makes the
/// "renders as neutral" promise true post gamut-mapping, not just in raw XYZ.
const D65_WHITE_X: f32 = 0.3127;
const D65_WHITE_Y: f32 = 0.3290;

/// Bradford chromatic-adaptation cone-response matrix (XYZ -> LMS; Lam 1985), the
/// standard basis proper von Kries adaptation diagonalises in -- used by ICC v4 and
/// most colour-management pipelines. Row-major, same convention as
/// `color::space::ColorSpace::xyz_to_rgb_matrix`. See
/// [`compute_illuminant_white_balance`]'s doc comment for why this basis matters:
/// scaling X and Z directly is not von Kries adaptation, since XYZ tristimulus values
/// are not cone responses -- a diagonal scale there distorts hue under a non-D65
/// illuminant.
const BRADFORD_XYZ_TO_LMS: [[f32; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Exact matrix inverse of [`BRADFORD_XYZ_TO_LMS`] (LMS -> XYZ), the standard published
/// Bradford inverse (cross-checked: `BRADFORD_XYZ_TO_LMS * BRADFORD_LMS_TO_XYZ` is the
/// identity to ~1e-7, comfortably within `f32` rounding).
const BRADFORD_LMS_TO_XYZ: [[f32; 3]; 3] = [
    [0.986_993, -0.147_054, 0.159_963],
    [0.432_305, 0.518_360, 0.049_291],
    [-0.008_529, 0.040_043, 0.968_487],
];

/// Converts CIE XYZ to Bradford-space cone responses (LMS). See
/// [`BRADFORD_XYZ_TO_LMS`]'s doc comment.
#[inline]
fn xyz_to_lms_bradford(xyz: Vec3) -> Vec3 {
    let m = BRADFORD_XYZ_TO_LMS;
    Vec3::new(
        m[0][0].mul_add(xyz.x, m[0][1].mul_add(xyz.y, m[0][2] * xyz.z)),
        m[1][0].mul_add(xyz.x, m[1][1].mul_add(xyz.y, m[1][2] * xyz.z)),
        m[2][0].mul_add(xyz.x, m[2][1].mul_add(xyz.y, m[2][2] * xyz.z)),
    )
}

/// Converts Bradford-space cone responses (LMS) back to CIE XYZ. See
/// [`BRADFORD_LMS_TO_XYZ`]'s doc comment.
#[inline]
fn lms_to_xyz_bradford(lms: Vec3) -> Vec3 {
    let m = BRADFORD_LMS_TO_XYZ;
    Vec3::new(
        m[0][0].mul_add(lms.x, m[0][1].mul_add(lms.y, m[0][2] * lms.z)),
        m[1][0].mul_add(lms.x, m[1][1].mul_add(lms.y, m[1][2] * lms.z)),
        m[2][0].mul_add(lms.x, m[2][1].mul_add(lms.y, m[2][2] * lms.z)),
    )
}

/// Applies a von Kries white-balance scale (as returned by
/// [`compute_illuminant_white_balance`]) to `xyz`: transforms to Bradford LMS, scales
/// each cone response independently, transforms back. This -- not a direct per-channel
/// scale of X and Z -- is what "diagonalise the adaptation in cone space" means.
/// Mirrored bit-for-bit-in-spirit by `shaders/spectral_transport.wgsl`'s own
/// application of `params.white_balance`.
// `pub(crate)`: `renderer::gpu::estimator_check::run_spectral_debug` reapplies this
// same scale, the same way, to its CPU-side recombination of the GPU kernel's raw
// per-channel radiance -- it must match the megakernel's own application exactly, or
// that self-consistency check compares two different white-balance conventions.
pub(crate) fn apply_von_kries_white_balance(xyz: Vec3, lms_scale: Vec3) -> Vec3 {
    lms_to_xyz_bradford(xyz_to_lms_bradford(xyz) * lms_scale)
}

/// Integrates a blackbody spectrum at `temp_k` against the CIE 1931 CMFs over
/// 380..=780 nm at 1 nm steps to obtain the per-channel von Kries white-balance scale,
/// in Bradford LMS space, that adapts that illuminant's own white point toward
/// [`D65_WHITE_X`]/[`D65_WHITE_Y`]. Applying this scale via
/// [`apply_von_kries_white_balance`] to a rendered XYZ value neutralizes the
/// illuminant's own colour cast without altering the light sources' colour
/// temperatures or the `blackbody_spectrum` clamp.
///
/// # Diagonalised in LMS, not XYZ
///
/// A diagonal scale of raw X and Z tristimulus values (`[Y_w/X_w, 1.0, Y_w/Z_w]`
/// multiplied directly into XYZ) is not von Kries adaptation: chromatic adaptation
/// happens per cone class, and X/Y/Z each mix all three cone types, so every colour
/// but the illuminant white itself picks up a hue shift. Instead, both the source
/// illuminant's white and the [`D65_WHITE_X`]/[`D65_WHITE_Y`] reference white (at the
/// same luminance, for a directly comparable ratio) are converted to Bradford LMS via
/// [`xyz_to_lms_bradford`] before taking the per-component ratio.
// `pub(crate)`: this exact 401-point (380..=780nm, 1nm step) quadrature is what the
// GPU white-balance self-test (`renderer::gpu::environment_check`) must reproduce on
// the GPU and compare ULP against -- calling the real function rather than
// re-deriving the loop in test code.
pub(crate) fn compute_illuminant_white_balance(temp_k: f32) -> Vec3 {
    let mut xyz_w = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        xyz_w += cie_1931_cmf(lambda) * blackbody_spectrum(lambda, temp_k);
    }

    let target_y = xyz_w.y.max(1e-6);
    let xyz_target = Vec3::new(
        (D65_WHITE_X / D65_WHITE_Y) * target_y,
        target_y,
        ((1.0 - D65_WHITE_X - D65_WHITE_Y) / D65_WHITE_Y) * target_y,
    );

    let lms_source = xyz_to_lms_bradford(xyz_w);
    let lms_target = xyz_to_lms_bradford(xyz_target);

    Vec3::new(
        if lms_source.x > 1e-6 {
            lms_target.x / lms_source.x
        } else {
            1.0
        },
        if lms_source.y > 1e-6 {
            lms_target.y / lms_source.y
        } else {
            1.0
        },
        if lms_source.z > 1e-6 {
            lms_target.z / lms_source.z
        } else {
            1.0
        },
    )
}

/// Returns the (lazily-computed, cached) per-channel von Kries white-balance scale for
/// a given lighting preset.
///
/// Called from `trace_spectral_ray` once per ray sample -- up to millions of times
/// per frame across every render worker thread. A `Mutex<HashMap<..>>` cache would
/// serialize every ray sample on one lock, collapsing the parallel render loop to
/// effectively single-threaded. Since the full set of lighting presets is small and
/// known ahead of time, each preset instead gets its own `OnceLock<Vec3>` static,
/// selected with a `match` -- a lock-free atomic read with no allocation after the
/// first call per preset.
pub(super) fn illuminant_white_balance(lighting_preset: LightingPreset) -> Vec3 {
    static INCANDESCENT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static RING_LIGHTS: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static DARK_SPOTLIGHT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static DAYLIGHT_DEFAULT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static ISO_HEMISPHERE: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static SOFT_DOME: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static DAYLIGHT_DOME: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();

    let temp_k = illuminant_temperature_k(lighting_preset);
    match lighting_preset {
        LightingPreset::Incandescent => {
            *INCANDESCENT.get_or_init(|| compute_illuminant_white_balance(temp_k))
        }
        LightingPreset::RingLights => {
            *RING_LIGHTS.get_or_init(|| compute_illuminant_white_balance(temp_k))
        }
        LightingPreset::DarkSpotlight => {
            *DARK_SPOTLIGHT.get_or_init(|| compute_illuminant_white_balance(temp_k))
        }
        LightingPreset::IsoHemisphere => {
            *ISO_HEMISPHERE.get_or_init(|| compute_illuminant_white_balance(temp_k))
        }
        LightingPreset::SoftDome => {
            *SOFT_DOME.get_or_init(|| compute_illuminant_white_balance(temp_k))
        }
        LightingPreset::DaylightDome => {
            *DAYLIGHT_DOME.get_or_init(|| compute_illuminant_white_balance(temp_k))
        }
        LightingPreset::Daylight => {
            *DAYLIGHT_DEFAULT.get_or_init(|| compute_illuminant_white_balance(temp_k))
        }
    }
}

/// ACES Filmic Tone Mapping Curve, applied to a scalar luminance value.
///
/// Reduced from the old per-channel-Vec3 form: applying this curve independently per
/// RGB channel is a hue-shifting operator, which is exactly wrong for saturated
/// dispersion "fire" colours. `xyz_to_srgb_gamma` now applies it to luminance only.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "ACES filmic tonemap (Narkowicz 2015) fit constants, named a..e to match \
              the canonical y*(a*y+b) / (y*(c*y+d)+e) formula as published everywhere \
              it's referenced; renaming them would only obscure the connection to the \
              reference formula for anyone checking this against the source"
)]
pub fn aces_tonemap(y: f32) -> f32 {
    let a = 2.51f32;
    let b = 0.03f32;
    let c = 2.43f32;
    let d = 0.59f32;
    let e = 0.14f32;
    (y * a.mul_add(y, b)) / y.mul_add(c.mul_add(y, d), e)
}

/// Converts a CIE XYZ radiance sample to encoded 8-bit RGBA in an arbitrary wide-gamut
/// [`crate::color::ColorSpace`].
///
/// Space-aware chromaticity-preserving gamut mapping (radially compressing
/// out-of-gamut colours toward the space's own white point in CIE xyY at constant
/// luminance, rather than desaturating/hue-shifting via naive per-channel clamping),
/// ACES filmic tone mapping applied to luminance only, and finally that space's own
/// transfer function. See `crate::color::space` and `crate::color::gamut` for the
/// implementation this delegates to.
///
/// No caller currently lets the user pick `space` -- every call site goes through
/// [`xyz_to_srgb_gamma`] below. A future render-export path is the natural place to
/// expose `space` as a user-facing choice (sRGB / Display P3 / Rec.2020 / `ACEScg`).
#[must_use]
pub fn xyz_to_rgb_in_space(xyz: Vec3, space: crate::color::ColorSpace) -> [u8; 4] {
    space.encode(xyz, crate::color::ToneMap::AcesFilmic { exposure: 1.0 })
}

/// Converts CIE XYZ to sRGB via chromaticity-preserving gamut mapping (Procedure 3).
///
/// Thin wrapper around [`xyz_to_rgb_in_space`] targeting
/// [`crate::color::ColorSpace::Srgb`], which uses the true piecewise sRGB transfer
/// curve (`12.92 * x` below the breakpoint, `1.055 * x^(1/2.4) - 0.055` above) rather
/// than a flat `1/2.2` gamma approximation. The two curves diverge most in deep shadow
/// (peak absolute difference ~0.0335, ~8.5 of 255 levels, at linear x ~= 0.00216) and
/// stay under ~1.5 levels across the 0.1-0.9 midtone/highlight range -- acceptable
/// since the true curve is physically correct and the deviation is confined to shadow
/// detail below the range most gemstone renders spend their dynamic range in; see
/// `tests/color_tests.rs::
/// srgb_encode_matches_xyz_to_srgb_gamma_reference_within_the_known_gamma_curve_difference`
/// for the regression pinning this tolerance.
#[must_use]
pub fn xyz_to_srgb_gamma(xyz: Vec3) -> [u8; 4] {
    xyz_to_rgb_in_space(xyz, crate::color::ColorSpace::Srgb)
}

#[cfg(test)]
mod white_balance_cache_tests {
    use super::*;

    /// The `OnceLock`-per-preset cache must not change the visible value: asserts the
    /// cached value for every preset (the three named ones plus the default fallback
    /// arm) exactly matches a fresh, uncached recomputation.
    #[test]
    fn illuminant_white_balance_matches_direct_computation_for_all_presets() {
        for preset in LightingPreset::ALL {
            let cached = illuminant_white_balance(preset);
            let direct = compute_illuminant_white_balance(illuminant_temperature_k(preset));
            assert!(
                (cached - direct).length() < 1e-5,
                "cached white balance for {preset:?} should exactly match direct integration (cached={cached:?}, direct={direct:?})"
            );
        }
    }

    /// Every unrecognized preset label must parse (via `LightingPreset::from_label`)
    /// and fall through to the same default (D65 6500K) preset -- including the
    /// legacy, mislabelled `"D65 Daylight (5500K)"` string an older settings file may
    /// still contain (see `LightingPreset::from_label`'s doc comment).
    #[test]
    fn illuminant_white_balance_default_arm_is_shared() {
        let a = illuminant_white_balance(LightingPreset::from_label("Totally Unknown Preset A"));
        let b = illuminant_white_balance(LightingPreset::from_label("Totally Unknown Preset B"));
        let legacy = illuminant_white_balance(LightingPreset::from_label("D65 Daylight (5500K)"));
        assert!(
            (a - b).length() < 1e-6,
            "distinct unrecognized presets must share the default D65 white balance"
        );
        assert!(
            (a - legacy).length() < 1e-6,
            "the legacy mislabelled D65 string must still migrate to the default D65 white balance"
        );
    }

    /// Confirms the lock-free `OnceLock`-per-preset statics are race-free: many
    /// threads racing to initialize the same preset's `OnceLock` must all observe the
    /// identical value.
    #[test]
    fn illuminant_white_balance_is_stable_across_concurrent_threads() {
        let presets = LightingPreset::ALL;

        let handles: Vec<_> = (0..32)
            .map(|i| {
                std::thread::spawn(move || {
                    let preset = presets[i % presets.len()];
                    (preset, illuminant_white_balance(preset))
                })
            })
            .collect();

        let mut by_preset: std::collections::HashMap<LightingPreset, Vec3> =
            std::collections::HashMap::new();
        for h in handles {
            let (preset, v) = h.join().unwrap();
            if let Some(existing) = by_preset.get(&preset) {
                assert!(
                    (*existing - v).length() < 1e-6,
                    "value for {preset:?} differs across threads"
                );
            } else {
                by_preset.insert(preset, v);
            }
        }
    }
}

#[cfg(test)]
mod spectral_mis_tests {
    use super::{
        super::{sampling::hash_u32, transport::wrapped_hero_wavelengths},
        *,
    };

    /// `mis_weighted_radiance` is now the identity function -- see its doc comment for
    /// why no reweighting is valid under `trace_spectral_ray`'s current wavelength
    /// stratification. This just pins that contract down directly.
    #[test]
    fn mis_weighted_radiance_is_identity() {
        for &r in &[0.0f32, 1.0, 4.2, 1.0e6, -3.5] {
            assert_eq!(
                mis_weighted_radiance(r),
                r,
                "mis_weighted_radiance must return its input unchanged (r={r})"
            );
        }
    }

    /// Deterministic unit-interval draw built from the same `hash_u32` PRNG
    /// `trace_spectral_ray` itself uses, so this test's Monte Carlo trials are
    /// reproducible without pulling in an external `rand` dependency.
    fn unit_rand(seed: u32) -> f32 {
        (hash_u32(seed) as f32) / 4_294_967_295.0
    }

    /// The rejected `own_pdf / sum_pdf * num_channels` balance-heuristic weight (see
    /// `mis_weighted_radiance`'s doc comment). Reproduced here directly so this
    /// regression test can permanently guard against reintroducing the bias it causes.
    fn shipped_biased_weight(
        radiance: f32,
        own_pdf: f32,
        sum_pdf: f32,
        num_channels: usize,
    ) -> f32 {
        let weight = (own_pdf / sum_pdf.max(1e-8)) * num_channels as f32;
        radiance * weight
    }

    /// Discriminating Monte Carlo regression test for the spectral-MIS bias bug, using
    /// UNEQUAL per-channel pdfs (channel 0 hero R0=0.2, channel 1 companion R1=0.6) with
    /// closed-form ground truth `L_k = 1.0` (a Fresnel interface reflects or transmits
    /// with unit total probability) -- an equal-pdf test cannot discriminate, since
    /// `own_pdf/sum_pdf * N` and the constant weight 1 are then algebraically identical.
    /// Asserts the fixed estimator (weight=1) converges within a few percent while the
    /// old `own_pdf/sum_pdf * N` weight is biased by roughly +17% on both channels.
    #[test]
    fn two_channel_fresnel_monte_carlo_discriminates_correct_from_biased_weighting() {
        const R0: f32 = 0.2; // hero (channel 0) reflectance
        const R1: f32 = 0.6; // companion (channel 1) reflectance, deliberately different
        const TRIALS: u32 = 400_000;
        const GROUND_TRUTH: f32 = 1.0;

        let mut plain_sum = [0.0f64; 2];
        let mut biased_sum = [0.0f64; 2];

        for trial in 0..TRIALS {
            let xi = unit_rand(trial ^ 0xA5A5_5A5A);

            // radiance[k] and path_pdf[k] for the branch actually taken this trial,
            // mirroring trace_spectral_ray's own per-channel bookkeeping.
            let (radiance, path_pdf) = if xi < R0 {
                // Reflect branch, selected with the HERO's own probability R0.
                ([1.0f32, R1 / R0], [R0, R1])
            } else {
                // Transmit branch, selected with the HERO's own probability (1 - R0).
                ([1.0f32, (1.0 - R1) / (1.0 - R0)], [1.0 - R0, 1.0 - R1])
            };

            let sum_pdf = path_pdf[0] + path_pdf[1];
            for k in 0..2 {
                plain_sum[k] += f64::from(mis_weighted_radiance(radiance[k]));
                biased_sum[k] +=
                    f64::from(shipped_biased_weight(radiance[k], path_pdf[k], sum_pdf, 2));
            }
        }

        let plain_avg: Vec<f32> = plain_sum
            .iter()
            .map(|s| (*s / f64::from(TRIALS)) as f32)
            .collect();
        let biased_avg: Vec<f32> = biased_sum
            .iter()
            .map(|s| (*s / f64::from(TRIALS)) as f32)
            .collect();

        for (k, &avg) in plain_avg.iter().enumerate() {
            let err = (avg - GROUND_TRUTH).abs() / GROUND_TRUTH;
            assert!(
                err < 0.03,
                "FIXED (weight=1) estimator for channel {} should converge to the ground truth {} within 3% over {} trials (got {}, {:.2}% error)",
                k,
                GROUND_TRUTH,
                TRIALS,
                avg,
                err * 100.0
            );
        }

        // The old formula must be clearly, substantially biased -- proving the test
        // actually discriminates between the two formulas.
        for (k, &avg) in biased_avg.iter().enumerate() {
            let err = (avg - GROUND_TRUTH).abs() / GROUND_TRUTH;
            assert!(
                err > 0.10,
                "the OLD shipped own_pdf/sum_pdf*N weight is expected to be substantially biased (>10%) on this scenario for channel {} (got {}, {:.2}% error) -- if this assertion fails, this regression test has lost its discriminating power",
                k,
                avg,
                err * 100.0
            );
        }
    }

    /// The wrapped hero-wavelength construction must (a) keep every generated
    /// wavelength within the visible range [380, 780] regardless of the hero draw,
    /// including right at the wraparound boundary, and (b) always place the hero
    /// (`lambda_hero` itself) at array index 0.
    #[test]
    fn wrapped_hero_wavelengths_stay_in_visible_range_and_hero_is_always_index_0() {
        for seed in 0..20_000u32 {
            let hero_rand = unit_rand(seed);
            let lambdas: [f32; 8] = wrapped_hero_wavelengths(hero_rand);
            let lambda_hero = hero_rand.mul_add(780.0 - 380.0, 380.0);

            for (k, &l) in lambdas.iter().enumerate() {
                assert!(
                    (380.0..=780.0).contains(&l),
                    "wavelength at channel {k} must stay within [380, 780] (seed={seed}, hero_rand={hero_rand}, got {l})"
                );
            }
            assert!(
                (lambdas[0] - lambda_hero).abs() < 1e-3,
                "hero must always land at array index 0 (seed={}, hero_rand={}, lambdas[0]={}, lambda_hero={})",
                seed,
                hero_rand,
                lambdas[0],
                lambda_hero
            );
        }

        // Boundary check: a hero_rand right at the top of its range wraps the highest
        // companion channels back down past 380nm rather than running off past 780nm.
        let lambdas_top: [f32; 8] = wrapped_hero_wavelengths(0.999_999);
        for &l in &lambdas_top {
            assert!(
                (380.0..=780.0).contains(&l),
                "boundary hero draw produced an out-of-range wavelength: {l}"
            );
        }
    }

    /// Confirms the key statistical property the wrapped construction buys: every one
    /// of the N channel slots is, across many draws, uniformly distributed over the
    /// full comb-relative rotation, i.e. no channel index is structurally privileged.
    #[test]
    fn wrapped_hero_wavelengths_cover_every_channel_slot_uniformly() {
        let mut min_seen = [1000.0f32; 8];
        let mut max_seen = [0.0f32; 8];
        for seed in 0..20_000u32 {
            let hero_rand = unit_rand(seed ^ 0xDEAD_BEEF);
            let lambdas: [f32; 8] = wrapped_hero_wavelengths(hero_rand);
            for k in 0..8 {
                min_seen[k] = min_seen[k].min(lambdas[k]);
                max_seen[k] = max_seen[k].max(lambdas[k]);
            }
        }
        for k in 0..8 {
            // Each channel should, across enough draws, range across nearly the
            // entire [380, 780] spectrum, not just its "home" 50nm sub-band.
            assert!(
                max_seen[k] - min_seen[k] > 350.0,
                "channel {} should range across nearly the full spectrum over many hero draws (got min={}, max={}, span={})",
                k,
                min_seen[k],
                max_seen[k],
                max_seen[k] - min_seen[k]
            );
        }
    }

    /// `spectral_mis_weight` must reduce to exactly 1.0 whenever every channel's
    /// `path_pdf` is identical -- the case a non-dispersive material forces (identical
    /// n(lambda) makes every per-channel Fresnel probability, and hence every
    /// `path_pdf` factor, identical across channels): `sum_pdf` collapses to exactly
    /// `N * path_pdf[hero_idx]`, so the weight is `N * p / (N * p) == 1.0` for any
    /// common value `p`, checked here across several hero indices and common values.
    #[test]
    fn spectral_mis_weight_is_exactly_unity_when_all_channels_agree() {
        for &p in &[1.0f32, 0.5, 1e-4, 1e-3, 0.999_9] {
            for hero_idx in 0..8 {
                let path_pdf = [p; 8];
                let w = spectral_mis_weight(&path_pdf, hero_idx);
                // `sum_pdf` is an iterative float sum of 8 equal values, not
                // necessarily bit-identical to `8.0 * p` -- checks "1.0 up to a
                // couple ULPs", not literal f32 equality.
                assert!(
                    (w - 1.0).abs() < 1e-6,
                    "weight must be 1.0 (up to float rounding) when every channel's path_pdf is identical (p={p}, hero_idx={hero_idx}, got {w})"
                );
            }
        }
    }

    /// `spectral_mis_weight` must depart from 1.0 once channels disagree, and must
    /// approach `N` (here 8) as the non-hero channels' `path_pdf` collapses toward 0 --
    /// once chromatic termination kills off every companion, the surviving hero's own
    /// sample gets the full weight (the mechanism producing dispersion "fire").
    #[test]
    fn spectral_mis_weight_approaches_n_as_companions_are_chromatically_terminated() {
        let hero_idx = 0usize;
        let mut path_pdf = [0.3f32; 8];
        path_pdf[hero_idx] = 0.3;
        let w_all_alive = spectral_mis_weight(&path_pdf, hero_idx);
        assert!(
            (w_all_alive - 1.0).abs() < 1e-4,
            "all channels agreeing should give weight ~= 1.0 (got {w_all_alive})"
        );

        // Terminate every companion (path_pdf -> 0), leaving only the hero alive.
        for (k, p) in path_pdf.iter_mut().enumerate() {
            if k != hero_idx {
                *p = 0.0;
            }
        }
        let w_hero_only = spectral_mis_weight(&path_pdf, hero_idx);
        assert!(
            (w_hero_only - 8.0).abs() < 1e-4,
            "with every companion terminated, weight should approach N=8 (got {w_hero_only})"
        );
    }

    /// Discriminating Monte Carlo regression test for the spectral-MIS weight extended
    /// to a genuine dispersive-refraction "chromatic termination" event. Unlike the
    /// sibling test above (fixed hero at channel 0), this alternates which of the two
    /// channels drives (p=1/2 each trial) -- the combined weight is provably biased
    /// under a single fixed hero and only becomes unbiased once the ensemble genuinely
    /// alternates, matching how a real render accumulates independent samples with
    /// their own wrapped hero draw. The companion's transmission is modelled as
    /// genuinely dispersive: the non-driving channel's `path_pdf` AND radiance both
    /// zero at that event (chromatic termination). Ground truth is still exactly 1.0
    /// (Fresnel unitarity) by Veach's theorem applied per-channel.
    #[test]
    fn two_channel_dispersive_termination_monte_carlo_is_unbiased_under_alternating_hero() {
        const R_A: f32 = 0.2;
        const R_B: f32 = 0.6;
        const TRIALS: u32 = 400_000;
        const GROUND_TRUTH: f32 = 1.0;

        // One Fresnel-interface trial. `hero_is_a` selects which channel drives the
        // shared branch decision. Returns (F_A, F_B): this trial's combined (weighted)
        // estimate of channel A's and channel B's own integral.
        fn trial(xi: f32, hero_is_a: bool) -> (f32, f32) {
            let (r_hero, r_other) = if hero_is_a { (R_A, R_B) } else { (R_B, R_A) };

            let (rad_hero, rad_other, pdf_hero, pdf_other) = if xi < r_hero {
                // Reflect: never dispersive -- both channels' directions coincide.
                (1.0f32, r_other / r_hero, r_hero, r_other)
            } else {
                // Transmit: genuinely dispersive -- the companion's refracted
                // direction never coincides with the driving channel, so its path_pdf
                // and Stokes/radiance both collapse to 0 (chromatic termination).
                (1.0f32, 0.0f32, 1.0 - r_hero, 0.0f32)
            };

            let sum_pdf = pdf_hero + pdf_other;
            let weight = 2.0 * pdf_hero / sum_pdf.max(1e-8);

            if hero_is_a {
                (rad_hero * weight, rad_other * weight)
            } else {
                (rad_other * weight, rad_hero * weight)
            }
        }

        let mut sum_a = 0.0f64;
        let mut sum_b = 0.0f64;
        for trial_idx in 0..TRIALS {
            // Independent draws: which channel is hero this trial, and branch xi.
            let hero_is_a = unit_rand(trial_idx ^ 0x1234_5678) < 0.5;
            let xi = unit_rand(trial_idx ^ 0xA5A5_5A5A);
            let (f_a, f_b) = trial(xi, hero_is_a);
            sum_a += f64::from(f_a);
            sum_b += f64::from(f_b);
        }

        let avg_a = (sum_a / f64::from(TRIALS)) as f32;
        let avg_b = (sum_b / f64::from(TRIALS)) as f32;

        for (label, avg) in [("A", avg_a), ("B", avg_b)] {
            let err = (avg - GROUND_TRUTH).abs() / GROUND_TRUTH;
            assert!(
                err < 0.03,
                "channel {} combined estimator should converge to ground truth {} within 3% over {} trials under alternating hero (got {}, {:.2}% error)",
                label,
                GROUND_TRUTH,
                TRIALS,
                avg,
                err * 100.0
            );
        }
    }
}
