//! CPU-side HDR equirectangular environment-map loading and importance sampling.
//!
//! `optics::raytracer::sample_studio_environment` is a reasonable analytic studio
//! stand-in but cannot reproduce a real environment, and real gem photography is judged
//! against real environments. This module loads a real HDR panorama and
//! importance-samples it so the renderer spends samples where the environment is
//! actually bright, rather than wasting most of them on a typical HDR capture's dark
//! majority: [`EnvironmentMap::sample`] draws directions proportional to the map's own
//! (solid-angle-corrected) luminance via a 2D piecewise-constant inverse-CDF sampler
//! (see [`super::env_map_distribution`]).
//!
//! `trace_spectral_ray`'s ray-miss branch takes an `EnvironmentSource<'_>` -- `Studio`
//! (the analytic rig, still the default) or `HdrMap(&EnvironmentMap)` -- dispatching
//! each spectral channel's lookup to `sample_studio_environment` or
//! [`EnvironmentMap::radiance_at`] accordingly.
//!
//! [`EnvironmentMap::sample`]/[`EnvironmentMap::pdf`] back a genuine next-event-estimation
//! extension:
//! `optics::raytracer::environment::{sample_environment_for_nee, environment_nee_pdf}`
//! wrap them for `optics::raytracer::scattering`'s Henyey-Greenstein NEE
//! (`scattering::nee_contribution_hg_scatter`) and its frosted-facet-exterior
//! counterpart (`scattering::nee_contribution_frosted_exterior`, which handles a
//! frosted exit's own diffusely-sampled surface point), drawing a light-sampling
//! direction/pdf and evaluating an arbitrary direction's density for a Veach-style
//! balance-heuristic MIS weight, respectively.

use std::f32::consts::PI;

use glam::Vec3;

// Live alongside `env_map.rs` in `renderer/`, not in an `env_map/` subdirectory, so the
// path is spelled out explicitly.
#[path = "env_map_distribution.rs"]
mod env_map_distribution;
#[path = "env_map_spectrum.rs"]
mod env_map_spectrum;

// `Distribution1D` re-exported alongside `Distribution2D` (not just used privately here)
// so `renderer::env_map_gpu::HdrEnvGpuData::upload` can name the type it
// gets back from `Distribution2D::marginal`/`conditional` when flattening it into GPU
// buffers -- see that module's own doc comment. `env_map_distribution` itself stays a
// private submodule nested here (not a sibling `renderer::env_map_distribution`) so
// there is exactly one module path for these types; `env_map_gpu.rs` reaches them via
// this re-export, `crate::renderer::env_map::{Distribution1D, Distribution2D}`.
// `Distribution1D` itself is named only by that `feature = "gpu"` consumer (and
// `renderer::gpu::transport_check::nee`, same feature) -- `Distribution2D` is used
// unconditionally within this module (the `EnvironmentMap::distribution` field type),
// so only the former needs gating.
#[cfg(feature = "gpu")]
pub(crate) use env_map_distribution::Distribution1D;
pub(crate) use env_map_distribution::Distribution2D;
pub use env_map_spectrum::rgb_to_spectral_radiance;

/// Errors constructing an [`EnvironmentMap`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvMapError {
    /// `pixels.len() != width * height`.
    DimensionMismatch {
        width: usize,
        height: usize,
        len: usize,
    },
    /// `width == 0 || height == 0`.
    ZeroSized,
    /// Decoding the supplied HDR bytes failed (only constructible with the `hdr`
    /// feature enabled).
    #[cfg(feature = "hdr")]
    Decode(String),
}

impl std::fmt::Display for EnvMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DimensionMismatch { width, height, len } => {
                write!(
                    f,
                    "environment map pixel buffer has {len} texels, expected width*height = {width}*{height} = {}",
                    width * height
                )
            }
            Self::ZeroSized => write!(f, "environment map width and height must both be non-zero"),
            #[cfg(feature = "hdr")]
            Self::Decode(msg) => write!(f, "failed to decode HDR image: {msg}"),
        }
    }
}

impl std::error::Error for EnvMapError {}

/// An equirectangular HDR environment map plus a precomputed 2D importance-sampling
/// distribution over its texels.
///
/// # Direction / UV convention
///
/// `v` runs `0.0` (north pole, `+Y`) to `1.0` (south pole, `-Y`); `u` runs `0.0` to
/// `1.0` counter-clockwise around `Y` starting from `+Z`. Row `0` of the pixel buffer is
/// `v = 0` (the north pole row); column `0` is `u = 0`. See [`Self::uv_to_direction`]
/// and [`Self::direction_to_uv`] for the exact formulas.
#[derive(Debug, Clone)]
pub struct EnvironmentMap {
    width: usize,
    height: usize,
    /// Row-major linear RGB radiance, one texel per `[r, g, b]`.
    pixels: Vec<[f32; 3]>,
    distribution: Distribution2D,
}

impl EnvironmentMap {
    /// Builds an environment map from an already-decoded row-major RGB radiance buffer.
    /// The constructor every other constructor funnels through, and the one tests use
    /// directly to build synthetic maps without needing the `hdr` feature.
    ///
    /// # Errors
    ///
    /// Returns [`EnvMapError::ZeroSized`] if `width == 0 || height == 0`, or
    /// [`EnvMapError::DimensionMismatch`] if `pixels.len() != width * height`.
    pub fn from_rgb(
        width: usize,
        height: usize,
        pixels: Vec<[f32; 3]>,
    ) -> Result<Self, EnvMapError> {
        if width == 0 || height == 0 {
            return Err(EnvMapError::ZeroSized);
        }
        if pixels.len() != width * height {
            return Err(EnvMapError::DimensionMismatch {
                width,
                height,
                len: pixels.len(),
            });
        }

        let distribution = build_distribution(&pixels, width, height);
        Ok(Self {
            width,
            height,
            pixels,
            distribution,
        })
    }

    /// Builds a constant environment map (every direction returns `radiance`). Useful as
    /// a deliberately simple fallback and, principally, as the white-furnace test's
    /// fixture -- see `tests/env_map_tests.rs`.
    ///
    /// # Panics
    ///
    /// Never in practice: `width`/`height` are clamped to at least `1` before building
    /// the (always dimensionally-consistent) pixel buffer, so the internal
    /// `from_rgb` call cannot actually return `Err`.
    #[must_use]
    pub fn uniform(width: usize, height: usize, radiance: [f32; 3]) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self::from_rgb(width, height, vec![radiance; width * height])
            .expect("uniform() constructs a self-consistent buffer")
    }

    /// Decodes a Radiance `.hdr` equirectangular image from raw bytes.
    ///
    /// Requires the `hdr` feature (pulls in the `image` crate's HDR decoder), kept
    /// behind a feature so the base `indicatrix` build stays at its four core dependencies.
    ///
    /// # Errors
    ///
    /// Returns [`EnvMapError::Decode`] if the bytes are not a valid Radiance HDR image,
    /// or [`EnvMapError::ZeroSized`]/[`EnvMapError::DimensionMismatch`] if the decoded
    /// image is degenerate (should not happen for a well-formed file).
    #[cfg(feature = "hdr")]
    pub fn from_hdr_bytes(bytes: &[u8]) -> Result<Self, EnvMapError> {
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Hdr)
            .map_err(|e| EnvMapError::Decode(e.to_string()))?;
        let rgb = decoded.into_rgb32f();
        let (width, height) = (rgb.width() as usize, rgb.height() as usize);
        let pixels: Vec<[f32; 3]> = rgb.pixels().map(|p| p.0).collect();
        Self::from_rgb(width, height, pixels)
    }

    /// Reads and decodes a Radiance `.hdr` file from `path`. Requires the `hdr` feature.
    ///
    /// # Errors
    ///
    /// Returns [`EnvMapError::Decode`] if the file cannot be read or is not a valid
    /// Radiance HDR image.
    #[cfg(feature = "hdr")]
    pub fn from_hdr_file(path: impl AsRef<std::path::Path>) -> Result<Self, EnvMapError> {
        let bytes = std::fs::read(path).map_err(|e| EnvMapError::Decode(e.to_string()))?;
        Self::from_hdr_bytes(&bytes)
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Row-major `[r, g, b]` texel buffer, `width() * height()` entries -- exposed so
    /// `renderer::env_map_gpu::HdrEnvGpuData::upload` can build the
    /// `vec4<f32>`-padded storage buffer `spectral_transport.wgsl`'s `hdr_texels`
    /// binding reads, without duplicating this type's own row-major layout convention.
    /// `feature = "gpu"`: only that GPU-upload path calls this.
    #[cfg(feature = "gpu")]
    #[must_use]
    pub(crate) fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }

    /// The importance-sampling [`Distribution2D`] backing [`Self::sample`]/[`Self::pdf`].
    ///
    /// Exposed, like [`Self::pixels`], so `renderer::env_map_gpu::HdrEnvGpuData::upload`
    /// can flatten its marginal/conditional cdf and function
    /// arrays into the storage buffers `spectral_transport.wgsl`'s `dist1d_find_bucket`/
    /// `dist2d_sample`/`dist2d_pdf` binary-search, without duplicating this type's own
    /// row-major layout convention. `renderer::gpu::transport_check`'s Tier 2 self-test
    /// uses the same accessor to upload an independent synthetic map. `feature = "gpu"`:
    /// both callers only exist under that feature.
    #[cfg(feature = "gpu")]
    #[must_use]
    pub(crate) const fn distribution(&self) -> &Distribution2D {
        &self.distribution
    }

    /// Maps equirectangular `(u, v) in [0,1)^2` to a unit direction. See the struct docs
    /// for the convention. `u`/`v` outside `[0, 1)` are wrapped/clamped rather than
    /// producing an out-of-range angle.
    #[must_use]
    pub fn uv_to_direction(u: f32, v: f32) -> Vec3 {
        let u = u.rem_euclid(1.0);
        let v = v.clamp(0.0, 1.0);
        let theta = v * PI;
        let phi = u * 2.0 * PI;
        // `sin` of the REFLECTED argument, `cos` of the direct one: near the south pole
        // `sin(v * PI)` is off by percent purely from the rounding of `v * PI` (see
        // `pdf_uv_to_solid_angle`, "Why `sin(min(v, 1-v) * PI)`"), while `cos` is
        // well-conditioned there. Keeps this direction's `sin(theta)` bit-consistent
        // with the pdf [`Self::sample`] reports for it.
        let sin_theta = (v.min(1.0 - v) * PI).sin();
        let cos_theta = theta.cos();
        let (sin_phi, cos_phi) = phi.sin_cos();
        Vec3::new(sin_theta * sin_phi, cos_theta, sin_theta * cos_phi)
    }

    /// The inverse of [`Self::uv_to_direction`]: maps a (not-necessarily-normalized,
    /// non-zero) direction to equirectangular `(u, v) in [0,1)^2`. Degenerate at exactly
    /// the poles, where `u` is mathematically undefined (every `u` maps to the same
    /// point) -- this returns `u = 0.0` there, matching `atan2(0, 0) == 0`.
    #[must_use]
    pub fn direction_to_uv(dir: Vec3) -> (f32, f32) {
        let d = dir.normalize_or_zero();
        let theta = d.y.clamp(-1.0, 1.0).acos();
        let phi = d.x.atan2(d.z);
        let v = theta / PI;
        let u = (phi / (2.0 * PI)).rem_euclid(1.0);
        (u, v)
    }

    /// Bilinearly-filtered RGB radiance lookup for an arbitrary direction. Wraps around
    /// the seam in `u` and clamps at the poles in `v`.
    #[must_use]
    pub fn radiance_rgb(&self, dir: Vec3) -> [f32; 3] {
        let (u, v) = Self::direction_to_uv(dir);
        self.sample_bilinear(u, v)
    }

    /// Spectral radiance at `lambda_nm` for a direct (non-importance-sampled) direction
    /// lookup: bilinear RGB lookup, then [`rgb_to_spectral_radiance`].
    #[must_use]
    pub fn radiance_at(&self, dir: Vec3, lambda_nm: f32) -> f32 {
        rgb_to_spectral_radiance(self.radiance_rgb(dir), lambda_nm)
    }

    /// Importance-samples a direction from two independent uniform randoms in `[0, 1)`,
    /// proportional to the map's solid-angle-weighted luminance. Returns `(direction,
    /// rgb_radiance_at_that_direction, pdf)`, where `pdf` is in **solid-angle measure**
    /// (`integral of pdf(w) dw over the sphere == 1`), not the raw `(u,v)`-texel measure
    /// [`Distribution2D`] works in -- see [`pdf_uv_to_solid_angle`] for the Jacobian.
    #[must_use]
    pub fn sample(&self, u0: f32, u1: f32) -> (Vec3, [f32; 3], f32) {
        let (u, v, pdf_uv) = self.distribution.sample(u0, u1);
        let dir = Self::uv_to_direction(u, v);
        let rgb = self.sample_bilinear(u, v);
        let pdf = pdf_uv_to_solid_angle(pdf_uv, v);
        (dir, rgb, pdf)
    }

    /// The pdf (solid-angle measure) that [`Self::sample`] would assign to `dir`,
    /// computed independently of any particular sample -- what a future BSDF/light MIS
    /// combination needs (Veach's balance/power heuristic evaluates each technique's pdf
    /// at every sampled direction, including ones the other technique produced).
    #[must_use]
    pub fn pdf(&self, dir: Vec3) -> f32 {
        let (u, v) = Self::direction_to_uv(dir);
        let pdf_uv = self.distribution.pdf(u, v);
        // `sin(theta)` straight off the direction (`|(x, z)|` of the unit vector) rather
        // than `sin(acos(y))`: `acos` is ill-conditioned at the poles (its slope is
        // `1/sin(theta)`), and the GPU twin's `acos` is only accurate to about `1e-6`
        // absolute there -- which, divided by a `sin(theta)` of `1e-2`, was a `6e-4`
        // relative pdf gap (9062 ULP) at a direction 0.57 degrees off the south pole.
        // `(x, z)` carry `sin(theta)` to full precision on both sides. `hypot` here
        // (clippy `imprecise_flops` forbids the hand-rolled `sqrt(x*x + z*z)`) pairs with
        // `length(vec2(x, z))` in `transport_bounce.wgsl`'s `dist2d_pdf`; on a unit
        // vector both are the correctly-rounded-to-a-ULP norm of `(x, z)`.
        let d = dir.normalize_or_zero();
        let sin_theta = d.x.hypot(d.z);
        pdf_uv_to_solid_angle_from_sin(pdf_uv, sin_theta)
    }

    /// Bilinear sample of the texel grid at continuous `(u, v)`, wrapping in `u` and
    /// clamping in `v`.
    fn sample_bilinear(&self, u: f32, v: f32) -> [f32; 3] {
        if self.width == 1 && self.height == 1 {
            return self.pixels[0];
        }
        let fx = u.rem_euclid(1.0).mul_add(self.width as f32, -0.5);
        let fy = v.clamp(0.0, 1.0).mul_add(self.height as f32, -0.5);

        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;

        let wrap_x = |x: i64| -> usize { x.rem_euclid(self.width as i64) as usize };
        let clamp_y = |y: i64| -> usize { y.clamp(0, self.height as i64 - 1) as usize };

        let x0i = wrap_x(x0 as i64);
        let x1i = wrap_x(x0 as i64 + 1);
        let y0i = clamp_y(y0 as i64);
        let y1i = clamp_y(y0 as i64 + 1);

        let p00 = self.texel(x0i, y0i);
        let p10 = self.texel(x1i, y0i);
        let p01 = self.texel(x0i, y1i);
        let p11 = self.texel(x1i, y1i);

        let mut out = [0.0f32; 3];
        for c in 0..3 {
            let top = p10[c].mul_add(tx, p00[c] * (1.0 - tx));
            let bottom = p11[c].mul_add(tx, p01[c] * (1.0 - tx));
            out[c] = bottom.mul_add(ty, top * (1.0 - ty));
        }
        out
    }

    fn texel(&self, x: usize, y: usize) -> [f32; 3] {
        self.pixels[y * self.width + x]
    }
}

/// Converts a `(u,v)`-measure pdf (from [`Distribution2D`], `integral over the unit
/// square == 1`) to solid-angle measure for the equirectangular mapping used by
/// [`EnvironmentMap::uv_to_direction`].
///
/// # The Jacobian
///
/// `u = phi / (2*PI)` so `d(phi) = 2*PI du`; `v = theta / PI` so `d(theta) = PI dv`.
/// Solid angle `dw = sin(theta) d(theta) d(phi) = 2*PI^2*sin(theta) du dv`, and a pdf
/// transforms as the inverse of that Jacobian:
/// `pdf_solid_angle(w) = pdf_uv(u,v) / (2*PI^2*sin(theta))`.
///
/// Near the poles `sin(theta) -> 0` and this blows up; a texel row exactly at a pole
/// subtends zero solid angle, so this returns `0.0` there rather than `inf`/`NaN`
/// (matching the `sin(theta)` row-weighting in [`build_distribution`]).
///
/// # Why `sin(min(v, 1-v) * PI)` and not `sin(v * PI)`
///
/// The two are the same number mathematically (`sin(PI - x) == sin(x)`), but not in
/// `f32`: with `v` a few ULPs below `1.0`, `v * PI` rounds to within half an ULP of
/// `PI` (an absolute error of up to `1.2e-7`) and `sin` of that argument is a value of
/// order `1e-6` -- so the *rounding of the argument alone* perturbs `sin_theta`, and
/// hence the returned pdf, by several percent (measured: `v = 0.9999984`
/// gave a pdf 2.7 % high on the CPU and 3.7 % low on the GPU, whose `sin` reduces the
/// argument differently). Folding the reflection in first keeps the argument in
/// `[0, PI/2]`, where `1.0 - v` is exact (Sterbenz) and both `libm` and GPU `sin`
/// are accurate to a couple of ULPs, so the CPU and the WGSL twin
/// (`transport_physics.wgsl`'s `pdf_uv_to_solid_angle`) agree near the south pole
/// exactly as they already did near the north pole. Away from the poles the two forms
/// differ by at most one rounding of the argument.
fn pdf_uv_to_solid_angle(pdf_uv: f32, v: f32) -> f32 {
    let theta = v.min(1.0 - v) * PI;
    pdf_uv_to_solid_angle_from_sin(pdf_uv, theta.sin())
}

/// The Jacobian division of [`pdf_uv_to_solid_angle`] with `sin(theta)` supplied by the
/// caller -- [`EnvironmentMap::pdf`] takes it straight off the direction, where it is
/// better conditioned than any `v`-derived form (see that function's own comment).
fn pdf_uv_to_solid_angle_from_sin(pdf_uv: f32, sin_theta: f32) -> f32 {
    if sin_theta <= 1e-6 {
        return 0.0;
    }
    pdf_uv / (2.0 * PI * PI * sin_theta)
}

/// Rec.709 relative luminance -- used only to weight texels for the importance-sampling
/// distribution, not for any colourimetric output.
fn luminance(rgb: [f32; 3]) -> f32 {
    0.0722f32.mul_add(rgb[2], 0.7152f32.mul_add(rgb[1], 0.2126 * rgb[0]))
}

/// Builds the [`Distribution2D`] for an equirectangular image, weighting each row by
/// `sin(theta)` (theta measured at the row's vertical centre) before handing the
/// weights to `Distribution2D::new`.
///
/// The single most important line in this module: an equirectangular map compresses
/// solid angle toward the poles, so importance-sampling proportional to raw texel
/// luminance would over-sample them relative to their actual visual contribution. The
/// white-furnace test in `tests/env_map_tests.rs` catches a missing/misapplied version.
fn build_distribution(pixels: &[[f32; 3]], width: usize, height: usize) -> Distribution2D {
    let mut weighted = Vec::with_capacity(width * height);
    for y in 0..height {
        let theta = (y as f32 + 0.5) / height as f32 * PI;
        let sin_theta = theta.sin().max(0.0);
        for x in 0..width {
            weighted.push(luminance(pixels[y * width + x]) * sin_theta);
        }
    }
    Distribution2D::new(&weighted, width, height)
}

/// [`EnvironmentMap::sample`]/[`EnvironmentMap::pdf`] consistency at the
/// SOLID-ANGLE-measure level (as opposed to [`env_map_distribution`]'s own
/// `distribution_2d_pdf_matches_sample_pdf`, which checks the same property one layer
/// down in raw `(u, v)`-measure) -- the property `optics::raytracer::scattering`'s NEE
/// balance heuristic actually depends on: the pdf `sample` reports for its own draw must
/// equal what `pdf` independently computes for that same direction, in the measure the
/// rendering-equation integral is actually taken over.
#[cfg(test)]
mod nee_sample_pdf_tests {
    use super::*;

    /// A non-uniform synthetic map (so the importance distribution is genuinely
    /// non-constant, not a degenerate case every direction would trivially agree on).
    fn synthetic_map() -> EnvironmentMap {
        let width = 12;
        let height = 6;
        let pixels: Vec<[f32; 3]> = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let u = x as f32 / width as f32;
                    let v = y as f32 / height as f32;
                    [0.1 + u, 0.2 + v, u.mul_add(v, 0.05)]
                })
            })
            .collect();
        EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
    }

    /// `sample(u0, u1)`'s own returned pdf must equal `pdf(dir)` evaluated independently
    /// at the direction it drew, for many `(u0, u1)` pairs -- exactly the invariant a
    /// balance-heuristic MIS weight assumes when it calls `pdf()` on a direction `sample()`
    /// produced (see `optics::raytracer::scattering::nee_contribution_hg_scatter`'s own
    /// use of the companion technique's pdf at the light-sampled direction).
    #[test]
    fn sample_pdf_matches_independently_evaluated_pdf_in_solid_angle_measure() {
        let map = synthetic_map();
        let mut checked = 0u32;
        for i in 0..37u32 {
            for j in 0..29u32 {
                let u0 = (i as f32 + 0.5) / 37.0;
                let u1 = (j as f32 + 0.5) / 29.0;
                let (dir, _rgb, pdf_sampled) = map.sample(u0, u1);
                let pdf_looked_up = map.pdf(dir);
                assert!(
                    (pdf_sampled - pdf_looked_up).abs() < 1e-3 * pdf_sampled.max(1.0),
                    "sample()'s own pdf ({pdf_sampled}) must match pdf() evaluated \
                     independently at the sampled direction ({pdf_looked_up}) for \
                     u0={u0}, u1={u1}, dir={dir:?}"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 900,
            "sanity: should have checked ~1000 (u0,u1) pairs"
        );
    }
}

/// South/north-pole numerics regression test for `pdf_uv_to_solid_angle`,
/// `uv_to_direction` and `EnvironmentMap::pdf`'s `sin(min(v, 1-v) * PI)` /
/// `x.hypot(z)` reformulations.
///
/// See those functions' own doc comments for the
/// pole-conditioning problem they fix (the WGSL twins in `transport_physics.wgsl` /
/// `transport_bounce.wgsl` / `transport_functions.wgsl` already match; this module
/// checks the CPU side against an independent `f64` reference).
///
/// The reference distribution below is a closed-form re-derivation of
/// [`Distribution2D`]/[`Distribution1D`]'s piecewise-constant density, carried
/// entirely in `f64`: for row-major weighted texel weights `w_i` (the same
/// `luminance(texel) * sin(theta_row_center)` [`build_distribution`] feeds to
/// [`Distribution2D::new`]), the two-stage marginal/conditional construction reduces
/// algebraically to the single closed form `pdf_uv(row, col) = w[row][col] * width *
/// height / sum(w)` (a piecewise-constant density over the unit square is just each
/// bucket's weight over the mean weight; the marginal/conditional split's own
/// normalizations telescope away) -- see `pdf_uv_f64`'s own comment for the algebra.
/// That closed form is exact (no iteration, no `f32`), so it is a trustworthy
/// independent ground truth for [`Distribution2D::pdf`]/[`Distribution1D::pdf`]'s own
/// bucket-lookup formula, which this module's `pole_numerics_tests` doesn't otherwise
/// duplicate.
#[cfg(test)]
mod pole_numerics_tests {
    use super::*;

    const PI64: f64 = std::f64::consts::PI;

    /// A non-uniform synthetic map (distinct pixel data from
    /// `nee_sample_pdf_tests::synthetic_map`, since that helper is private to its own
    /// module) with a texel resolution fine enough to give the poles' bucket rows
    /// (`row == 0` and `row == height - 1`) genuinely different weights from their
    /// neighbours, so this module's pole-focused checks aren't accidentally trivial.
    fn synthetic_map() -> EnvironmentMap {
        let width = 16;
        let height = 8;
        let pixels: Vec<[f32; 3]> = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let u = x as f32 / width as f32;
                    let v = y as f32 / height as f32;
                    [
                        0.9f32.mul_add(u, 0.05),
                        0.6f32.mul_add(v, 0.3),
                        (u * 3.0).sin().mul_add(0.2, 0.4f32.mul_add(v, 0.15)),
                    ]
                })
            })
            .collect();
        EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
    }

    /// `f64` replica of [`luminance`] (Rec.709 relative luminance).
    fn luminance64(rgb: [f32; 3]) -> f64 {
        0.0722f64.mul_add(
            f64::from(rgb[2]),
            0.7152f64.mul_add(f64::from(rgb[1]), 0.2126 * f64::from(rgb[0])),
        )
    }

    /// The `f64` row-major weighted texel array (`luminance64(texel) *
    /// sin(theta_row_center)`, mirroring [`build_distribution`]'s own `f32` weighting
    /// exactly but carried in `f64`) plus its total sum, built directly off `map`'s own
    /// private `pixels`/`width`/`height` fields -- this test module is nested inside
    /// the same module [`EnvironmentMap`] is defined in, so those private fields are
    /// visible here (the "crate-private accessor" this module uses, in lieu of the
    /// `feature = "gpu"`-gated [`Distribution2D::marginal`]/[`Distribution2D::conditional`]
    /// accessors, which this test binary does not have enabled).
    fn weighted_texels_f64(map: &EnvironmentMap) -> (Vec<f64>, f64) {
        let mut weighted = vec![0.0f64; map.width * map.height];
        for y in 0..map.height {
            let theta = (y as f64 + 0.5) / map.height as f64 * PI64;
            let sin_theta = theta.sin().max(0.0);
            for x in 0..map.width {
                weighted[y * map.width + x] =
                    luminance64(map.pixels[y * map.width + x]) * sin_theta;
            }
        }
        let total: f64 = weighted.iter().sum();
        (weighted, total)
    }

    /// The `f64`-exact `pdf_uv` (unit-square measure) [`Distribution2D::pdf`] computes,
    /// at arbitrary continuous `(u, v)`.
    ///
    /// Derivation of the closed form: `Distribution1D::bucket_pdf(i) = func[i] /
    /// mean(func)`. The marginal's own `func` is each row's mean weight
    /// (`mean(row_y) = sum(row_y) / width`), so `marginal.pdf(v) = mean(row_row) /
    /// mean_y(mean(row_y)) = sum(row_row) * height / sum_all`. The conditional row's
    /// `pdf(u) = w[row][col] / mean(row_row) = w[row][col] * width / sum(row_row)`.
    /// `Distribution2D::pdf` multiplies the two, and `sum(row_row)` cancels exactly:
    /// `pdf_uv(row, col) = w[row][col] * width * height / sum_all`. The bucket indices
    /// themselves (`row`/`col`) use the same `floor(x.clamp(0, 0.999_999_94) * n)` rule
    /// [`Distribution1D::pdf`]/[`Distribution2D::pdf`] use, just evaluated in `f64`.
    fn pdf_uv_f64(map: &EnvironmentMap, weighted: &[f64], total: f64, u: f64, v: f64) -> f64 {
        let width = map.width;
        let height = map.height;
        let row = (v.clamp(0.0, 0.999_999_94) * height as f64) as usize;
        let row = row.min(height - 1);
        let col = (u.clamp(0.0, 0.999_999_94) * width as f64) as usize;
        let col = col.min(width - 1);
        weighted[row * width + col] * width as f64 * height as f64 / total
    }

    /// The `f64`-exact solid-angle-measure pdf at continuous `(u, v)`, mirroring
    /// [`pdf_uv_to_solid_angle`]'s Jacobian division (`pdf_uv / (2 * PI^2 *
    /// sin(theta))`) but with `theta = v * PI` and `sin(theta)` both computed directly
    /// in `f64` -- exactly the computation the `f32` production code's `sin(min(v, 1 -
    /// v) * PI)` reflection trick approximates near the poles (see that function's own
    /// doc comment for why the naive `sin(v * PI)` loses precision in `f32` there;
    /// `f64` has ~9 extra decimal digits of headroom, so the naive form needs no
    /// reflection to stay accurate at the `v` values this module's checks use).
    fn pdf_solid_angle_f64(
        map: &EnvironmentMap,
        weighted: &[f64],
        total: f64,
        u: f64,
        v: f64,
    ) -> f64 {
        let theta = v * PI64;
        let sin_theta = theta.sin();
        pdf_uv_f64(map, weighted, total, u, v) / (2.0 * PI64 * PI64 * sin_theta)
    }

    /// (a) [`EnvironmentMap::sample`]'s own returned pdf against the `f64` reference,
    /// for `u1` pushing the marginal (row/`v`) draw from the north pole (`u1` near
    /// `0.0`) to the south pole (`u1` near `1.0`) and through the middle.
    #[test]
    fn sample_pdf_matches_f64_reference_across_the_v_range() {
        let map = synthetic_map();
        let (weighted, total) = weighted_texels_f64(&map);
        for u1 in [0.999_999f32, 0.9999, 0.999, 0.5, 0.001, 1e-6] {
            for u0 in [0.13f32, 0.68] {
                let (u, v, pdf_uv) = map.distribution.sample(u0, u1);
                let (_dir, _rgb, pdf_sampled) = map.sample(u0, u1);
                // Sanity: `map.sample`'s internal `distribution.sample` call is a pure
                // function of `(u0, u1)`, so calling it again here must reproduce the
                // exact same `(u, v)` this assertion's reference is built from.
                let pdf_uv_recomputed = map.distribution.pdf(u, v);
                assert!(
                    (pdf_uv - pdf_uv_recomputed).abs() < 1e-4 * pdf_uv.max(1.0),
                    "test premise: distribution.sample/pdf must agree at (u={u}, v={v})"
                );

                let reference =
                    pdf_solid_angle_f64(&map, &weighted, total, f64::from(u), f64::from(v));
                let rel_err = (f64::from(pdf_sampled) - reference).abs() / reference;
                assert!(
                    rel_err < 1e-5,
                    "u0={u0}, u1={u1}: sample()'s pdf ({pdf_sampled}) must match the f64 \
                     reference ({reference}) to within 1e-5 relative (got {rel_err:e}), \
                     u={u}, v={v}"
                );
            }
        }
    }

    /// (b) [`EnvironmentMap::pdf`] at directions built directly from an exact `(theta,
    /// phi)` a known angular distance off each pole -- `phi` deliberately not a
    /// multiple of `PI/8` so these directions never land exactly on a texel-column
    /// edge, which would make the reference's bucket lookup degenerate/ambiguous.
    #[test]
    fn pdf_at_direction_matches_f64_reference_near_both_poles() {
        let map = synthetic_map();
        let (weighted, total) = weighted_texels_f64(&map);
        let phis_deg = [10.0f64, 61.0, 137.0];
        let angles_deg = [0.05f64, 0.5, 5.0];
        for &angle_deg in &angles_deg {
            for &phi_deg in &phis_deg {
                for north_pole in [true, false] {
                    let angle_rad = angle_deg.to_radians();
                    let theta = if north_pole {
                        angle_rad
                    } else {
                        PI64 - angle_rad
                    };
                    let phi = phi_deg.to_radians();
                    let (sin_theta, cos_theta) = theta.sin_cos();
                    let (sin_phi, cos_phi) = phi.sin_cos();
                    let dir_f64 = (sin_theta * sin_phi, cos_theta, sin_theta * cos_phi);
                    let dir_f32 = Vec3::new(dir_f64.0 as f32, dir_f64.1 as f32, dir_f64.2 as f32);

                    let u_exact = (phi / (2.0 * PI64)).rem_euclid(1.0);
                    let v_exact = theta / PI64;
                    let reference = pdf_solid_angle_f64(&map, &weighted, total, u_exact, v_exact);

                    let actual = map.pdf(dir_f32);
                    let rel_err = (f64::from(actual) - reference).abs() / reference;
                    assert!(
                        rel_err < 1e-5,
                        "angle_deg={angle_deg} off the {} pole, phi_deg={phi_deg}: \
                         pdf() ({actual}) must match the f64 reference ({reference}) to \
                         within 1e-5 relative (got {rel_err:e}), dir={dir_f32:?}",
                        if north_pole { "north" } else { "south" }
                    );
                }
            }
        }
    }

    /// (c) [`EnvironmentMap::sample`]/[`EnvironmentMap::pdf`] self-consistency
    /// (production-vs-production, no `f64` reference needed) specifically for `(u0,
    /// u1)` pairs that push the sampled direction to within a texel or so of either
    /// pole -- the general `nee_sample_pdf_tests` check sweeps `u1 in [0, 1)` on a
    /// coarse 29-point grid, which does not specifically probe `u1` within `1e-6` of
    /// either end.
    #[test]
    fn sample_and_pdf_agree_at_sampled_direction_within_texel_of_either_pole() {
        let map = synthetic_map();
        for u1 in [1e-7f32, 1e-6, 1e-5, 1.0 - 1e-6, 1.0 - 1e-7] {
            for u0 in [0.21f32, 0.77] {
                let (dir, _rgb, pdf_sampled) = map.sample(u0, u1);
                let pdf_looked_up = map.pdf(dir);
                let rel_err =
                    (pdf_sampled - pdf_looked_up).abs() / pdf_sampled.max(pdf_looked_up).max(1e-8);
                assert!(
                    rel_err < 1e-4,
                    "u0={u0}, u1={u1}: sample()'s pdf ({pdf_sampled}) and pdf() at the \
                     sampled direction ({pdf_looked_up}) must agree to within 1e-4 \
                     relative near the poles (got {rel_err:e}), dir={dir:?}"
                );
            }
        }
    }
}
