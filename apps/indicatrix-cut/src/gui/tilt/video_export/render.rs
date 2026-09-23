//! Per-frame CPU rendering for the tilt performance video.
//!
//! Reuses `bridge::export_thread::batch::render_batch` (the still-image export
//! pipeline's own CPU tracer, already `pub` for `bridge::preview_render`'s reuse)
//! directly, rather than the full local+remote+GPU `spawn_export` orchestration.
//! Deliberately CPU-only: this app has a single shared GPU adapter that must never run
//! two programs at once, and the live viewport keeps using
//! it while a video export runs in the background on its own thread, so this never
//! touches the GPU at all rather than trying to time-share it. The cost is wall-clock
//! time -- noticeably slower than the still-image export dialog's own hybrid CPU+GPU
//! split -- in exchange for being simple, deterministic, and safe to run alongside the
//! live viewport with no coordination needed.

use crate::bridge::export_thread::{SceneSnapshot, batch::render_batch};
use glam::Vec3;
use indicatrix::{
    color::{ColorSpace, ToneMap},
    optics::raytracer::Camera,
    renderer::tonemap::tonemap_to_rgba,
};

/// Renders one frame at `(cam_yaw, cam_pitch)` and tone-maps it to RGBA8 bytes.
/// Everything else -- material, geometry, light, exposure, bounce cap -- comes from
/// `scene`; only the camera pose changes frame to frame (`scene.yaw`/`scene.pitch` are
/// therefore never read here -- every frame builds its own [`Camera`] from the pose the
/// sweep computed for it, via the same `42.0` FOV convention every other one-off
/// `Camera::new` call site in this app uses).
#[must_use]
pub fn render_frame_rgba(
    scene: &SceneSnapshot,
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    cam_yaw: f32,
    cam_pitch: f32,
    color_space: ColorSpace,
) -> Vec<u8> {
    let spp = samples_per_pixel.max(1);
    let camera = Camera::new(cam_yaw, cam_pitch, scene.distance, 42.0);
    let mut accum = vec![Vec3::ZERO; (width * height) as usize];
    render_batch(width, height, spp, 0, &camera, scene, &mut accum);
    tonemap(&accum, spp, color_space)
}

/// Tone-maps a finished accumulation buffer to RGBA8 bytes. `Srgb` goes through the
/// live viewport's own `xyz_to_srgb_gamma`-based path; any other colour space goes
/// through `ColorSpace::encode` with the same `AcesFilmic` tone curve --
/// `bridge::export_thread::tonemap_png` draws this exact same distinction, but its
/// helpers are private to that module, so this is a small, deliberate re-implementation
/// against the same public `indicatrix` crate functions it itself wraps (no embedded
/// ICC profile: see `encode`'s own module doc comment on why a video frame sequence
/// consumed by ffmpeg/a GIF encoder has no use for one).
fn tonemap(accum: &[Vec3], samples_per_pixel: u32, color_space: ColorSpace) -> Vec<u8> {
    let inv_samples = 1.0 / samples_per_pixel as f32;
    if color_space == ColorSpace::Srgb {
        return tonemap_to_rgba(accum, inv_samples);
    }
    let mut out = Vec::with_capacity(accum.len() * 4);
    for xyz in accum {
        let rgba = color_space.encode(*xyz * inv_samples, ToneMap::AcesFilmic { exposure: 1.0 });
        out.extend_from_slice(&rgba);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::RenderContext;
    use std::sync::Mutex;

    /// A minimal end-to-end smoke test: one CPU frame at a tiny size/sample count
    /// must produce a full, opaque RGBA buffer of the right length. The dialog's own
    /// end-to-end test (`mod.rs`) covers the whole multi-frame pipeline; this isolates
    /// just the render+tonemap step.
    #[test]
    fn render_frame_rgba_produces_a_full_opaque_buffer() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()));
        let rgba = render_frame_rgba(&scene, 8, 8, 1, 0.6, 0.5, ColorSpace::Srgb);
        assert_eq!(rgba.len(), 8 * 8 * 4);
        assert!(
            rgba.chunks(4).all(|px| px[3] == 255),
            "every pixel must be fully opaque"
        );
    }

    #[test]
    fn render_frame_rgba_accepts_wide_gamut_color_spaces_too() {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()));
        let rgba = render_frame_rgba(&scene, 4, 4, 1, 0.0, 1.4, ColorSpace::DisplayP3);
        assert_eq!(rgba.len(), 4 * 4 * 4);
    }
}
