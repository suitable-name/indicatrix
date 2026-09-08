//! RGB -> spectral radiance reconstruction.
//!
//! `indicatrix`'s tracer needs radiance at a single continuous wavelength per channel
//! (see `optics::raytracer::sample_studio_environment`'s `lambda_nm` parameter), not
//! an RGB triple, but an HDR environment map is stored as RGB texels. This module
//! bridges the two, kept to one small, explicitly replaceable function.
//!
//! # Method: three overlapping Gaussian "primary" bumps
//!
//! [`rgb_to_spectral_radiance`] treats the RGB triple as weights on three fixed,
//! smooth, strictly non-negative basis functions of wavelength -- asymmetric Gaussian
//! bumps centred near a red, green, and blue primary (615 nm / 545 nm / 465 nm).
//! Spectral radiance at `lambda_nm` is the weighted sum
//! `r*R(lambda) + g*G(lambda) + b*B(lambda)`.
//!
//! Deliberately the simplest defensible option, not the most accurate one: unlike a
//! real spectral upsampler (e.g. Jakob & Hanika 2019) it gives no round-trip guarantee
//! back to the input RGB and no metamerism, so HDR-sourced dispersion "fire" is
//! smoother/less spiky than a narrow-band real source. Also not designed for RGB
//! above 1.0 -- an extreme highlight just scales the bump height rather than acting
//! narrow-band. A real upsampler would slot in here since every call funnels through
//! this one function.

/// Converts a linear RGB radiance (or reflectance-like) triple into an approximate
/// spectral radiance value at `lambda_nm` (nanometres). See the module docs for the
/// method and its limitations.
///
/// Negative input components are treated as `0.0` (radiance should never be negative,
/// but a caller should not have to pre-clamp). The result is always finite and
/// non-negative for finite, non-negative-clamped input.
#[must_use]
pub fn rgb_to_spectral_radiance(rgb: [f32; 3], lambda_nm: f32) -> f32 {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);

    r.mul_add(
        asymmetric_gaussian(lambda_nm, 615.0, 45.0, 65.0),
        g.mul_add(
            asymmetric_gaussian(lambda_nm, 545.0, 45.0, 45.0),
            b * asymmetric_gaussian(lambda_nm, 465.0, 40.0, 45.0),
        ),
    )
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
}
