//! Pixels from an accumulation, each step the desktop's own call, so the same float sum
//! gives the same bytes on the same target.
//!
//! Bitwise identity holds only on one target: wasm and native take `powf` and the other
//! transcendental functions from different libms, and the tests here run natively.
//!
//! - **Live view:** [`live_rgba`] is the desktop's `render_thread::tonemap_running_average`
//!   (`tonemap::tonemap_to_rgba(sum, 1 / samples)`), the picture it shows while
//!   accumulating with denoise off.
//! - **Settled view:** [`Denoiser`] runs the desktop's live-view denoise: the first-hit
//!   guides from `renderer::guide_pass::generate_guide_buffers`, computed once per scene,
//!   then `renderer::frame_denoise::denoise_and_tonemap_frame` with the desktop's
//!   `AtrousParams::default()`.
//! - **PNG export:** [`export_png`] is the desktop's still export
//!   (`bridge/export_thread`: `render_image_rgba`'s full-data branch, then `save_png`):
//!   `tonemap::tonemap_accumulation` for the chosen color space, then
//!   `render_setup::encode_png_with_icc`. The desktop export never denoises; the web
//!   dialog's optional denoise filters the mean with the live view's filter first
//!   ([`Denoiser::denoised_mean`]) and tone-maps the filtered mean through the same
//!   `tonemap_accumulation` (at one sample, whose `1 / 1` scale is exact), so an sRGB
//!   denoised export equals the settled live view pixel for pixel.
//! - **File name:** [`export_file_name`] is the desktop's default export template,
//!   `gem_export_{material}_{width}x{height}_{spp}spp_{timestamp}.png`.

use glam::Vec3;
use indicatrix::{
    color::ColorSpace,
    geometry::GpuFacetPlane,
    optics::raytracer::Camera,
    render_setup::encode_png_with_icc,
    renderer::{
        denoise::{AtrousDenoiser, AtrousParams, GBuffers},
        frame_denoise::{DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame},
        guide_pass::{GuideBuffers, generate_guide_buffers},
        tonemap::{tonemap_accumulation, tonemap_to_rgba},
    },
};

#[cfg(test)]
mod tests;

/// The live view's picture for a running sum of `sample_count` samples per pixel: the
/// desktop's `tonemap_running_average` (a zero count is treated as one, like there).
#[must_use]
pub fn live_rgba(sum: &[Vec3], sample_count: u32) -> Vec<u8> {
    tonemap_to_rgba(sum, 1.0 / sample_count.max(1) as f32)
}

/// The color spaces the web export offers, in the dialog's order.
pub const EXPORT_COLOR_SPACES: [ColorSpace; 2] = [ColorSpace::Srgb, ColorSpace::DisplayP3];

/// The dialog's label for an export color space (the desktop's `{colorspace}` text).
#[must_use]
pub const fn color_space_label(color_space: ColorSpace) -> &'static str {
    match color_space {
        ColorSpace::Srgb => "sRGB",
        ColorSpace::DisplayP3 => "Display P3",
        ColorSpace::Rec2020 => "Rec.2020",
        ColorSpace::AcesCg => "ACEScg",
    }
}

/// The export color space for a dialog index (out of range = sRGB).
#[must_use]
pub fn export_color_space(index: i32) -> ColorSpace {
    usize::try_from(index)
        .ok()
        .and_then(|i| EXPORT_COLOR_SPACES.get(i).copied())
        .unwrap_or(ColorSpace::Srgb)
}

/// The desktop live view's denoiser, with the scratch buffers and the guide buffers it
/// reuses between frames.
///
/// The guides depend only on the pose, the frame size and the planes, so they are
/// computed once per `guide_key` (the caller passes its scene id) and reused for every
/// later denoise of that scene -- the desktop's `GuideCache` rule.
#[derive(Default)]
pub struct Denoiser {
    guides: Option<(u64, GuideBuffers)>,
    filter: AtrousDenoiser,
    avg: Vec<Vec3>,
    filtered: Vec<Vec3>,
}

/// What a [`Denoiser`] call needs to know about the frame.
#[derive(Clone, Copy)]
pub struct DenoiseFrame<'a> {
    /// Identifies the pose, size and planes: guides are rebuilt only when it changes.
    pub guide_key: u64,
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// The camera the frame was traced with.
    pub camera: &'a Camera,
    /// The planes the frame was traced with.
    pub planes: &'a [GpuFacetPlane],
    /// Samples per pixel in `sum`.
    pub sample_count: u32,
    /// The running per-pixel sum, `width * height` long.
    pub sum: &'a [Vec3],
}

impl Denoiser {
    /// A denoiser with no guides yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the guides for `guide_key` are already built (the next call skips the
    /// guide pass).
    #[must_use]
    pub fn has_guides_for(&self, guide_key: u64) -> bool {
        self.guides
            .as_ref()
            .is_some_and(|(key, _)| *key == guide_key)
    }

    /// Drops the guides and scratch buffers (a large export's memory, say).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Builds the guides for `frame` unless they are already held.
    fn ensure_guides(&mut self, frame: &DenoiseFrame<'_>) {
        if !self.has_guides_for(frame.guide_key) {
            // Free the old guides before building new ones.
            self.guides = None;
            let guides =
                generate_guide_buffers(frame.width, frame.height, frame.camera, frame.planes);
            self.guides = Some((frame.guide_key, guides));
        }
    }

    /// The settled live view: `denoise_and_tonemap_frame` exactly as the desktop's
    /// display thread calls it.
    pub fn denoised_rgba(&mut self, frame: &DenoiseFrame<'_>) -> Vec<u8> {
        self.ensure_guides(frame);
        let Some((_, guides)) = &self.guides else {
            return live_rgba(frame.sum, frame.sample_count);
        };
        denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width: frame.width,
                height: frame.height,
                current_sample_count: frame.sample_count.max(1),
                accum_buffer: frame.sum,
                first_hit_depth: &guides.depth,
                first_hit_normal: &guides.normal,
                first_hit_facet_id: &guides.facet_id,
            },
            &mut DenoiseScratch {
                denoiser: &mut self.filter,
                avg_color_buf: &mut self.avg,
                filtered_buf: &mut self.filtered,
            },
        )
    }

    /// The filtered per-pixel mean -- the same filter [`Self::denoised_rgba`] applies
    /// (same averaging, guides and `AtrousParams::default()`), before tone mapping.
    pub fn denoised_mean(&mut self, frame: &DenoiseFrame<'_>) -> Vec<Vec3> {
        self.ensure_guides(frame);
        let Some((_, guides)) = &self.guides else {
            return Vec::new();
        };
        let inv_samples = 1.0 / frame.sample_count.max(1) as f32;
        self.avg.clear();
        self.avg.extend(frame.sum.iter().map(|v| *v * inv_samples));
        let gbuffers = GBuffers {
            color: &self.avg,
            depth: &guides.depth,
            normal: &guides.normal,
            facet_id: &guides.facet_id,
            width: frame.width as usize,
            height: frame.height as usize,
            spp: frame.sample_count.max(1),
        };
        let mut filtered = Vec::new();
        self.filter
            .denoise_into(&gbuffers, &AtrousParams::default(), &mut filtered);
        filtered
    }
}

/// Encodes a finished export: `tonemap_accumulation` for `color_space`, then
/// `encode_png_with_icc` -- the desktop's still export, byte for byte.
///
/// `sum` holds `sample_count` samples per pixel. With `denoised_mean` (from
/// [`Denoiser::denoised_mean`]) that filtered mean is tone-mapped instead, as a
/// one-sample sum.
///
/// # Errors
///
/// When a buffer does not hold `width * height` pixels, or the PNG encoder fails.
pub fn export_png(
    width: u32,
    height: u32,
    sample_count: u32,
    sum: &[Vec3],
    color_space: ColorSpace,
    denoised_mean: Option<&[Vec3]>,
) -> Result<Vec<u8>, String> {
    let pixels = width as usize * height as usize;
    let (buffer, samples) = denoised_mean.map_or((sum, sample_count), |mean| (mean, 1));
    if buffer.len() != pixels {
        return Err(format!(
            "the export buffer holds {} pixels, not {width}x{height}",
            buffer.len()
        ));
    }
    let rgba = tonemap_accumulation(width, height, samples.max(1), buffer, color_space);
    encode_png_with_icc(&rgba, width, height, color_space)
}

/// The desktop's default export file name for these values.
///
/// The template is `gem_export_{material}_{width}x{height}_{spp}spp_{timestamp}`
/// (`bridge::export_thread::filename_template::DEFAULT_TEMPLATE`), with its
/// sanitising (forbidden characters to `_`, trailing dots and spaces trimmed) and
/// `.png` appended. `unix_seconds` is the desktop's `{timestamp}`.
#[must_use]
pub fn export_file_name(
    material: &str,
    width: u32,
    height: u32,
    spp: u32,
    unix_seconds: u64,
) -> String {
    let raw = format!("gem_export_{material}_{width}x{height}_{spp}spp_{unix_seconds}");
    let mut cleaned: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                || (c as u32) < 0x20
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    while matches!(cleaned.chars().last(), Some('.' | ' ')) {
        cleaned.pop();
    }
    format!("{cleaned}.png")
}

/// The export frame size: the view's aspect (`view_width : view_height`) with its long
/// edge at `long_edge` (clamped to `1..=4096`); each edge at least 1 pixel.
#[must_use]
pub fn export_size(view_width: u32, view_height: u32, long_edge: u32) -> (u32, u32) {
    let long_edge = long_edge.clamp(1, MAX_EXPORT_EDGE);
    let (w, h) = (f64::from(view_width.max(1)), f64::from(view_height.max(1)));
    let scale = f64::from(long_edge) / w.max(h);
    let fit = |edge: f64| ((edge * scale).round() as u32).clamp(1, MAX_EXPORT_EDGE);
    (fit(w), fit(h))
}

/// The live render's throughput in samples per second, for the progress pill.
///
/// `anchor` is the `(time_ms, samples)` of the first progress report of the scene;
/// measuring from it leaves out the Workers' start-up (the wasm download, the scene
/// build), which would otherwise dominate the first seconds and read as "0 spp/s".
/// Until a second report exists (`samples` has not moved past the anchor, or no time
/// passed) the rate falls back to `samples / (now_ms - started_ms)`.
#[must_use]
pub fn samples_per_second(
    samples: u32,
    started_ms: f64,
    anchor: Option<(f64, u32)>,
    now_ms: f64,
) -> f64 {
    if let Some((anchor_ms, anchor_samples)) = anchor {
        let elapsed = (now_ms - anchor_ms) / 1000.0;
        if samples > anchor_samples && elapsed > 1e-3 {
            return f64::from(samples - anchor_samples) / elapsed;
        }
    }
    f64::from(samples) / ((now_ms - started_ms) / 1000.0).max(1e-3)
}

/// A throughput as the pill shows it: one decimal below 10 (so a slow start reads
/// "0.4", never "0"), whole numbers above.
#[must_use]
pub fn format_rate(per_second: f64) -> String {
    if per_second < 10.0 {
        format!("{per_second:.1}")
    } else {
        format!("{per_second:.0}")
    }
}

/// The export's long-edge cap, in pixels.
pub const MAX_EXPORT_EDGE: u32 = 4096;

/// The export dialog's default long edge.
pub const DEFAULT_EXPORT_EDGE: u32 = 1600;
