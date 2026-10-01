//! Illuminant spectral power curves: the tabulated CIE D65 daylight measurement and the
//! Planckian blackbody approximation used for every other colour temperature.

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

/// Linearly-interpolated CIE D65 relative spectral power at `lambda_nm`.
///
/// Normalized so that 560nm (the table's own `100.000` entry) reads exactly `1.0`,
/// matching [`blackbody_spectrum`]'s own normalization point -- both curves agree at
/// 560nm by construction, keeping the "D65 Daylight" preset's overall exposure/white
/// point unchanged relative to the old blackbody approximation.
///
/// Wavelengths outside the tabulated 380-780nm range clamp to the nearest table edge
/// (a defensive bound: this renderer's own channel sampling never leaves 380-780nm).
#[must_use]
pub fn d65_relative_spectral_power(lambda_nm: f32) -> f32 {
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

/// `hc / k_B` in nm * K, the Planck exponent's numerator.
const H_C_OVER_K_NM_K: f32 = 14_388_000.0;

/// The wavelength-independent constants of [`blackbody_spectrum`] at one colour
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
    /// The constants for colour temperature `temp_k`.
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
    /// A Planckian fit at a fixed colour temperature.
    Blackbody(BlackbodyNorm),
}

impl IlluminantSpectrum {
    /// Relative spectral power at `lambda_nm`.
    pub(super) fn power(self, lambda_nm: f32) -> f32 {
        match self {
            Self::D65 => d65_relative_spectral_power(lambda_nm),
            Self::Blackbody(norm) => blackbody_spectrum_with(norm, lambda_nm),
        }
    }
}
