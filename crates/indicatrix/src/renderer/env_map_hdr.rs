//! [`environment_from_hdr_bytes`]: the ONE function that turns Radiance `.hdr` bytes
//! into an [`EnvironmentMap`] (texels plus its importance-sampling `Distribution2D`).
//!
//! # Why one function
//!
//! A remote render worker lights a scene with an HDR panorama it received as raw file
//! bytes (protocol v14, `indicatrix_net`'s content-addressed assets), while the viewer
//! lights the same scene from the file it loaded. Their samples are only mergeable if
//! both sides build the identical map -- the same texels AND the same marginal and
//! conditional CDFs, bit for bit. Every node therefore decodes through this one
//! function: the same pinned `image` HDR decoder (`Cargo.lock`), the same RGBE-to-float
//! conversion, the same `EnvironmentMap::from_rgb` importance build. The viewer reads its
//! file and calls this; a worker calls it on the bytes it was sent.
//!
//! # Untrusted input
//!
//! A worker decodes bytes a peer sent it. [`HdrLimits`] bounds the declared dimensions
//! and the decoded size, checked against the HEADER before the pixel buffer is
//! allocated; malformed data surfaces as [`EnvMapError::Decode`], never a panic.

use super::{EnvMapError, EnvironmentMap};
use image::ImageDecoder;

/// Bytes one decoded texel occupies in the `image` crate's `Rgb32F` buffer.
const DECODED_BYTES_PER_TEXEL: u64 = 12;

/// Bounds for [`environment_from_hdr_bytes`], checked against the file's header before
/// any pixel buffer is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdrLimits {
    /// Largest accepted width in texels.
    pub max_width: u32,
    /// Largest accepted height in texels.
    pub max_height: u32,
    /// Largest accepted decoded size, `width * height * 12` bytes (`Rgb32F`).
    pub max_decoded_bytes: u64,
}

impl HdrLimits {
    /// The limits every node applies, viewer and worker alike, so a map the viewer can
    /// load is always one a worker accepts: at most 16384 x 8192 texels and at most
    /// 512 MiB decoded -- the decoded cap the `image` crate's default `Limits` already
    /// imposed on the viewer's loader before this function existed.
    pub const DEFAULT: Self = Self {
        max_width: 16_384,
        max_height: 8_192,
        max_decoded_bytes: 512 * 1024 * 1024,
    };

    /// Whether a `width x height` image fits these limits.
    #[must_use]
    pub fn admits(&self, width: u32, height: u32) -> bool {
        width <= self.max_width
            && height <= self.max_height
            && u64::from(width) * u64::from(height) * DECODED_BYTES_PER_TEXEL
                <= self.max_decoded_bytes
    }
}

/// Decodes Radiance `.hdr` `bytes` into an [`EnvironmentMap`] under `limits` -- see the
/// module doc comment for why every node must build its map through this function.
///
/// # Errors
///
/// - [`EnvMapError::TooLarge`] when the header declares an image outside `limits`
///   (decided from the header alone; nothing is allocated for the pixels).
/// - [`EnvMapError::Decode`] when the bytes are not a well-formed Radiance HDR image
///   (bad signature, truncated header or scanlines, corrupt run-length data).
/// - [`EnvMapError::ZeroSized`] for a `0 x n` image.
pub fn environment_from_hdr_bytes(
    bytes: &[u8],
    limits: HdrLimits,
) -> Result<EnvironmentMap, EnvMapError> {
    let decoder = image::codecs::hdr::HdrDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| EnvMapError::Decode(e.to_string()))?;
    let (width, height) = decoder.dimensions();
    if !limits.admits(width, height) {
        return Err(EnvMapError::TooLarge {
            width,
            height,
            limits,
        });
    }
    if width == 0 || height == 0 {
        return Err(EnvMapError::ZeroSized);
    }
    // `from_decoder` is exactly what `image::load_from_memory_with_format(.., Hdr)` (the
    // loader this replaced) runs after its own header parse: one `Rgb32F` buffer filled
    // scanline by scanline. `into_rgb32f` is a no-op move for an `Rgb32F` image.
    let rgb = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| EnvMapError::Decode(e.to_string()))?
        .into_rgb32f();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let pixels: Vec<[f32; 3]> = rgb.pixels().map(|p| p.0).collect();
    EnvironmentMap::from_rgb(w, h, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, genuinely non-uniform Radiance `.hdr` file, encoded by the `image`
    /// crate's own HDR encoder (no fixture file needed).
    fn synthetic_hdr(width: u32, height: u32) -> Vec<u8> {
        let pixels: Vec<image::Rgb<f32>> = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let u = x as f32 / width as f32;
                    let v = y as f32 / height as f32;
                    image::Rgb([
                        (4.0 * u).mul_add(u, 0.05),
                        v.mul_add(2.0, 0.1),
                        (u * 7.0).sin().abs(),
                    ])
                })
            })
            .collect();
        let mut out = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut out)
            .encode(&pixels, width as usize, height as usize)
            .expect("encoding a synthetic HDR image into a Vec cannot fail");
        out
    }

    /// The map built from a transferred COPY of the bytes equals the locally built one
    /// bit for bit -- texels, marginal and every conditional CDF.
    #[test]
    fn transferred_bytes_build_a_bitwise_identical_map_and_distribution() {
        let bytes = synthetic_hdr(64, 32);
        let transferred = bytes.clone();
        let local = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
        let remote = environment_from_hdr_bytes(&transferred, HdrLimits::DEFAULT).unwrap();
        assert!(local.bitwise_eq(&remote));
        assert_eq!((local.width(), local.height()), (64, 32));
    }

    /// The shared builder reproduces the pre-E loader (`load_from_memory_with_format`
    /// then `from_rgb`) bit for bit, so a viewer upgraded to it renders exactly as before.
    #[test]
    fn the_shared_builder_matches_the_previous_loader_bit_for_bit() {
        let bytes = synthetic_hdr(48, 24);
        let legacy = {
            let rgb = image::load_from_memory_with_format(&bytes, image::ImageFormat::Hdr)
                .unwrap()
                .into_rgb32f();
            let pixels: Vec<[f32; 3]> = rgb.pixels().map(|p| p.0).collect();
            EnvironmentMap::from_rgb(48, 24, pixels).unwrap()
        };
        let shared = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
        assert!(legacy.bitwise_eq(&shared));
        // A different image is detected as different.
        let other = environment_from_hdr_bytes(&synthetic_hdr(48, 23), HdrLimits::DEFAULT);
        assert!(!legacy.bitwise_eq(&other.unwrap()));
    }

    /// A header declaring a huge image is refused from the header alone: no pixel data
    /// follows, so a decoder that allocated and started reading scanlines first would
    /// report a truncation instead of `TooLarge`.
    #[test]
    fn an_oversized_header_is_rejected_before_any_pixel_allocation() {
        let header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 8193 +X 16384\n";
        let err = environment_from_hdr_bytes(header, HdrLimits::DEFAULT).unwrap_err();
        assert!(
            matches!(
                err,
                EnvMapError::TooLarge {
                    width: 16_384,
                    height: 8_193,
                    ..
                }
            ),
            "{err:?}"
        );
        // Within both axis caps but over the decoded-size cap.
        let tight = HdrLimits {
            max_decoded_bytes: 1024,
            ..HdrLimits::DEFAULT
        };
        let err = environment_from_hdr_bytes(&synthetic_hdr(16, 16), tight).unwrap_err();
        assert!(matches!(err, EnvMapError::TooLarge { .. }), "{err:?}");
        assert!(err.to_string().contains("exceeds the decode limits"));
    }

    /// Malformed bytes are a clean `Decode` error, never a panic.
    #[test]
    fn malformed_bytes_are_a_decode_error() {
        for bad in [
            &b""[..],
            b"not an hdr file at all",
            b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n",
            b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 4 +X 4\n\x02\x02",
        ] {
            let err = environment_from_hdr_bytes(bad, HdrLimits::DEFAULT).unwrap_err();
            assert!(matches!(err, EnvMapError::Decode(_)), "{bad:?}: {err:?}");
        }
        // A valid file cut short mid-scanline.
        let mut truncated = synthetic_hdr(32, 16);
        truncated.truncate(truncated.len() - 40);
        assert!(matches!(
            environment_from_hdr_bytes(&truncated, HdrLimits::DEFAULT),
            Err(EnvMapError::Decode(_))
        ));
    }

    #[test]
    fn limits_admit_exactly_their_bounds() {
        let limits = HdrLimits::DEFAULT;
        assert!(limits.admits(8_192, 4_096));
        assert!(!limits.admits(16_385, 1));
        assert!(!limits.admits(1, 8_193));
        // 16384 x 8192 fits both axes but is 1.5 GiB decoded.
        assert!(!limits.admits(16_384, 8_192));
    }
}
