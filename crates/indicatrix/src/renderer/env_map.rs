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
//! extension as of finding G7:
//! `optics::raytracer::environment::{sample_environment_for_nee, environment_nee_pdf}`
//! wrap them for `optics::raytracer::scattering`'s Henyey-Greenstein NEE (a frosted-facet
//! transmission is not yet wired up -- see `scattering::NeeContext`'s doc comment for the
//! exact remaining work), which draws a light-sampling direction/pdf and evaluates an
//! arbitrary direction's density for a Veach-style balance-heuristic MIS weight,
//! respectively.

use std::f32::consts::PI;

use glam::Vec3;

// Live alongside `env_map.rs` in `renderer/`, not in an `env_map/` subdirectory, so the
// path is spelled out explicitly.
#[path = "env_map_distribution.rs"]
mod env_map_distribution;
#[path = "env_map_spectrum.rs"]
mod env_map_spectrum;

// `Distribution1D` re-exported alongside `Distribution2D` (not just used privately here)
// so `renderer::env_map_gpu::HdrEnvGpuData::upload` (finding G7) can name the type it
// gets back from `Distribution2D::marginal`/`conditional` when flattening it into GPU
// buffers -- see that module's own doc comment. `env_map_distribution` itself stays a
// private submodule nested here (not a sibling `renderer::env_map_distribution`) so
// there is exactly one module path for these types; `env_map_gpu.rs` reaches them via
// this re-export, `crate::renderer::env_map::{Distribution1D, Distribution2D}`.
pub(crate) use env_map_distribution::{Distribution1D, Distribution2D};
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
    /// `renderer::env_map_gpu::HdrEnvGpuData::upload` (finding G6) can build the
    /// `vec4<f32>`-padded storage buffer `spectral_transport.wgsl`'s `hdr_texels`
    /// binding reads, without duplicating this type's own row-major layout convention.
    #[must_use]
    pub(crate) fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }

    /// The importance-sampling [`Distribution2D`] backing [`Self::sample`]/[`Self::pdf`].
    ///
    /// Exposed, like [`Self::pixels`], so `renderer::env_map_gpu::HdrEnvGpuData::upload`
    /// (finding G7's GPU NEE port) can flatten its marginal/conditional cdf and function
    /// arrays into the storage buffers `spectral_transport.wgsl`'s `dist1d_find_bucket`/
    /// `dist2d_sample`/`dist2d_pdf` binary-search, without duplicating this type's own
    /// row-major layout convention. `renderer::gpu::transport_check`'s Tier 2 self-test
    /// uses the same accessor to upload an independent synthetic map.
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
        let (sin_theta, cos_theta) = theta.sin_cos();
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
        pdf_uv_to_solid_angle(pdf_uv, v)
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
fn pdf_uv_to_solid_angle(pdf_uv: f32, v: f32) -> f32 {
    let theta = v * PI;
    let sin_theta = theta.sin();
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
