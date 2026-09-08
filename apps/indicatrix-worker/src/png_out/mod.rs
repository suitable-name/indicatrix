//! Tone-mapping a summed radiance buffer to RGBA bytes, and writing those bytes out as
//! a PNG.
//!
//! Used only by `render_cmd` -- `serve` sends raw summed radiance over the wire instead
//! (see `indicatrix_net::radiance`), never tone-maps or encodes an image itself.

use glam::Vec3;
use std::path::Path;

/// Divides `sums` by `total_samples` and tone-maps the result to RGBA bytes.
///
/// Uses the same `xyz_to_srgb_gamma` the viewer uses every frame, so a `render` output
/// matches the live viewport for the same scene and sample count. `sums` is expected to
/// be what [`crate::render_core::trace_samples`] returns.
///
/// The one place in this crate that turns a sum into an average -- see
/// `render_core::trace_samples`'s doc comment for why radiance stays summed elsewhere.
#[must_use]
pub fn tonemap_to_rgba(width: u32, height: u32, total_samples: u32, sums: &[Vec3]) -> Vec<u8> {
    let mut bytes = vec![0u8; width as usize * height as usize * 4];
    let inv_samples = 1.0 / total_samples.max(1) as f32;
    for (i, xyz) in sums.iter().enumerate() {
        let rgba = indicatrix::optics::raytracer::xyz_to_srgb_gamma(*xyz * inv_samples);
        bytes[i * 4] = rgba[0];
        bytes[i * 4 + 1] = rgba[1];
        bytes[i * 4 + 2] = rgba[2];
        bytes[i * 4 + 3] = rgba[3];
    }
    bytes
}

/// Writes `rgba` (as produced by [`tonemap_to_rgba`]) to `out_path` as a PNG, creating
/// parent directories as needed.
///
/// # Errors
///
/// Returns a human-readable message if `rgba`'s length doesn't match
/// `width * height * 4`, if the parent directory can't be created, or if PNG encoding
/// fails.
pub fn write_png(width: u32, height: u32, rgba: Vec<u8>, out_path: &Path) -> Result<(), String> {
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "could not create output directory {}: {e}",
                parent.display()
            )
        })?;
    }

    let img = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "internal error: pixel buffer size did not match dimensions".to_string())?;
    img.save(out_path)
        .map_err(|e| format!("failed to write PNG to {}: {e}", out_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tonemap_produces_the_right_number_of_bytes() {
        let sums = vec![Vec3::new(1.0, 2.0, 3.0); 16];
        let rgba = tonemap_to_rgba(4, 4, 8, &sums);
        assert_eq!(rgba.len(), 4 * 4 * 4);
    }

    #[test]
    fn tonemap_alpha_channel_is_always_opaque() {
        let sums = vec![Vec3::ZERO; 4];
        let rgba = tonemap_to_rgba(2, 2, 1, &sums);
        for px in rgba.as_chunks::<4>().0 {
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn write_png_round_trips_a_tiny_image() {
        let sums = vec![Vec3::new(0.5, 0.5, 0.5); 64];
        let rgba = tonemap_to_rgba(8, 8, 1, &sums);
        let dir =
            std::env::temp_dir().join(format!("indicatrix-worker-png-test-{}", std::process::id()));
        let path = dir.join("tiny.png");

        write_png(8, 8, rgba, &path).unwrap();
        let img = image::open(&path).expect("written file must be a valid, readable PNG");
        assert_eq!(img.width(), 8);
        assert_eq!(img.height(), 8);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
