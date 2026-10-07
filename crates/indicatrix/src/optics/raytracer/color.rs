//! Spectral-to-tristimulus color conversion.
//!
//! The CIE 1931 color-matching functions, the spectral-MIS combination weight
//! applied at XYZ integration, von Kries white balance, and the final XYZ -> sRGB
//! gamut/gamma mapping.

use super::{
    NUM_CHANNELS,
    environment::{LightingPreset, blackbody_spectrum},
};
use glam::Vec3;

/// CIE 1931 2° Standard Observer Color Matching Functions, linearly interpolated from
/// the tabulated 5 nm CIE 15:2004 table.
///
/// Delegates to [`crate::color::cie1931::cie_1931_cmf`] (the single source of truth for
/// the table); a thin `Vec3`-returning wrapper since the raytracer's hot paths want a
/// `glam::Vec3` rather than `[f32; 3]`.
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
/// (in `color/spectral_mis_tests.rs`) shows this biased by roughly +17% on a two-channel Fresnel analogue.
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
/// sample's contribution onto the hero's own color, the mechanism that produces
/// dispersion "fire" at the image level: different samples have different hero
/// wavelengths, so they concentrate onto different colors.
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

/// color temperature (Kelvin) associated with each named lighting preset. A thin
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
/// most color-management pipelines. Row-major, same convention as
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
/// Mirrored ULP-budgeted (fma contraction, `/`, `sqrt` are implementation-defined, so
/// WGSL cannot guarantee literal bit-identity) by `shaders/spectral_transport.wgsl`'s
/// own application of `params.white_balance`.
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
/// illuminant's own color cast without altering the light sources' color
/// temperatures or the `blackbody_spectrum` clamp.
///
/// # Diagonalised in LMS, not XYZ
///
/// A diagonal scale of raw X and Z tristimulus values (`[Y_w/X_w, 1.0, Y_w/Z_w]`
/// multiplied directly into XYZ) is not von Kries adaptation: chromatic adaptation
/// happens per cone class, and X/Y/Z each mix all three cone types, so every color
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
///
/// Only presets with [`LightingPreset::uses_white_balance`] (the Planckian ones) carry
/// an adaptation. The D65 presets get the identity (that white IS the sRGB white; a
/// Planckian 6500 K scale would push neutrals green) and so do the UV lamps (no white
/// point to adapt from).
pub(super) fn illuminant_white_balance(lighting_preset: LightingPreset) -> Vec3 {
    static INCANDESCENT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static RING_LIGHTS: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static DARK_SPOTLIGHT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static LIGHT_TENT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static ILLUMINANT_A: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();

    if !lighting_preset.uses_white_balance() {
        return Vec3::ONE;
    }
    let cell = match lighting_preset {
        LightingPreset::Incandescent => &INCANDESCENT,
        LightingPreset::RingLights => &RING_LIGHTS,
        LightingPreset::DarkSpotlight => &DARK_SPOTLIGHT,
        LightingPreset::LightTent => &LIGHT_TENT,
        LightingPreset::IlluminantA => &ILLUMINANT_A,
        LightingPreset::Daylight
        | LightingPreset::IsoHemisphere
        | LightingPreset::DaylightDome
        | LightingPreset::DaylightSun
        | LightingPreset::Aset
        | LightingPreset::ShopLights
        | LightingPreset::WindowDaylight
        | LightingPreset::WhiteTray
        | LightingPreset::UvLamp365
        | LightingPreset::UvLamp395 => {
            return Vec3::ONE;
        }
    };
    *cell
        .get_or_init(|| compute_illuminant_white_balance(illuminant_temperature_k(lighting_preset)))
}

/// ACES Filmic Tone Mapping Curve, applied to a scalar luminance value.
///
/// Reduced from the old per-channel-Vec3 form: applying this curve independently per
/// RGB channel is a hue-shifting operator, which is exactly wrong for saturated
/// dispersion "fire" colors. `xyz_to_srgb_gamma` now applies it to luminance only.
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
/// out-of-gamut colors toward the space's own white point in CIE xyY at constant
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
mod spectral_mis_tests;
#[cfg(test)]
mod white_balance_cache_tests;
