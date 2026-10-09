//! The camera sensitivity matrix and tier 1 (measured) import.

use super::{
    GRID_LEN, GRID_STEP_NM, SpectralError,
    csv::{parse_table, resample_to_grid},
    grid_wavelength_nm,
};

/// Where a [`CameraResponse`] came from, best to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseTier {
    /// Imported sensitivity curves.
    Measured,
    /// Solved from reference filters photographed in the rig.
    FilterCalibrated,
    /// Built from a camera-to-XYZ matrix and the CIE 1931 colour matching functions.
    MatrixFallback,
}

/// A 3 x 81 spectral sensitivity matrix on the 380 to 780 nm, 5 nm grid.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraResponse {
    sensitivity: [[f64; GRID_LEN]; 3],
    tier: ResponseTier,
}

/// Trapezoid weight (times the step) of grid sample `index`.
#[must_use]
pub(super) const fn trapezoid_weight(index: usize) -> f64 {
    if index == 0 || index + 1 == GRID_LEN {
        0.5 * GRID_STEP_NM
    } else {
        GRID_STEP_NM
    }
}

impl CameraResponse {
    /// Wrap an already-gridded matrix. Values must be finite (matrix-derived responses may be
    /// negative, so no sign check is made here).
    ///
    /// # Errors
    /// [`SpectralError::InvalidInput`] if any value is not finite.
    pub fn from_grid(
        sensitivity: [[f64; GRID_LEN]; 3],
        tier: ResponseTier,
    ) -> Result<Self, SpectralError> {
        if sensitivity.iter().flatten().any(|v| !v.is_finite()) {
            return Err(SpectralError::InvalidInput(
                "camera response holds a non-finite value".to_owned(),
            ));
        }
        Ok(Self { sensitivity, tier })
    }

    /// Tier 1: build from samples at arbitrary, strictly increasing wavelengths (nm).
    ///
    /// # Errors
    /// Length mismatch, non-monotone wavelengths, negative values beyond -1e-3, no overlap with
    /// the grid.
    pub fn from_samples(
        wavelengths_nm: &[f64],
        r: &[f64],
        g: &[f64],
        b: &[f64],
    ) -> Result<Self, SpectralError> {
        let channels = [
            resample_to_grid(wavelengths_nm, r)?,
            resample_to_grid(wavelengths_nm, g)?,
            resample_to_grid(wavelengths_nm, b)?,
        ];
        Self::from_grid(channels, ResponseTier::Measured)
    }

    /// Tier 1: import a CSV with columns `wavelength_nm, r, g, b`.
    ///
    /// Any spacing; delimiters comma, semicolon, tab or blanks; `#` comments and one header
    /// line are skipped. Resampled linearly onto the grid (0 outside the source range).
    ///
    /// # Errors
    /// See [`SpectralError`].
    pub fn from_csv(text: &str) -> Result<Self, SpectralError> {
        let rows = parse_table(text, 4)?;
        let column = |c: usize| rows.iter().map(|row| row[c]).collect::<Vec<f64>>();
        Self::from_samples(&column(0), &column(1), &column(2), &column(3))
    }

    /// How this response was obtained.
    #[must_use]
    pub const fn tier(&self) -> ResponseTier {
        self.tier
    }

    /// The matrix, channel-major (r, g, b), 81 samples each.
    #[must_use]
    pub const fn sensitivity(&self) -> &[[f64; GRID_LEN]; 3] {
        &self.sensitivity
    }

    /// Camera RGB of `spectrum(lambda_nm)` (any radiometric quantity): trapezoid integral of
    /// `S_c(lambda) * spectrum(lambda)` over the grid, in nm.
    #[must_use]
    pub fn camera_rgb(&self, spectrum: &dyn Fn(f64) -> f64) -> [f64; 3] {
        let mut samples = [0.0; GRID_LEN];
        for (i, slot) in samples.iter_mut().enumerate() {
            *slot = spectrum(grid_wavelength_nm(i));
        }
        self.camera_rgb_grid(&samples)
    }

    /// [`Self::camera_rgb`] for a spectrum already sampled on the grid.
    #[must_use]
    pub fn camera_rgb_grid(&self, spectrum: &[f64; GRID_LEN]) -> [f64; 3] {
        let mut out = [0.0; 3];
        for (channel, slot) in self.sensitivity.iter().zip(out.iter_mut()) {
            let mut sum = 0.0;
            for i in 0..GRID_LEN {
                sum = trapezoid_weight(i).mul_add(channel[i] * spectrum[i], sum);
            }
            *slot = sum;
        }
        out
    }
}
