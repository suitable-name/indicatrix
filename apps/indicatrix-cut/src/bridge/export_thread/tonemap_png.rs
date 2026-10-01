//! Tone-map and PNG/ICC output: turning the finished accumulation buffer into RGBA
//! bytes (sRGB or wide-gamut) and writing it to disk.
//!
//! The tone-mapping itself ([`tonemap_accumulation`]) lives in
//! `indicatrix::renderer::tonemap` so a server answering a "final picture only"
//! `FinalImageRequest` runs the SAME code and its PNG is byte-identical to what this
//! viewer would write from the same float sum; it is re-exported here so every caller in
//! this crate stays unchanged. The actual PNG+ICC encoding
//! ([`indicatrix::render_setup::encode_png_with_icc`]) is pure/in-memory and lives in
//! `indicatrix` too, for the same byte-identical-output reason -- [`save_png`] here
//! just calls it and writes the result to disk, which is the one part that must stay
//! viewer-side.

pub use indicatrix::renderer::tonemap::tonemap_accumulation;
use indicatrix::{color::ColorSpace, render_setup::encode_png_with_icc};
use std::path::Path;

/// Writes `rgba` (`width * height * 4` bytes) to `path` as PNG, embedding an ICC
/// profile for any non-`Srgb` `color_space` -- see
/// [`indicatrix::render_setup::encode_png_with_icc`] for the encoding itself.
pub(super) fn save_png(
    path: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
    color_space: ColorSpace,
) -> Result<(), String> {
    let bytes = encode_png_with_icc(rgba, width, height, color_space)?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}
