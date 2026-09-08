//! Live export-progress thumbnail: box-downsamples the in-progress accumulation state
//! directly into a small sRGB preview image, rate-limited so a fast-ticking export
//! doesn't regenerate one on every batch.
//!
//! Split out of `bridge::export_thread` purely to keep that module (already sizeable)
//! from growing further -- same reasoning as `batch`/`tonemap_png`/`scene_snapshot`.

use super::tonemap_png::tonemap_to_rgba;
use glam::Vec3;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::time::{Duration, Instant};

/// Preview thumbnail's long edge, in pixels. Large enough to read the composition at a
/// glance; small enough that box-downsampling it costs a small, bounded fraction of a
/// batch even at an 8K export. The downsample pass has to touch every source pixel, so
/// its cost scales with the EXPORT's resolution rather than the thumbnail's -- see
/// `downsample_preview` on why the column mapping is a lookup table, which is what
/// keeps that pass memory-bound.
const PREVIEW_MAX_LONG_EDGE: u32 = 360;

/// Rate limit for preview regeneration -- about 2 per second, independent of how often
/// `run_export`'s batch loop ticks. Nobody can perceive a thumbnail changing faster than
/// this, so regenerating on every tick (a 4K export ticks far more often) would be pure
/// waste.
const PREVIEW_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// Rate-limits preview regeneration across `run_export`'s batch loop. One instance lives
/// for the whole export, not per batch.
pub(super) struct PreviewThrottle {
    last: Option<Instant>,
}

impl PreviewThrottle {
    pub(super) const fn new() -> Self {
        Self { last: None }
    }

    /// Returns a freshly regenerated thumbnail if at least `PREVIEW_MIN_INTERVAL` has
    /// elapsed since the last one (the first call always produces one), `None`
    /// otherwise. `accum`/`gpu_accum` are the export's two live local accumulation
    /// buffers -- see [`downsample_preview`] for why both are needed. `remote_accum`,
    /// when `Some`, is a live snapshot of the concurrently-dispatched remote engine's
    /// in-progress accumulator; `None` when no remote engine is in play.
    pub(super) fn maybe_generate(
        &mut self,
        width: u32,
        height: u32,
        accum: &[Vec3],
        gpu_accum: &[Vec3],
        remote_accum: Option<&[Vec3]>,
        samples_done: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let now = Instant::now();
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < PREVIEW_MIN_INTERVAL)
        {
            return None;
        }
        self.last = Some(now);
        Some(generate_preview_buffer(
            width,
            height,
            accum,
            gpu_accum,
            remote_accum,
            samples_done,
        ))
    }
}

/// Builds the small `SharedPixelBuffer` handed to the UI thread -- see
/// [`downsample_preview`] for the actual downsample/tone-map work. Kept separate so
/// `PreviewThrottle::maybe_generate` reads as "rate limit, then generate" rather than
/// interleaving the two concerns.
fn generate_preview_buffer(
    width: u32,
    height: u32,
    accum: &[Vec3],
    gpu_accum: &[Vec3],
    remote_accum: Option<&[Vec3]>,
    samples_done: u32,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let (thumb_w, thumb_h, rgba) =
        downsample_preview(width, height, accum, gpu_accum, remote_accum, samples_done);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(thumb_w, thumb_h);
    // Zero-copy reinterpret RGBA8 bytes into `Rgba8Pixel` -- same idiom as
    // `bridge::pixel_buffer::FramebufferTransfer::copy_from_gpu_slice`.
    let dst = buffer.make_mut_slice();
    let src: &[Rgba8Pixel] = bytemuck::cast_slice(&rgba);
    dst.copy_from_slice(src);
    buffer
}

/// Box-downsamples the combined CPU+GPU accumulation state directly into a small sRGB
/// thumbnail (returns `(thumb_width, thumb_height, rgba_bytes)`), without ever
/// tone-mapping -- or even fully materialising -- a full-resolution intermediate: each
/// source pixel is visited exactly once and folded straight into its destination
/// bucket.
///
/// # Combining the hybrid buffers
///
/// A hybrid export keeps separate accumulation buffers per engine (`accum`, `gpu_accum`,
/// and -- while a remote worker is also in flight -- a live snapshot of its
/// accumulator as `remote_accum`) that `run_export` only sums together once, at the
/// end. A preview taken mid-export has to reflect ALL of them or it silently
/// under-represents whatever share of the frame that engine is carrying. So every
/// source pixel's contribution is `accum[i] + gpu_accum[i] + remote_accum[i]` (the
/// last term `0` with no remote engine), summed on the fly.
///
/// # Normalising by samples actually done, not the export's target
///
/// Each combined source pixel is a SUM of `samples_done` samples, not an average --
/// same convention as `accum` itself. This box-averages within each thumbnail bucket
/// but leaves sample normalisation to [`tonemap_to_rgba`], called with `samples_done`,
/// never the export's target: normalising by the target would leave the preview
/// looking almost black for most of the export.
///
/// # Reusing the export's sRGB tone-mapping
///
/// Always goes through [`tonemap_to_rgba`] -- the sRGB path -- regardless of the
/// export's chosen `ColorSpace`. This thumbnail is displayed on screen by Slint, which
/// expects sRGB bytes; a preview can't carry an ICC profile the way the exported PNG
/// can, so it always renders through the one path guaranteed correct without one.
fn downsample_preview(
    width: u32,
    height: u32,
    accum: &[Vec3],
    gpu_accum: &[Vec3],
    remote_accum: Option<&[Vec3]>,
    samples_done: u32,
) -> (u32, u32, Vec<u8>) {
    debug_assert_eq!(accum.len(), gpu_accum.len());
    debug_assert_eq!(accum.len(), (width as usize) * (height as usize));
    debug_assert!(remote_accum.is_none_or(|r| r.len() == accum.len()));

    let long_edge = width.max(height).max(1);
    let scale = (f64::from(PREVIEW_MAX_LONG_EDGE) / f64::from(long_edge)).min(1.0);
    let thumb_w = ((f64::from(width) * scale).round() as u32).max(1);
    let thumb_h = ((f64::from(height) * scale).round() as u32).max(1);

    let width_usize = width as usize;
    let height_usize = height as usize;
    let thumb_w_usize = thumb_w as usize;
    let thumb_h_usize = thumb_h as usize;

    let mut sum = vec![Vec3::ZERO; thumb_w_usize * thumb_h_usize];
    let mut count = vec![0u32; sum.len()];

    // Forward (source -> bucket) box mapping via floored division. Surjective onto
    // `0..thumb_h`/`0..thumb_w` since `scale` never upsamples, so `count` never holds
    // a zero once this loop finishes.
    //
    // The column mapping is hoisted into a lookup table rather than recomputed per
    // pixel: it depends only on `x`, and recomputing it per pixel measured ~15 ms per
    // preview at 3840x2160. One divide per COLUMN instead leaves this loop
    // memory-bound on the two accumulation buffers it has to read either way.
    let col_bucket: Vec<usize> = (0..width_usize)
        .map(|x| x * thumb_w_usize / width_usize)
        .collect();

    for y in 0..height_usize {
        let ty = y * thumb_h_usize / height_usize;
        let row = y * width_usize;
        let dst_row = ty * thumb_w_usize;
        for (x, &tx) in col_bucket.iter().enumerate() {
            let src = row + x;
            let dst = dst_row + tx;
            sum[dst] += accum[src] + gpu_accum[src] + remote_accum.map_or(Vec3::ZERO, |r| r[src]);
            count[dst] += 1;
        }
    }

    let thumb_accum: Vec<Vec3> = sum
        .iter()
        .zip(&count)
        .map(|(s, c)| *s / (*c).max(1) as f32)
        .collect();

    let rgba = tonemap_to_rgba(thumb_w, thumb_h, samples_done.max(1), &thumb_accum);
    (thumb_w, thumb_h, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A uniform accum buffer (every pixel identical) must downsample to a uniform
    /// thumbnail of the expected dimensions -- the simplest possible correctness check
    /// on the bucket mapping and averaging.
    #[test]
    fn downsample_preview_of_a_uniform_buffer_is_uniform_and_correctly_sized() {
        let width = 37;
        let height = 21;
        let accum = vec![Vec3::new(1.0, 0.5, 0.25); (width * height) as usize];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];

        let (thumb_w, thumb_h, rgba) =
            downsample_preview(width, height, &accum, &gpu_accum, None, 4);

        assert!(thumb_w <= width && thumb_h <= height);
        assert_eq!(rgba.len(), (thumb_w * thumb_h * 4) as usize);
        let (chunks, _) = rgba.as_chunks::<4>();
        let first = chunks[0];
        for px in chunks {
            assert_eq!(
                px, &first,
                "a uniform source buffer must downsample uniformly"
            );
        }
    }

    /// Long-edge cap: an image already smaller than `PREVIEW_MAX_LONG_EDGE` must not be
    /// upsampled -- the preview is a downsample-only operation.
    #[test]
    fn downsample_preview_never_upsamples_a_small_export() {
        let accum = vec![Vec3::ONE; 8 * 8];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];
        let (thumb_w, thumb_h, _) = downsample_preview(8, 8, &accum, &gpu_accum, None, 1);
        assert_eq!((thumb_w, thumb_h), (8, 8));
    }

    /// The GPU's contribution must not be dropped: an all-zero CPU `accum` with a
    /// nonzero `gpu_accum` must still produce a nonzero (non-black) preview.
    #[test]
    fn downsample_preview_includes_the_gpu_buffers_contribution() {
        let accum = vec![Vec3::ZERO; 4 * 4];
        let gpu_accum = vec![Vec3::new(2.0, 2.0, 2.0); accum.len()];
        let (_, _, rgba) = downsample_preview(4, 4, &accum, &gpu_accum, None, 1);
        assert!(
            rgba.iter().any(|&b| b > 0),
            "a nonzero gpu_accum-only buffer must not downsample to an all-black preview"
        );
    }

    /// The remote engine's live snapshot must not be dropped either -- same shape as
    /// the GPU check above, but for `remote_accum`.
    #[test]
    fn downsample_preview_includes_the_remote_buffers_contribution() {
        let accum = vec![Vec3::ZERO; 4 * 4];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];
        let remote_accum = vec![Vec3::new(3.0, 3.0, 3.0); accum.len()];
        let (_, _, rgba) = downsample_preview(4, 4, &accum, &gpu_accum, Some(&remote_accum), 1);
        assert!(
            rgba.iter().any(|&b| b > 0),
            "a nonzero remote_accum-only buffer must not downsample to an all-black preview"
        );
    }

    /// `PreviewThrottle` must emit on the first call and then withhold a call made
    /// immediately afterwards, until `PREVIEW_MIN_INTERVAL` has elapsed.
    #[test]
    fn preview_throttle_emits_first_then_withholds_until_the_interval_elapses() {
        let accum = vec![Vec3::ONE; 4 * 4];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];
        let mut throttle = PreviewThrottle::new();

        assert!(
            throttle
                .maybe_generate(4, 4, &accum, &gpu_accum, None, 1)
                .is_some(),
            "the first call must always produce a preview"
        );
        assert!(
            throttle
                .maybe_generate(4, 4, &accum, &gpu_accum, None, 1)
                .is_none(),
            "a call immediately after the first must be rate-limited"
        );
    }
}
