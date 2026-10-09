//! Camera spectral sensitivity and backlight spectrum for the rig colour fit (feature `zoning`,
//! plan 2026-10-09 sections 4.3 and 4.4).
//!
//! The colour fit needs `S_cam(lambda)`, the 3-channel spectral sensitivity of the camera that
//! took the transmission photos, and `E(lambda)`, the spectrum of the backlight. Everything here
//! lives on one fixed wavelength grid, 380 to 780 nm in 5 nm steps ([`GRID_LEN`] = 81 samples,
//! the same grid as the CIE 1931 table and the CIE LED illuminant table of `indicatrix`).
//!
//! # Three tiers of camera response ([`ResponseTier`])
//!
//! 1. **Measured**: [`CameraResponse::from_csv`] imports `wavelength, r, g, b` rows of any
//!    spacing and resamples them linearly onto the grid.
//! 2. **Filter calibrated**: [`calibrate_from_filters`] solves for the 3 x 81 matrix from at
//!    least [`MIN_REFERENCE_FILTERS`] reference filters of known transmission, photographed in
//!    the rig. Regularised non-negative least squares with a second-difference smoothness prior;
//!    the weight is chosen by generalised cross-validation over [`LAMBDA_GRID_LEN`] fixed values.
//! 3. **Matrix fallback**: [`CameraResponse::from_camera_to_xyz`] (DNG `ColorMatrix` /
//!    `ForwardMatrix`) and [`CameraResponse::from_srgb`] build `M^-1 * CMF` on the grid.
//!
//! # Units of a response
//!
//! A response from tiers 1 and 3 keeps whatever scale it was given. A tier 2 response is
//! normalised so that the backlight-weighted mean of every channel is 1 (see
//! [`backlight_weights`]); the rig data are transmittances *relative to the backlight*
//! ((stone - dark) / (white - dark)), which cannot fix an absolute scale. Consequently
//! `camera_rgb(E * T) / camera_rgb(E)` is the quantity that is meaningful for every tier, and it
//! is scale free per channel.
//!
//! # Determinism
//!
//! All arithmetic is `f64`, with fixed loop orders and no hashed collections or randomness. The
//! tilt fit in [`backlight`] uses `exp`/`ln`, so results are reproducible for one binary on one
//! platform, not bitwise across libm implementations.

mod backlight;
mod calibrate;
mod csv;
mod fallback;
mod linalg;
mod response;
#[cfg(test)]
mod tests;

pub use backlight::{
    BacklightFit, BacklightSource, BacklightSpectrum, SpectralTilt, fit_white_point_tilt,
};
pub use calibrate::{
    CalibrationReport, LAMBDA_GRID_LEN, MIN_REFERENCE_FILTERS, ReferenceFilter, backlight_weights,
    calibrate_from_filters,
};
pub use fallback::MatrixXyzWhite;
pub use response::{CameraResponse, ResponseTier};

/// First wavelength of the grid in nm.
pub const GRID_FIRST_NM: f64 = 380.0;
/// Grid spacing in nm.
pub const GRID_STEP_NM: f64 = 5.0;
/// Number of grid samples (380 to 780 nm inclusive).
pub const GRID_LEN: usize = 81;

/// Wavelength in nm of grid sample `index`.
#[must_use]
pub const fn grid_wavelength_nm(index: usize) -> f64 {
    GRID_STEP_NM.mul_add(index as f64, GRID_FIRST_NM)
}

/// Errors of the import, calibration and fitting functions in this module.
#[derive(Debug, Clone, PartialEq)]
pub enum SpectralError {
    /// A text line could not be read (1-based line number).
    Parse {
        /// 1-based line number in the input text.
        line: usize,
        /// What was wrong.
        message: String,
    },
    /// Fewer than two data rows.
    TooFewRows {
        /// Number of rows found.
        found: usize,
    },
    /// Wavelengths are not strictly increasing; `index` is the first offending row (0-based).
    NotMonotone {
        /// 0-based row index of the first row that does not increase.
        index: usize,
    },
    /// A value below the tolerated -1e-3 (0-based row index).
    NegativeValue {
        /// 0-based row index.
        index: usize,
        /// The value found.
        value: f64,
    },
    /// The data do not overlap the 380 to 780 nm grid.
    NoOverlap,
    /// Fewer reference filters than the calibration needs.
    TooFewFilters {
        /// Filters supplied.
        found: usize,
        /// Filters required.
        required: usize,
    },
    /// The camera matrix is singular.
    Singular,
    /// Any other invalid input, with a description.
    InvalidInput(String),
}

impl core::fmt::Display for SpectralError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Parse { line, message } => write!(f, "line {line}: {message}"),
            Self::TooFewRows { found } => {
                write!(f, "need at least 2 data rows, found {found}")
            }
            Self::NotMonotone { index } => {
                write!(f, "wavelengths must increase strictly (row {index})")
            }
            Self::NegativeValue { index, value } => {
                write!(f, "negative value {value} in row {index} (limit -0.001)")
            }
            Self::NoOverlap => write!(f, "the data do not overlap 380 to 780 nm"),
            Self::TooFewFilters { found, required } => {
                write!(f, "need at least {required} reference filters, got {found}")
            }
            Self::Singular => write!(f, "the camera matrix is singular"),
            Self::InvalidInput(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SpectralError {}
