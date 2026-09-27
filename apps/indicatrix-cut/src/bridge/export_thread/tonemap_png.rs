//! Tone-map and PNG/ICC output: turning the finished accumulation buffer into RGBA
//! bytes (sRGB or wide-gamut) and writing it to disk.
//!
//! The tone-mapping itself ([`tonemap_accumulation`]) lives in
//! `indicatrix::renderer::tonemap` so a server answering a "final picture only"
//! `FinalImageRequest` runs the SAME code and its PNG is byte-identical to what this
//! viewer would write from the same float sum; it is re-exported here so every caller in
//! this crate stays unchanged. Writing the file ([`save_png`], ICC embedding included)
//! stays viewer-side: a server's final picture is decoded to RGBA8 and written through
//! [`save_png`] exactly like a local render.

use super::icc_profile;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use indicatrix::color::ColorSpace;
pub use indicatrix::renderer::tonemap::tonemap_accumulation;
use std::{io::BufWriter, path::Path};

/// Writes `rgba` (`width * height * 4` bytes) to `path` as PNG. `ColorSpace::Srgb`
/// goes through the plain `image::RgbaImage::save` call -- untagged, since every
/// viewer already assumes sRGB for an unlabeled PNG. Any other space is written via
/// `PngEncoder` directly so an ICC profile (`icc_profile::build`) can be attached
/// first: an untagged Display P3/Rec.2020 PNG would be silently misread as sRGB.
pub(super) fn save_png(
    path: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
    color_space: ColorSpace,
) -> Result<(), String> {
    if color_space == ColorSpace::Srgb {
        return image::RgbaImage::from_raw(width, height, rgba.to_vec()).map_or_else(
            || Err("Internal error: pixel buffer size did not match dimensions.".to_string()),
            |img| img.save(path).map_err(|e| e.to_string()),
        );
    }

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = PngEncoder::new(BufWriter::new(file));
    encoder
        .set_icc_profile(icc_profile::build(color_space))
        // Only fails for an encoder that can't carry a profile at all -- PNG can, so
        // this is unreachable in practice, but threaded through rather than unwrapped.
        .map_err(|e| e.to_string())?;
    encoder
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}
