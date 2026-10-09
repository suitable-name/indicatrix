//! In-memory PNG encoding with an embedded ICC profile for wide-gamut color spaces.
//!
//! Behind the `hdr` feature: that feature already pulls in the `image` crate (for
//! `renderer::env_map`'s Radiance `.hdr` decoding), so reusing it here for PNG
//! encoding costs nothing beyond one extra codec feature (`image`'s `png`, added
//! alongside `hdr` in this crate's `Cargo.toml`) rather than a genuinely new
//! dependency.
//!
//! [`encode_png_with_icc`] is pure/in-memory -- no filesystem access -- so both the
//! desktop viewer (whose `save_png` writes the returned bytes to a file) and the
//! browser app (which hands them to a download) produce byte-identical PNGs for the
//! same pixel buffer and color space.

use super::icc_profile;
use crate::color::ColorSpace;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

/// Encodes `rgba` (`width * height * 4` bytes) as PNG bytes.
///
/// `ColorSpace::Srgb` is written untagged -- every viewer already assumes sRGB for an
/// unlabeled PNG. Any other space embeds an ICC profile ([`icc_profile::build`])
/// first: an untagged Display P3/Rec.2020 PNG would be silently misread as sRGB.
///
/// # Errors
///
/// Returns `Err` when `rgba`'s length doesn't match `width * height * 4`, or when the
/// underlying PNG encoder itself fails (in practice, only for a write failure into the
/// in-memory buffer, which does not happen).
pub fn encode_png_with_icc(
    rgba: &[u8],
    width: u32,
    height: u32,
    color_space: ColorSpace,
) -> Result<Vec<u8>, String> {
    if rgba.len() != (width as usize) * (height as usize) * 4 {
        return Err("Internal error: pixel buffer size did not match dimensions.".to_string());
    }

    let mut bytes = Vec::new();
    let mut encoder = PngEncoder::new(&mut bytes);
    if color_space != ColorSpace::Srgb {
        encoder
            .set_icc_profile(icc_profile::build(color_space))
            // Only fails for an encoder that can't carry a profile at all -- PNG can,
            // so this is unreachable in practice, but threaded through rather than
            // unwrapped.
            .map_err(|e| e.to_string())?;
    }
    encoder
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// The finished catalogue-preview picture for a float radiance sum.
///
/// Divides `accum` by `samples`, tone-maps it to sRGB
/// ([`crate::renderer::tonemap::tonemap_to_rgba`] at `1.0 / samples`) and encodes the result as
/// an untagged sRGB PNG.
///
/// The ONE function the viewer's local preview path and a remote worker's
/// `BATCH_ITEM_DONE` both call, so a preview rendered remotely is byte-identical to one
/// rendered locally from the same sum. `None` only when `accum` is not
/// `width * height` long or the encoder fails (neither happens for a well-formed sum).
/// `samples` of zero is treated as one.
#[must_use]
pub fn encode_preview_png(
    width: u32,
    height: u32,
    accum: &[glam::Vec3],
    samples: u32,
) -> Option<Vec<u8>> {
    if accum.len() != (width as usize) * (height as usize) {
        return None;
    }
    let rgba = crate::renderer::tonemap::tonemap_to_rgba(accum, 1.0 / samples.max(1) as f32);
    encode_png_with_icc(&rgba, width, height, ColorSpace::Srgb).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_png_is_the_srgb_tone_map_of_the_averaged_sum() {
        let accum: Vec<glam::Vec3> = (0..12)
            .map(|i| glam::Vec3::new(0.2, 0.3, 0.1) * (i as f32 + 1.0) * 8.0)
            .collect();
        let png = encode_preview_png(4, 3, &accum, 8).expect("a well-formed sum encodes");
        let rgba = crate::renderer::tonemap::tonemap_to_rgba(&accum, 1.0 / 8.0);
        let expected = encode_png_with_icc(&rgba, 4, 3, ColorSpace::Srgb).unwrap();
        assert_eq!(png, expected);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn preview_png_refuses_a_wrongly_sized_sum() {
        assert!(encode_preview_png(4, 3, &[glam::Vec3::ZERO; 11], 1).is_none());
    }

    fn fixed_rgba(width: u32, height: u32) -> Vec<u8> {
        (0..width * height * 4)
            .map(|i| (i * 37 + 11).to_le_bytes()[0])
            .collect()
    }

    #[test]
    fn rejects_a_mismatched_buffer_size() {
        let err = encode_png_with_icc(&[0, 0, 0, 0], 2, 2, ColorSpace::Srgb).unwrap_err();
        assert!(err.contains("did not match dimensions"));
    }

    #[test]
    fn srgb_and_display_p3_encode_to_different_bytes_for_the_same_pixels() {
        let rgba = fixed_rgba(4, 3);
        let srgb = encode_png_with_icc(&rgba, 4, 3, ColorSpace::Srgb).unwrap();
        let p3 = encode_png_with_icc(&rgba, 4, 3, ColorSpace::DisplayP3).unwrap();
        assert_ne!(srgb, p3, "an embedded ICC profile must change the bytes");
    }

    #[test]
    fn encoding_is_deterministic_for_identical_input() {
        let rgba = fixed_rgba(5, 5);
        let a = encode_png_with_icc(&rgba, 5, 5, ColorSpace::Rec2020).unwrap();
        let b = encode_png_with_icc(&rgba, 5, 5, ColorSpace::Rec2020).unwrap();
        assert_eq!(a, b);
    }
}
