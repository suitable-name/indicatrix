//! Tone-map and PNG/ICC output: turning the finished accumulation buffer into RGBA
//! bytes (sRGB or wide-gamut) and writing it to disk.
//!
//! Split out of `bridge::export_thread` purely to keep that module (already sizeable)
//! from growing further.

use super::icc_profile;
use glam::Vec3;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use indicatrix::{
    color::{ColorSpace, ToneMap},
    renderer::tonemap::tonemap_to_rgba as tonemap_xyz_buffer,
};
use std::{io::BufWriter, path::Path, thread};

/// Tone-maps the finished accumulation buffer to RGBA bytes using the exact same
/// `xyz_to_srgb_gamma` the live viewport uses every frame, so an sRGB export matches
/// what was on screen. The ONLY path used for `ColorSpace::Srgb` -- see
/// [`tonemap_wide_gamut`] for every other offered space.
pub(super) fn tonemap_to_rgba(
    width: u32,
    height: u32,
    total_samples: u32,
    accum: &[Vec3],
) -> Vec<u8> {
    debug_assert_eq!(
        accum.len(),
        (width as usize) * (height as usize),
        "accum must hold exactly width*height pixels"
    );
    let inv_samples = 1.0 / total_samples as f32;
    // Parallelised via `indicatrix::renderer::tonemap`'s `xyz_to_srgb_gamma`-based
    // helper -- `render_thread`'s two tone-mapping call sites share this same function.
    tonemap_xyz_buffer(accum, inv_samples)
}

/// Tone-maps the finished accumulation buffer to RGBA bytes for any non-`Srgb`
/// [`ColorSpace`], via `ColorSpace::encode` with `ToneMap::AcesFilmic { exposure: 1.0
/// }` -- that reproduces `xyz_to_srgb_gamma`'s gamut-compression/tone-mapping exactly,
/// so only `color_space`'s primaries and transfer curve change relative to
/// [`tonemap_to_rgba`], never exposure or tone-curve shape. Parallelised the same way
/// `indicatrix::renderer::tonemap::tonemap_to_rgba_with_threads` is, duplicated here
/// since that function is hardwired to `xyz_to_srgb_gamma`.
pub(super) fn tonemap_wide_gamut(
    width: u32,
    height: u32,
    total_samples: u32,
    accum: &[Vec3],
    color_space: ColorSpace,
) -> Vec<u8> {
    debug_assert_eq!(
        accum.len(),
        (width as usize) * (height as usize),
        "accum must hold exactly width*height pixels"
    );
    let inv_samples = 1.0 / total_samples as f32;
    let mut out = vec![0u8; accum.len() * 4];
    if accum.is_empty() {
        return out;
    }

    let num_threads = thread::available_parallelism().map_or(8, std::num::NonZero::get);
    let chunk_len = accum.len().div_ceil(num_threads).max(1);

    thread::scope(|s| {
        let color_chunks = accum.chunks(chunk_len);
        let byte_chunks = out.chunks_mut(chunk_len * 4);
        for (colors, dst) in color_chunks.zip(byte_chunks) {
            s.spawn(move || {
                for (i, xyz) in colors.iter().enumerate() {
                    let rgba = color_space
                        .encode(*xyz * inv_samples, ToneMap::AcesFilmic { exposure: 1.0 });
                    dst[i * 4..i * 4 + 4].copy_from_slice(&rgba);
                }
            });
        }
    });

    out
}

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
