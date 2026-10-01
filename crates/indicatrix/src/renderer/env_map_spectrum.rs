//! RGB -> spectral radiance reconstruction.
//!
//! `indicatrix`'s tracer needs radiance at a single continuous wavelength per channel
//! (see `optics::raytracer::sample_studio_environment`'s `lambda_nm` parameter), not
//! an RGB triple, but an HDR environment map is stored as RGB texels. This module
//! bridges the two, kept to one small, explicitly replaceable function.
//!
//! # Method: a neutral part and a chroma remainder, each on its own bump basis
//!
//! [`rgb_to_spectral_radiance`] splits the (non-negative) triple into `w = min(r, g, b)`
//! and the chroma `(r - w, g - w, b - w)`, which is non-negative with at least one zero
//! component. The spectrum is the sum of one bump spectrum for each part.
//!
//! * **Neutral part.** `(w, w, w)` is lifted with three wide, overlapping, asymmetric
//!   Gaussian bumps centred near a red, green and blue primary (615 nm / 545 nm / 465 nm).
//!   Summing the bumps directly would not be neutral: against the CIE 1931 observer,
//!   `rgb = (1, 1, 1)` lands near xy (0.345, 0.381) at about 1.43 times the luminance of
//!   D65 white. The triple is therefore first mapped through the fixed 3x3 matrix
//!   `NEUTRAL_TO_BUMP` to bump coefficients, chosen so that the spectrum's CIE XYZ
//!   (normalised by the Y integral) equals the linear-sRGB-to-XYZ image of the triple.
//!   The wide bumps keep every grey texel's spectrum smooth. For a grey input the chroma
//!   is exactly zero, so the result is bit-for-bit the wide-bump spectrum alone.
//! * **Chroma part.** The chroma is lifted with a second, narrower bump triple
//!   (635 nm / 540 nm / 450 nm) through `CHROMA_TO_BUMP`, solved against the same
//!   observer so that the spectrum's XYZ equals the sRGB image of the chroma. The narrow
//!   bumps overlap so little that the matrix is entrywise positive: any non-negative
//!   chroma then yields non-negative coefficients and the spectrum is non-negative
//!   without any clamping.
//!
//! # Why it is exact
//!
//! XYZ is linear in the spectrum. The neutral part reproduces the XYZ of `(w, w, w)`
//! exactly, the chroma part reproduces the XYZ of the chroma exactly because no
//! coefficient is negative (so the final `max(0, ..)` never removes anything), and the
//! sum is the XYZ of the input. Every in-gamut linear-sRGB colour, including the primaries
//! and secondaries, and every HDR value, therefore round-trips through the CIE 1931
//! observer to within single-precision rounding.
//!
//! The matrix constants, the bump parameters and the exact `mul_add` (`fma`) order below
//! are mirrored by the WGSL twins `transport_physics/05_nee_env_sampling.wgsl` and
//! `environment.wgsl`; keep all copies in lock-step so CPU and GPU agree bit for bit.
//!
//! # Limitations
//!
//! Deliberately the simplest defensible option, not the most physical one: unlike a real
//! spectral upsampler (e.g. Jakob & Hanika 2019) it has no metamerism -- each colour gets
//! one fixed smooth spectrum -- so HDR-sourced dispersion "fire" is smoother than for a
//! narrow-band real source. Values above 1.0 only scale the bump heights rather than
//! acting narrow-band. A real upsampler would slot in here since every call funnels
//! through this one function.

/// Row-major matrix taking a neutral triple `(w, w, w)` of linear sRGB to the coefficients
/// of the three wide bumps; see the module docs for the property it guarantees.
const NEUTRAL_TO_BUMP: [[f32; 3]; 3] = [
    [0.823_989_87, -0.305_067_78, 0.263_915_57],
    [-0.297_157_8, 1.252_894_9, -0.461_147_6],
    [0.078_837_544, -0.122_806_296, 1.231_782_6],
];

/// Row-major matrix taking a non-negative chroma triple of linear sRGB to the coefficients
/// of the three narrow bumps. Every entry is positive, so non-negative chroma gives
/// non-negative coefficients; see the module docs.
const CHROMA_TO_BUMP: [[f32; 3]; 3] = [
    [1.099_529_1, 0.087_112_45, 0.017_712_66],
    [0.007_455_747, 1.334_538_5, 0.014_020_192],
    [0.024_874_75, 0.058_899_768, 1.260_904_8],
];

/// Dot product of one matrix row with a triple, in the `fma` nesting shared with the
/// WGSL twins.
#[inline]
fn row_dot(m: &[f32; 3], v: [f32; 3]) -> f32 {
    m[0].mul_add(v[0], m[1].mul_add(v[1], m[2] * v[2]))
}

/// Converts a linear RGB radiance (or reflectance-like) triple into a spectral radiance
/// value at `lambda_nm` (nanometres). See the module docs for the method and its
/// limitations.
///
/// Negative input components are treated as `0.0` (radiance should never be negative,
/// but a caller should not have to pre-clamp). The result is always finite and
/// non-negative for finite, non-negative-clamped input.
#[must_use]
pub fn rgb_to_spectral_radiance(rgb: [f32; 3], lambda_nm: f32) -> f32 {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);
    let w = r.min(g).min(b);
    let chroma = [r - w, g - w, b - w];
    let grey = [w, w, w];

    let n_r = row_dot(&NEUTRAL_TO_BUMP[0], grey);
    let n_g = row_dot(&NEUTRAL_TO_BUMP[1], grey);
    let n_b = row_dot(&NEUTRAL_TO_BUMP[2], grey);
    let neutral = n_r.mul_add(
        asymmetric_gaussian(lambda_nm, 615.0, 45.0, 65.0),
        n_g.mul_add(
            asymmetric_gaussian(lambda_nm, 545.0, 45.0, 45.0),
            n_b * asymmetric_gaussian(lambda_nm, 465.0, 40.0, 45.0),
        ),
    );

    let k_r = row_dot(&CHROMA_TO_BUMP[0], chroma);
    let k_g = row_dot(&CHROMA_TO_BUMP[1], chroma);
    let k_b = row_dot(&CHROMA_TO_BUMP[2], chroma);
    let saturated = k_r.mul_add(
        asymmetric_gaussian(lambda_nm, 635.0, 28.0, 39.2),
        k_g.mul_add(
            asymmetric_gaussian(lambda_nm, 540.0, 28.0, 28.0),
            k_b * asymmetric_gaussian(lambda_nm, 450.0, 25.2, 28.0),
        ),
    );

    (neutral + saturated).max(0.0)
}

/// A Gaussian bump centred at `mu`, using `sigma_lo` below the peak and `sigma_hi` above
/// it -- the asymmetry is what keeps three overlapping bumps from collapsing into an
/// almost-flat sum across the visible range while still individually staying smooth.
#[inline]
fn asymmetric_gaussian(x: f32, mu: f32, sigma_lo: f32, sigma_hi: f32) -> f32 {
    let sigma = if x < mu { sigma_lo } else { sigma_hi };
    let t = (x - mu) / sigma;
    (-0.5 * t * t).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rgb_gives_zero_spectrum() {
        for lambda in [380.0, 500.0, 560.0, 650.0, 780.0] {
            assert_eq!(rgb_to_spectral_radiance([0.0, 0.0, 0.0], lambda), 0.0);
        }
    }

    #[test]
    fn negative_components_are_clamped_not_propagated() {
        let v = rgb_to_spectral_radiance([-1.0, -2.0, -3.0], 550.0);
        assert_eq!(v, 0.0);
        assert!(!v.is_nan());
    }

    #[test]
    fn result_is_always_finite_and_non_negative() {
        for lambda in [300.0, 380.0, 450.0, 550.0, 650.0, 780.0, 900.0] {
            let v = rgb_to_spectral_radiance([1.0, 2.5, 100.0], lambda);
            assert!(v.is_finite());
            assert!(v >= 0.0);
        }
    }

    #[test]
    fn red_weight_dominates_the_red_end_of_the_spectrum() {
        let red_only = rgb_to_spectral_radiance([1.0, 0.0, 0.0], 615.0);
        let blue_only = rgb_to_spectral_radiance([0.0, 0.0, 1.0], 615.0);
        assert!(
            red_only > blue_only,
            "at 615nm the red basis peak should dominate the blue basis's tail"
        );
    }

    #[test]
    fn neutral_input_is_the_wide_bump_spectrum_bit_for_bit() {
        let reference = |l: f32, lambda: f32| -> f32 {
            let row = |m: &[f32; 3]| m[0].mul_add(l, m[1].mul_add(l, m[2] * l));
            let c_r = row(&NEUTRAL_TO_BUMP[0]);
            let c_g = row(&NEUTRAL_TO_BUMP[1]);
            let c_b = row(&NEUTRAL_TO_BUMP[2]);
            c_r.mul_add(
                asymmetric_gaussian(lambda, 615.0, 45.0, 65.0),
                c_g.mul_add(
                    asymmetric_gaussian(lambda, 545.0, 45.0, 45.0),
                    c_b * asymmetric_gaussian(lambda, 465.0, 40.0, 45.0),
                ),
            )
            .max(0.0)
        };
        for l in [0.0, 0.001, 0.18, 0.5, 1.0, 3.7, 250.0] {
            for lambda in [
                380.0, 420.0, 465.0, 500.0, 545.0, 580.0, 615.0, 700.0, 780.0,
            ] {
                assert_eq!(
                    rgb_to_spectral_radiance([l, l, l], lambda).to_bits(),
                    reference(l, lambda).to_bits(),
                    "grey {l} at {lambda} nm"
                );
            }
        }
    }

    #[test]
    fn chroma_matrix_is_entrywise_positive() {
        for (i, row) in CHROMA_TO_BUMP.iter().enumerate() {
            for (j, &entry) in row.iter().enumerate() {
                assert!(entry > 0.0, "CHROMA_TO_BUMP[{i}][{j}] = {entry}");
            }
        }
    }
}
