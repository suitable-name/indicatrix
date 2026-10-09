//! Photometry for the rough-colour fit (plan 2026-10-09, sections 4.1, 4.2 and 3.3): turning rig
//! photos into calibrated, masked, working-resolution transmittance data.
//!
//! Only built with the `zoning` feature. No UI and no I/O beyond reading photo files.
//!
//! # Pipeline per view
//!
//! 1. **Decode** ([`decode_file`], [`decode_raw_file`], [`decode_standard_file`]) into a
//!    [`LinearImage`]: camera-native linear RGB for RAW (bilinear demosaic, no tone curve),
//!    inverse sRGB for 8-bit and non-linear 16-bit sources (flagged `non_linear_source`).
//! 2. **Calibration frames** ([`CalibrationFrames`], [`ViewCalibration`]): the transmittance is
//!    `(stone - dark) / (white - dark)`, with a noise-floor guard that masks pixels where the
//!    backlight does not clear the noise.
//! 3. **HDR merge** ([`merge_hdr`]) of two or more exposures before step 2.
//! 4. **Noise model** ([`estimate_noise`], [`NoiseModel`]): `variance = a + b * signal` per
//!    channel from flat patches of the white and dark frames.
//! 5. **Consistency** ([`check_view`]): warnings for EXIF, size and saturation mismatches.
//! 6. **Masks** ([`PixelMask`], [`flag`], [`mesh_masks`]): saturated, below noise, outside the
//!    outline, edge band, inclusion, ghost, user.
//! 7. **Working resolution** ([`WorkingGrid`], [`resample`]): the stone's region area-averaged
//!    to at most 384 pixels across, with the footprint of every working pixel kept for the
//!    forward model and the variances propagated.
//!
//! Everything is deterministic: no hashed collections, no randomness, fixed scan orders.

mod calibrate;
mod consistency;
mod decode;
mod exif;
mod hdr;
mod linear;
mod mask;
mod noise;
mod raw;
mod resample;
mod srgb;
#[cfg(test)]
mod tests;

pub use calibrate::{CalibrationFrames, TransmittanceImage, TransmittanceOptions, ViewCalibration};
pub use consistency::{
    ConsistencyOptions, ConsistencyReport, ConsistencyWarning, FrameRole, check_view,
};
pub use decode::{
    decode_file, decode_standard_bytes, decode_standard_file, from_rgb16, from_srgb8,
    icc_is_linear, linear_from_dynamic,
};
pub use exif::{parse_jpeg_exif, parse_tiff_exif};
pub use hdr::{HdrInput, HdrMerged, HdrOptions, merge_hdr};
pub use linear::{
    CameraColour, CaptureMeta, ColourMatrix, LinearImage, PhotometryError, SourceKind,
};
pub use mask::{InclusionMarker, MeshMaskOptions, PixelMask, flag, mesh_masks, stone_region};
pub use noise::{NoiseModel, NoiseOptions, estimate_noise};
pub use raw::{decode_raw_file, demosaic_bilinear, linear_from_raw};
pub use resample::{
    DEFAULT_WORKING_PX, ResampleOptions, ResampleSource, ResampledImage, WorkingGrid, resample,
};
pub use srgb::{
    COMPRESSION_SIGMA_ENCODED, COMPRESSION_SIGMA_MAX, encoding_variance, encoding_variance_with,
    estimate_compression_sigma, linear_to_srgb, srgb_to_linear,
};
