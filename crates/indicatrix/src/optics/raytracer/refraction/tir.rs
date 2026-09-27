//! Total Internal Reflection: the phase-retardation formula and the hero-forced TIR
//! bounce applier.

use super::{context::RayMaterialContext, geometry::BounceRefractionGeometry};
use crate::optics::{
    polarization::{MuellerMatrix, StokesVector},
    raytracer::{NUM_CHANNELS, uniaxial_fresnel},
};
use glam::Vec3;

/// The wave normal `k` and the Poynting (energy-propagation) direction `S` coincide
/// exactly in an isotropic medium (air, or a cubic material) and for the uniaxial
/// ordinary eigenmode (no walk-off by definition); they diverge only for the
/// extraordinary/mode-B eigenmode of a genuinely anisotropic material while inside it,
/// by the walk-off angle (order 1-2 degrees for zircon-class birefringence). Snell's
/// law and Fresnel phase matching apply to `k`, not `S`, so this module threads both
/// through the bounce loop: `S` (== `Ray::dir` / `current_ray.dir` in `transport.rs`)
/// for intersection/advancement and path length, `k` (== `current_k` in `transport.rs`)
/// for every index lookup, `cos_i`/`sin_i`, Snell refraction, Fresnel coefficients, the
/// TIR decision and phase, and the Stokes plane-of-incidence frame. [`poynting_dir_for_mode`]
/// recovers `S` from a freshly-reflected/refracted `k`.
///
/// Total Internal Reflection phase retardation delta = `delta_p` - `delta_s` (Fresnel
/// rhomb formula) for a wave whose channel index `n1k` puts it past its own critical
/// angle at this interface (`n1k * sin_i > 1`). Shared by the hero-past-critical
/// (deterministic reflect) branch and the partial-reflection branch's per-channel loop,
/// where an individual channel k can be past its own critical angle even though the
/// hero isn't.
#[inline]
pub(crate) fn tir_phase_delta(n1k: f32, cos_i: f32, sin_i: f32) -> f32 {
    let tan_half_delta_k = (cos_i * (n1k * n1k * sin_i).mul_add(sin_i, -1.0).max(0.0).sqrt())
        / (n1k * sin_i * sin_i).max(1e-6);
    2.0 * tan_half_delta_k.atan()
}

/// Total Internal Reflection for the hero channel forces a deterministic reflect for
/// the shared path (probability 1, no pdf division needed). Each channel still gets its
/// own physically-correct outcome for that reflect event: if channel k is itself past
/// its own (wavelength-dependent) critical angle it gets the exact TIR phase
/// retardation at its own index; otherwise -- since the critical angle depends on
/// n(lambda), a channel can be below the hero's critical angle even though the hero
/// isn't -- it gets its own ordinary partial-reflectance Fresnel matrix. No probability
/// division is needed here: the hero's own selection probability for this action is 1
/// (forced), and each channel's Stokes value already carries the correct
/// importance-sampling division from whichever earlier decision had a nontrivial hero
/// probability. The reflection law acts on the wave normal `k_hat`, not `S` -- returns
/// the reflected wave normal `k'`; the caller derives the reflected Poynting direction
/// `S'` via [`poynting_dir_for_mode`] and sets `current_ray.origin` itself.
pub(in crate::optics::raytracer) fn apply_tir_bounce(
    ctx: &RayMaterialContext,
    geo: &BounceRefractionGeometry,
    is_extraordinary: bool,
    k_hat: Vec3,
    normal: Vec3,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> (Vec3, Option<f32>) {
    // The degenerate wave-normal-parallel-to-optic-axis guard the two
    // sibling uniaxial dispatch sites (`apply_uniaxial_entry_bounce`'s own internal
    // check, and `try_dispatch_uniaxial_bounce`'s internal-event dispatch) both have --
    // without it, `internal_solve`'s boundary system singularizes exactly along the
    // c-axis (its own `flux_inc` floors to the `1e-12` guard instead of the true
    // nonzero value), which very nearly zeroed EVERY uniaxial internal bounce along a
    // c-axis-aligned ray. This site is reachable only for a hero-forced TIR bounce
    // (`geo.sin2_t > 1.0`), so it shares the exact same hazard whenever `c_axis = Y`
    // (every uniaxial built-in but Tourmaline) and a top-view centre ray TIRs on a
    // pavilion main. Falls through to the plain scalar per-channel loop below at this
    // limit, exactly as the sibling sites do -- see their own doc comments for why that
    // is the exact physics here, not an approximation.
    if let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        // Uniaxial closed-form solve. `apply_tir_bounce` is reachable only from inside
        // the gem, so every channel's incident state is a single eigenmode (o or e,
        // whichever `is_extraordinary` names) -- the propagating Stokes vector is
        // therefore always fully linearly polarized along that mode's own fixed axis
        // in the current `(s_axis, p_axis)` frame. Reflecting a single real-valued mode
        // amplitude by a complex coefficient only rescales its magnitude -- an overall
        // phase on a one-component state is unobservable, so no `tir_retardation`-style
        // rotation is needed here (unlike the isotropic case, where s and p genuinely
        // retard relative to each other). The energy that couples into the other mode
        // is not represented in this Stokes vector at all -- it is instead captured by
        // `apply_internal_mode_coupling`'s own Poynting-weighted relabeling probability
        // for the next bounce.
        //
        // The hero channel's own `R_o`/`R_e` split (`ro_pow`/`re_pow` below) is also
        // the exact o<->e relabeling probability `transport::apply_internal_mode_coupling`
        // needs for the path's next bounce (`p_o = R_o / (R_o + R_e)`, both
        // Poynting-flux-weighted) -- captured here from the same closed-form solve this
        // loop already runs for the energy accounting.
        let mut exact_p_o = None;
        for k in 0..NUM_CHANNELS {
            let n_inc = geo.n1_ch[k];
            let n_o_k = geo.n_o_ch[k];
            let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
            let sol = uniaxial_fresnel::internal_solve(
                n_inc,
                n_o_k,
                n_e_k,
                ctx.c_axis,
                &frame,
                !is_extraordinary,
            );
            let flux_inc = sol.flux_inc.max(1e-12);
            let ro_pow = sol.r_o.norm_sqr() * sol.flux_ro;
            let re_pow = sol.r_e.norm_sqr() * sol.flux_re;
            let r_total_k = ((ro_pow + re_pow) / flux_inc).min(1.0);
            stokes[k] = stokes[k].scale(r_total_k);
            if geo.sin2_t_ch[k] <= 1.0 {
                // Channel k is genuinely below its own critical angle here even though
                // the hero forced a reflect -- under k's own technique, reflecting has
                // probability r_total_k.
                path_pdf[k] *= r_total_k.clamp(1e-4, 1.0 - 1e-4);
            }
            if k == ctx.hero_idx {
                exact_p_o = Some(ro_pow / (ro_pow + re_pow).max(1e-12));
            }
        }
        return (k_hat - 2.0 * k_hat.dot(normal) * normal, exact_p_o);
    }

    for k in 0..NUM_CHANNELS {
        let n1k = geo.n1_ch[k];
        let n2k = geo.n2_ch[k];
        if geo.sin2_t_ch[k] > 1.0 {
            let delta_k = tir_phase_delta(n1k, geo.cos_i, geo.sin_i);
            let tir_matrix_k = MuellerMatrix::tir_retardation(delta_k);
            stokes[k] = stokes[k].apply_matrix(&tir_matrix_k);
            // Channel k is also past its own critical angle here, so under k's own
            // technique this reflect is also forced (probability 1); reflection
            // direction never depends on wavelength, so channel k's path-pdf factor
            // here is exactly 1, a no-op left unwritten.
        } else {
            let cos_t_k = (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt();
            let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * geo.cos_i)
                / f32::mul_add(n2k, cos_t_k, n1k * geo.cos_i);
            let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * geo.cos_i)
                / f32::mul_add(n1k, cos_t_k, n2k * geo.cos_i);
            let refl_matrix_k = MuellerMatrix::fresnel_reflection(r_s_k, r_p_k);
            stokes[k] = stokes[k].apply_matrix(&refl_matrix_k);
            // Channel k is genuinely below its own critical angle here even though
            // the hero forced a reflect -- under k's own technique, reflecting (the
            // observed outcome) has probability equal to k's own unpolarized
            // reflectance. Direction still matches trivially (reflection is never
            // dispersive), so no chromatic-termination check applies at a reflect
            // event.
            let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
            path_pdf[k] *= r_unpol_k;
        }
    }
    (k_hat - 2.0 * k_hat.dot(normal) * normal, None)
}
