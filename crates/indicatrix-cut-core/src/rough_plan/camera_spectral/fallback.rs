//! Tier 3: a camera response from a camera-to-XYZ matrix and the CIE 1931 colour matching
//! functions.
//!
//! With `M` the matrix that maps camera RGB to XYZ, the camera responds to a spectral line at
//! `lambda` with `M^-1 * cmf(lambda)`, so `S_cam(lambda) = M^-1 * cmf(lambda)` on the grid. The
//! CIE 1931 functions are taken from `indicatrix::color::cie1931::cie_1931_cmf` (5 nm table,
//! exact at the grid wavelengths).
//!
//! # White point handling
//!
//! The CIE functions are the unadapted observer (equal-energy based). A DNG `ForwardMatrix` maps
//! white-balanced camera RGB to XYZ *adapted to D50* (the DNG profile connection space), so its
//! output is `Bradford(D65 -> D50)` applied to the native XYZ. [`MatrixXyzWhite::D50`] undoes
//! that with the Bradford matrix `D50 -> D65` before inverting, assuming the scene white was D65
//! (the usual daylight default; a different true illuminant leaves a residual of a few percent
//! in the channel ratios). [`MatrixXyzWhite::Native`] uses the matrix as is, which is right for
//! sRGB (linear sRGB -> XYZ is already D65 with the unadapted observer) and for a `ColorMatrix`
//! inverse that the caller has already converted.
//!
//! The white balance multipliers of a `ForwardMatrix` are per-channel scales on the camera side.
//! Because the rig data are transmittances relative to the empty backlit rig, a per-channel
//! scale of `S_cam` cancels and does not matter.

use super::{
    GRID_LEN, SpectralError, grid_wavelength_nm,
    response::{CameraResponse, ResponseTier},
};
use indicatrix::color::cie1931::cie_1931_cmf;

type Mat3 = [[f64; 3]; 3];

const WHITE_D50: [f64; 3] = [0.964_22, 1.0, 0.825_21];
const WHITE_D65: [f64; 3] = [0.950_47, 1.0, 1.088_83];
const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];
/// Linear sRGB to XYZ (D65).
const SRGB_TO_XYZ: Mat3 = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192_0, 0.950_304_1],
];

/// The white point the output XYZ of a camera-to-XYZ matrix refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixXyzWhite {
    /// Unadapted CIE XYZ (use the matrix as is). Right for sRGB.
    Native,
    /// XYZ adapted to D50 (DNG `ForwardMatrix`); undone with Bradford D50 to D65.
    D50,
}

pub(super) fn mul3(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                out[i][j] = a[i][k].mul_add(b[k][j], out[i][j]);
            }
        }
    }
    out
}

pub(super) fn mul_vec3(a: &Mat3, v: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for i in 0..3 {
        for k in 0..3 {
            out[i] = a[i][k].mul_add(v[k], out[i]);
        }
    }
    out
}

pub(super) fn invert3(m: &Mat3) -> Result<Mat3, SpectralError> {
    let c00 = m[1][1].mul_add(m[2][2], -(m[1][2] * m[2][1]));
    let c01 = m[1][2].mul_add(m[2][0], -(m[1][0] * m[2][2]));
    let c02 = m[1][0].mul_add(m[2][1], -(m[1][1] * m[2][0]));
    let det = m[0][2].mul_add(c02, m[0][1].mul_add(c01, m[0][0] * c00));
    let largest = m.iter().flatten().fold(0.0_f64, |a, v| a.max(v.abs()));
    if !det.is_finite() || det.abs() <= 1e-12 * largest.powi(3) {
        return Err(SpectralError::Singular);
    }
    let inv_det = 1.0 / det;
    Ok([
        [
            c00 * inv_det,
            m[0][2].mul_add(m[2][1], -(m[0][1] * m[2][2])) * inv_det,
            m[0][1].mul_add(m[1][2], -(m[0][2] * m[1][1])) * inv_det,
        ],
        [
            c01 * inv_det,
            m[0][0].mul_add(m[2][2], -(m[0][2] * m[2][0])) * inv_det,
            m[0][2].mul_add(m[1][0], -(m[0][0] * m[1][2])) * inv_det,
        ],
        [
            c02 * inv_det,
            m[0][1].mul_add(m[2][0], -(m[0][0] * m[2][1])) * inv_det,
            m[0][0].mul_add(m[1][1], -(m[0][1] * m[1][0])) * inv_det,
        ],
    ])
}

/// Bradford chromatic adaptation matrix from white `source` to white `target`.
pub(super) fn bradford_adaptation(
    source: [f64; 3],
    target: [f64; 3],
) -> Result<Mat3, SpectralError> {
    let source_cone = mul_vec3(&BRADFORD, source);
    let target_cone = mul_vec3(&BRADFORD, target);
    let mut gain = [[0.0; 3]; 3];
    for i in 0..3 {
        gain[i][i] = target_cone[i] / source_cone[i];
    }
    let inverse = invert3(&BRADFORD)?;
    Ok(mul3(&inverse, &mul3(&gain, &BRADFORD)))
}

/// Bradford adaptation D50 to D65.
pub(super) fn bradford_d50_to_d65() -> Result<Mat3, SpectralError> {
    bradford_adaptation(WHITE_D50, WHITE_D65)
}

impl CameraResponse {
    /// Tier 3: response `M_native^-1 * CMF(lambda)` from a camera-to-XYZ matrix (rows are X, Y,
    /// Z; columns camera R, G, B), e.g. a DNG `ForwardMatrix` with
    /// [`MatrixXyzWhite::D50`].
    ///
    /// Module docs describe the white point handling.
    ///
    /// # Errors
    /// [`SpectralError::Singular`] for a singular matrix, [`SpectralError::InvalidInput`] for
    /// non-finite entries.
    pub fn from_camera_to_xyz(
        camera_to_xyz: [[f64; 3]; 3],
        white: MatrixXyzWhite,
    ) -> Result<Self, SpectralError> {
        if camera_to_xyz.iter().flatten().any(|v| !v.is_finite()) {
            return Err(SpectralError::InvalidInput(
                "camera matrix holds a non-finite value".to_owned(),
            ));
        }
        let native = match white {
            MatrixXyzWhite::Native => camera_to_xyz,
            MatrixXyzWhite::D50 => mul3(&bradford_d50_to_d65()?, &camera_to_xyz),
        };
        let inverse = invert3(&native)?;
        let columns: [[f64; 3]; GRID_LEN] = std::array::from_fn(|i| {
            let cmf = cie_1931_cmf(grid_wavelength_nm(i) as f32);
            let xyz = [f64::from(cmf[0]), f64::from(cmf[1]), f64::from(cmf[2])];
            mul_vec3(&inverse, xyz)
        });
        let sensitivity: [[f64; GRID_LEN]; 3] =
            std::array::from_fn(|c| std::array::from_fn(|i| columns[i][c]));
        Self::from_grid(sensitivity, ResponseTier::MatrixFallback)
    }

    /// Tier 3 for non-RAW sources: linear sRGB to XYZ (D65, [`MatrixXyzWhite::Native`]).
    ///
    /// # Errors
    /// Never in practice (the sRGB matrix is regular); the `Result` keeps the signature uniform.
    pub fn from_srgb() -> Result<Self, SpectralError> {
        Self::from_camera_to_xyz(SRGB_TO_XYZ, MatrixXyzWhite::Native)
    }
}
