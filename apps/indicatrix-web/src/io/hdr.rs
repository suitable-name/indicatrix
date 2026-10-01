//! Accepting an uploaded Radiance `.hdr` environment map under the browser caps: at
//! most [`MAX_HDR_FILE_BYTES`] on disk and [`MAX_HDR_WIDTH`] x [`MAX_HDR_HEIGHT`] texels.
//!
//! Only the header is read here (`image`'s `HdrDecoder::new` parses the header and
//! stops), so a map that is too large is refused before anything is decoded. The
//! bytes are kept as uploaded; each render Worker and the analysis Worker decode them
//! once, through `indicatrix::renderer::env_map_hdr::environment_from_hdr_bytes`.

use crate::app::state::HdrUpload;
use image::ImageDecoder;

/// The largest accepted `.hdr` file, in bytes (64 MiB).
pub const MAX_HDR_FILE_BYTES: usize = 64 * 1024 * 1024;
/// The widest accepted map, in texels.
pub const MAX_HDR_WIDTH: u32 = 8192;
/// The tallest accepted map, in texels.
pub const MAX_HDR_HEIGHT: u32 = 4096;

/// Checks `bytes` against the caps and returns the upload to keep.
///
/// # Errors
///
/// A readable message when the file is too large, is not a Radiance HDR image, or
/// declares a zero or over-cap size.
pub fn accept_hdr(file_name: &str, bytes: Vec<u8>) -> Result<HdrUpload, String> {
    if bytes.len() > MAX_HDR_FILE_BYTES {
        return Err(format!(
            "\"{file_name}\" is {:.1} MiB; environment maps are limited to 64 MiB in the browser.",
            bytes.len() as f64 / (1024.0 * 1024.0)
        ));
    }
    let (width, height) = {
        let decoder = image::codecs::hdr::HdrDecoder::new(std::io::Cursor::new(bytes.as_slice()))
            .map_err(|e| {
            format!("\"{file_name}\" is not a readable Radiance .hdr image: {e}")
        })?;
        decoder.dimensions()
    };
    if width == 0 || height == 0 {
        return Err(format!(
            "\"{file_name}\" declares an empty ({width} x {height}) image."
        ));
    }
    if width > MAX_HDR_WIDTH || height > MAX_HDR_HEIGHT {
        return Err(format!(
            "\"{file_name}\" is {width} x {height}; environment maps are limited to \
             {MAX_HDR_WIDTH} x {MAX_HDR_HEIGHT} in the browser."
        ));
    }
    Ok(HdrUpload {
        name: file_name.to_string(),
        bytes,
        width,
        height,
    })
}
