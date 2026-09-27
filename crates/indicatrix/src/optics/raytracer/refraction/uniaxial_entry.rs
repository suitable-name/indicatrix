//! The closed-form uniaxial air->crystal entry bounce -- [`apply_uniaxial_entry_bounce`]
//! and its reflect/transmit per-channel loops -- plus [`entry_eigenmode_selection`], the
//! polarization-weighted ordinary/extraordinary eigenmode draw every anisotropic entry
//! (uniaxial or biaxial scalar-Fresnel) shares.

use super::{
    context::{
        BounceRay, BounceState, ExitSplitCtx, RngDraw, UniaxialBounceContext, narrow_compat,
    },
    geometry::BounceRefractionGeometry,
};
use crate::optics::{
    birefringence::BirefringenceParams,
    polarization::{MuellerMatrix, StokesVector},
    raytracer::{
        NUM_CHANNELS,
        sampling::{BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, hash_u32},
        uniaxial_fresnel,
    },
};
use glam::Vec3;

/// Polarization-weighted probability that the uniaxial ordinary eigenmode is the
/// physically correct label for this bounce's shared hero-driven path, together with
/// that eigenmode's own doubled polarization azimuth (`cos(2*psi_o)`, `sin(2*psi_o)`)
/// expressed in the same `(s, p)` Stokes frame `current_plane_normal` establishes
/// (`s_axis == current_plane_normal`, `p_axis == k_hat x s_axis`, matching
/// `MuellerMatrix::fresnel_reflection`/`fresnel_transmission`).
///
/// `p_o = 1/2 * (1 + DoP_linear * cos(2*(psi - psi_o)))`, where `psi` is the incident
/// Stokes state's own polarization azimuth and `DoP_linear = sqrt(Q^2+U^2)/I` -- the
/// physical energy fraction Malus's law puts into the ordinary eigenmode for a
/// partially linearly polarized beam (the unpolarized remainder splits 50/50; the
/// polarized remainder follows `cos^2(psi-psi_o)`). Circular polarization does not
/// bias the split between two linear eigenmodes, so `V` plays no role. Expanded
/// directly in `Q`/`U` rather than via `psi`/`DoP_linear` intermediates, which also
/// makes `p_o` reduce to exactly 0.5 for unpolarized light with no separate branch.
/// `cos(2*psi_o)`/`sin(2*psi_o)` come from `o_hat`'s components in the local
/// `(s_hat, p_hat)` basis via the double-angle identities `cos(2x) = s^2 - p^2`,
/// `sin(2x) = 2*s*p`, avoiding an `atan2`/half-angle round-trip.
///
/// Returns `None` (fall back to a flat 50/50 draw, no eigenmode projection) whenever
/// there isn't enough signal to weight the draw meaningfully: `i` negligibly small, a
/// degenerate plane of incidence (`current_plane_normal` near zero, near-normal
/// incidence), or negligible linear polarization (`q`/`u` both near zero). Callers also
/// skip this entirely for a biaxial material -- `ordinary_eigen_polarization` is
/// uniaxial-only -- keeping a biaxial entry at a blanket 50/50 unconditionally.
#[must_use]
pub(in crate::optics::raytracer) fn entry_eigenmode_selection(
    c_axis: Vec3,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    hero_stokes: StokesVector,
) -> Option<(f32, f32, f32)> {
    if hero_stokes.i <= 1e-7 || current_plane_normal.length_squared() <= 1e-6 {
        return None;
    }
    let (q, u) = (hero_stokes.q, hero_stokes.u);
    if q.mul_add(q, u * u) <= 1e-12 {
        return None;
    }
    let o_hat = BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis);
    // `current_plane_normal` is already unit and exactly perpendicular to `k_hat`
    // (`k_hat.cross(normal)`, normalized) -- no re-orthogonalization needed.
    let s_hat = current_plane_normal;
    let p_hat = k_hat.cross(s_hat);
    let s_comp = o_hat.dot(s_hat);
    let p_comp = o_hat.dot(p_hat);
    let cos_2psi_o = s_comp.mul_add(s_comp, -(p_comp * p_comp));
    let sin_2psi_o = 2.0 * s_comp * p_comp;
    let p_o = (0.5 + 0.5 * q.mul_add(cos_2psi_o, u * sin_2psi_o) / hero_stokes.i).clamp(0.0, 1.0);
    Some((p_o, cos_2psi_o, sin_2psi_o))
}

/// The degenerate (`k_hat` parallel to the optic axis) fallback [`apply_uniaxial_entry_bounce`]
/// dispatches to: plain scalar isotropic Fresnel at each channel's own `n_o_ch[k]`
/// (`effective_extraordinary_index(n_o, n_e, theta_c=0) == n_o` exactly, so `n_o` is
/// the correct, exact index here), applied to the incident Stokes state unprojected --
/// the isotropic-material code path elsewhere in this file, inlined for this
/// anisotropic-material special case. Always labels the resulting internal state
/// ordinary (`Some(false)`): with both eigenmodes truly degenerate to `n_o` here,
/// `n_medium_ch`'s selector reads the same value either way, so the label is bookkeeping
/// only, not a physical claim.
fn apply_uniaxial_entry_bounce_isotropic_fallback(
    geo: &BounceRefractionGeometry,
    k_hat: Vec3,
    normal: Vec3,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> (Vec3, Vec3, bool, Option<bool>) {
    const R_UNPOL_SELECT_MIN: f32 = 0.02;
    const R_UNPOL_SELECT_MAX: f32 = 0.98;

    let n1 = 1.0f32;
    let n2_hero = geo.n_o_hero;
    let cos_i = geo.cos_i;
    let sin2_t = (n1 / n2_hero).powi(2) * sin_i_sq(cos_i);
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s_hero = n2_hero.mul_add(-cos_t, n1 * cos_i) / n2_hero.mul_add(cos_t, n1 * cos_i);
    let r_p_hero = n1.mul_add(-cos_t, n2_hero * cos_i) / n1.mul_add(cos_t, n2_hero * cos_i);
    let r_unpol = (0.5 * r_p_hero.mul_add(r_p_hero, r_s_hero * r_s_hero))
        .clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);

    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_unpol {
        for k in 0..NUM_CHANNELS {
            let n2k = geo.n_o_ch[k];
            let sin2_t_k = (n1 / n2k).powi(2) * sin_i_sq(cos_i);
            let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
            let r_s_k = n2k.mul_add(-cos_t_k, n1 * cos_i) / n2k.mul_add(cos_t_k, n1 * cos_i);
            let r_p_k = n1.mul_add(-cos_t_k, n2k * cos_i) / n1.mul_add(cos_t_k, n2k * cos_i);
            let refl_matrix_k = MuellerMatrix::fresnel_reflection(r_s_k, r_p_k);
            let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
            stokes[k] = stokes[k].apply_matrix(&refl_matrix_k).scale(1.0 / r_unpol);
            path_pdf[k] *= r_unpol_k;
        }
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        (new_k, new_k, false, None)
    } else {
        for k in 0..NUM_CHANNELS {
            let n2k = geo.n_o_ch[k];
            let sin2_t_k = (n1 / n2k).powi(2) * sin_i_sq(cos_i);
            let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
            let t_s_k = (2.0 * n1 * cos_i) / n1.mul_add(cos_i, n2k * cos_t_k);
            let t_p_k = (2.0 * n1 * cos_i) / n2k.mul_add(cos_i, n1 * cos_t_k);
            let trans_matrix_k =
                MuellerMatrix::fresnel_transmission(n1, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
            stokes[k] = stokes[k]
                .apply_matrix(&trans_matrix_k)
                .scale(1.0 / (1.0 - r_unpol));
            let r_s_k = n2k.mul_add(-cos_t_k, n1 * cos_i) / n2k.mul_add(cos_t_k, n1 * cos_i);
            let r_p_k = n1.mul_add(-cos_t_k, n2k * cos_i) / n1.mul_add(cos_t_k, n2k * cos_i);
            let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
            path_pdf[k] *= 1.0 - r_unpol_k;
        }
        let eta = n1 / n2_hero;
        let new_k = (eta * k_hat + f32::mul_add(eta, cos_i, -cos_t) * normal).normalize();
        (new_k, new_k, true, Some(false))
    }
}

#[inline]
fn sin_i_sq(cos_i: f32) -> f32 {
    cos_i.mul_add(-cos_i, 1.0)
}

/// The entire isotropic-air -> uniaxial-crystal entry bounce, via the closed-form
/// `uniaxial_fresnel` solver -- replaces a per-mode scalar-Fresnel entry path (a
/// Malus-law mode split plus isotropic-style `r_s`/`r_p`/`t_s`/`t_p` at a single
/// "effective index") for `entering_anisotropic && !geo.is_biaxial`. Called as an
/// early, self-contained branch from `apply_partial_fresnel_bounce`, leaving the
/// biaxial/isotropic/internal-exit code paths unaffected.
///
/// Reuses the existing Snell-refraction/extraordinary-walk-off geometry formulas
/// (`compute_bounce_refraction_geometry`'s `geo.n2`/`geo.n2_ch`/`geo.n_o_hero`
/// scaffolding) unchanged: only the Fresnel amplitude/polarization physics changes
/// here, not the direction the transmitted ray travels in.
///
/// # Reflect vs. transmit, and mode selection
///
/// `r_branch` is the hero's own unpolarized-average total reflectance from the full
/// coupled Jones solution (`jones_to_mueller(...).col(0)[0]`) -- used only to draw the
/// reflect/transmit branch, never to weight the actual contribution. If transmitting,
/// `p_o_hero` is the hero's true Poynting-weighted ordinary-mode power fraction
/// (`uniaxial_fresnel::mode_power`, fed the hero's actual Stokes state): it already
/// incorporates both the incident polarization's alignment with each mode's eigenaxis
/// and that mode's own transmission efficiency in one physically exact quantity.
/// Reduces to 50/50 only where the geometry actually makes it symmetric (e.g. optic
/// axis along the surface normal) -- see `mode_power`'s own doc comment.
///
/// Each channel's deposited transmitted intensity is `P_mode(stokes[k]) /
/// p_mode_hero_frac` (standard importance-sampling `target / p_sample`, unbiased for
/// any valid `p_sample`), polarized along the drawn mode's own eigenaxis (`o_hat`/
/// `e_hat`, always real -- an entry interface's transmitted side never evanesces).
pub(super) fn apply_uniaxial_entry_bounce(
    ubctx: &UniaxialBounceContext<'_, '_>,
    ray: BounceRay,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit: &mut ExitSplitCtx<'_>,
) -> (Vec3, Vec3, bool, Option<bool>) {
    const R_UNPOL_SELECT_MIN: f32 = 0.02;
    const R_UNPOL_SELECT_MAX: f32 = 0.98;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let BounceRay { k_hat, normal } = ray;
    let RngDraw { rng_seed, bounce } = rng;

    let hero = ctx.hero_idx;
    let c_axis = ctx.c_axis;
    let n_o_hero = geo.n_o_ch[hero];
    let n_e_hero = ctx
        .material
        .extraordinary_index_at(ctx.lambdas[hero], n_o_hero);

    // Degenerate case: wave normal parallel to the optic axis (light travelling
    // straight down the c-axis sees no birefringence; both eigenmodes collapse to
    // `n_o`). The closed-form ordinary D-direction `k x c_axis` vanishes exactly here,
    // singularizing the boundary-match system -- checked via `k_hat` (the incident
    // direction) since at this limit Snell's law leaves the transmitted direction
    // parallel to it too. Falls back to the plain scalar isotropic-at-`n_o` path,
    // without projecting onto either eigenmode's own axis, since there is no
    // meaningful axis to project onto at this exact limit.
    if k_hat.cross(c_axis).length_squared() < 1e-6 {
        return apply_uniaxial_entry_bounce_isotropic_fallback(
            geo,
            k_hat,
            normal,
            rng_seed,
            bounce,
            state.stokes,
            state.path_pdf,
        );
    }

    // Built once per bounce, shared by the hero's own branch-decision solve below and
    // every channel's solve in
    // `apply_uniaxial_entry_reflect_channels`/`apply_uniaxial_entry_transmit_channels`
    // -- bit-identical to each of those calling `entry_solve_pair` (which rebuilds
    // this internally) fresh, since `n1 == 1.0` for every channel at an air->crystal
    // entry -- see `EntryIncidenceFrame`'s own doc comment.
    let inc_frame = uniaxial_fresnel::entry_incidence_frame(1.0, frame);
    let (sol_s_hero, sol_p_hero) = uniaxial_fresnel::entry_solve_pair_with_incidence(
        &inc_frame, 1.0, n_o_hero, n_e_hero, c_axis, frame,
    );
    let reflect_mueller_hero = uniaxial_fresnel::jones_to_mueller(
        sol_s_hero.r_s,
        sol_p_hero.r_s,
        sol_s_hero.r_p,
        sol_p_hero.r_p,
    );
    let r_branch = reflect_mueller_hero.col(0)[0].clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);

    // Amplitude-1 isotropic-mode intrinsic flux (`poynting_z`'s own 0.5 time-average
    // factor included) -- shared by every channel at entry (`n1 == 1.0` for air,
    // `frame.cos_i` is the hero-driven shared geometry every channel's own solve also
    // uses), so computed once here rather than re-derived per `mode_power` call.
    let inc_flux = 0.5 * frame.cos_i;
    let p_o_raw = uniaxial_fresnel::mode_power(
        sol_s_hero.t_o,
        sol_p_hero.t_o,
        sol_s_hero.flux_o,
        inc_flux,
        state.stokes[hero],
    );
    let p_e_raw = uniaxial_fresnel::mode_power(
        sol_s_hero.t_e,
        sol_p_hero.t_e,
        sol_s_hero.flux_e,
        inc_flux,
        state.stokes[hero],
    );
    let p_o_hero =
        (p_o_raw / (p_o_raw + p_e_raw).max(1e-12)).clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let mode_split_rand = (hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
        as f32)
        / 4_294_967_295.0;
    let use_extraordinary = mode_split_rand < (1.0 - p_o_hero);
    let p_mode_hero_frac = if use_extraordinary {
        1.0 - p_o_hero
    } else {
        p_o_hero
    };

    // Geometry: bit-identical to apply_refract_bounce's own non-biaxial branch.
    let n2_hero_dir = if use_extraordinary {
        geo.n2
    } else {
        geo.n_o_hero
    };
    let eta_dir = geo.n1 / n2_hero_dir;
    let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
    let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
    let refr_wave_dir =
        (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();
    let final_refr_dir = if use_extraordinary {
        BirefringenceParams::extraordinary_poynting_dir(
            refr_wave_dir,
            c_axis,
            geo.n_o_hero,
            geo.n_e_hero,
        )
    } else {
        refr_wave_dir
    };
    // `o_hat`/`e_hat` are only guaranteed perpendicular to `refr_wave_dir` (the
    // transmitted wave normal), not to `k_hat` (the incident one) -- `frame.p_axis` is
    // built from `k_hat`, so projecting a mode direction onto it directly would lose
    // information whenever Snell's law bends the ray. `s_axis` is shared between the
    // incident and transmitted sides (Snell coplanarity), so
    // `p_axis_transmitted = refr_wave_dir x s_axis` is the correct partner axis, and
    // `refr_wave_dir` is exactly the axis `current_k` becomes for the next bounce's
    // own rotation.
    let p_axis_transmitted = refr_wave_dir.cross(frame.s_axis);

    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_branch {
        apply_uniaxial_entry_reflect_channels(ubctx, &inc_frame, r_branch, state);
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        (new_k, new_k, false, None)
    } else {
        let mode = EntryTransmitMode {
            use_extraordinary,
            final_refr_dir,
            p_axis_transmitted,
            p_mode_hero_frac,
            inc_flux,
            r_branch,
        };
        apply_uniaxial_entry_transmit_channels(ubctx, &inc_frame, ray, &mode, state, exit);
        (refr_wave_dir, final_refr_dir, true, Some(use_extraordinary))
    }
}

/// The reflect branch's per-channel loop, extracted from [`apply_uniaxial_entry_bounce`]
/// to keep that function under clippy's function-length lint. `inc_frame` is the
/// caller's already-built [`EntryIncidenceFrame`] -- see that type's own doc comment.
fn apply_uniaxial_entry_reflect_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    r_branch: f32,
    state: &mut BounceState<'_>,
) {
    let ctx = ubctx.ctx;
    let geo = ubctx.geo;
    let frame = ubctx.frame;
    let c_axis = ctx.c_axis;
    let stokes = &mut *state.stokes;
    let path_pdf = &mut *state.path_pdf;
    for k in 0..NUM_CHANNELS {
        let n_o_k = geo.n_o_ch[k];
        let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
            inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
        );
        let mueller_k =
            uniaxial_fresnel::jones_to_mueller(sol_s_k.r_s, sol_p_k.r_s, sol_s_k.r_p, sol_p_k.r_p);
        let r_unpol_k = mueller_k.col(0)[0].clamp(1e-4, 1.0 - 1e-4);
        stokes[k] = stokes[k].apply_matrix(&mueller_k).scale(1.0 / r_branch);
        path_pdf[k] *= r_unpol_k;
    }
}

/// The transmit branch's per-channel loop (chromatic-termination geometry check plus
/// the closed-form mode-power amplitude deposit), extracted from
/// [`apply_uniaxial_entry_bounce`] for the same reason as
/// [`apply_uniaxial_entry_reflect_channels`].
/// [`apply_uniaxial_entry_bounce`]'s own resolved transmit-branch outcome, passed down
/// to [`apply_uniaxial_entry_transmit_channels`]'s per-channel loop unchanged.
struct EntryTransmitMode {
    use_extraordinary: bool,
    final_refr_dir: Vec3,
    p_axis_transmitted: Vec3,
    p_mode_hero_frac: f32,
    inc_flux: f32,
    r_branch: f32,
}

fn apply_uniaxial_entry_transmit_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    ray: BounceRay,
    mode: &EntryTransmitMode,
    state: &mut BounceState<'_>,
    exit: &mut ExitSplitCtx<'_>,
) {
    const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let c_axis = ctx.c_axis;
    let BounceRay { k_hat, normal } = ray;
    let &EntryTransmitMode {
        use_extraordinary,
        final_refr_dir,
        p_axis_transmitted,
        p_mode_hero_frac,
        inc_flux,
        r_branch,
    } = mode;
    let (stokes, path_pdf) = (&mut *state.stokes, &mut *state.path_pdf);

    let mut dirs = [None; NUM_CHANNELS];
    let mut hero_match = [false; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let n_o_k = geo.n_o_ch[k];
        // Chromatic-termination geometry: apply_refract_channel's own formula,
        // reusing geo.n2_ch[k] (the channel's own effective extraordinary index) for
        // the direction check only.
        let n2k_dir = if use_extraordinary {
            geo.n2_ch[k]
        } else {
            n_o_k
        };
        let sin2_t_k = (geo.n1 / n2k_dir).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
        if sin2_t_k > 1.0 {
            stokes[k] = stokes[k].scale(0.0);
            path_pdf[k] = 0.0;
            continue;
        }
        let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
        let eta_dir_k = geo.n1 / n2k_dir;
        let refr_wave_dir_k =
            (eta_dir_k * k_hat + f32::mul_add(eta_dir_k, geo.cos_i, -cos_t_k) * normal).normalize();
        let final_dir_k = if use_extraordinary {
            let n_e_eff_k = n_o_k + ctx.material.birefringence_delta;
            BirefringenceParams::extraordinary_poynting_dir(
                refr_wave_dir_k,
                c_axis,
                n_o_k,
                n_e_eff_k,
            )
        } else {
            refr_wave_dir_k
        };
        dirs[k] = Some(final_dir_k);
        let direction_matches = final_dir_k.dot(final_refr_dir) >= DIRECTION_MATCH_COS_TOL;
        hero_match[k] = direction_matches;
        if !direction_matches {
            stokes[k] = stokes[k].scale(0.0);
            if exit.enabled {
                // Technique k stays a live member of every compatible channel's MIS
                // family -- keep its own entry-transmit density accumulating (the same
                // `t_unpol_k` the matching case below folds in); only its radiance
                // ends here. See `apply_refract_channel`'s scalar counterpart.
                let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
                let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
                    inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
                );
                let mueller_k = uniaxial_fresnel::jones_to_mueller(
                    sol_s_k.r_s,
                    sol_p_k.r_s,
                    sol_s_k.r_p,
                    sol_p_k.r_p,
                );
                let t_unpol_k = (1.0 - mueller_k.col(0)[0]).clamp(1e-4, 1.0 - 1e-4);
                path_pdf[k] *= t_unpol_k;
            } else {
                path_pdf[k] = 0.0;
            }
            continue;
        }

        let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
            inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
        );
        let (t_s_row, t_p_row, flux_k, mode_dir) = if use_extraordinary {
            (sol_s_k.t_e, sol_p_k.t_e, sol_s_k.flux_e, sol_s_k.e_hat)
        } else {
            (sol_s_k.t_o, sol_p_k.t_o, sol_s_k.flux_o, sol_s_k.o_hat)
        };
        let p_mode_k = uniaxial_fresnel::mode_power(t_s_row, t_p_row, flux_k, inc_flux, stokes[k]);
        let deposit_i = p_mode_k / p_mode_hero_frac;
        let (cos_2psi, sin_2psi) =
            uniaxial_fresnel::azimuth2_in_frame(mode_dir, frame.s_axis, p_axis_transmitted);
        stokes[k] = StokesVector::new(deposit_i, deposit_i * cos_2psi, deposit_i * sin_2psi, 0.0)
            .scale(1.0 / (1.0 - r_branch));

        let mueller_k =
            uniaxial_fresnel::jones_to_mueller(sol_s_k.r_s, sol_p_k.r_s, sol_s_k.r_p, sol_p_k.r_p);
        let t_unpol_k = (1.0 - mueller_k.col(0)[0]).clamp(1e-4, 1.0 - 1e-4);
        path_pdf[k] *= t_unpol_k;
    }
    // The uniaxial entry is an interior dispersive event -- narrow every channel's MIS
    // family, see `narrow_compat`'s doc comment.
    if exit.enabled {
        narrow_compat(&mut exit.compat, &dirs, ctx.hero_idx, hero_match);
    }
}
