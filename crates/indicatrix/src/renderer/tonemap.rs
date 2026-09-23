//! Parallel batched CIE XYZ -> sRGB tone-mapping.
//!
//! `xyz_to_srgb_gamma` is not cheap, and three call sites
//! (`render_thread::denoise_and_tonemap_frame`, `render_thread::tonemap_running_average`,
//! `export_thread::tonemap_to_rgba`) each tone-map a full frame; doing so in a
//! single-threaded loop on the UI thread is a visible synchronous stall on every
//! progressive redraw (measured at 3840x2160, ~392ms).
//!
//! This module factors the three call sites' identical inner loop into one parallel
//! implementation, following the same row/slice-chunked `std::thread::scope` pattern
//! `renderer::denoise::atrous_pass` and `indicatrix-worker::render_core::trace_samples`
//! use. Unlike the A-Trous denoiser, tone-mapping has no stencil -- each output pixel
//! is a pure function of exactly one input pixel -- so flat contiguous slice chunks
//! are simpler and equally valid, and the result is trivially bit-identical for any
//! chunking or thread count.
//!
//! The three call sites differ only in what scale factor is applied to each XYZ value
//! before tone-mapping, so [`tonemap_to_rgba_with_threads`] takes that scale as a
//! parameter rather than three near-duplicate functions.

use crate::optics::raytracer::xyz_to_srgb_gamma;
use glam::Vec3;

/// Resolves a `--threads`-style argument (`0` meaning "let the OS decide") to an actual
/// thread count. Mirrors `renderer::denoise::effective_thread_count` and
/// `indicatrix-worker::render_core::effective_thread_count` exactly (duplicated rather
/// than shared across the three independent `thread::scope` call sites).
//
// `wasm32-unknown-unknown` has no OS thread to spawn: `std::thread::Scope::spawn` (this
// module's only caller of the value returned here) panics at runtime on this target, so
// this returns `1` unconditionally rather than merely capping `threads` -- that makes
// [`tonemap_to_rgba_with_threads`]'s "whole buffer fits in one chunk" fast path always
// taken, so `std::thread::scope` is never reached on this target at all. A separate
// `cfg`-gated definition (rather than one function with an internal `#[cfg]` block) so
// this wasm32 body can be `const fn`, since the native arm calls
// `std::thread::available_parallelism`, which is not `const`-compatible.
#[cfg(target_arch = "wasm32")]
#[must_use]
const fn effective_thread_count(_threads: usize) -> usize {
    1
}

#[cfg(not(target_arch = "wasm32"))]
#[must_use]
fn effective_thread_count(threads: usize) -> usize {
    if threads == 0 {
        std::thread::available_parallelism().map_or(8, std::num::NonZero::get)
    } else {
        threads
    }
}

/// Tone-maps one contiguous slice of `colors` (each value first multiplied by `scale`)
/// into the corresponding `dst` byte slice. `dst` must be exactly `colors.len() * 4`
/// bytes. Pure function of its inputs -- no cross-pixel state -- which is what makes
/// chunked parallelisation in [`tonemap_to_rgba_with_threads`] bit-identical regardless
/// of chunk boundaries or thread count.
fn tonemap_chunk(colors: &[Vec3], dst: &mut [u8], scale: f32) {
    debug_assert_eq!(dst.len(), colors.len() * 4);
    for (i, xyz) in colors.iter().enumerate() {
        let rgba = xyz_to_srgb_gamma(*xyz * scale);
        let p = i * 4;
        dst[p] = rgba[0];
        dst[p + 1] = rgba[1];
        dst[p + 2] = rgba[2];
        dst[p + 3] = rgba[3];
    }
}

/// Tone-maps `colors` into a fresh `colors.len() * 4`-byte RGBA buffer.
///
/// Each value is scaled by `scale` first. Parallelised across `threads` OS threads via
/// `std::thread::scope` (`threads == 0` auto-detects, see [`effective_thread_count`]).
///
/// Because [`tonemap_chunk`] is a pure per-pixel function with no cross-pixel
/// dependency, the output is bit-identical for any `threads >= 1`, including thread
/// counts that do not evenly divide `colors.len()` and thread counts that exceed
/// `colors.len()`.
#[must_use]
pub fn tonemap_to_rgba_with_threads(colors: &[Vec3], scale: f32, threads: usize) -> Vec<u8> {
    let mut out = vec![0u8; colors.len() * 4];
    if colors.is_empty() {
        return out;
    }

    let num_threads = effective_thread_count(threads).max(1);
    let chunk_len = colors.len().div_ceil(num_threads).max(1);

    if chunk_len >= colors.len() {
        // Whole buffer fits in one chunk (small image, or threads == 1): skip the
        // thread::scope machinery entirely rather than spawn a single worker for it.
        tonemap_chunk(colors, &mut out, scale);
        return out;
    }

    std::thread::scope(|s| {
        let color_chunks = colors.chunks(chunk_len);
        let byte_chunks = out.chunks_mut(chunk_len * 4);
        for (color_chunk, byte_chunk) in color_chunks.zip(byte_chunks) {
            s.spawn(move || tonemap_chunk(color_chunk, byte_chunk, scale));
        }
    });

    out
}

/// Same as [`tonemap_to_rgba_with_threads`] with the OS-decided ("auto") thread count.
///
/// The entry point every real call site should use; the explicit-thread-count form
/// exists mainly for tests and callers that already manage their own thread budget.
#[must_use]
pub fn tonemap_to_rgba(colors: &[Vec3], scale: f32) -> Vec<u8> {
    tonemap_to_rgba_with_threads(colors, scale, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift32 PRNG, matching the one in `renderer::denoise`'s own
    /// tests (kept separate rather than shared -- test-only helper, not worth a shared
    /// dependency).
    struct Xorshift32(u32);
    impl Xorshift32 {
        const fn new(seed: u32) -> Self {
            Self(if seed == 0 { 0xdead_beef } else { seed })
        }
        const fn next_u32(&mut self) -> u32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            x
        }
        fn next_f32(&mut self) -> f32 {
            (f64::from(self.next_u32()) / f64::from(u32::MAX)) as f32
        }
    }

    /// Irregular pixel count (doesn't divide evenly across most thread counts) with a
    /// wide dynamic range, including some out-of-gamut and near-zero values, so the
    /// tone-mapping/gamut-mapping edge cases actually get exercised rather than
    /// degenerating to a uniform buffer.
    fn irregular_colors(len: usize, seed: u32) -> Vec<Vec3> {
        let mut rng = Xorshift32::new(seed);
        (0..len)
            .map(|_| {
                Vec3::new(
                    rng.next_f32() * 4.0,
                    rng.next_f32() * 4.0,
                    rng.next_f32() * 4.0,
                )
            })
            .collect()
    }

    #[test]
    fn tonemap_is_thread_count_invariant() {
        let colors = irregular_colors(10_007, 0x1234_5678);
        let scale = 0.37;

        let reference = tonemap_to_rgba_with_threads(&colors, scale, 1);
        for threads in [2usize, 3, 8, 16, 200] {
            let out = tonemap_to_rgba_with_threads(&colors, scale, threads);
            assert_eq!(out, reference, "threads={threads}");
        }

        let auto = tonemap_to_rgba(&colors, scale);
        assert_eq!(auto, reference, "auto thread count");
    }

    #[test]
    fn matches_the_single_threaded_reference_loop() {
        let colors = irregular_colors(2_503, 0xabcd_ef01);
        let scale = 1.0;

        let parallel = tonemap_to_rgba(&colors, scale);

        let mut expected = vec![0u8; colors.len() * 4];
        for (i, xyz) in colors.iter().enumerate() {
            let rgba = xyz_to_srgb_gamma(*xyz * scale);
            expected[i * 4] = rgba[0];
            expected[i * 4 + 1] = rgba[1];
            expected[i * 4 + 2] = rgba[2];
            expected[i * 4 + 3] = rgba[3];
        }

        assert_eq!(parallel, expected);
    }

    #[test]
    fn empty_input_does_not_panic() {
        let colors: Vec<Vec3> = Vec::new();
        assert_eq!(tonemap_to_rgba(&colors, 1.0), Vec::<u8>::new());
    }

    #[test]
    fn single_pixel_does_not_panic() {
        let colors = vec![Vec3::new(0.5, 0.5, 0.5)];
        let out = tonemap_to_rgba_with_threads(&colors, 1.0, 8);
        assert_eq!(out.len(), 4);
    }
}
