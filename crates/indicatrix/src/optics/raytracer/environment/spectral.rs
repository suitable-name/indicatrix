//! Illuminant spectral power curves: the tabulated CIE D65 daylight measurement and the
//! Planckian blackbody approximation used for every other color temperature.

/// Tabulated CIE Standard Illuminant D65 relative spectral power distribution,
/// 380-780nm at 10nm intervals (41 values), as published in CIE 15:2004 "Colorimetry,"
/// 3rd ed., Table T.3. D65 is the standard's own measurement of "average daylight," NOT
/// a 6500K Planckian blackbody curve -- it has real irregularities (most visibly a
/// broad peak/dip through the blue-green region) no smooth Planckian curve reproduces.
/// See [`d65_relative_spectral_power`], used by the `"D65 Daylight"` [`super::LightingPreset`]
/// instead of [`blackbody_spectrum`] at 6500K.
const CIE_D65_SPD_380_780_10NM: [f32; 41] = [
    49.9755, 54.6482, 82.7549, 91.4860, 93.4318, 86.6823, 104.865, 117.008, 117.812, 114.861,
    115.923, 108.811, 109.354, 107.802, 104.790, 107.689, 104.405, 104.046, 100.000, 96.3342,
    95.7880, 88.6856, 90.0062, 89.5991, 87.6987, 83.2886, 83.6992, 80.0268, 80.2146, 82.2778,
    78.2842, 69.7213, 71.6091, 74.3496, 61.6045, 69.8856, 75.0870, 63.5928, 46.4182, 66.8054,
    63.3828,
];

/// CIE D65 relative spectral power, 300-380 nm at 10 nm intervals (9 values; the last is
/// the first entry of [`CIE_D65_SPD_380_780_10NM`]), CIE 15:2004 Table T.3 (the UV part
/// that fluorescence excitation paths need).
const CIE_D65_SPD_300_380_10NM: [f32; 9] = [
    0.0341, 3.2945, 20.236, 37.0535, 39.9488, 44.9117, 46.6383, 52.0891, 49.9755,
];

/// Linearly-interpolated CIE D65 relative spectral power at `lambda_nm`.
///
/// Normalized so that 560nm (the table's own `100.000` entry) reads exactly `1.0`,
/// matching [`blackbody_spectrum`]'s own normalization point -- both curves agree at
/// 560nm by construction, keeping the "D65 Daylight" preset's overall exposure/white
/// point unchanged relative to the old blackbody approximation.
///
/// The table is extended down to 300nm for fluorescence excitation paths
/// ([`CIE_D65_SPD_300_380_10NM`]); the result at and above 380nm is bit-identical to the
/// 380-780nm-only version. Wavelengths outside 300-780nm clamp to the nearest table edge.
/// (The GPU port in `shaders/*.wgsl` keeps the 380nm clamp: UV and fluorescent scenes
/// never reach the GPU.)
#[must_use]
pub fn d65_relative_spectral_power(lambda_nm: f32) -> f32 {
    const START_NM: f32 = 380.0;
    const STEP_NM: f32 = 10.0;
    const LAST_INDEX: usize = CIE_D65_SPD_380_780_10NM.len() - 1;

    if lambda_nm < START_NM {
        let clamped = lambda_nm.max(300.0);
        let position = (clamped - 300.0) / 10.0;
        let index0 = (position.floor() as usize).min(7);
        let frac = position - index0 as f32;
        let v0 = CIE_D65_SPD_300_380_10NM[index0];
        let v1 = CIE_D65_SPD_300_380_10NM[index0 + 1];
        return frac.mul_add(v1 - v0, v0) / 100.0;
    }

    let clamped = lambda_nm.clamp(START_NM, STEP_NM.mul_add(LAST_INDEX as f32, START_NM));
    let position = (clamped - START_NM) / STEP_NM;
    let index0 = (position.floor() as usize).min(LAST_INDEX - 1);
    let index1 = index0 + 1;
    let frac = position - index0 as f32;

    let v0 = CIE_D65_SPD_380_780_10NM[index0];
    let v1 = CIE_D65_SPD_380_780_10NM[index1];
    frac.mul_add(v1 - v0, v0) / 100.0
}

/// `hc / k_B` in nm * K, the Planck exponent's numerator.
const H_C_OVER_K_NM_K: f32 = 14_388_000.0;

/// The wavelength-independent constants of [`blackbody_spectrum`] at one color
/// temperature: the clamped temperature and the 560nm reference denominator
/// `max(exp(hc / (k 560 T)) - 1, 1e-6)`.
///
/// Both depend on `temp_k` alone, so a caller evaluating many wavelengths at one fixed
/// temperature builds this once and calls [`blackbody_spectrum_with`]; every value that
/// comes out is bit-identical to a per-call [`blackbody_spectrum`], because the same
/// expressions run on the same operands -- only the point in time they run at moves.
#[derive(Clone, Copy, Debug)]
pub(super) struct BlackbodyNorm {
    /// `temp_k` floored at 1000K, as [`blackbody_spectrum`] clamps it.
    t_k: f32,
    /// The 560nm normalisation point's denominator.
    denom_560: f32,
}

impl BlackbodyNorm {
    /// The constants for color temperature `temp_k`.
    pub(super) fn new(temp_k: f32) -> Self {
        let t_k = temp_k.max(1000.0);
        let exp_560 = (H_C_OVER_K_NM_K / (560.0 * t_k)).min(80.0).exp();
        let denom_560 = (exp_560 - 1.0).max(1e-6);
        Self { t_k, denom_560 }
    }
}

/// [`blackbody_spectrum`] with the temperature-only constants precomputed in `norm`.
pub(super) fn blackbody_spectrum_with(norm: BlackbodyNorm, lambda_nm: f32) -> f32 {
    let exp_val = (H_C_OVER_K_NM_K / (lambda_nm * norm.t_k)).min(80.0).exp();
    let denom = (exp_val - 1.0).max(1e-6);
    let ratio = norm.denom_560 / denom;
    ((560.0 / lambda_nm).powi(5) * ratio).clamp(0.01, 20.0)
}

/// Physical Planck Blackbody Spectral Radiance S(lambda, T) normalized to 1.0 at 560nm
#[must_use]
pub fn blackbody_spectrum(lambda_nm: f32, temp_k: f32) -> f32 {
    blackbody_spectrum_with(BlackbodyNorm::new(temp_k), lambda_nm)
}

/// A lighting preset's illuminant curve with its per-temperature constants resolved, so a
/// loop over wavelengths evaluates [`Self::power`] without redoing them. Built by
/// `LightingPreset::illuminant_spectrum`.
#[derive(Clone, Copy, Debug)]
pub(super) enum IlluminantSpectrum {
    /// The tabulated CIE D65 curve ([`d65_relative_spectral_power`]).
    D65,
    /// A Planckian fit at a fixed color temperature.
    Blackbody(BlackbodyNorm),
    /// A unit-peak Gaussian line (the UV lamps).
    Gaussian { centre_nm: f32, sigma_nm: f32 },
}

impl IlluminantSpectrum {
    /// Relative spectral power at `lambda_nm`.
    pub(super) fn power(self, lambda_nm: f32) -> f32 {
        match self {
            Self::D65 => d65_relative_spectral_power(lambda_nm),
            Self::Blackbody(norm) => blackbody_spectrum_with(norm, lambda_nm),
            Self::Gaussian {
                centre_nm,
                sigma_nm,
            } => {
                let z = (lambda_nm - centre_nm) / sigma_nm;
                (-0.5 * z * z).exp()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-extension algorithm, verbatim: the 380-780 nm table with clamped edges.
    fn d65_380_only(lambda_nm: f32) -> f32 {
        const START_NM: f32 = 380.0;
        const STEP_NM: f32 = 10.0;
        const LAST_INDEX: usize = CIE_D65_SPD_380_780_10NM.len() - 1;
        let clamped = lambda_nm.clamp(START_NM, STEP_NM.mul_add(LAST_INDEX as f32, START_NM));
        let position = (clamped - START_NM) / STEP_NM;
        let index0 = (position.floor() as usize).min(LAST_INDEX - 1);
        let index1 = index0 + 1;
        let frac = position - index0 as f32;
        let v0 = CIE_D65_SPD_380_780_10NM[index0];
        let v1 = CIE_D65_SPD_380_780_10NM[index1];
        frac.mul_add(v1 - v0, v0) / 100.0
    }

    /// Plan section 2: the extension to 300 nm leaves every value at and above 380 nm
    /// bit-identical (0 ULP), including the clamp above 780 nm.
    #[test]
    fn d65_extension_is_bit_identical_at_and_above_380nm() {
        for step in 0..=2430 {
            let lambda = (step as f32).mul_add(0.173, 380.0);
            assert_eq!(
                d65_relative_spectral_power(lambda).to_bits(),
                d65_380_only(lambda).to_bits(),
                "{lambda} nm"
            );
        }
        for lambda in [380.0f32, 400.0, 555.0, 560.0, 780.0, 1000.0] {
            assert_eq!(
                d65_relative_spectral_power(lambda).to_bits(),
                d65_380_only(lambda).to_bits()
            );
        }
    }

    /// Below 380 nm the CIE 15:2004 UV table (10 nm steps; 5 nm values interpolate), and
    /// a clamp at 300 nm.
    #[test]
    fn d65_matches_the_cie_table_down_to_300nm() {
        let cie = [
            (300.0f32, 0.0341f32),
            (310.0, 3.2945),
            (320.0, 20.236),
            (330.0, 37.0535),
            (340.0, 39.9488),
            (350.0, 44.9117),
            (360.0, 46.6383),
            (370.0, 52.0891),
            (380.0, 49.9755),
        ];
        for (lambda, value) in cie {
            let got = d65_relative_spectral_power(lambda);
            assert!((got - value / 100.0).abs() < 1e-6, "{lambda}: {got}");
        }
        // CIE's own 5 nm entries agree with the linear interpolation to a few percent.
        for (lambda, value) in [(305.0f32, 1.6643f32), (365.0, 49.3637), (375.0, 51.0323)] {
            let got = d65_relative_spectral_power(lambda) * 100.0;
            assert!((got - value).abs() < 3.5, "{lambda}: {got} vs {value}");
        }
        assert_eq!(
            d65_relative_spectral_power(250.0),
            d65_relative_spectral_power(300.0)
        );
        // Continuous across 380 nm.
        let below = d65_relative_spectral_power(379.999);
        let at = d65_relative_spectral_power(380.0);
        assert!((below - at).abs() < 1e-4);
    }
}
