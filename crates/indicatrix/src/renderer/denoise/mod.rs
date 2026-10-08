//! Edge-avoiding À-Trous wavelet denoiser.
//!
//! Correct spectral dispersion means that once a path refracts dispersively, only the
//! hero wavelength survives -- each sample carries one wavelength's color rather than
//! an average of eight, producing both the fire and the chromatic speckle in the
//! viewport (the same phenomenon). That noise is removed by filtering (this module) or
//! by more samples, not by making transport less correct.
//!
//! An edge-avoiding À-Trous wavelet filter (Dammertz, Sewtz, Hachisuka, Hensley Danskin
//! 2010), not full A-SVGF: a handful of separable-in-spirit passes, no temporal history
//! or motion vectors, capturing most of the benefit of a full spatiotemporal filter for
//! a fraction of the complexity. Each pass convolves with a widening (dilated) 5x5
//! B3-spline kernel while down-weighting neighbours whose guide buffers disagree with
//! the centre pixel's.
//!
//! # Wiring
//!
//! Self-contained: does not import `optics` or app code.
//! `optics::raytracer::trace_spectral_ray` takes an optional `primary_hit_out` and, at
//! bounce 0 only, copies that ray's [`crate::optics::raytracer::HitRecord`] (`t`,
//! `normal`, `facet_idx`) into it.
//! The GUI's render loop owns the accumulation buffer and a persistent
//! [`AtrousDenoiser`] (called through `renderer::frame_denoise`): it captures each
//! pixel's first hit into three parallel depth/normal/facet-id buffers, then calls
//! [`AtrousDenoiser::denoise_into`] on the averaged-XYZ accumulation buffer plus those
//! three before tone-mapping. The accumulation buffer itself is never overwritten.
//!
//! ```ignore
//! let mut denoiser = AtrousDenoiser::new(); // persistent, alongside the accum buffer
//! let inputs = GBuffers {
//!     color: &avg_color_buf,        // accum_buffer[i] / current_sample_count
//!     depth: &first_hit_depth,
//!     normal: &first_hit_normal,
//!     facet_id: &first_hit_facet_id, // -1 for background/miss pixels
//!     path_sig: None,                // or Some(&guides.path_sig), see below
//!     width, height, spp: current_sample_count,
//! };
//! denoiser.denoise_into(&inputs, &AtrousParams::default(), &mut filtered_buf);
//! // tone-map `filtered_buf` instead of `avg_color_buf`
//! ```
//!
//! When the eight hero wavelengths disperse to different facets on one primary ray, the
//! guide hit is simply the hero channel's own first hit.
//!
//! # Guide signals and edge-stopping
//!
//! Four terms gate every neighbour's contribution, multiplied with the spatial kernel
//! weight (plus one optional hard test, the path signature, applied first):
//!
//! - **Facet identity** (dominant, constant across sample counts): hard Kronecker-delta
//!   weight (`1.0` on match, `0.0` otherwise) -- facet index is discrete geometry, not a
//!   noisy estimator, and a blurred facet edge destroys the crisp cut look.
//! - **Path signature** (optional, [`GBuffers::path_sig`]): a second hard match on the
//!   hash of the facets the centre ray meets inside the stone. Inside one crown facet the
//!   reflection pattern is the map of those interior regions; colour alone cannot tell it
//!   from speckle, geometry can. A pure early return, so with `None` the filter is
//!   bit-identical to the three-term-plus-colour form below.
//! - **Normal**: `max(0, dot(n_p, n_q))^normal_power` (SVGF-style cosine-power weight),
//!   a safety net for normal discontinuities facet id alone might miss; also constant.
//! - **Depth**: `exp(-|z_p - z_q| / sigma_depth)`. A secondary tie-breaker (facets are
//!   planar, so depth is already near-continuous within one); also constant.
//! - **color** (the only tapered term): `exp(-||c_p - c_q||^2 / (2 * sigma_color^2))`,
//!   compared as full XYZ distance rather than luminance, since the targeted noise is
//!   chromatic. The only guide sigma that scales with sample count, since color is the
//!   only one of the four that is actually a noisy Monte-Carlo estimator here.
//!
//! # Background and non-finite texels
//!
//! A pixel whose facet id is negative is background: its own value is copied through
//! unfiltered, so the backdrop is never touched and costs no tap evaluations. (No
//! foreground pixel can draw on it either: a negative id never equals a real facet id.)
//!
//! A non-finite color texel cannot contaminate its neighbours: as a neighbour it is
//! skipped (weight zero), and as a pixel's own value it is treated as black. Whenever
//! the filter runs, every output value is therefore finite.
//!
//! # The taper curve
//!
//! MC estimator error scales as `O(1/sqrt(N))`, so `sigma_color` scales with the
//! expected remaining noise rather than staying fixed -- otherwise at high sample
//! counts the filter keeps smearing real detail no longer distinguishable from noise:
//!
//! ```text
//! taper(spp) = 1 / sqrt(1 + spp / N0)          (N0 = TAPER_REFERENCE_SPP = 4.0)
//! sigma_color_effective(spp) = sigma_color_base * taper(spp)
//! ```
//!
//! `taper = 1.0` at `spp = 0`, `~0.71` at `spp = N0`, `0.447` at `spp = 4*N0 = 16`;
//! smooth and never exactly zero, matching real MC noise.
//!
//! Below `taper < TAPER_IDENTITY_EPSILON = 0.02` (around `spp ~= 10_000`) the filter
//! short-circuits to a plain copy: every neighbour's color weight is by then
//! numerically indistinguishable from the centre's own weight-of-one, and this also
//! gives an exact bit-identical identity result.
//!
//! # Allocation and threads
//!
//! [`AtrousDenoiser`] owns two scratch images ping-ponged across passes (plus the
//! pre-normalised normal buffer), resized only on a frame-dimension change -- zero
//! steady-state heap allocation when kept alive across frames. All passes of one call
//! share a single `std::thread::scope` (see the private `pass` module), so a denoise
//! spawns its worker threads once, not once per pass. [`atrous_denoise`] is a one-shot
//! convenience wrapper that allocates a fresh denoiser per call; prefer the struct form
//! on any hot path.

mod pass;

use super::tonemap::effective_thread_count;
use glam::Vec3;
use pass::{SharedColors, TapConstants, run_passes, unit_normal_or_zero};

/// The auxiliary ("guide") buffers the filter needs alongside the noisy color buffer,
/// one entry per pixel, row-major (`index = y * width + x`).
///
/// All buffer slices must have exactly `width * height` elements; [`AtrousDenoiser::denoise`]
/// treats a mismatched length as a degenerate/empty input (see its docs) rather than
/// panicking.
#[derive(Clone, Copy)]
pub struct GBuffers<'a> {
    /// Averaged CIE XYZ radiance per pixel (i.e. the accumulation buffer already
    /// divided by `spp`), row-major.
    pub color: &'a [Vec3],
    /// First-hit depth (camera-space ray parameter `t`, or any monotonic distance
    /// measure) per pixel. Non-finite depth values are treated as "no information"
    /// (contribute a neutral depth weight) rather than propagating NaNs, but a large
    /// finite sentinel (e.g. `1.0e6`) is still the recommended convention for
    /// background/miss pixels since it composes better with future refinements.
    pub depth: &'a [f32],
    /// First-hit shading normal per pixel, need not be pre-normalised (the filter
    /// normalises defensively).
    pub normal: &'a [Vec3],
    /// First-hit facet index per pixel, as `i32` so that background/miss pixels can be
    /// encoded as `-1`. Any negative value marks a background pixel, which the filter
    /// copies through unfiltered; a caller should use a single consistent sentinel such
    /// as `-1` for every miss.
    pub facet_id: &'a [i32],
    /// Optional interior path signature per pixel (`GuideBuffers::path_sig`): a hash of
    /// the facets the pixel's centre ray meets inside the stone. A tap whose signature
    /// differs from the centre's is rejected outright, like a different facet id, so the
    /// blur stops at the boundaries of the reflection regions inside one facet. `None`
    /// leaves the filter exactly as it is without the term. A slice that is not
    /// `width * height` long degrades to the identity copy, like the other guides.
    pub path_sig: Option<&'a [u32]>,
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Accumulated sample count backing `color`. Drives the convergence taper --
    /// larger `spp` means less filtering.
    pub spp: u32,
}

/// Tunable parameters for the À-Trous filter. [`AtrousParams::default`] gives
/// reasonable starting points for a normalised-radiance (roughly `0..~4` XYZ Y)
/// gemstone render; scene-specific tuning is expected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtrousParams {
    /// Number of À-Trous passes. Kernel stride doubles each pass starting at 1 (so 5
    /// passes covers dilation strides 1, 2, 4, 8, 16). 4-5 is the standard range;
    /// values outside `1..=8` are clamped.
    pub num_passes: u32,
    /// Base color edge-stopping sigma (XYZ Euclidean distance) at `spp = 0`, before
    /// the convergence taper is applied. Smaller = stricter (less blur).
    pub sigma_color: f32,
    /// Depth edge-stopping sigma, in the same units as [`GBuffers::depth`]. Smaller =
    /// stricter.
    pub sigma_depth: f32,
    /// Exponent on the clamped normal dot product (`max(0,dot)^normal_power`). Larger
    /// = stricter (SVGF commonly uses values in the 32-128 range).
    pub normal_power: f32,
    /// Reference sample count `N0` in the taper curve `1 / sqrt(1 + spp / N0)`. Smaller
    /// = the filter backs off faster as samples accumulate.
    pub taper_reference_spp: f32,
    /// Taper value below which the filter short-circuits to an exact identity copy
    /// instead of running the pass pipeline.
    pub taper_identity_epsilon: f32,
}

impl Default for AtrousParams {
    fn default() -> Self {
        Self {
            num_passes: 5,
            sigma_color: 0.35,
            sigma_depth: 0.1,
            normal_power: 64.0,
            taper_reference_spp: 4.0,
            taper_identity_epsilon: 0.02,
        }
    }
}

/// Small denominator guard shared by every `exp(-x / sigma)` edge-stopping term, so a
/// caller-supplied sigma of exactly zero degrades to "reject everything but an exact
/// match" instead of dividing by zero.
const SIGMA_EPS: f32 = 1.0e-8;

/// `taper(spp) = 1 / sqrt(1 + spp / N0)`. See the module docs for the derivation.
#[must_use]
pub fn taper_strength(spp: u32, taper_reference_spp: f32) -> f32 {
    let n0 = taper_reference_spp.max(SIGMA_EPS);
    1.0 / (1.0 + spp as f32 / n0).sqrt()
}

/// A persistent À-Trous denoiser.
///
/// Owns two scratch images sized to the last-seen frame dimensions and ping-pongs
/// between them across passes, so steady-state use (one instance kept alive across
/// frames) performs no per-call heap allocation beyond the returned/written output.
#[derive(Default)]
pub struct AtrousDenoiser {
    /// The ping-pong images: pass `n` reads `buffers[n % 2]` and writes the other.
    buffers: [SharedColors; 2],
    /// Scratch buffer for the per-call pre-normalised normal buffer -- see
    /// [`Self::denoise_into_with_threads`] and `pass::unit_normal_or_zero`.
    normal_n: Vec<Vec3>,
}

impl AtrousDenoiser {
    /// Creates a denoiser with no scratch buffers allocated yet; the first call to
    /// [`Self::denoise`] sizes them.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffers: [SharedColors::new(), SharedColors::new()],
            normal_n: Vec::new(),
        }
    }

    fn ensure_capacity(&mut self, len: usize) {
        for buffer in &mut self.buffers {
            buffer.resize(len);
        }
        if self.normal_n.len() != len {
            self.normal_n.clear();
            self.normal_n.resize(len, Vec3::ZERO);
        }
    }

    /// Filters `inputs.color` using the auxiliary guide buffers and returns a new
    /// `Vec<Vec3>` of the same length. See [`Self::denoise_into`] for the
    /// allocation-free form.
    ///
    /// Degenerate inputs (zero-size image, mismatched buffer lengths, `num_passes ==
    /// 0`) return a plain clone of `inputs.color` (or an empty vec for a zero-size
    /// image) rather than panicking.
    #[must_use]
    pub fn denoise(&mut self, inputs: &GBuffers<'_>, params: &AtrousParams) -> Vec<Vec3> {
        let mut output = vec![Vec3::ZERO; inputs.color.len()];
        self.denoise_into(inputs, params, &mut output);
        output
    }

    /// Filters `inputs.color` into `output` in place. `output` is resized to
    /// `inputs.color.len()` if it does not already match.
    ///
    /// Robustness guarantees: never panics, regardless of image size (including 0x0 and
    /// 1x1), `spp` (including 0), or buffer contents. A non-finite color texel never
    /// reaches another pixel (see the module docs), so whenever the filter runs every
    /// output value is finite; the two paths that copy `color` verbatim -- the
    /// converged-taper identity short-circuit and the guide-buffer fallback below -- pass
    /// the input through as it is. A guide-buffer slice shorter than `width * height` is
    /// treated as "no filtering" (falls back to a copy of `color`).
    ///
    /// Runs each pass across all available CPU cores (see
    /// [`Self::denoise_into_with_threads`] for a pinned thread count) -- the difference
    /// between multi-second and sub-second passes at high resolution.
    pub fn denoise_into(
        &mut self,
        inputs: &GBuffers<'_>,
        params: &AtrousParams,
        output: &mut Vec<Vec3>,
    ) {
        self.denoise_into_with_threads(inputs, params, output, 0);
    }

    /// Same as [`Self::denoise_into`], but with an explicit thread count instead of the
    /// auto-detected one. `threads == 0` means "let the OS decide" (i.e. what
    /// [`Self::denoise_into`] does internally), matching the `--threads`-style
    /// convention [`effective_thread_count`] resolves.
    ///
    /// Exposed mainly so callers (and this module's tests) can pin a thread count -- to
    /// verify thread-count invariance, or bound worker threads in an environment that
    /// manages its own pool. Every À-Trous pass is a pure per-pixel function of the
    /// previous pass's output, so the result is bit-identical for any `threads >= 1`.
    pub fn denoise_into_with_threads(
        &mut self,
        inputs: &GBuffers<'_>,
        params: &AtrousParams,
        output: &mut Vec<Vec3>,
        threads: usize,
    ) {
        let len = inputs.width * inputs.height;
        output.clear();
        output.resize(len, Vec3::ZERO);

        if len == 0 {
            return;
        }

        // Any missing/short guide buffer -> we cannot safely index it per pixel, so
        // degrade to an identity copy rather than panicking or reading out of bounds.
        let buffers_ok = inputs.color.len() == len
            && inputs.depth.len() == len
            && inputs.normal.len() == len
            && inputs.facet_id.len() == len
            && inputs.path_sig.is_none_or(|sig| sig.len() == len);
        if !buffers_ok {
            let n = inputs.color.len().min(len);
            output[..n].copy_from_slice(&inputs.color[..n]);
            for v in &mut output[n..] {
                *v = Vec3::ZERO;
            }
            return;
        }

        let taper = taper_strength(inputs.spp, params.taper_reference_spp);
        let num_passes = params.num_passes.clamp(1, 8);

        if taper < params.taper_identity_epsilon {
            output.copy_from_slice(inputs.color);
            return;
        }

        let sigma_color_effective = (params.sigma_color * taper).max(0.0);
        let num_threads = effective_thread_count(threads);

        self.ensure_capacity(len);
        self.buffers[0].fill_from(inputs.color);

        // Pre-normalise the normal buffer once per call rather than up to 250 times
        // (25 taps x up to 8 passes) per pixel -- see `unit_normal_or_zero` and
        // `TapConstants::normal_n`. `buffers_ok` above already guarantees
        // `inputs.normal.len() == len`.
        for (dst, &src_n) in self.normal_n.iter_mut().zip(inputs.normal.iter()) {
            *dst = unit_normal_or_zero(src_n);
        }

        let sigma_depth = params.sigma_depth.max(SIGMA_EPS);
        let sigma_color_sq = (sigma_color_effective * sigma_color_effective).max(SIGMA_EPS);
        let tap = TapConstants {
            normal_n: &self.normal_n,
            neg_inv_sigma_depth: -1.0 / sigma_depth,
            neg_inv_two_sigma_color_sq: -1.0 / (2.0 * sigma_color_sq),
            normal_power: params.normal_power,
        };

        let result = run_passes(&self.buffers, inputs, &tap, num_passes, num_threads);
        self.buffers[result].copy_to(output);
    }
}

/// One-shot convenience wrapper around [`AtrousDenoiser`].
///
/// Allocates a fresh denoiser (and therefore fresh scratch buffers) per call. Prefer
/// keeping an [`AtrousDenoiser`] alive across frames on any real render loop -- see
/// the module docs' "Wiring" section.
#[must_use]
pub fn atrous_denoise(inputs: &GBuffers<'_>, params: &AtrousParams) -> Vec<Vec3> {
    AtrousDenoiser::new().denoise(inputs, params)
}

#[cfg(test)]
mod tests {
    use super::{pass::cos_pow, *};

    /// Deterministic xorshift32 PRNG, matching the one in
    /// `crates/indicatrix/tests/denoise_tests.rs` (kept separate rather than shared since
    /// that file is a different crate as far as visibility is concerned).
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
        fn next_signed(&mut self) -> f32 {
            self.next_f32().mul_add(2.0, -1.0)
        }
    }

    /// Irregular dimensions, several facet regions, jittered normals, varying depth, and
    /// noisy color -- exercises every edge-stopping term rather than a uniform image.
    fn irregular_scene(width: usize, height: usize, seed: u32) -> (GBuffers<'static>, Vec<Vec3>) {
        let len = width * height;
        let mut rng = Xorshift32::new(seed);
        let mut color = vec![Vec3::ZERO; len];
        let mut depth = vec![0.0f32; len];
        let mut normal = vec![Vec3::Z; len];
        let mut facet_id = vec![0i32; len];
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                facet_id[idx] = ((x / 5 + y / 7) % 6) as i32;
                depth[idx] = rng.next_f32() * 4.0;
                let jitter =
                    Vec3::new(rng.next_signed() * 0.3, rng.next_signed() * 0.3, 1.0).normalize();
                normal[idx] = jitter;
                color[idx] = Vec3::new(rng.next_f32(), rng.next_f32(), rng.next_f32());
            }
        }
        // Leak so we can hand back `&'static` slices from owned Vecs without fighting
        // the borrow checker in a test helper; test-only, freed at process exit.
        let color_s: &'static [Vec3] = Box::leak(color.clone().into_boxed_slice());
        let depth_s: &'static [f32] = Box::leak(depth.into_boxed_slice());
        let normal_s: &'static [Vec3] = Box::leak(normal.into_boxed_slice());
        let facet_s: &'static [i32] = Box::leak(facet_id.into_boxed_slice());
        (
            GBuffers {
                color: color_s,
                depth: depth_s,
                normal: normal_s,
                facet_id: facet_s,
                path_sig: None,
                width,
                height,
                spp: 1,
            },
            color,
        )
    }

    fn assert_bit_identical(a: &[Vec3], b: &[Vec3], context: &str) {
        assert_eq!(a.len(), b.len(), "{context}: length mismatch");
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(
                x.x.to_bits(),
                y.x.to_bits(),
                "{context}: pixel {i} x differs"
            );
            assert_eq!(
                x.y.to_bits(),
                y.y.to_bits(),
                "{context}: pixel {i} y differs"
            );
            assert_eq!(
                x.z.to_bits(),
                y.z.to_bits(),
                "{context}: pixel {i} z differs"
            );
        }
    }

    /// Several consecutive passes must produce bit-identical output regardless of
    /// thread count, including counts that don't evenly divide the row count or exceed
    /// it. Three passes cover strides 1, 2 and 4 and both ping-pong directions.
    #[test]
    fn passes_are_thread_count_invariant() {
        let width = 137;
        let height = 91;
        let (g, color) = irregular_scene(width, height, 0x1234_5678);
        let params = AtrousParams::default();
        let sigma = params.sigma_color;

        let normal_n: Vec<Vec3> = g.normal.iter().map(|&n| unit_normal_or_zero(n)).collect();
        let tap = TapConstants {
            normal_n: &normal_n,
            neg_inv_sigma_depth: -1.0 / params.sigma_depth.max(SIGMA_EPS),
            neg_inv_two_sigma_color_sq: -1.0 / (2.0 * (sigma * sigma).max(SIGMA_EPS)),
            normal_power: params.normal_power,
        };

        let run = |threads: usize| {
            let mut buffers = [SharedColors::new(), SharedColors::new()];
            for buffer in &mut buffers {
                buffer.resize(width * height);
            }
            buffers[0].fill_from(&color);
            let result = run_passes(&buffers, &g, &tap, 3, threads);
            let mut out = vec![Vec3::ZERO; width * height];
            buffers[result].copy_to(&mut out);
            out
        };

        let reference = run(1);
        for threads in [2usize, 3, 8, 16, 200] {
            assert_bit_identical(&reference, &run(threads), &format!("threads={threads}"));
        }
    }

    /// [`cos_pow`]'s fast path (repeated squaring for a power-of-two exponent) must
    /// agree with `f32::powf` to within a small ULP tolerance, for the exact exponent
    /// [`AtrousParams::default`] uses (64) as well as a few others, and its fallback
    /// path must be bit-identical to `powf` (it just calls it) for a non-power-of-two
    /// exponent.
    #[test]
    fn cos_pow_matches_powf_within_tolerance() {
        for &power in &[1.0f32, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0] {
            for &base in &[0.0f32, 0.1, 0.37, 0.5, 0.9, 1.0] {
                let fast = cos_pow(base, power);
                let reference = base.powf(power);
                let diff = (fast - reference).abs();
                assert!(
                    diff <= reference.abs().mul_add(1.0e-5, 1.0e-6),
                    "cos_pow({base}, {power}) = {fast}, powf = {reference}, diff = {diff}"
                );
            }
        }

        // Non-power-of-two exponent: must fall through to powf exactly, since the fast
        // path never engages. Iterated rather than literal .powf(5.0) calls so clippy's
        // suboptimal_flops lint has no literal integer exponent to flag.
        for &power in &[5.0f32, 60.0] {
            for &base in &[0.0f32, 0.3, 0.7, 1.0] {
                assert_eq!(cos_pow(base, power), base.powf(power));
            }
        }
    }

    /// Same guarantee through the full multi-pass pipeline rather than a single pass --
    /// what the render loop actually calls.
    #[test]
    fn denoise_into_is_thread_count_invariant() {
        let width = 113;
        let height = 67;
        let (g, _color) = irregular_scene(width, height, 0xabcd_ef01);
        let params = AtrousParams::default();

        let mut denoiser = AtrousDenoiser::new();
        let mut reference = Vec::new();
        denoiser.denoise_into_with_threads(&g, &params, &mut reference, 1);

        for threads in [2usize, 4, 8, 16] {
            let mut denoiser = AtrousDenoiser::new();
            let mut out = Vec::new();
            denoiser.denoise_into_with_threads(&g, &params, &mut out, threads);
            assert_bit_identical(&reference, &out, &format!("threads={threads}"));
        }

        // And the public auto-thread-count entry point must agree with the pinned
        // single-threaded reference too.
        let mut denoiser = AtrousDenoiser::new();
        let mut out = Vec::new();
        denoiser.denoise_into(&g, &params, &mut out);
        assert_bit_identical(&reference, &out, "auto thread count");
    }

    /// A signature that is the same everywhere changes nothing: `Some(constant)` is
    /// bit-identical to `None`, so the term is a pure early return.
    #[test]
    fn a_constant_signature_is_bit_identical_to_none() {
        let (g, _color) = irregular_scene(61, 47, 0x5151);
        let params = AtrousParams::default();
        let sig = vec![0xdead_u32; 61 * 47];
        let with_sig = GBuffers {
            path_sig: Some(&sig),
            ..g
        };
        let mut a = Vec::new();
        let mut b = Vec::new();
        AtrousDenoiser::new().denoise_into_with_threads(&g, &params, &mut a, 1);
        AtrousDenoiser::new().denoise_into_with_threads(&with_sig, &params, &mut b, 1);
        assert_bit_identical(&a, &b, "constant signature");
    }

    /// With a varied signature the result is still independent of the thread count.
    #[test]
    fn signature_filtering_is_thread_count_invariant() {
        let (g, _color) = irregular_scene(83, 59, 0x77aa);
        let sig: Vec<u32> = (0..83 * 59)
            .map(|i| ((i % 83) / 6 + (i / 83) / 5) as u32 % 4)
            .collect();
        let with_sig = GBuffers {
            path_sig: Some(&sig),
            ..g
        };
        let params = AtrousParams::default();
        let mut reference = Vec::new();
        AtrousDenoiser::new().denoise_into_with_threads(&with_sig, &params, &mut reference, 1);
        for threads in [2usize, 3, 8] {
            let mut out = Vec::new();
            AtrousDenoiser::new().denoise_into_with_threads(&with_sig, &params, &mut out, threads);
            assert_bit_identical(&reference, &out, &format!("threads={threads}"));
        }
    }

    /// A signature slice of the wrong length falls back to the identity copy.
    #[test]
    fn a_short_signature_buffer_falls_back_to_a_copy() {
        let (g, color) = irregular_scene(20, 10, 9);
        let sig = vec![0u32; 5];
        let with_sig = GBuffers {
            path_sig: Some(&sig),
            ..g
        };
        let mut out = Vec::new();
        AtrousDenoiser::new().denoise_into(&with_sig, &AtrousParams::default(), &mut out);
        assert_bit_identical(&color, &out, "short signature");
    }

    /// Reusing one denoiser across frames of different sizes must not leak state from
    /// the previous frame into the next.
    #[test]
    fn a_reused_denoiser_matches_a_fresh_one_across_size_changes() {
        let params = AtrousParams::default();
        let mut reused = AtrousDenoiser::new();
        for (width, height, seed) in [(40, 30, 1_u32), (17, 53, 2), (40, 30, 3)] {
            let (g, _color) = irregular_scene(width, height, seed);
            let mut from_reused = Vec::new();
            reused.denoise_into_with_threads(&g, &params, &mut from_reused, 4);
            let mut from_fresh = Vec::new();
            AtrousDenoiser::new().denoise_into_with_threads(&g, &params, &mut from_fresh, 4);
            assert_bit_identical(&from_fresh, &from_reused, &format!("{width}x{height}"));
        }
    }
}
