//! CIE 1931 2° Standard Observer Color Matching Functions (CMFs), tabulated at 5 nm and
//! linearly interpolated.
//!
//! # Why a table, not an analytic fit
//!
//! The Wyman/Sloan/Shirley (2013) multi-lobe Gaussian analytic fit to the CIE 1931
//! CMFs is a *fit*, not the standard: it carries 1-3% XYZ error relative to the real
//! tabulated observer, worst in the x-bar trough
//! between the fit's two x-bar lobes -- the real x-bar dips to a near-zero minimum
//! around 495-510nm (table entries as small as 0.0024), and a sum of two Gaussians
//! does not fall to zero between two peaks as sharply as the real curve does. Relative
//! error is already ~5% by 480nm (the falling flank into the trough) and grows to
//! roughly 50% right at the ~500nm minimum, where even a small absolute miss is a huge
//! fraction of the tiny true value. See
//! `legacy_fit_diverges_from_the_table_at_the_x_bar_trough` below for a test pinning
//! that divergence down numerically.
//!
//! [`CIE_1931_TABLE`] below is the standard CIE 1931 2° observer, from CIE 15:2004
//! "Colorimetry," 3rd ed., Table T.4 (also widely reproduced, e.g. Wyszecki & Stiles
//! Table I(3.3.1) and <http://www.cvrl.org/>), at its conventional 5nm publication
//! resolution, 380-780nm. [`cie_1931_cmf`] linearly interpolates between table entries.
//!
//! Single source of truth: [`optics::raytracer`](crate::optics::raytracer) delegates to
//! [`cie_1931_cmf`] rather than keeping its own copy of the table.
//!
//! # Bit-identical WGSL transcription
//!
//! [`cie_1931_cmf`] is written so a WGSL port (`const` array + the same `floor`/
//! fraction/lerp steps, in the same order) reproduces it bit-for-bit modulo ordinary
//! driver-level `f32` rounding: only `f32` arithmetic, no `mul_add`/fma, no `f64`
//! intermediates anywhere in the lookup or interpolation.

/// Wavelength (nm) of the table's first entry.
const TABLE_START_NM: f32 = 380.0;

/// Wavelength (nm) spacing between consecutive table entries.
const TABLE_STEP_NM: f32 = 5.0;

/// Standard CIE 1931 2° observer color matching functions `[x_bar, y_bar, z_bar]`,
/// 380-780nm at 5nm intervals (81 entries) -- CIE 15:2004 Table T.4 / CIE 1931 2°
/// observer, 5 nm, as conventionally published to 4 decimal places.
#[rustfmt::skip]
const CIE_1931_TABLE: [[f32; 3]; 81] = [
    [0.0014, 0.0000, 0.0065], // 380nm
    [0.0022, 0.0001, 0.0105], // 385nm
    [0.0042, 0.0001, 0.0201], // 390nm
    [0.0076, 0.0002, 0.0362], // 395nm
    [0.0143, 0.0004, 0.0679], // 400nm
    [0.0232, 0.0006, 0.1102], // 405nm
    [0.0435, 0.0012, 0.2074], // 410nm
    [0.0776, 0.0022, 0.3713], // 415nm
    [0.1344, 0.0040, 0.6456], // 420nm
    [0.2148, 0.0073, 1.0391], // 425nm
    [0.2839, 0.0116, 1.3856], // 430nm
    [0.3285, 0.0168, 1.6230], // 435nm
    [0.3483, 0.0230, 1.7471], // 440nm
    [0.3481, 0.0298, 1.7826], // 445nm
    [0.3362, 0.0380, 1.7721], // 450nm
    [0.3187, 0.0480, 1.7441], // 455nm
    [0.2908, 0.0600, 1.6692], // 460nm
    [0.2511, 0.0739, 1.5281], // 465nm
    [0.1954, 0.0910, 1.2876], // 470nm
    [0.1421, 0.1126, 1.0419], // 475nm
    [0.0956, 0.1390, 0.8130], // 480nm
    [0.0580, 0.1693, 0.6162], // 485nm
    [0.0320, 0.2080, 0.4652], // 490nm
    [0.0147, 0.2586, 0.3533], // 495nm
    [0.0049, 0.3230, 0.2720], // 500nm
    [0.0024, 0.4073, 0.2123], // 505nm
    [0.0093, 0.5030, 0.1582], // 510nm
    [0.0291, 0.6082, 0.1117], // 515nm
    [0.0633, 0.7100, 0.0782], // 520nm
    [0.1096, 0.7932, 0.0573], // 525nm
    [0.1655, 0.8620, 0.0422], // 530nm
    [0.2257, 0.9149, 0.0298], // 535nm
    [0.2904, 0.9540, 0.0203], // 540nm
    [0.3597, 0.9803, 0.0134], // 545nm
    [0.4334, 0.9950, 0.0087], // 550nm
    [0.5121, 1.0000, 0.0057], // 555nm
    [0.5945, 0.9950, 0.0039], // 560nm
    [0.6784, 0.9786, 0.0027], // 565nm
    [0.7621, 0.9520, 0.0021], // 570nm
    [0.8425, 0.9154, 0.0018], // 575nm
    [0.9163, 0.8700, 0.0017], // 580nm
    [0.9786, 0.8163, 0.0014], // 585nm
    [1.0263, 0.7570, 0.0011], // 590nm
    [1.0567, 0.6949, 0.0010], // 595nm
    [1.0622, 0.6310, 0.0008], // 600nm
    [1.0456, 0.5668, 0.0006], // 605nm
    [1.0026, 0.5030, 0.0003], // 610nm
    [0.9384, 0.4412, 0.0002], // 615nm
    [0.8544, 0.3810, 0.0002], // 620nm
    [0.7514, 0.3210, 0.0001], // 625nm
    [0.6424, 0.2650, 0.0000], // 630nm
    [0.5419, 0.2170, 0.0000], // 635nm
    [0.4479, 0.1750, 0.0000], // 640nm
    [0.3608, 0.1382, 0.0000], // 645nm
    [0.2835, 0.1070, 0.0000], // 650nm
    [0.2187, 0.0816, 0.0000], // 655nm
    [0.1649, 0.0610, 0.0000], // 660nm
    [0.1212, 0.0446, 0.0000], // 665nm
    [0.0874, 0.0320, 0.0000], // 670nm
    [0.0636, 0.0232, 0.0000], // 675nm
    [0.0468, 0.0170, 0.0000], // 680nm
    [0.0329, 0.0119, 0.0000], // 685nm
    [0.0227, 0.0082, 0.0000], // 690nm
    [0.0158, 0.0057, 0.0000], // 695nm
    [0.0114, 0.0041, 0.0000], // 700nm
    [0.0081, 0.0029, 0.0000], // 705nm
    [0.0058, 0.0021, 0.0000], // 710nm
    [0.0041, 0.0015, 0.0000], // 715nm
    [0.0029, 0.0010, 0.0000], // 720nm
    [0.0020, 0.0007, 0.0000], // 725nm
    [0.0014, 0.0005, 0.0000], // 730nm
    [0.0010, 0.0004, 0.0000], // 735nm
    [0.0007, 0.0002, 0.0000], // 740nm
    [0.0005, 0.0002, 0.0000], // 745nm
    [0.0003, 0.0001, 0.0000], // 750nm
    [0.0002, 0.0001, 0.0000], // 755nm
    [0.0002, 0.0001, 0.0000], // 760nm
    [0.0001, 0.0000, 0.0000], // 765nm
    [0.0001, 0.0000, 0.0000], // 770nm
    [0.0001, 0.0000, 0.0000], // 775nm
    [0.0000, 0.0000, 0.0000], // 780nm
];

/// The definite integral of the tabulated `y_bar` column over 380-780nm, as a
/// left-Riemann sum at the table's own 5nm step: `step * sum(y_bar_i)`.
///
/// This is the SAME normalization constant every `cie_1931_cmf` caller elsewhere in the
/// crate already divides an unbiased 1nm-step spectral quadrature by -- see
/// `optics::raytracer::color::{integrate_channels_to_xyz, compute_illuminant_white_balance}`,
/// `optics::raytracer::{intersect, scattering}`, `renderer::gpu::{furnace_check,
/// estimator_check::furnace}`, and `shaders/{environment,furnace,spectral_transport}.wgsl`'s
/// `NORM_FACTOR` -- all of which hard-code it as the literal `106.856`, so that an
/// equal-energy white spectrum integrates to `Y = 1`.
///
/// Computed here as a `const` directly from [`CIE_1931_TABLE`] (not copied by hand) so
/// it can never silently drift from the table it is derived from. Evaluates to
/// ~106.8555 -- the SAME value, to the precision every other caller already hard-codes,
/// as their `106.856` literal (that literal is the true tabulated integral, not an
/// analytic-fit integral -- see
/// `y_integral_matches_every_other_callers_106_856_literal` below, which pins this down
/// numerically instead of asserting it in prose). No file outside `color/` needs to
/// change its normalization constant to use this tabulated CMF.
pub const CIE_1931_Y_INTEGRAL_5NM: f32 = {
    let mut sum = 0.0f32;
    let mut i = 0usize;
    while i < CIE_1931_TABLE.len() {
        sum += CIE_1931_TABLE[i][1];
        i += 1;
    }
    sum * TABLE_STEP_NM
};

/// CIE 1931 2° Standard Observer Color Matching Functions at `lambda_nm`, by linear
/// interpolation of [`CIE_1931_TABLE`].
///
/// Returns `[0.0, 0.0, 0.0]` outside the tabulated 380-780nm range (this renderer's own
/// wavelength sampling never leaves that range -- see `optics::raytracer::transport`'s
/// `SPECTRUM_SPAN`/hero-wavelength wrapping -- so this is a defensive bound, not a path
/// any real caller takes).
///
/// # Bit-identical WGSL transcription
///
/// Deliberately plain `f32` arithmetic in a fixed order (`index = floor((lambda -
/// start) / step)`, `t = position - index`, `lo + (hi - lo) * t`), no `mul_add`/fma and
/// no `f64` intermediate anywhere -- a WGSL port doing the exact same steps against the
/// same table reproduces this bit-for-bit modulo ordinary driver rounding. See
/// `shaders/environment.wgsl`, `shaders/furnace.wgsl`, and `shaders/spectral_transport.wgsl`'s
/// own copies of this function.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the lerp below is deliberately plain `lo + (hi - lo) * t`, no `mul_add`/fma, \
              so a WGSL port doing the exact same steps reproduces this bit-for-bit -- see \
              this function's own doc comment and `shaders/{environment,furnace,\
              spectral_transport}.wgsl`'s copies of it"
)]
pub fn cie_1931_cmf(lambda_nm: f32) -> [f32; 3] {
    const TABLE_END_NM: f32 = TABLE_START_NM + (CIE_1931_TABLE.len() - 1) as f32 * TABLE_STEP_NM;
    const LAST_INDEX: usize = CIE_1931_TABLE.len() - 1;

    if !(TABLE_START_NM..=TABLE_END_NM).contains(&lambda_nm) {
        return [0.0, 0.0, 0.0];
    }

    let position = (lambda_nm - TABLE_START_NM) / TABLE_STEP_NM;
    let index0 = (position.floor() as usize).min(LAST_INDEX - 1);
    let index1 = index0 + 1;
    let t = position - index0 as f32;

    let lo = CIE_1931_TABLE[index0];
    let hi = CIE_1931_TABLE[index1];
    [
        lo[0] + (hi[0] - lo[0]) * t,
        lo[1] + (hi[1] - lo[1]) * t,
        lo[2] + (hi[2] - lo[2]) * t,
    ]
}

/// Batched form of [`cie_1931_cmf`] over 8 wavelengths.
///
/// A table lookup has no transcendental function to batch (unlike a Gaussian-lobe fit,
/// which would need all 8 lanes to share one [`crate::simd::exp_f32x8`] call instead of
/// paying for 8 scalar `f32::exp` calls), so this is a plain per-lane [`cie_1931_cmf`]
/// call, kept only so `optics::raytracer::color::cie_1931_cmf_x8` (its
/// `Vec3`-returning wrapper) and its callers don't need to change. Bit-identical to 8
/// separate [`cie_1931_cmf`] calls.
#[must_use]
pub fn cie_1931_cmf_x8(lambdas: &[f32; 8]) -> [[f32; 3]; 8] {
    lambdas.map(cie_1931_cmf)
}

/// The Wyman, Sloan, Shirley (2013) multi-lobe analytic fit to the CIE 1931 CMFs.
///
/// Kept test-only to quantify how much more accurate the tabulated version above is
/// (see the module doc comment).
#[cfg(test)]
fn cie_1931_cmf_legacy_gaussian_fit(lambda_nm: f32) -> [f32; 3] {
    fn lobe(x: f32, mu: f32, sigma_lo: f32, sigma_hi: f32) -> f32 {
        let sigma = if x < mu { sigma_lo } else { sigma_hi };
        let t = (x - mu) / sigma;
        (-0.5 * t * t).exp()
    }

    let l = lambda_nm;
    let x = 0.065f32.mul_add(
        -lobe(l, 501.1, 20.4, 26.2),
        1.056f32.mul_add(
            lobe(l, 599.8, 37.9, 31.0),
            0.362 * lobe(l, 442.0, 16.0, 26.7),
        ),
    );
    let y = 0.286f32.mul_add(
        lobe(l, 530.9, 16.3, 31.1),
        0.821 * lobe(l, 568.8, 46.9, 40.5),
    );
    let z = 0.681f32.mul_add(
        lobe(l, 459.0, 26.0, 13.8),
        1.217 * lobe(l, 437.0, 11.8, 36.0),
    );
    [x.max(0.0), y.max(0.0), z.max(0.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`cie_1931_cmf_x8`] must agree with 8 separate [`cie_1931_cmf`] calls exactly --
    /// both go through the identical per-lane table lookup, with no SIMD exponential
    /// involved (unlike a Gaussian-lobe analytic fit, which would only guarantee a
    /// few-ULP agreement).
    #[test]
    fn cmf_x8_matches_scalar_exactly() {
        let mut state = 99u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (((state >> 33) as f32) / (u32::MAX as f32)).mul_add(700.0 - 380.0, 380.0)
        };
        for _round in 0..200 {
            let lambdas = [
                next(),
                next(),
                next(),
                next(),
                next(),
                next(),
                next(),
                next(),
            ];
            let batched = cie_1931_cmf_x8(&lambdas);
            for (k, &l) in lambdas.iter().enumerate() {
                let scalar = cie_1931_cmf(l);
                assert_eq!(batched[k], scalar, "lambda={l}");
            }
        }
    }

    /// A wavelength exactly on a table entry (555nm = 380 + 35*5, an exact multiple of
    /// the 5nm step) must return that entry exactly: `t` lands on exactly `0.0`, so the
    /// interpolation reduces to `lo + 0.0 == lo` with no rounding.
    #[test]
    fn table_point_555nm_returns_the_table_exactly() {
        let [x, y, z] = cie_1931_cmf(555.0);
        assert_eq!(x, 0.5121, "x_bar(555) must equal the table entry exactly");
        assert_eq!(
            y, 1.0000,
            "y_bar(555) must equal the table entry exactly (the CMF peak)"
        );
        assert_eq!(z, 0.0057, "z_bar(555) must equal the table entry exactly");
    }

    /// Another exact table point away from the peak, confirming the previous test
    /// isn't accidentally passing only because `y_bar` peaks at exactly `1.0`.
    #[test]
    fn table_point_480nm_returns_the_table_exactly() {
        let [x, y, z] = cie_1931_cmf(480.0);
        assert_eq!(x, 0.0956);
        assert_eq!(y, 0.1390);
        assert_eq!(z, 0.8130);
    }

    /// Halfway between two table entries (382.5nm, between the 380nm and 385nm rows)
    /// must return their exact arithmetic mean -- `t == 0.5`, and `lo + (hi-lo)*0.5`.
    #[test]
    fn interpolation_midpoint_is_the_mean_of_its_two_table_entries() {
        let [x, y, z] = cie_1931_cmf(382.5);
        let lo = CIE_1931_TABLE[0];
        let hi = CIE_1931_TABLE[1];
        for (got, (l, h)) in [x, y, z].into_iter().zip(lo.into_iter().zip(hi)) {
            let expected = l.midpoint(h);
            assert!(
                (got - expected).abs() < 1e-6,
                "got {got}, expected midpoint {expected}"
            );
        }
    }

    /// Wavelengths outside the tabulated 380-780nm range must return exactly zero, not
    /// clamp to the nearest edge value (unlike, e.g., `d65_relative_spectral_power`,
    /// which clamps -- CMFs genuinely are ~zero for the human eye outside this range,
    /// so zero is the physically correct extrapolation here).
    #[test]
    fn out_of_range_wavelengths_return_zero() {
        assert_eq!(cie_1931_cmf(379.999), [0.0, 0.0, 0.0]);
        assert_eq!(cie_1931_cmf(780.001), [0.0, 0.0, 0.0]);
        assert_eq!(cie_1931_cmf(300.0), [0.0, 0.0, 0.0]);
        assert_eq!(cie_1931_cmf(850.0), [0.0, 0.0, 0.0]);
    }

    /// The table's boundary wavelengths themselves are IN range and must return the
    /// table's first/last rows exactly (not the zero the previous test checks just
    /// outside them).
    #[test]
    fn boundary_wavelengths_return_the_table_edges_exactly() {
        assert_eq!(cie_1931_cmf(380.0), CIE_1931_TABLE[0]);
        assert_eq!(cie_1931_cmf(780.0), CIE_1931_TABLE[80]);
    }

    /// [`CIE_1931_Y_INTEGRAL_5NM`] (derived purely from the table above) must agree with
    /// the `106.856` literal every other CMF-integrating file in the crate still
    /// hard-codes, to the precision those files use -- otherwise equal-energy white
    /// would stop mapping to `Y = 1` in every file except this one.
    #[test]
    fn y_integral_matches_every_other_callers_106_856_literal() {
        assert!(
            (CIE_1931_Y_INTEGRAL_5NM - 106.856).abs() < 0.01,
            "CIE_1931_Y_INTEGRAL_5NM = {CIE_1931_Y_INTEGRAL_5NM}, expected ~106.856 to match \
             every other caller's hard-coded normalization literal"
        );
    }

    /// An equal-energy spectrum (unit radiance at every wavelength) must integrate to
    /// `Y = 1`, using the SAME 1nm-step rectangle-rule quadrature every real caller uses
    /// (see [`CIE_1931_Y_INTEGRAL_5NM`]'s doc comment for the full list) and dividing by
    /// that same constant.
    #[test]
    fn equal_energy_spectrum_integrates_to_y_one() {
        let mut xyz = [0.0f32; 3];
        for step in 0..=(780 - 380) {
            let lambda = 380.0f32 + step as f32;
            let cmf = cie_1931_cmf(lambda);
            xyz[0] += cmf[0];
            xyz[1] += cmf[1];
            xyz[2] += cmf[2];
        }
        let y = xyz[1] / CIE_1931_Y_INTEGRAL_5NM;
        assert!(
            (y - 1.0).abs() < 1e-3,
            "equal-energy Y should be ~1.0 (got {y}); tolerance is looser than a 1e-4 an \
             exact analytic integral would allow because this is a 1nm-step rectangle-rule \
             quadrature of a piecewise-LINEAR interpolant of a 5nm table, not the closed-form \
             integral of a smooth curve"
        );
    }

    /// D65's chromaticity, reconstructed the same way `optics::raytracer::color`'s own
    /// `compute_illuminant_white_balance` does (401-point 1nm quadrature against the
    /// tabulated D65 SPD), must land near the standard D65 chromaticity (0.3127, 0.3290)
    /// -- confirms the new tabulated CMF, not just the old fit, reproduces a real,
    /// externally-known reference point.
    #[test]
    fn d65_chromaticity_matches_the_standard_value() {
        use crate::optics::raytracer::environment::d65_relative_spectral_power;

        let mut xyz = [0.0f32; 3];
        for step in 0..=(780 - 380) {
            let lambda = 380.0f32 + step as f32;
            let cmf = cie_1931_cmf(lambda);
            let power = d65_relative_spectral_power(lambda);
            xyz[0] = cmf[0].mul_add(power, xyz[0]);
            xyz[1] = cmf[1].mul_add(power, xyz[1]);
            xyz[2] = cmf[2].mul_add(power, xyz[2]);
        }
        let sum = xyz[0] + xyz[1] + xyz[2];
        let x = xyz[0] / sum;
        let y = xyz[1] / sum;
        assert!(
            (x - 0.3127).abs() < 2e-3,
            "D65 chromaticity x should be ~0.3127 (got {x})"
        );
        assert!(
            (y - 0.3290).abs() < 2e-3,
            "D65 chromaticity y should be ~0.3290 (got {y})"
        );
    }

    /// Away from the x-bar trough (e.g. at either lobe's own peak), the analytic fit and
    /// the table stay within 5% relative of each other -- the fit's problem is
    /// specifically the trough between its two lobes, not a globally wrong shape.
    #[test]
    fn legacy_fit_and_table_agree_within_5_percent_at_the_x_bar_lobe_peaks() {
        for lambda in [445.0f32, 600.0] {
            let table_x = cie_1931_cmf(lambda)[0];
            let fit_x = cie_1931_cmf_legacy_gaussian_fit(lambda)[0];
            let rel_err = (fit_x - table_x).abs() / table_x;
            assert!(
                rel_err < 0.05,
                "at {lambda}nm (a lobe peak): table x_bar={table_x}, fit x_bar={fit_x}, \
                 relative error {rel_err} should be < 5%"
            );
        }
    }

    /// The `x_bar` trough between the fit's two lobes (its near-zero minimum, around
    /// 495-510nm -- table entries as small as 0.0024) is exactly where the module doc
    /// comment says the Gaussian fit is worst: a sum of two asymmetric Gaussians
    /// doesn't fall to the real curve's near-zero trough value as sharply as the
    /// tabulated observer actually does, so the fit overshoots there by well over 5%
    /// relative -- this is why this module uses a table instead of an analytic fit.
    #[test]
    fn legacy_fit_diverges_from_the_table_at_the_x_bar_trough() {
        let lambda = 500.0f32;
        let table_x = cie_1931_cmf(lambda)[0];
        let fit_x = cie_1931_cmf_legacy_gaussian_fit(lambda)[0];
        let rel_err = (fit_x - table_x).abs() / table_x;
        assert!(
            rel_err > 0.05,
            "at {lambda}nm (the x_bar trough): table x_bar={table_x}, fit x_bar={fit_x}, \
             expected the old fit to diverge by MORE than 5% relative here -- if this now \
             passes with a smaller error, the trough claim in this module's doc comment is \
             stale and should be re-measured"
        );
    }
}
