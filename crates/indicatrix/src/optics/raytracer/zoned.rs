//! Zoned (colour-zoned) absorption in the CPU tracer: `zoning` feature only.
//!
//! A material with `GemMaterial::zoning == Some(..)` replaces the homogeneous Beer-Lambert
//! term `alpha(lambda) * path` of every interior segment by `sum_z alpha_z(lambda) * len_z`,
//! with `len_z` the length of the segment inside zone `z` ([`crate::optics::zoning`]). Refraction
//! geometry is untouched (zones share the refractive index).
//!
//! # Frame and unit chain
//!
//! 1. The tracer works in MODEL units, with the stone in its model frame.
//! 2. Zoned materials are [`AbsorptionUnit::PerMm`]: `GemMaterial::absorption_path_scale` is
//!    already "millimetres per model unit" (`render_setup::absorption_path_scale_for`,
//!    `stone_width_mm / model_width`). The segment end points are multiplied by it to give
//!    STONE MILLIMETRES (origin = the model origin).
//! 3. `ZonedAbsorption::frame` maps zone-local coordinates into that stone-mm frame; the zone
//!    kernels apply its inverse themselves. So model units -> (x `absorption_path_scale`) stone mm
//!    -> (`frame` inverse, inside the kernel) zone frame.
//! 4. The kernel's lengths are used as FRACTIONS of the segment: `len_z = L * raw_z / sum(raw)`
//!    with `L = (t1 - t0) * absorption_path_scale`, the same millimetre length the homogeneous
//!    code uses. A segment inside one zone therefore gets exactly `1.0 * L` (bit for bit the
//!    unzoned product), and the f32 kernel's rounding can never create or lose path.
//!
//! `GemMaterial::absorption` is IGNORED for a zoned material: the base zone
//! (`ZonedAbsorption::base`) takes its place. Keep them equal when a default build must show the
//! same stone (the adopt path stores the base zone as the ordinary material).
//!
//! # Pleochroism
//!
//! Each zone's tensor is evaluated exactly as `absorption::channel_absorption_alphas_assigned`
//! evaluates the material's own: the assigned eigenmode's E-field direction through
//! `assigned_mode_alpha` on an anisotropic host, the midpoint of the two eigen directions'
//! quadratic forms on an isotropic-by-symmetry one (`ZoneAbsorption::alpha_midpoint` is the same
//! rule). Never the orientation mean, which is what `ZoneAbsorption::alpha(.., None)` returns.
//!
//! # Fluorescence
//!
//! Emitters are not supported on a zoned material (the fluorescence vertex sampler assumes a
//! homogeneous medium): `trace_spectral_ray_core` asserts that no zoned trace is fluorescent.

use super::{
    NUM_CHANNELS,
    absorption::spectral_absorption,
    camera::Ray,
    refraction::{RayMaterialContext, RayWavelengthCache},
};
use crate::optics::{
    birefringence::{
        AbsorptionTensor3, BirefringenceParams, assigned_mode_alpha, assigned_mode_e_field_uniaxial,
    },
    materials::AbsorptionUnit,
    zoning::{MAX_ZONES, ZoneAbsorption, ZoneKernel, ZoneKernelF32},
};
use glam::Vec3;

/// Zone slots in every per-zone array: the base zone, then up to [`MAX_ZONES`] shaped zones.
pub const ZONE_SLOTS: usize = MAX_ZONES + 1;

/// Per-zone, per-channel absorption coefficients (absorption-length units, i.e. per mm).
pub type ZoneAlphas = [[f32; NUM_CHANNELS]; ZONE_SLOTS];

/// Per-zone path lengths of one segment in absorption-length units (mm).
pub type ZoneLengthsMm = [f32; ZONE_SLOTS];

/// The path-length kernel a trace uses: the allocation-free f32 twin, or the f64 reference when
/// the zones contain a mesh shell (which only the f64 kernel supports).
enum ZoneKernels {
    Single(Box<ZoneKernelF32>),
    Double(Box<ZoneKernel>),
}

/// Everything the zoned code needs, built once per sample next to [`RayWavelengthCache`]'s
/// other per-wavelength tables: the kernel, and each zone's per-channel [`AbsorptionTensor3`]
/// (the base zone first), at the sample's wavelength comb.
pub struct ZonedCache {
    kernels: ZoneKernels,
    /// `tensors[z][k]`: zone `z`'s tensor at channel `k`'s wavelength. Zone 0 is the base.
    tensors: Vec<[AbsorptionTensor3; NUM_CHANNELS]>,
}

/// One zone's per-channel tensors, built like `build_ray_wavelength_cache` builds the
/// material's own (`beta` only on a biaxial host).
fn channel_tensors(
    zone: &ZoneAbsorption,
    lambdas: &[f32; NUM_CHANNELS],
    c_axis: Vec3,
    is_biaxial: bool,
) -> [AbsorptionTensor3; NUM_CHANNELS] {
    let t = &zone.tensor;
    std::array::from_fn(|k| {
        let alpha_o = spectral_absorption(&t.o_ray, lambdas[k]);
        let alpha_e = spectral_absorption(&t.e_ray, lambdas[k]);
        let alpha_beta = if is_biaxial {
            t.beta_ray
                .as_deref()
                .map(|bands| spectral_absorption(bands, lambdas[k]))
        } else {
            None
        };
        alpha_beta.map_or_else(
            || AbsorptionTensor3::uniaxial(alpha_o, alpha_e, c_axis),
            |beta| AbsorptionTensor3::biaxial(alpha_o, beta, alpha_e, c_axis),
        )
    })
}

/// Builds the [`ZonedCache`] of a sample, or `None` for a material without zones.
///
/// The zones are assumed valid (`ZonedAbsorption::validate`, checked by whoever installs them);
/// a debug build asserts it.
pub(super) fn build_zoned_cache(ctx: &RayMaterialContext, is_biaxial: bool) -> Option<ZonedCache> {
    let zoned = ctx.material.zoning.as_ref()?;
    debug_assert!(
        ctx.material.absorption_unit == AbsorptionUnit::PerMm,
        "a zoned material must be PerMm (its zones are in millimetres)"
    );
    debug_assert!(
        zoned.validate().is_ok(),
        "invalid zoning installed on a material: {:?}",
        zoned.validate()
    );
    let kernels = ZoneKernelF32::new(zoned).map_or_else(
        || ZoneKernels::Double(Box::new(ZoneKernel::new(zoned))),
        |kernel| ZoneKernels::Single(Box::new(kernel)),
    );
    let tensors = (0..=zoned.zones.len().min(MAX_ZONES))
        .filter_map(|i| zoned.zone_absorption(i))
        .map(|zone| channel_tensors(zone, &ctx.lambdas, ctx.c_axis, is_biaxial))
        .collect();
    Some(ZonedCache { kernels, tensors })
}

impl ZonedCache {
    /// Each zone's per-channel absorption coefficient for a ray of wave normal `k_hat`, assigned
    /// to eigenmode `is_extraordinary`. Unused zone slots are zero.
    ///
    /// The mode selection is a twin of `absorption::channel_absorption_alphas_assigned`
    /// (kept next to it by name); only the tensor differs per zone.
    pub(crate) fn zone_alphas(
        &self,
        ctx: &RayMaterialContext,
        cache: &RayWavelengthCache,
        k_hat: Vec3,
        is_extraordinary: bool,
    ) -> ZoneAlphas {
        let c_axis = ctx.c_axis;
        let mut out = [[0.0f32; NUM_CHANNELS]; ZONE_SLOTS];
        if !ctx.is_anisotropic {
            let (eigen_a, eigen_b) = cache.hero_indicatrix.map_or_else(
                || {
                    (
                        BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis),
                        BirefringenceParams::extraordinary_eigen_polarization(k_hat, c_axis),
                    )
                },
                |ind| ind.eigen_polarizations(k_hat),
            );
            for (slot, tensors) in out.iter_mut().zip(&self.tensors) {
                for (alpha, tensor) in slot.iter_mut().zip(tensors) {
                    *alpha = f32::midpoint(
                        tensor.quadratic_form(eigen_a),
                        tensor.quadratic_form(eigen_b),
                    );
                }
            }
            return out;
        }
        let e_mode_hat = cache.hero_indicatrix.map_or_else(
            || {
                let n_o_hero = cache.n_o_ch[ctx.hero_idx];
                let n_e_hero = ctx
                    .material
                    .extraordinary_index_at(ctx.lambdas[ctx.hero_idx], n_o_hero);
                assigned_mode_e_field_uniaxial(k_hat, c_axis, is_extraordinary, n_o_hero, n_e_hero)
            },
            |ind| ind.assigned_mode_e_field(k_hat, is_extraordinary),
        );
        for (slot, tensors) in out.iter_mut().zip(&self.tensors) {
            for (alpha, tensor) in slot.iter_mut().zip(tensors) {
                *alpha = assigned_mode_alpha(tensor, e_mode_hat);
            }
        }
        out
    }

    /// Per-zone path length (absorption-length units, mm) of the part of the ray
    /// `origin + dir * t` with `t` in `[t0, t1]` (model units), see the module notes for the
    /// frame chain and why the kernel's lengths are used as fractions.
    pub(crate) fn lengths_mm(
        &self,
        origin: Vec3,
        dir: Vec3,
        t0: f32,
        t1: f32,
        path_scale: f32,
    ) -> ZoneLengthsMm {
        let mut out = [0.0f32; ZONE_SLOTS];
        let scaled = (t1 - t0) * path_scale;
        if scaled.is_nan() || scaled <= 0.0 {
            return out;
        }
        let from = (origin + dir * t0) * path_scale;
        let to = (origin + dir * t1) * path_scale;
        let raw: [f32; ZONE_SLOTS] = match &self.kernels {
            ZoneKernels::Single(kernel) => kernel.lengths(from, to),
            ZoneKernels::Double(kernel) => {
                let lengths = kernel.lengths(from.as_dvec3(), to.as_dvec3());
                lengths.map(|l| l as f32)
            }
        };
        let total: f32 = raw.iter().sum();
        if total.is_nan() || total <= 0.0 {
            // A degenerate kernel answer: charge the base zone rather than lose the path.
            out[0] = scaled;
            return out;
        }
        for (o, r) in out.iter_mut().zip(raw) {
            *o = scaled * (r / total);
        }
        out
    }
}

/// `sum_z (alpha_z[k] + sigma_s) * lengths[z]` per channel: the optical depth of a segment in
/// absorption-length units. `sigma_s = 0.0` is the plain absorption depth (adding `+0.0` is an
/// exact no-op), a scattering medium passes its achromatic extinction share.
pub fn optical_depths(
    alphas: &ZoneAlphas,
    lengths: &ZoneLengthsMm,
    sigma_s: f32,
) -> [f32; NUM_CHANNELS] {
    let mut depth = [0.0f32; NUM_CHANNELS];
    for (zone_alphas, &len) in alphas.iter().zip(lengths) {
        for (d, &alpha) in depth.iter_mut().zip(zone_alphas) {
            *d = (alpha + sigma_s).mul_add(len, *d);
        }
    }
    depth
}

/// The per-channel optical depth of one straight interior segment (the ray from `ray.origin`
/// along `ray.dir`, `path_len` model units long), or `None` when the material has no zones, in
/// which case the caller keeps the homogeneous path.
///
/// This is the single zoned replacement for the homogeneous `alpha * scaled_path_len` of
/// `absorption::apply_segment_absorption`.
pub(super) fn segment_optical_depths(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    k_hat: Vec3,
    is_extraordinary: bool,
    ray: Ray,
    path_len: f32,
) -> Option<[f32; NUM_CHANNELS]> {
    let zoned = cache.zoned.as_ref()?;
    let alphas = zoned.zone_alphas(ctx, cache, k_hat, is_extraordinary);
    let lengths = zoned.lengths_mm(
        ray.origin,
        ray.dir,
        0.0,
        path_len,
        ctx.material.absorption_path_scale,
    );
    Some(optical_depths(&alphas, &lengths, 0.0))
}

/// The local extinction `sigma_t[k] = alpha_z(x)[k] + sigma_s` at the point `t` along the ray,
/// read off a short interval centred on it (a zone boundary inside the interval blends the two
/// zones; that has measure zero for a sampled `t`). `hit_t` bounds the interval.
pub fn local_extinction(
    zoned: &ZonedCache,
    alphas: &ZoneAlphas,
    ray: Ray,
    t: f32,
    hit_t: f32,
    path_scale: f32,
    sigma_s: f32,
) -> [f32; NUM_CHANNELS] {
    let delta = (1e-4 * hit_t).max(1e-7);
    let t_a = (t - delta).max(0.0);
    let t_b = (t + delta).min(hit_t);
    let lengths = zoned.lengths_mm(ray.origin, ray.dir, t_a, t_b, path_scale);
    let total: f32 = lengths.iter().sum();
    let depth = optical_depths(alphas, &lengths, sigma_s);
    if total > 0.0 {
        depth.map(|d| d / total)
    } else {
        // Zero-length interval (t == hit_t == 0): the base zone's extinction.
        std::array::from_fn(|k| alphas[0][k] + sigma_s)
    }
}
