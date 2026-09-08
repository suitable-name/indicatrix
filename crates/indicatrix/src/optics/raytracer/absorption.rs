//! Pleochroic Beer-Lambert absorption and Stokes/Mueller frame rotation.
//!
//! [`apply_absorption`]'s deterministic per-bounce attenuation, the shared
//! [`channel_absorption_alphas_assigned`] it and
//! [`super::scattering::maybe_scatter_or_extinguish`] both build on, and
//! [`signed_frame_rotation_psi`]'s signed rotation angle recovery.

use super::{
    NUM_CHANNELS,
    refraction::{RayMaterialContext, RayWavelengthCache},
};
use crate::optics::{
    absorption::AbsorptionBand,
    birefringence::{BirefringenceParams, assigned_mode_alpha, assigned_mode_e_field_uniaxial},
    polarization::{MuellerMatrix, StokesVector},
};
use glam::Vec3;

// Only the OLD Stokes-DOP-based `channel_absorption_alphas` (kept for this file's own
// regression tests and `raytracer::scattering`'s tests -- see that function's own doc
// comment) still needs these.
#[cfg(test)]
use crate::optics::{
    birefringence::effective_pleochroic_alpha, polarization::electric_field_direction,
};

/// Evaluates a material's absorption coefficient at `lambda_nm` as the sum of its
/// individual chromophore [`AbsorptionBand`](crate::optics::absorption::AbsorptionBand)s.
/// An empty band slice sums to `0.0` (colourless material).
#[must_use]
pub fn spectral_absorption(bands: &[AbsorptionBand], lambda_nm: f32) -> f32 {
    bands.iter().map(|band| band.evaluate(lambda_nm)).sum()
}

/// The SIGNED azimuth needed to rotate the Stokes reference frame from `prev_normal`
/// (previous bounce's plane-of-incidence normal) to `current_normal` (this bounce's),
/// about propagation `axis`. Plain `acos` alone always returns a value in [0, pi] and
/// discards which way the frame actually turned; recovering the sign via
/// `atan2(sin_psi, cos_psi)`, with `sin_psi` read off the component of
/// `prev_normal x current_normal` along `axis`, matches the true rotation instead of
/// mixing Q and U with an arbitrary sign on roughly half of all bounces. The call site
/// in `trace_spectral_ray` guards near-zero-length normals (an undefined plane of
/// incidence, e.g. at normal incidence) before calling this.
#[inline]
pub(crate) fn signed_frame_rotation_psi(
    prev_normal: Vec3,
    current_normal: Vec3,
    axis: Vec3,
) -> f32 {
    let cos_psi = prev_normal.dot(current_normal).clamp(-1.0, 1.0);
    let sin_psi = prev_normal
        .cross(current_normal)
        .dot(axis.normalize_or_zero());
    sin_psi.atan2(cos_psi)
}

/// Rotates `stokes` into this bounce's plane-of-incidence frame via
/// [`signed_frame_rotation_psi`] (see its doc comment for why the rotation angle must
/// be signed). A no-op on `stokes` when there is no previous well-defined plane, or
/// either plane's cross product degenerates (near-normal incidence). Returns this
/// bounce's plane-of-incidence normal, for the caller to store as `prev_plane_normal`
/// for the NEXT bounce.
///
/// `k_hat` is the WAVE NORMAL `k`, not the Poynting direction `S` -- the plane of
/// incidence (and the Fresnel/TIR physics that plane feeds) is a property of `k`, not
/// `S`. See `refraction`'s own design note.
pub(super) fn rotate_stokes_to_plane_of_incidence(
    k_hat: Vec3,
    normal: Vec3,
    prev_plane_normal: Option<Vec3>,
    stokes: &mut [StokesVector; NUM_CHANNELS],
) -> Vec3 {
    let current_plane_normal = k_hat.cross(normal).normalize_or_zero();
    if let Some(prev_normal) = prev_plane_normal
        && current_plane_normal.length_squared() > 1e-6
        && prev_normal.length_squared() > 1e-6
    {
        let psi = signed_frame_rotation_psi(prev_normal, current_plane_normal, k_hat);
        let rot_matrix = MuellerMatrix::frame_rotation(psi);
        for s in stokes.iter_mut() {
            *s = s.apply_matrix(&rot_matrix);
        }
    }
    current_plane_normal
}

/// Directional Pleochroic Beer-Lambert absorption via the polarization quadratic form
/// `alpha = e_mode_hat . A . e_mode_hat` -- applied to every channel's Stokes vector for
/// one internal bounce's path length. `e_mode_hat` is the path's ASSIGNED eigenmode's own
/// geometric E-field direction (see [`channel_absorption_alphas_assigned`]'s doc
/// comment), shared across channels since it depends only on geometry, not wavelength.
/// Called only when `inside_gem` -- see the call site.
///
/// `is_extraordinary` names which eigenmode this path was assigned to at its most recent
/// air->crystal entry (uniaxial: ordinary/extraordinary; biaxial: mode A/mode B) -- see
/// `trace_spectral_ray_inner`'s own `is_extraordinary` doc comment.
pub(super) fn apply_absorption(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    k_hat: Vec3,
    is_extraordinary: bool,
    path_len: f32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
) {
    let alphas = channel_absorption_alphas_assigned(ctx, cache, k_hat, is_extraordinary);
    // Model units -> absorption-length units. See `GemMaterial::absorption_path_scale`'s
    // doc comment. `absorption_path_scale == 1.0` (every built-in) makes this multiply
    // an exact IEEE 754 no-op.
    let scaled_path_len = path_len * ctx.material.absorption_path_scale;
    // Vectorized Beer-Lambert; exp_f32x8 is a few-ULP polynomial exponential, not
    // bit-identical to f32::exp -- see src/simd.rs module docs.
    let mut args = [0f32; NUM_CHANNELS];
    for (a, alpha) in args.iter_mut().zip(&alphas) {
        *a = -alpha * scaled_path_len;
    }
    let trans = crate::simd::exp_f32x8(args);
    for (k, s) in stokes.iter_mut().enumerate() {
        *s = s.scale(trans[k]);
    }
}

/// The per-channel ASSIGNED-MODE pleochroic absorption coefficient (`alpha_eff`) for
/// every spectral channel, evaluated once and shared by two callers: [`apply_absorption`]
/// itself (the deterministic Beer-Lambert path) and
/// [`super::scattering::maybe_scatter_or_extinguish`], which needs the SAME per-channel
/// `sigma_a` to build `sigma_t = sigma_a + sigma_s` (see that function's doc comment for
/// hazard 1: "Beer-Lambert becomes extinction, not absorption").
///
/// Replaces the degree-of-polarization heuristic [`channel_absorption_alphas`] used to
/// apply here -- see [`crate::optics::birefringence::assigned_mode_alpha`]'s doc comment
/// for why. This function takes NO Stokes vector at all: the assigned mode's
/// polarization is exact geometry, not something read off the light's (possibly
/// importance-carrying, possibly azimuth-drifted) Stokes state.
///
/// # Isotropic-by-symmetry materials
///
/// `!ctx.is_anisotropic` (a cubic material, or any material whose `birefringence_delta`
/// is negligible) has no meaningful eigenmode assignment -- `is_extraordinary` is unused
/// while `!inside_gem`-adjacent code never sets it meaningfully for such a material. This
/// branch instead reproduces the OLD unpolarized-average behaviour directly:
/// `midpoint(quadratic_form(eigen_a), quadratic_form(eigen_b))`, i.e. exactly what
/// [`crate::optics::birefringence::effective_pleochroic_alpha`] computes at
/// `degree_of_polarization == 0.0` -- the two eigenmode directions `eigen_a`/`eigen_b`
/// here are themselves geometry-only (never derived from Stokes), so this is not a
/// Stokes-independence violation, just a reuse of the same unpolarized-limit formula. For
/// every built-in isotropic material `alpha_o == alpha_e` (the tensor is genuinely
/// isotropic), so this equals `alpha_o` regardless of which eigen directions are fed in --
/// a hypothetical isotropic-but-DICHROIC material (not represented by any built-in) would
/// need a genuine diattenuator Mueller matrix acting on the full Stokes vector to handle
/// correctly, which this function does not implement.
///
/// `pub(crate)`, not private: `renderer::gpu::transport_check`'s GPU self-test for
/// [`super::scattering::maybe_scatter_or_extinguish`] needs to compute the SAME real
/// per-channel alphas this function produces to feed the standalone WGSL kernel's
/// explicit `alphas` input -- calling this REAL function, never a reimplementation.
///
/// `k_hat` is the WAVE NORMAL `k`, not the Poynting direction `S` -- the eigen-
/// polarizations (transverse to the wave that's actually propagating) are properties of
/// `k`. See `refraction`'s own design note, rule 6.
pub(crate) fn channel_absorption_alphas_assigned(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    k_hat: Vec3,
    is_extraordinary: bool,
) -> [f32; NUM_CHANNELS] {
    let c_axis = ctx.c_axis;
    let mut alphas = [0.0f32; NUM_CHANNELS];

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
        for (k, alpha_slot) in alphas.iter_mut().enumerate() {
            *alpha_slot = f32::midpoint(
                cache.tensor_ch[k].quadratic_form(eigen_a),
                cache.tensor_ch[k].quadratic_form(eigen_b),
            );
        }
        return alphas;
    }

    // Bit-identical to a fresh `material.biaxial_indicatrix(ctx.lambdas[ctx.hero_idx])`
    // call -- see `RayWavelengthCache::hero_indicatrix`'s doc comment.
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
    for (k, alpha_slot) in alphas.iter_mut().enumerate() {
        // Against the cached tensor rather than rebuilding a fresh `AbsorptionTensor3`
        // every bounce -- see `RayWavelengthCache::tensor_ch`'s doc comment.
        *alpha_slot = assigned_mode_alpha(&cache.tensor_ch[k], e_mode_hat);
    }
    alphas
}

/// The OLD per-channel pleochroic absorption coefficient: a degree-of-polarization blend
/// between the light's own Stokes-derived electric-field direction and the two
/// eigenmodes' unpolarized average. No longer called by the interior transport path (see
/// [`channel_absorption_alphas_assigned`], which replaced it there) -- kept ONLY for this
/// file's own regression tests (pinning the new function's isotropic branch and Stokes-
/// independence against this OLD Stokes-dependent one) and `raytracer::scattering`'s own
/// tests (a convenient unpolarized-average probe for `maybe_scatter_or_extinguish`'s
/// weight-formula tests). The Tier 2 GPU harness exercises `birefringence::
/// pleochroic_channel_alpha` directly (built on `effective_pleochroic_alpha`, the same
/// combination this function performs per channel) against its WGSL translation, never
/// this function -- hence `#[cfg(test)]` here, unlike `henyey_greenstein_phase`'s
/// `any(test, feature = "gpu")` gate in `raytracer::mod`: nothing in a non-test
/// `gpu`-feature build calls this any more.
#[cfg(test)]
pub(crate) fn channel_absorption_alphas(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    stokes: &[StokesVector; NUM_CHANNELS],
) -> [f32; NUM_CHANNELS] {
    let c_axis = ctx.c_axis;
    // Bit-identical to a fresh `material.biaxial_indicatrix(ctx.lambdas[ctx.hero_idx])`
    // call -- see `RayWavelengthCache::hero_indicatrix`'s doc comment.
    let indicatrix = cache.hero_indicatrix;
    let (eigen_a, eigen_b) = indicatrix.map_or_else(
        || {
            (
                BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis),
                BirefringenceParams::extraordinary_eigen_polarization(k_hat, c_axis),
            )
        },
        |ind| ind.eigen_polarizations(k_hat),
    );

    let mut alphas = [0.0f32; NUM_CHANNELS];
    for (k, alpha_slot) in alphas.iter_mut().enumerate() {
        // Calls `effective_pleochroic_alpha` against the cached tensor rather than
        // rebuilding a fresh `AbsorptionTensor3` every bounce -- see
        // `RayWavelengthCache::tensor_ch`'s doc comment.
        let e_hat = electric_field_direction(&stokes[k], current_plane_normal, k_hat);
        *alpha_slot = effective_pleochroic_alpha(
            &cache.tensor_ch[k],
            e_hat,
            eigen_a,
            eigen_b,
            stokes[k].degree_of_polarization(),
        );
    }
    alphas
}

/// A Ruby-like slab at `absorption_path_scale = 2.0` must attenuate EXACTLY like a slab
/// of twice the model-unit thickness at `absorption_path_scale = 1.0` -- the whole
/// physical point of the field. Proven algebraically: `(2.0 * d) * 1.0` and `d * 2.0`
/// are the same IEEE 754 value (multiplication by exactly-representable `2.0` is
/// exact), so the two results must come out bit-for-bit identical.
#[cfg(test)]
mod absorption_path_scale_tests {
    use super::*;
    use crate::optics::{
        materials::GemMaterial, polarization::StokesVector,
        raytracer::refraction::build_ray_wavelength_cache,
    };

    #[test]
    fn scaled_slab_attenuates_bit_identically_to_doubled_thickness_slab() {
        let ruby_like =
            GemMaterial::new_custom("ruby-like slab probe", 1.76, 0.0, 0.0, [0.5, 1.0, 1.5]);
        assert_eq!(
            ruby_like.crystal_system,
            crate::optics::materials::CrystalSystem::Cubic,
            "test premise: birefringence_delta=0.0 must yield an isotropic material \
             (no birefringence machinery to complicate the comparison)"
        );

        let d = 0.37f32;
        let material_scale1_thickness2d = ruby_like.clone(); // absorption_path_scale == 1.0
        let material_scale2_thicknessd = ruby_like.with_absorption_path_scale(2.0);

        let lambdas: [f32; NUM_CHANNELS] = [420.0, 460.0, 500.0, 540.0, 580.0, 620.0, 660.0, 700.0];
        let ray_dir = Vec3::new(0.0, -1.0, 0.0);
        let stokes_in = [StokesVector::unpolarized(1.0); NUM_CHANNELS];

        let ctx_a = RayMaterialContext {
            material: &material_scale1_thickness2d,
            lambdas,
            hero_idx: 0,
            c_axis: Vec3::Y,
            is_anisotropic: false,
            enable_internal_mode_coupling: true,
        };
        let ctx_b = RayMaterialContext {
            material: &material_scale2_thicknessd,
            lambdas,
            hero_idx: 0,
            c_axis: Vec3::Y,
            is_anisotropic: false,
            enable_internal_mode_coupling: true,
        };
        let cache_a = build_ray_wavelength_cache(&ctx_a);
        let cache_b = build_ray_wavelength_cache(&ctx_b);

        let mut stokes_a = stokes_in;
        let mut stokes_b = stokes_in;
        apply_absorption(
            &ctx_a,
            &cache_a,
            ray_dir,
            false,   // is_extraordinary: irrelevant here (is_anisotropic == false)
            2.0 * d, // scale=1.0: full doubled thickness in model units
            &mut stokes_a,
        );
        apply_absorption(
            &ctx_b,
            &cache_b,
            ray_dir,
            false, // is_extraordinary: irrelevant here (is_anisotropic == false)
            d,     // scale=2.0: half the model-unit thickness
            &mut stokes_b,
        );

        for k in 0..NUM_CHANNELS {
            let a = (stokes_a[k].i, stokes_a[k].q, stokes_a[k].u, stokes_a[k].v);
            let b = (stokes_b[k].i, stokes_b[k].q, stokes_b[k].u, stokes_b[k].v);
            assert_eq!(
                a, b,
                "channel {k}: scale=1.0/thickness=2d must attenuate bit-identically to \
                 scale=2.0/thickness=d (got {a:?} vs {b:?})"
            );
        }
    }
}

/// [`channel_absorption_alphas_assigned`] (assigned-mode absorption) against the OLD
/// DOP-blended [`channel_absorption_alphas`].
#[cfg(test)]
mod assigned_mode_alpha_tests {
    use super::*;
    use crate::optics::{
        materials::{CrystalSystem, GemMaterial},
        raytracer::{refraction::build_ray_wavelength_cache, spectral_absorption},
    };

    /// (i) For an isotropic material, the new function's alphas must equal the OLD
    /// function's own output BIT-FOR-BIT, when the old function is fed a fully
    /// unpolarized Stokes vector (`degree_of_polarization == 0.0`) -- at `p == 0.0`,
    /// `effective_pleochroic_alpha`'s `p.mul_add(alpha_polarized - alpha_unpolarized,
    /// alpha_unpolarized)` collapses to EXACTLY `alpha_unpolarized` (multiplying by
    /// exactly-representable `0.0` is exact IEEE 754, and `0.0 * x + y == y` for any
    /// finite `x`), the same `midpoint(quadratic_form(eigen_a), quadratic_form(eigen_b))`
    /// expression the new function's isotropic branch computes directly.
    #[test]
    fn isotropic_material_assigned_alphas_match_old_unpolarized_output_bit_for_bit() {
        let material = GemMaterial::new_custom("isotropic probe", 1.62, 0.01, 0.0, [0.3, 0.6, 0.9]);
        assert_eq!(
            material.crystal_system,
            CrystalSystem::Cubic,
            "test premise: birefringence_delta=0.0 must yield an isotropic material"
        );
        let lambdas: [f32; NUM_CHANNELS] = [420.0, 460.0, 500.0, 540.0, 580.0, 620.0, 660.0, 700.0];
        let ctx = RayMaterialContext {
            material: &material,
            lambdas,
            hero_idx: 0,
            c_axis: Vec3::Y,
            is_anisotropic: false,
            enable_internal_mode_coupling: true,
        };
        let cache = build_ray_wavelength_cache(&ctx);
        let k_hat = Vec3::new(0.3, -0.9, 0.2).normalize();
        let stokes_unpolarized = [StokesVector::unpolarized(1.0); NUM_CHANNELS];

        let old_alphas =
            channel_absorption_alphas(&ctx, &cache, Vec3::ZERO, k_hat, &stokes_unpolarized);
        for is_extraordinary in [false, true] {
            let new_alphas =
                channel_absorption_alphas_assigned(&ctx, &cache, k_hat, is_extraordinary);
            for k in 0..NUM_CHANNELS {
                assert_eq!(
                    old_alphas[k].to_bits(),
                    new_alphas[k].to_bits(),
                    "channel {k} (is_extraordinary={is_extraordinary}): isotropic new alpha \
                     must equal the old unpolarized-DOP output bit-for-bit (old={}, new={})",
                    old_alphas[k],
                    new_alphas[k]
                );
            }
        }
    }

    /// (ii) Real Tourmaline data (strongly dichroic at 430nm): an ordinary-mode path
    /// propagating exactly perpendicular to `c_axis` must read exactly the o-ray band
    /// sum, and an extraordinary-mode path along the SAME direction must read exactly
    /// the e-ray band sum -- mirrors
    /// `birefringence::polarized_probe_matches_band_sums_for_real_tourmaline_data`'s own
    /// setup, but through the real assigned-mode driver rather than
    /// `pleochroic_channel_alpha` directly.
    #[test]
    fn tourmaline_assigned_mode_alphas_read_o_and_e_ray_band_sums_perpendicular_to_c_axis() {
        const LAMBDA_NM: f32 = 430.0; // Fe2+-Ti4+ IVCT band, strongly dichroic at this wavelength
        let tourmaline =
            GemMaterial::by_name("Tourmaline").expect("Tourmaline must be a built-in material");
        let c_axis = tourmaline.c_axis; // Vec3::X, the documented cut-orientation override
        let alpha_o = spectral_absorption(&tourmaline.absorption.o_ray, LAMBDA_NM);
        let alpha_e = spectral_absorption(&tourmaline.absorption.e_ray, LAMBDA_NM);
        assert!(
            alpha_o > alpha_e * 2.0,
            "test premise: Tourmaline's o-ray must be substantially stronger than its e-ray at \
             430nm (o={alpha_o}, e={alpha_e})"
        );

        let propagation_dir = if c_axis.x.abs() > 0.9 {
            Vec3::Y
        } else {
            Vec3::X
        };
        assert!(
            propagation_dir.dot(c_axis).abs() < 1e-6,
            "test premise: propagation_dir must be exactly perpendicular to c_axis"
        );

        let lambdas = [LAMBDA_NM; NUM_CHANNELS];
        let ctx = RayMaterialContext {
            material: &tourmaline,
            lambdas,
            hero_idx: 0,
            c_axis,
            is_anisotropic: true,
            enable_internal_mode_coupling: true,
        };
        let cache = build_ray_wavelength_cache(&ctx);

        let ordinary_alphas =
            channel_absorption_alphas_assigned(&ctx, &cache, propagation_dir, false);
        let extraordinary_alphas =
            channel_absorption_alphas_assigned(&ctx, &cache, propagation_dir, true);
        for k in 0..NUM_CHANNELS {
            assert!(
                (ordinary_alphas[k] - alpha_o).abs() < 1e-4,
                "channel {k}: ordinary-mode alpha ({}) must equal the o-ray band sum ({alpha_o})",
                ordinary_alphas[k]
            );
            assert!(
                (extraordinary_alphas[k] - alpha_e).abs() < 1e-4,
                "channel {k}: extraordinary-mode alpha ({}) must equal the e-ray band sum \
                 ({alpha_e})",
                extraordinary_alphas[k]
            );
        }
    }

    /// (iii) The new driver has NO Stokes parameter in its signature at all, so its
    /// result cannot depend on the light's polarization state by construction -- unlike
    /// the OLD function, which this test pins as genuinely Stokes-sensitive: two Stokes
    /// vectors sharing the exact same (Q, U) azimuth (hence the exact same
    /// `electric_field_direction`) but different V (hence different
    /// `degree_of_polarization`) must give the OLD function DIFFERENT alphas for a
    /// strongly dichroic extraordinary-mode Tourmaline path -- the precise defect this
    /// pins (a camera path's Stokes vector is importance, not the light's true state,
    /// and drifts azimuth after internal reflections; blending against it is therefore
    /// unsound), which the new Stokes-free driver cannot reproduce.
    #[test]
    fn assigned_mode_alphas_have_no_stokes_dependence_unlike_old_dop_blend() {
        const LAMBDA_NM: f32 = 430.0;
        let tourmaline =
            GemMaterial::by_name("Tourmaline").expect("Tourmaline must be a built-in material");
        let c_axis = tourmaline.c_axis;
        let lambdas = [LAMBDA_NM; NUM_CHANNELS];
        let ctx = RayMaterialContext {
            material: &tourmaline,
            lambdas,
            hero_idx: 0,
            c_axis,
            is_anisotropic: true,
            enable_internal_mode_coupling: true,
        };
        let cache = build_ray_wavelength_cache(&ctx);
        // Oblique to c_axis, so `electric_field_direction`'s azimuth is well-defined and
        // the extraordinary mode is genuinely dichroic here.
        let k_hat = Vec3::new(0.4, 0.5, 0.767_2).normalize();
        let s_axis = k_hat.cross(c_axis).normalize();

        // Same (Q, U) -- same azimuth, same electric_field_direction -- different V.
        let stokes_low_dop = [StokesVector::new(1.0, 0.6, 0.3, 0.0); NUM_CHANNELS];
        let stokes_high_dop = [StokesVector::new(1.0, 0.6, 0.3, 0.7); NUM_CHANNELS];
        let old_low = channel_absorption_alphas(&ctx, &cache, s_axis, k_hat, &stokes_low_dop);
        let old_high = channel_absorption_alphas(&ctx, &cache, s_axis, k_hat, &stokes_high_dop);
        assert!(
            (old_low[0] - old_high[0]).abs() > 1e-4,
            "test premise: the OLD DOP-blended formula must actually be sensitive to a Stokes \
             V component that leaves the azimuth (Q, U) unchanged (old_low={}, old_high={})",
            old_low[0],
            old_high[0]
        );

        // The new driver's signature has no Stokes parameter -- nothing to vary.
        let new_alphas = channel_absorption_alphas_assigned(&ctx, &cache, k_hat, true);
        assert!(
            new_alphas.iter().all(|a| a.is_finite()),
            "new_alphas must be finite: {new_alphas:?}"
        );
    }
}

#[cfg(test)]
mod frame_rotation_sign_tests {
    use super::*;

    /// Two successive bounces that share the SAME plane of incidence (parallel
    /// plane normals) must produce a rotation angle psi ~= 0 -- no Q/U mixing should be
    /// introduced when the frame hasn't actually turned.
    #[test]
    fn same_plane_of_incidence_gives_zero_psi() {
        let prev_normal = Vec3::new(0.0, 0.0, 1.0);
        let current_normal = Vec3::new(0.0, 0.0, 1.0);
        let axis = Vec3::new(1.0, 0.0, 0.0);
        let psi = signed_frame_rotation_psi(prev_normal, current_normal, axis);
        assert!(
            psi.abs() < 1e-5,
            "psi should be ~0 for identical plane-of-incidence normals (got {psi})"
        );
    }

    /// Swapping which plane normal is "previous" and which is "current" must
    /// flip the sign of psi -- this is exactly the defect the old unsigned `acos`-only
    /// computation could not represent (it always returned the same non-negative angle
    /// regardless of rotation direction).
    #[test]
    fn reversing_plane_normal_order_flips_psi_sign() {
        // Two plane-of-incidence normals at a genuine angle to each other, and a
        // propagation axis with a nonzero component along their cross product so the
        // sign is well-defined (not a degenerate coplanar case).
        let normal_a = Vec3::new(1.0, 0.0, 0.0);
        let normal_b = Vec3::new(0.0, 1.0, 0.0).normalize();
        let axis = Vec3::new(0.0, 0.0, 1.0);

        let psi_forward = signed_frame_rotation_psi(normal_a, normal_b, axis);
        let psi_reversed = signed_frame_rotation_psi(normal_b, normal_a, axis);

        assert!(
            psi_forward.abs() > 0.1,
            "psi_forward should be a genuine nonzero rotation (got {psi_forward})"
        );
        assert!(
            (psi_forward + psi_reversed).abs() < 1e-4,
            "reversing the normal order should flip the sign of psi (forward={psi_forward}, reversed={psi_reversed})"
        );
    }

    /// Sanity check that the recovered angle actually matches the geometric angle
    /// between the two normals (90 degrees here), not just that it's nonzero.
    #[test]
    fn psi_magnitude_matches_geometric_angle_between_normals() {
        let normal_a = Vec3::new(1.0, 0.0, 0.0);
        let normal_b = Vec3::new(0.0, 1.0, 0.0);
        let axis = Vec3::new(0.0, 0.0, 1.0);
        let psi = signed_frame_rotation_psi(normal_a, normal_b, axis);
        assert!(
            (psi.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "psi magnitude should match the 90 degree angle between the normals (got {})",
            psi.to_degrees()
        );
    }
}
