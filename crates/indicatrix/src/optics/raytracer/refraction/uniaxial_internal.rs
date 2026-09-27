//! The closed-form uniaxial internal bounce -- o<->e coupled partial reflection and
//! uniaxial->isotropic exit transmission -- via `uniaxial_fresnel::internal_solve`.

use super::{
    context::{BounceRay, BounceState, ExitEvent, RngDraw, UniaxialBounceContext},
    exit_split::try_split_exit_channel,
};
use crate::optics::{
    polarization::StokesVector,
    raytracer::{
        NUM_CHANNELS,
        sampling::{FRESNEL_BRANCH_STREAM, hash_u32},
        uniaxial_fresnel,
    },
};
use glam::Vec3;

/// The uniaxial-crystal internal bounce -- o<->e coupled partial reflection and
/// uniaxial->isotropic exit transmission -- via the closed-form
/// `uniaxial_fresnel::internal_solve`. The internal-bounce analogue of
/// [`super::uniaxial_entry::apply_uniaxial_entry_bounce`]; reached from
/// [`super::dispatch::apply_partial_fresnel_bounce`] whenever
/// `inside_gem && geo.uniaxial_frame.is_some()` (any uniaxial internal event
/// that is not already hero-forced past critical angle -- that case is
/// [`super::tir::apply_tir_bounce`]'s own uniaxial branch, reached from a different
/// call site in `transport::dispatch_bounce`). The shared `apply_partial_reflect_bounce`/
/// `apply_refract_bounce`/`apply_refract_channel` machinery is reached only by an
/// isotropic or biaxial material, unaffected by this function.
///
/// Unlike the entry case there is only ONE incident polarization state to solve for
/// (the propagating Stokes vector is always fully linearly polarized along the current
/// eigenmode's own axis while inside the crystal -- see `apply_tir_bounce`'s own doc
/// comment for why), so `internal_solve` is called once per channel (not as an (s, p)
/// pair, unlike `entry_solve_pair`).
///
/// # Reflect vs. transmit
///
/// `r_branch` is the hero's own Poynting-flux-weighted total reflectance `R_o + R_e`
/// (mirrors `apply_uniaxial_entry_bounce`'s `r_branch` role: drives the reflect/transmit
/// coin flip only, never the per-channel deposit, each channel re-evaluating
/// `internal_solve` at its own wavelength).
///
/// # Exit-transmission Stokes construction
///
/// The incident state is a coherent, fully-polarized single mode (unlike entry's
/// general (s, p) input), so the transmitted Stokes vector for a unit incident
/// amplitude is the standard Jones-vector-to-Stokes identity applied to the
/// flux-normalized amplitudes `t_s' = t_s * sqrt(flux_ts / flux_inc)`, `t_p' = t_p *
/// sqrt(flux_tp / flux_inc)` (needed because the isotropic exit side has two output
/// channels, s and p, with generally different intrinsic flux-per-amplitude, so
/// combining them into one coherent Stokes vector requires the same normalization
/// before `Q`/`U`/`V` are meaningful): `i_unit = |t_s'|^2 + |t_p'|^2` is the physical
/// power transmittance fraction for unit incident amplitude. The incident mode's own
/// current `stokes[k].i` is then the only scale factor needed: the crystal-internal
/// invariant above already guarantees `stokes[k]` carries no other information this
/// event needs.
///
/// Returns `(new_k, new_s, new_inside_gem, is_extraordinary_update, exact_p_o)`. The
/// last element is `Some` only on the reflect branch (o<->e coupling is a
/// reflection-time event only): the hero channel's own `R_o / (R_o + R_e)`, fed to
/// `transport::apply_internal_mode_coupling` as its o<->e relabeling probability.
pub(super) fn apply_uniaxial_internal_bounce(
    ubctx: &UniaxialBounceContext<'_, '_>,
    is_extraordinary: bool,
    ray: BounceRay,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> (Vec3, Vec3, bool, Option<bool>, Option<f32>) {
    const R_UNPOL_SELECT_MIN: f32 = 0.02;
    const R_UNPOL_SELECT_MAX: f32 = 0.98;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let BounceRay { k_hat, normal } = ray;
    let RngDraw { rng_seed, bounce } = rng;

    let hero = ctx.hero_idx;
    let c_axis = ctx.c_axis;
    let n_inc_hero = geo.n1_ch[hero];
    let n_o_hero = geo.n_o_ch[hero];
    let n_e_hero = ctx
        .material
        .extraordinary_index_at(ctx.lambdas[hero], n_o_hero);

    let sol_hero = uniaxial_fresnel::internal_solve(
        n_inc_hero,
        n_o_hero,
        n_e_hero,
        c_axis,
        frame,
        !is_extraordinary,
    );
    let flux_inc_hero = sol_hero.flux_inc.max(1e-12);
    let ro_pow = sol_hero.r_o.norm_sqr() * sol_hero.flux_ro;
    let re_pow = sol_hero.r_e.norm_sqr() * sol_hero.flux_re;
    let r_branch =
        ((ro_pow + re_pow) / flux_inc_hero).clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let p_o_exact = ro_pow / (ro_pow + re_pow).max(1e-12);

    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    let hero_solve = HeroInternalSolve {
        hero,
        is_extraordinary,
        sol_hero,
        r_branch,
    };
    if rng_bounce < r_branch {
        apply_uniaxial_internal_reflect_channels(ubctx, &hero_solve, state);
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        (new_k, new_k, true, None, Some(p_o_exact))
    } else {
        // Exit transmission into isotropic air -- Snell geometry matching
        // `apply_refract_bounce`'s own non-biaxial branch, using k_hat rather than the
        // walked-off S. `geo.n2 == 1.0` (air) and `geo.n1 == n_inc_hero` always hold
        // here (`inside_gem` is true on every path that reaches this function).
        let eta_dir = geo.n1 / geo.n2;
        let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
        let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
        let refr_wave_dir =
            (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();
        apply_uniaxial_internal_transmit_channels(ubctx, &hero_solve, ray, state, exit_event);
        (refr_wave_dir, refr_wave_dir, false, None, None)
    }
}

/// The hero channel's already-solved [`uniaxial_fresnel::internal_solve`] result, shared
/// by [`apply_uniaxial_internal_reflect_channels`] and
/// [`apply_uniaxial_internal_transmit_channels`] so neither re-solves the hero's own
/// boundary system a second time.
#[derive(Clone, Copy)]
pub(super) struct HeroInternalSolve {
    pub(super) hero: usize,
    pub(super) is_extraordinary: bool,
    pub(super) sol_hero: uniaxial_fresnel::InternalPolarizationSolution,
    pub(super) r_branch: f32,
}

/// The reflect branch's per-channel loop for [`apply_uniaxial_internal_bounce`] --
/// same magnitude-only simplification `apply_tir_bounce`'s own uniaxial branch uses
/// (see its doc comment), just importance-sampling-corrected by `1 / r_branch` since
/// this reflect event is NOT forced (probability `r_branch`, not 1).
pub(super) fn apply_uniaxial_internal_reflect_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    hero_solve: &HeroInternalSolve,
    state: &mut BounceState<'_>,
) {
    let ctx = ubctx.ctx;
    let geo = ubctx.geo;
    let frame = ubctx.frame;
    let c_axis = ctx.c_axis;
    let &HeroInternalSolve {
        hero,
        is_extraordinary,
        sol_hero,
        r_branch,
    } = hero_solve;
    let stokes = &mut *state.stokes;
    let path_pdf = &mut *state.path_pdf;
    for k in 0..NUM_CHANNELS {
        // The caller already solved the hero channel's own boundary system once, to
        // decide `r_branch` -- reuse it here instead of solving the identical system
        // again.
        let sol = if k == hero {
            sol_hero
        } else {
            let n_inc_k = geo.n1_ch[k];
            let n_o_k = geo.n_o_ch[k];
            let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
            uniaxial_fresnel::internal_solve(
                n_inc_k,
                n_o_k,
                n_e_k,
                c_axis,
                frame,
                !is_extraordinary,
            )
        };
        let flux_inc = sol.flux_inc.max(1e-12);
        let r_total_k = (sol
            .r_e
            .norm_sqr()
            .mul_add(sol.flux_re, sol.r_o.norm_sqr() * sol.flux_ro)
            / flux_inc)
            .min(1.0);
        stokes[k] = stokes[k].scale(r_total_k / r_branch);
        path_pdf[k] *= r_total_k.clamp(1e-4, 1.0 - 1e-4);
    }
}

/// Channel k's own flux-normalized transmitted Stokes state at the uniaxial exact exit
/// interface -- the same per-channel computation
/// [`apply_uniaxial_internal_transmit_channels`]'s matching-direction branch runs
/// inline, factored out so the split branch can compute the identical value without
/// duplicating the formula. The crystal-internal invariant (`stokes[k].i` is the only
/// scale factor needed) is what makes `incident_i`, rather than the full incident
/// Stokes vector, sufficient here. Returns the transmitted state and `i_unit` (the
/// matching branch's own `path_pdf[k] *= i_unit.clamp(..)` factor; the split branch
/// applies this same factor unconditionally at every exit event).
fn compute_uniaxial_exit_transmission(
    sol: uniaxial_fresnel::InternalPolarizationSolution,
    r_branch: f32,
    incident_i: f32,
) -> (StokesVector, f32) {
    let flux_inc = sol.flux_inc.max(1e-12);
    let ts_n = sol.t_s.scale((sol.flux_ts / flux_inc).sqrt());
    let tp_n = sol.t_p.scale((sol.flux_tp / flux_inc).sqrt());
    let i_unit = ts_n.norm_sqr() + tp_n.norm_sqr();
    let q_unit = ts_n.norm_sqr() - tp_n.norm_sqr();
    let cross = ts_n.mul(tp_n.conj());
    let u_unit = 2.0 * cross.re;
    let v_unit = -2.0 * cross.im;
    let transmitted = StokesVector::new(
        incident_i * i_unit,
        incident_i * q_unit,
        incident_i * u_unit,
        incident_i * v_unit,
    )
    .scale(1.0 / (1.0 - r_branch));
    (transmitted, i_unit)
}

/// The exit-transmission branch's per-channel loop for
/// [`apply_uniaxial_internal_bounce`] -- see [`compute_uniaxial_exit_transmission`]'s
/// own doc comment for the flux-normalized-amplitude-to-Stokes derivation. This is the
/// uniaxial exact exit event, so channel k's own mismatch here is always the
/// exit-splitting case, never an interior one -- see [`super`]'s top-of-file
/// doc comment. `exit.enabled == false` reproduces plain chromatic termination exactly
/// (channel k's own Snell direction at k's own index, compared against the hero's,
/// zeroed on mismatch).
pub(super) fn apply_uniaxial_internal_transmit_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    hero_solve: &HeroInternalSolve,
    ray: BounceRay,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) {
    const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let c_axis = ctx.c_axis;
    let &HeroInternalSolve {
        hero,
        is_extraordinary,
        sol_hero,
        r_branch,
    } = hero_solve;
    let BounceRay { k_hat, normal } = ray;
    let (stokes, path_pdf) = (&mut *state.stokes, &mut *state.path_pdf);
    let (exit, hit_point) = (&mut *exit_event.exit, exit_event.hit_point);

    // The hero's own exit direction, recomputed here from `geo.n1`/`geo.n2`/`k_hat`/
    // `normal` rather than threaded through -- needed as the chromatic-termination
    // reference direction for every channel's own check below.
    let eta_dir = geo.n1 / geo.n2;
    let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
    let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
    let hero_refr_dir =
        (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();

    for k in 0..NUM_CHANNELS {
        // Captured before any mutation below, so the split branch (which needs the
        // original incident value after `stokes[k]` has already been zeroed) can still
        // compute k's own transmission.
        let original_stokes_i = stokes[k].i;

        let n_inc_k = geo.n1_ch[k];
        let n2k = geo.n2_ch[k];
        let sin2_t_k = (n_inc_k / n2k).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
        if sin2_t_k > 1.0 {
            stokes[k] = stokes[k].scale(0.0);
            path_pdf[k] = 0.0;
            continue;
        }
        let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
        let eta_dir_k = n_inc_k / n2k;
        let refr_wave_dir_k =
            (eta_dir_k * k_hat + f32::mul_add(eta_dir_k, geo.cos_i, -cos_t_k) * normal).normalize();
        let direction_matches = refr_wave_dir_k.dot(hero_refr_dir) >= DIRECTION_MATCH_COS_TOL;

        // Reuse the caller's already-solved hero channel instead of re-solving the
        // identical boundary system. Needed by both branches below (the matching one
        // directly; the split one via `compute_uniaxial_exit_transmission`), so solved
        // once here regardless.
        let sol = if k == hero {
            sol_hero
        } else {
            let n_o_k = geo.n_o_ch[k];
            let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
            uniaxial_fresnel::internal_solve(
                n_inc_k,
                n_o_k,
                n_e_k,
                c_axis,
                frame,
                !is_extraordinary,
            )
        };

        if !direction_matches {
            // Chromatic termination -- this is always the exit event, so this is
            // exactly `apply_refract_channel`'s own `else` branch's uniaxial-exact
            // counterpart. With splitting enabled, channel k keeps its own density of
            // having produced the shared path (prefix times its own exit transmit
            // probability, the same `t_unpol_k` the matching branch below folds in)
            // and resolves its own transmitted radiance along its own direction.
            let prefix_path_pdf_k = path_pdf[k];
            stokes[k] = stokes[k].scale(0.0);
            path_pdf[k] = 0.0;

            if exit.enabled {
                let (transmitted, i_unit) =
                    compute_uniaxial_exit_transmission(sol, r_branch, original_stokes_i);
                let t_unpol_k = i_unit.clamp(1e-4, 1.0 - 1e-4);
                path_pdf[k] = prefix_path_pdf_k * t_unpol_k;
                if original_stokes_i > 0.0 {
                    try_split_exit_channel(
                        exit,
                        hit_point,
                        k,
                        ctx.lambdas[k],
                        refr_wave_dir_k,
                        transmitted.intensity(),
                    );
                }
            }
            continue;
        }

        // direction_matches: this is hero's own realized path, or a companion that
        // genuinely coincides with it -- both need `path_pdf[k]`'s normal accumulation
        // for `spectral_mis_weight`.
        let (transmitted, i_unit) =
            compute_uniaxial_exit_transmission(sol, r_branch, original_stokes_i);
        stokes[k] = transmitted;
        let t_unpol_k = i_unit.clamp(1e-4, 1.0 - 1e-4);
        path_pdf[k] *= t_unpol_k;
    }
}
