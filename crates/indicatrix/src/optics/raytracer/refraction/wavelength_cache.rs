//! Builds the per-sample [`RayWavelengthCache`].

use super::{
    context::{RayMaterialContext, RayWavelengthCache},
    geometry::per_channel_uniaxial_indices,
};
use crate::optics::{
    birefringence::{AbsorptionTensor3, BiaxialIndicatrix},
    raytracer::{NUM_CHANNELS, absorption::spectral_absorption},
};

/// Builds [`RayWavelengthCache`] once per sample.
pub(in crate::optics::raytracer) fn build_ray_wavelength_cache(
    ctx: &RayMaterialContext,
) -> RayWavelengthCache {
    let material = ctx.material;

    // `n_o_ch` never depends on `theta_c` (only the discarded `n_eff_ch` half does), so
    // the `0.0` argument here is an arbitrary placeholder; the real per-bounce
    // `n_eff_ch` is computed fresh every bounce by
    // `per_channel_effective_extraordinary_indices` from this cached `n_o_ch`.
    let (n_o_ch, _n_eff_ch_unused_theta_c_independent_half) =
        per_channel_uniaxial_indices(ctx, 0.0);

    let biaxial_ch: [Option<BiaxialIndicatrix>; NUM_CHANNELS] =
        std::array::from_fn(|k| material.biaxial_indicatrix(ctx.lambdas[k]));
    let hero_indicatrix = biaxial_ch[ctx.hero_idx];
    let is_biaxial = hero_indicatrix.is_some();

    let abs_o = &material.absorption.o_ray;
    let abs_e = &material.absorption.e_ray;
    let abs_beta = material.absorption.beta_ray.as_deref();
    let alpha_o_ch: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| spectral_absorption(abs_o, ctx.lambdas[k]));
    let alpha_e_ch: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| spectral_absorption(abs_e, ctx.lambdas[k]));
    let alpha_beta_ch: [Option<f32>; NUM_CHANNELS] = std::array::from_fn(|k| {
        if is_biaxial {
            abs_beta.map(|bands| spectral_absorption(bands, ctx.lambdas[k]))
        } else {
            None
        }
    });
    let tensor_ch: [AbsorptionTensor3; NUM_CHANNELS] = std::array::from_fn(|k| {
        alpha_beta_ch[k].map_or_else(
            || AbsorptionTensor3::uniaxial(alpha_o_ch[k], alpha_e_ch[k], ctx.c_axis),
            |beta| AbsorptionTensor3::biaxial(alpha_o_ch[k], beta, alpha_e_ch[k], ctx.c_axis),
        )
    });

    RayWavelengthCache {
        n_o_ch,
        hero_indicatrix,
        biaxial_ch,
        tensor_ch,
    }
}
