//! Parallel batched CIE XYZ -> sRGB tone-mapping.
//!
//! `xyz_to_srgb_gamma` is not cheap, and three call sites
//! (`frame_denoise::denoise_and_tonemap_frame`, `render_thread::tonemap_running_average`,
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
//!
//! A live view converts a frame many times a second, so [`tonemap_to_rgba_into`] and
//! [`tonemap_to_rgba_with_threads_into`] write into a buffer the caller keeps (or straight
//! into a display surface); the `Vec`-returning forms are thin wrappers around them.
//!
//! # The export's final 8-bit conversion
//!
//! [`tonemap_accumulation`] (float sum -> RGBA8 for an export's colour space) lives here,
//! not in the viewer, so the viewer's still export and tilt video and a server rendering
//! a "final picture only" request (`FinalImageRequest`) run the SAME code: the PNG a
//! server sends is then byte-identical to the one the viewer would have written from the
//! same float sum. It moved here from `indicatrix-cut`'s `export_thread::tonemap_png`
//! unchanged; `tests::tonemap_accumulation_matches_the_pre_move_export_code` pins that.

use crate::{
    color::{ColorSpace, ToneMap},
    optics::raytracer::xyz_to_srgb_gamma,
};
use glam::Vec3;

/// Resolves a `--threads`-style argument to an actual thread count (`0` meaning "let
/// the OS decide").
///
/// The one definition the tone-mapper, the A-Trous denoiser and the worker's tracer all
/// share, so every independent `thread::scope` site agrees on what "auto" means (the
/// OS's available parallelism, falling back to 8).
///
/// `wasm32-unknown-unknown` has no OS thread to spawn: `std::thread::Scope::spawn` panics
/// at runtime on this target, so this returns `1` unconditionally rather than merely
/// capping `threads` -- that makes [`tonemap_to_rgba_with_threads`]'s "whole buffer fits
/// in one chunk" fast path always taken, so `std::thread::scope` is never reached on this
/// target at all. A separate `cfg`-gated definition (rather than one function with an
/// internal `#[cfg]` block) so this wasm32 body can be `const fn`, since the native arm
/// calls `std::thread::available_parallelism`, which is not `const`-compatible.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub const fn effective_thread_count(_threads: usize) -> usize {
    1
}

/// Resolves a `--threads`-style argument to an actual thread count.
///
/// `0` ("let the OS decide") becomes the OS's available parallelism (falling back to 8);
/// any other value is returned unchanged. See the wasm32 definition above for the shared
/// rationale.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn effective_thread_count(threads: usize) -> usize {
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

/// Tone-maps `colors` into the caller's `out` buffer, allocating nothing.
///
/// Each value is scaled by `scale` first. `min(colors.len(), out.len() / 4)` pixels are
/// converted into the leading bytes of `out`; any bytes beyond that are left untouched,
/// so a caller that sizes `out` as `colors.len() * 4` converts everything. Parallelised
/// across `threads` OS threads via `std::thread::scope` (`threads == 0` auto-detects, see
/// [`effective_thread_count`]).
///
/// Because [`tonemap_chunk`] is a pure per-pixel function with no cross-pixel
/// dependency, the output is bit-identical for any `threads >= 1`, including thread
/// counts that do not evenly divide the pixel count and thread counts that exceed it.
pub fn tonemap_to_rgba_with_threads_into(
    colors: &[Vec3],
    scale: f32,
    threads: usize,
    out: &mut [u8],
) {
    let pixels = colors.len().min(out.len() / 4);
    let colors = &colors[..pixels];
    let out = &mut out[..pixels * 4];
    if colors.is_empty() {
        return;
    }

    let num_threads = effective_thread_count(threads).max(1);
    let chunk_len = pixels.div_ceil(num_threads).max(1);

    if chunk_len >= pixels {
        // Whole buffer fits in one chunk (small image, or threads == 1): skip the
        // thread::scope machinery entirely rather than spawn a single worker for it.
        tonemap_chunk(colors, out, scale);
        return;
    }

    std::thread::scope(|s| {
        let color_chunks = colors.chunks(chunk_len);
        let byte_chunks = out.chunks_mut(chunk_len * 4);
        for (color_chunk, byte_chunk) in color_chunks.zip(byte_chunks) {
            s.spawn(move || tonemap_chunk(color_chunk, byte_chunk, scale));
        }
    });
}

/// Same as [`tonemap_to_rgba_with_threads_into`] with the OS-decided ("auto") thread count.
///
/// The allocation-free entry point for a caller that reuses its byte buffer (or writes
/// straight into a display surface) every frame.
pub fn tonemap_to_rgba_into(colors: &[Vec3], scale: f32, out: &mut [u8]) {
    tonemap_to_rgba_with_threads_into(colors, scale, 0, out);
}

/// Tone-maps `colors` into a fresh `colors.len() * 4`-byte RGBA buffer.
///
/// [`tonemap_to_rgba_with_threads_into`] into a new allocation; see it for the scale,
/// threading and bit-identity contract.
#[must_use]
pub fn tonemap_to_rgba_with_threads(colors: &[Vec3], scale: f32, threads: usize) -> Vec<u8> {
    let mut out = vec![0u8; colors.len() * 4];
    tonemap_to_rgba_with_threads_into(colors, scale, threads, &mut out);
    out
}

/// Same as [`tonemap_to_rgba_with_threads`] with the OS-decided ("auto") thread count.
///
/// The entry point every real call site that needs a fresh buffer should use; the
/// explicit-thread-count form exists mainly for tests and callers that already manage
/// their own thread budget.
#[must_use]
pub fn tonemap_to_rgba(colors: &[Vec3], scale: f32) -> Vec<u8> {
    tonemap_to_rgba_with_threads(colors, scale, 0)
}

/// [`tonemap_chunk`]'s twin for any non-`Srgb` [`ColorSpace`]: `ColorSpace::encode` with
/// `ToneMap::AcesFilmic { exposure: 1.0 }`, which reproduces `xyz_to_srgb_gamma`'s
/// gamut compression and tone curve exactly, so only the primaries and transfer curve
/// change -- never exposure or tone-curve shape. Pure per pixel, like [`tonemap_chunk`].
fn tonemap_chunk_in(colors: &[Vec3], dst: &mut [u8], scale: f32, color_space: ColorSpace) {
    debug_assert_eq!(dst.len(), colors.len() * 4);
    for (i, xyz) in colors.iter().enumerate() {
        let rgba = color_space.encode(*xyz * scale, ToneMap::AcesFilmic { exposure: 1.0 });
        dst[i * 4..i * 4 + 4].copy_from_slice(&rgba);
    }
}

/// Tone-maps `colors` (each scaled by `scale`) into `color_space` via
/// [`tonemap_chunk_in`], parallelised exactly like [`tonemap_to_rgba_with_threads`]
/// (`threads == 0` auto-detects). Bit-identical for any thread count.
#[must_use]
pub fn tonemap_wide_gamut_with_threads(
    colors: &[Vec3],
    scale: f32,
    color_space: ColorSpace,
    threads: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; colors.len() * 4];
    if colors.is_empty() {
        return out;
    }
    let num_threads = effective_thread_count(threads).max(1);
    let chunk_len = colors.len().div_ceil(num_threads).max(1);
    if chunk_len >= colors.len() {
        tonemap_chunk_in(colors, &mut out, scale, color_space);
        return out;
    }
    std::thread::scope(|s| {
        let color_chunks = colors.chunks(chunk_len);
        let byte_chunks = out.chunks_mut(chunk_len * 4);
        for (color_chunk, byte_chunk) in color_chunks.zip(byte_chunks) {
            s.spawn(move || tonemap_chunk_in(color_chunk, byte_chunk, scale, color_space));
        }
    });
    out
}

/// Tone-maps a finished accumulation buffer to RGBA8 for `color_space`.
///
/// `accum` is summed XYZ radiance of `samples_per_pixel` samples per pixel,
/// `width * height` long. This is the export's one final 8-bit conversion (see the
/// module doc comment). `ColorSpace::Srgb` takes [`tonemap_to_rgba`] (the live viewport's own
/// `xyz_to_srgb_gamma` path, so an sRGB export matches the screen); every other space
/// takes [`tonemap_wide_gamut_with_threads`].
#[must_use]
pub fn tonemap_accumulation(
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    accum: &[Vec3],
    color_space: ColorSpace,
) -> Vec<u8> {
    debug_assert_eq!(
        accum.len(),
        (width as usize) * (height as usize),
        "accum must hold exactly width*height pixels"
    );
    let inv_samples = 1.0 / samples_per_pixel as f32;
    if color_space == ColorSpace::Srgb {
        tonemap_to_rgba(accum, inv_samples)
    } else {
        tonemap_wide_gamut_with_threads(accum, inv_samples, color_space, 0)
    }
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

    /// Writing into a caller-provided buffer gives exactly the bytes the allocating form
    /// returns, for every thread count, and a reused (dirty) buffer is fully overwritten.
    #[test]
    fn into_variant_matches_the_allocating_form() {
        let colors = irregular_colors(3_001, 0x5a5a_1234);
        let scale = 0.61;
        let expected = tonemap_to_rgba_with_threads(&colors, scale, 1);

        let mut reused = vec![0xAB_u8; colors.len() * 4];
        for threads in [1usize, 2, 7, 64] {
            reused.fill(0xAB);
            tonemap_to_rgba_with_threads_into(&colors, scale, threads, &mut reused);
            assert_eq!(reused, expected, "threads={threads}");
        }

        reused.fill(0xAB);
        tonemap_to_rgba_into(&colors, scale, &mut reused);
        assert_eq!(reused, expected, "auto thread count");
    }

    /// A mismatched buffer is handled without a panic: only the pixels that fit are
    /// converted and the rest of `out` is left as it was.
    #[test]
    fn into_variant_converts_only_what_fits() {
        let colors = irregular_colors(10, 0x0102_0304);
        let full = tonemap_to_rgba(&colors, 1.0);

        let mut short = vec![0xCD_u8; 4 * 6 + 3];
        tonemap_to_rgba_into(&colors, 1.0, &mut short);
        assert_eq!(short[..24], full[..24]);
        assert_eq!(short[24..], [0xCD; 3]);

        let mut long = vec![0xCD_u8; 4 * 12];
        tonemap_to_rgba_into(&colors, 1.0, &mut long);
        assert_eq!(long[..40], full[..]);
        assert_eq!(long[40..], [0xCD; 8]);
    }

    /// The export's tone-mapping exactly as `indicatrix-cut`'s
    /// `export_thread::tonemap_png::tonemap_accumulation` wrote it before it moved here,
    /// serial and unrefactored: the "before" side of the byte-identity pin.
    fn pre_move_export_tonemap(
        samples_per_pixel: u32,
        accum: &[Vec3],
        color_space: ColorSpace,
    ) -> Vec<u8> {
        let inv_samples = 1.0 / samples_per_pixel as f32;
        let mut out = vec![0u8; accum.len() * 4];
        for (i, xyz) in accum.iter().enumerate() {
            let rgba = if color_space == ColorSpace::Srgb {
                xyz_to_srgb_gamma(*xyz * inv_samples)
            } else {
                color_space.encode(*xyz * inv_samples, ToneMap::AcesFilmic { exposure: 1.0 })
            };
            out[i * 4..i * 4 + 4].copy_from_slice(&rgba);
        }
        out
    }

    /// FNV-1a 64 over `bytes` -- a dependency-free fingerprint for the pin below.
    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    const EVERY_COLOR_SPACE: [ColorSpace; 4] = [
        ColorSpace::Srgb,
        ColorSpace::DisplayP3,
        ColorSpace::Rec2020,
        ColorSpace::AcesCg,
    ];

    /// The moved function is byte-identical to the pre-move export code for every
    /// colour space, on a buffer big enough to be split across threads.
    #[test]
    fn tonemap_accumulation_matches_the_pre_move_export_code() {
        let (width, height) = (97_u32, 61_u32);
        let accum = irregular_colors((width * height) as usize, 0x0bad_cafe);
        for color_space in EVERY_COLOR_SPACE {
            assert_eq!(
                tonemap_accumulation(width, height, 37, &accum, color_space),
                pre_move_export_tonemap(37, &accum, color_space),
                "{color_space:?}"
            );
            for threads in [1_usize, 3, 16] {
                assert_eq!(
                    tonemap_wide_gamut_with_threads(&accum, 1.0 / 37.0, color_space, threads),
                    tonemap_wide_gamut_with_threads(&accum, 1.0 / 37.0, color_space, 1),
                    "{color_space:?} threads={threads}"
                );
            }
        }
    }

    /// Pins the output bytes for a small fixed buffer, per colour space, so a later
    /// change to the shared tone curve (which would silently change every export AND
    /// every server-side final picture) fails here first. The hashes were taken from
    /// [`pre_move_export_tonemap`], i.e. the code as it was before the move.
    #[test]
    fn tonemap_accumulation_output_is_pinned() {
        // Pinned with the sRGB encode's final 8-bit quantisation at round-to-nearest.
        const PINNED: [(ColorSpace, u64); 4] = [
            (ColorSpace::Srgb, 0x74aa_b8aa_51e1_87e5),
            (ColorSpace::DisplayP3, 0xdcf9_71d4_dbd5_c9d9),
            (ColorSpace::Rec2020, 0x4855_a258_e239_8cf4),
            (ColorSpace::AcesCg, 0x1f4f_8288_d988_9085),
        ];
        let (width, height) = (8_u32, 5_u32);
        let accum = irregular_colors((width * height) as usize, 0x1357_9bdf);
        let actual = PINNED.map(|(color_space, _)| {
            let before = fnv1a64(&pre_move_export_tonemap(16, &accum, color_space));
            let after = fnv1a64(&tonemap_accumulation(
                width,
                height,
                16,
                &accum,
                color_space,
            ));
            assert_eq!(before, after, "{color_space:?}");
            (color_space, after)
        });
        assert_eq!(actual, PINNED, "actual hashes: {actual:#x?}");
    }
}
