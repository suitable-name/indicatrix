//! The scalar-Fresnel reflect/refract machinery reached by an isotropic material, a
//! biaxial material, or the degenerate uniaxial wave-normal-parallel-to-optic-axis
//! limit -- [`apply_partial_reflect_bounce`] and [`apply_refract_bounce`]/
//! [`apply_refract_channel`]'s per-channel exit-splitting-aware chromatic termination.

use super::{
    DIRECTION_MATCH_COS_TOL, R_UNPOL_PDF_MAX, R_UNPOL_PDF_MIN,
    context::{BounceContext, BounceRay, BounceState, ExitEvent, narrow_compat},
    exit_split::{ChannelTransmissionInputs, compute_channel_transmission, try_split_exit_channel},
    geometry::BounceRefractionGeometry,
    tir::tir_phase_delta,
};
use crate::optics::{
    birefringence::BirefringenceParams,
    polarization::{MuellerMatrix, StokesVector},
    raytracer::NUM_CHANNELS,
};
use glam::Vec3;

/// Partial Fresnel Reflection & Refraction via Stokes-Mueller Polarized Wave Transport
/// -- the reflect half. Which branch is taken (reflect vs. transmit) is decided once,
/// from the hero's `r_unpol`, driving the single shared geometric path; each channel
/// then applies its own Fresnel reflection matrix, divided by the same hero selection
/// probability (`r_unpol`, the actual probability with which "reflect" was sampled).
/// When the material is non-dispersive, every channel's matrix is identical to the
/// hero's, reducing to a plain unweighted result. Reflects the wave normal `k_hat` and
/// returns the reflected wave normal `k'`; the caller derives `S'` via
/// [`super::geometry::poynting_dir_for_mode`] and sets `current_ray.origin` itself.
pub(super) fn apply_partial_reflect_bounce(
    geo: &BounceRefractionGeometry,
    r_unpol: f32,
    k_hat: Vec3,
    normal: Vec3,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Vec3 {
    for k in 0..NUM_CHANNELS {
        let n1k = geo.n1_ch[k];
        let n2k = geo.n2_ch[k];
        let refl_matrix_k = if geo.sin2_t_ch[k] > 1.0 {
            // Channel k is past its own critical angle here even though the hero
            // isn't; it also picks up the TIR phase retardation delta = delta_p -
            // delta_s, exactly as `apply_tir_bounce` applies for the hero. Channel k
            // is forced to TIR here regardless of the hero's own physics -- under k's
            // own technique this reflect also happens with probability 1, so no
            // path-pdf factor is needed.
            let delta_k = tir_phase_delta(n1k, geo.cos_i, geo.sin_i);
            MuellerMatrix::tir_retardation(delta_k)
        } else {
            let cos_t_k = (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt();
            let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * geo.cos_i)
                / f32::mul_add(n2k, cos_t_k, n1k * geo.cos_i);
            let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * geo.cos_i)
                / f32::mul_add(n1k, cos_t_k, n2k * geo.cos_i);
            // Channel k's own probability of choosing reflect here, using k's own
            // unpolarized reflectance (not the hero's r_unpol). Reflection direction
            // never depends on wavelength, so no chromatic-termination check applies
            // at a reflect event.
            let r_unpol_k =
                (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
            path_pdf[k] *= r_unpol_k;
            MuellerMatrix::fresnel_reflection(r_s_k, r_p_k)
        };
        stokes[k] = stokes[k].apply_matrix(&refl_matrix_k).scale(1.0 / r_unpol);
    }
    k_hat - 2.0 * k_hat.dot(normal) * normal
}

/// What [`apply_refract_bounce`] changes about the traced path beyond `stokes` and
/// `path_pdf` (which it mutates directly): the new wave normal `k'` and Poynting
/// direction `S'`, and -- only on an air->crystal entry into an anisotropic material --
/// the eigenmode this path was stochastically assigned to. `None` means "leave
/// `is_extraordinary` exactly as it was".
pub(super) struct RefractBounceOutcome {
    pub(super) new_k: Vec3,
    pub(super) new_s: Vec3,
    pub(super) is_extraordinary_update: Option<bool>,
}

/// Partial Fresnel Reflection & Refraction via Stokes-Mueller Polarized Wave Transport
/// -- the refract half (transmit branch, taken when the hero's `rng_bounce >=
/// r_unpol`). A companion channel's refracted direction must match the shared
/// hero-driven direction to within `DIRECTION_MATCH_COS_TOL` (not exact float equality:
/// two evaluations of the same refraction formula at the same index are bit-identical
/// or differ by a handful of ULPs, but two different indices generically produce a
/// direction difference many orders of magnitude larger).
///
/// At an air->crystal entry into an anisotropic material, unpolarized incident light
/// couples into two orthogonally polarized eigenmodes -- ordinary (no walk-off) and
/// extraordinary (Poynting direction displaced by the walk-off angle), each carrying
/// roughly half the incident energy. Only one geometric path is traced per sample, so
/// which eigenmode this path becomes is chosen stochastically 50/50 -- but this is an
/// energy share, not a 1-of-N selection: `trans_matrix_k` below already computes the
/// full transmitted intensity for a beam at the selected mode's own index, so weighting
/// it by that mode's ~0.5 energy share gives an unbiased estimator of
/// `0.5*T_o + 0.5*T_e` with a factor of exactly 1, not the `1/0.5 = 2.0` a naive
/// "divide by the selection probability" rule would suggest (that rule is correct for
/// the reflect/refract split's `r_unpol`/`1 - r_unpol`, where the two branches
/// partition disjoint energy; mode selection instead splits one shared energy pool).
///
/// Chromatic termination: when channel k's own specular refraction direction genuinely
/// diverges from the direction the hero-driven path actually took (or channel k cannot
/// transmit at this angle at all), channel k's path pdf and its Stokes/radiance
/// contribution are dropped to exactly 0, not merely down-weighted -- required for
/// unbiasedness, confirmed by a two-channel Fresnel Monte Carlo cross-check (see
/// `two_channel_dispersive_termination_monte_carlo_is_unbiased_under_alternating_hero`).
/// The already-resolved eigenmode selection [`super::dispatch::apply_partial_fresnel_bounce`]
/// passes down into [`apply_refract_bounce`]: which mode this path's transmission event
/// represents, and (when the caller found a meaningful polarization frame) that mode's
/// own doubled azimuth for [`apply_refract_channel`]'s eigenmode projection.
#[derive(Clone, Copy)]
pub(super) struct RefractSelection {
    pub(super) use_extraordinary: bool,
    pub(super) entry_mode_azimuth2: Option<(f32, f32)>,
}

pub(super) fn apply_refract_bounce(
    bctx: &BounceContext<'_, '_>,
    r_unpol: f32,
    ray: BounceRay,
    inside_gem: bool,
    selection: RefractSelection,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> RefractBounceOutcome {
    let ctx = bctx.ctx;
    let geo = bctx.geo;
    let BounceRay { k_hat, normal } = ray;
    let RefractSelection {
        use_extraordinary,
        entry_mode_azimuth2,
    } = selection;
    let c_axis = ctx.c_axis;

    // The choice is recorded in the returned `is_extraordinary_update` so subsequent
    // internal bounces keep using the same eigenmode's index (via `n_medium_ch` in
    // `compute_bounce_refraction_geometry`). Exiting the crystal, and any refraction in
    // an isotropic material, leaves `entering_anisotropic` false.
    let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
    // Mode selection -- both the coin flip (`entry_eigenmode_selection`'s
    // polarization-weighted `p_o` in place of a blanket 50/50) and the
    // reflect-vs-transmit decision that must be consistent with it -- is resolved once
    // by the caller, `apply_partial_fresnel_bounce`, before it decides whether this
    // bounce is even a refract event at all. `use_extraordinary` here is that
    // already-resolved value, not a fresh draw.
    //
    // Direction: the ordinary eigenmode's wave normal uses n_o and is never walked
    // off; the extraordinary eigenmode uses n_eff and its energy (Poynting) direction
    // is displaced by the walk-off angle. Computed before the per-channel loop below
    // because each companion channel's own hypothetical refracted direction must be
    // compared against this same hero-driven direction to detect a dispersive
    // mismatch. For a biaxial material entering the crystal, neither mode is a plain
    // constant-index Snell refraction -- both modes walk off via `mode_poynting_dir` --
    // using `geo.n_biax_a_hero`/`geo.n_biax_b_hero` (the same looked-up scalars the
    // per-channel loop's own `k == hero_idx` iteration uses) for self-consistency.
    // `refr_wave_dir` is the Snell-refracted wave normal `k'` (Snell's law acts on `k`,
    // not `S`) -- captured alongside the Poynting-converted `S'` in every branch below,
    // since the caller needs both (see [`RefractBounceOutcome`]'s own doc comment). At
    // an air->crystal entry, `k_hat == S` trivially (isotropic air). At an exit
    // (leaving an anisotropic crystal) or any isotropic refraction, Snell's law at the
    // interface must refract the wave normal, not the walked-off Poynting direction.
    let (new_k, final_refr_dir) = if let (true, Some(ind)) =
        (entering_anisotropic && geo.is_biaxial, geo.hero_indicatrix)
    {
        let n2_hero_dir = if use_extraordinary {
            geo.n_biax_b_hero
        } else {
            geo.n_biax_a_hero
        };
        let eta_dir = geo.n1 / n2_hero_dir;
        let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
        let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
        let refr_wave_dir =
            (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();
        (
            refr_wave_dir,
            ind.mode_poynting_dir(refr_wave_dir, use_extraordinary),
        )
    } else {
        let n2_hero_dir = if entering_anisotropic && !use_extraordinary {
            geo.n_o_hero
        } else {
            geo.n2
        };
        let eta_dir = geo.n1 / n2_hero_dir;
        let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
        let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
        let refr_wave_dir =
            (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();

        let s = if entering_anisotropic && use_extraordinary {
            BirefringenceParams::extraordinary_poynting_dir(
                refr_wave_dir,
                c_axis,
                geo.n_o_hero,
                geo.n_e_hero,
            )
        } else {
            refr_wave_dir
        };
        (refr_wave_dir, s)
    };

    let decision = RefractDecision {
        entering_anisotropic,
        use_extraordinary,
        entry_mode_azimuth2,
        final_refr_dir,
        r_unpol,
    };
    let mut dirs = [None; NUM_CHANNELS];
    let mut hero_match = [false; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let (dir_k, matches_hero) =
            apply_refract_channel(bctx, k, ray, inside_gem, &decision, state, exit_event);
        dirs[k] = dir_k;
        hero_match[k] = matches_hero;
    }
    // An interior dispersive event (an entry into the gem) narrows every channel's MIS
    // family; the exit event never does -- see `narrow_compat`'s doc comment.
    if exit_event.exit.enabled && !inside_gem {
        narrow_compat(&mut exit_event.exit.compat, &dirs, ctx.hero_idx, hero_match);
    }

    RefractBounceOutcome {
        new_k,
        new_s: final_refr_dir,
        is_extraordinary_update: entering_anisotropic.then_some(use_extraordinary),
    }
}

/// Every channel's shared per-bounce refraction decision -- computed once by
/// [`apply_refract_bounce`] and reused, unchanged, by every one of
/// [`apply_refract_channel`]'s `NUM_CHANNELS` per-channel calls.
#[derive(Clone, Copy)]
struct RefractDecision {
    entering_anisotropic: bool,
    use_extraordinary: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
    final_refr_dir: Vec3,
    r_unpol: f32,
}

/// Channel k's own refracted-direction geometry -- the fields
/// [`compute_channel_refraction_geometry`] resolves.
struct ChannelRefractionGeometry {
    n1k: f32,
    n2k: f32,
    cos_t_k: f32,
    refr_wave_dir_k: Vec3,
    final_dir_k: Vec3,
}

/// The direction-independent half of [`apply_refract_channel`]'s per-channel work,
/// split out purely to keep that function's own body under the workspace line-count
/// lint. Returns `None` when channel k cannot physically transmit at this angle even
/// though the hero-driven path did (`sin2_t_k > 1.0`); the caller zeroes
/// `stokes[k]`/`path_pdf[k]` for that case itself, since this helper never touches
/// either array.
fn compute_channel_refraction_geometry(
    bctx: &BounceContext<'_, '_>,
    k: usize,
    ray: BounceRay,
    decision: &RefractDecision,
) -> Option<ChannelRefractionGeometry> {
    let (cache, geo) = (bctx.cache, bctx.geo);
    let BounceRay { k_hat, normal } = ray;
    let &RefractDecision {
        entering_anisotropic,
        use_extraordinary,
        ..
    } = decision;
    let material = bctx.ctx.material;
    let c_axis = bctx.ctx.c_axis;

    let n1k = geo.n1_ch[k];
    // geo.n2_ch[k] (== n_eff_ch[k] here) is the extraordinary-biased index used to
    // decide reflect vs. refract; only correct for this channel's transmission if the
    // extraordinary mode was actually selected. If the ordinary mode was selected
    // instead, this channel transmits at its own ordinary index geo.n_o_ch[k]. For a
    // biaxial material, use this channel's own biaxial mode-A index (already resolved
    // at the shared hero direction) instead of the uniaxial index.
    let n2k = if entering_anisotropic && !use_extraordinary {
        if geo.is_biaxial {
            geo.n_biax_a_ch[k]
        } else {
            geo.n_o_ch[k]
        }
    } else {
        geo.n2_ch[k]
    };
    let sin2_t_k = (n1k / n2k).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
    if sin2_t_k > 1.0 {
        // Channel k cannot physically transmit at this angle even though the
        // hero-driven path did. Correct and unbiased, not a bias to be corrected: its
        // reflect-branch contributions elsewhere are already correctly weighted by
        // the hero's own selection probability.
        return None;
    }

    let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();

    // The direction channel k's own technique would have taken through this same
    // interface, using k's own index -- including its own ordinary/extraordinary
    // Poynting-direction walk-off, since that is also wavelength-dependent. Refraction
    // is specular, so channel k's technique has positive density of having produced
    // the realized path only where this direction coincides with `final_refr_dir`.
    let eta_dir_k = n1k / n2k;
    let refr_wave_dir_k =
        (eta_dir_k * k_hat + f32::mul_add(eta_dir_k, geo.cos_i, -cos_t_k) * normal).normalize();
    // Channel k's own biaxial walk-off, using k's own indicatrix evaluated at k's own
    // single-shot refracted wave direction -- the per-channel generalization of the
    // uniaxial `extraordinary_poynting_dir` call below.
    let final_dir_k = if let (true, Some(ind_k)) =
        (entering_anisotropic && geo.is_biaxial, cache.biaxial_ch[k])
    {
        ind_k.mode_poynting_dir(refr_wave_dir_k, use_extraordinary)
    } else if entering_anisotropic && use_extraordinary {
        let n_e_k = geo.n_o_ch[k] + material.birefringence_delta;
        BirefringenceParams::extraordinary_poynting_dir(
            refr_wave_dir_k,
            c_axis,
            geo.n_o_ch[k],
            n_e_k,
        )
    } else {
        refr_wave_dir_k
    };

    Some(ChannelRefractionGeometry {
        n1k,
        n2k,
        cos_t_k,
        refr_wave_dir_k,
        final_dir_k,
    })
}

/// Inputs [`apply_channel_transmission_match`] needs from its caller's own bounce
/// geometry and refraction decision.
struct ChannelTransmitMatchInputs {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
}

/// The `direction_matches` branch of [`apply_refract_channel`]'s per-channel work,
/// split out purely to keep that function's own body under the workspace line-count
/// lint -- see this crate's doc comments there for the full physical rationale.
fn apply_channel_transmission_match(
    inputs: &ChannelTransmitMatchInputs,
    stokes_k: &mut StokesVector,
    path_pdf_k: &mut f32,
) {
    let &ChannelTransmitMatchInputs {
        n1k,
        n2k,
        cos_i,
        cos_t_k,
        r_unpol,
        entering_anisotropic,
        entry_mode_azimuth2,
    } = inputs;

    let t_s_k = (2.0 * n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k =
        MuellerMatrix::fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    // Project this channel's incident Stokes state onto the selected eigenmode
    // before transmission -- see `entry_eigenmode_selection`'s doc comment for the
    // full derivation. The transmitted state becomes fully linearly polarized
    // along the mode's own axis (`V` zeroed), at this channel's own unscaled
    // intensity: the mode-selection draw's probability matches this mode's true
    // physical energy fraction exactly, so multiplying by that fraction and
    // dividing by the identical selection probability cancel, leaving `i_k` itself
    // rather than a scaled-down share. `entry_mode_azimuth2` is `None` for a
    // biaxial material or when the incident light carries negligible linear
    // polarization -- in both cases `stokes[k]` passes through unprojected.
    let incident_k =
        if entering_anisotropic && let Some((cos_2psi_x, sin_2psi_x)) = entry_mode_azimuth2 {
            let i_k = stokes_k.i;
            StokesVector::new(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0)
        } else {
            *stokes_k
        };
    // No `/ split_pdf` here: at an anisotropic entry, `trans_matrix_k` is already
    // the full transmitted intensity for a beam at the selected mode's own index,
    // and that mode carries only its ~0.5 energy share of the incident light --
    // dividing by the 0.5 selection probability on top of that would double-count.
    *stokes_k = incident_k
        .apply_matrix(&trans_matrix_k)
        .scale(1.0 / (1.0 - r_unpol));

    // Channel k's own probability of choosing "transmit" at this interface, using
    // k's own unpolarized reflectance. For k == hero_idx this reproduces r_unpol
    // exactly only when `n2k` is computed from the same index the branch decision
    // in `apply_partial_fresnel_bounce` used; at an anisotropic entry where the
    // ordinary mode is selected instead, `n2k` is genuinely different, so
    // `r_unpol_k` is not `r_unpol` there -- still the physically correct
    // probability for the mode actually selected.
    let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k =
        (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
    // `path_pdf`'s role is `spectral_mis_weight`'s per-channel weight,
    // `N * path_pdf[hero] / sum(path_pdf)`, which is scale-invariant under
    // multiplying every channel's `path_pdf` by the same uniform factor -- so no
    // `* split_pdf` is needed here either.
    *path_pdf_k *= 1.0 - r_unpol_k;
}

/// Inputs [`apply_channel_chromatic_termination`] needs from its caller's own bounce
/// geometry and refraction decision.
struct ChannelMismatchInputs {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    entry_mode_azimuth2: Option<(f32, f32)>,
    inside_gem: bool,
    lambda_k: f32,
    refr_wave_dir_k: Vec3,
    original_stokes_k: StokesVector,
}

/// The chromatic-termination (`!direction_matches`) branch of [`apply_refract_channel`]'s
/// per-channel work, split out purely to keep that function's own body under the
/// workspace line-count lint -- see this crate's doc comments there for the full
/// physical rationale.
fn apply_channel_chromatic_termination(
    inputs: &ChannelMismatchInputs,
    k: usize,
    stokes_k: &mut StokesVector,
    path_pdf_k: &mut f32,
    exit_event: &mut ExitEvent<'_, '_>,
) {
    let &ChannelMismatchInputs {
        n1k,
        n2k,
        cos_i,
        cos_t_k,
        r_unpol,
        entering_anisotropic,
        entry_mode_azimuth2,
        inside_gem,
        lambda_k,
        refr_wave_dir_k,
        original_stokes_k,
    } = inputs;
    // Chromatic termination. Reached by a genuine interior mismatch (an
    // anisotropic entry's eigenmode direction, or -- via the degenerate
    // wave-normal-parallel-to-optic-axis fallback this function also serves -- an
    // internal reflection's direction, diverging from the hero's), or an exit
    // mismatch (`is_exit_event` below). Channel k's own reflect-branch
    // contributions elsewhere are already correctly weighted by the hero's own
    // selection probability, same as the `sin2_t_k > 1.0` early return above --
    // correct and unbiased, not a bias to be corrected.
    //
    // With splitting enabled, a mismatched channel loses only its radiance here --
    // its `path_pdf[k]` keeps accumulating (this event's own transmit factor, the
    // same `1 - r_unpol_k` the matching branch above folds in), because technique
    // k remains a live member of every compatible channel's MIS family (see
    // `ExitSplitCtx::compat`). At the exit event the channel additionally resolves
    // its own transmitted radiance along its own refracted direction via
    // `try_split_exit_channel`. With splitting disabled this is the plain
    // chromatic termination, bit for bit.
    let prefix_path_pdf_k = *path_pdf_k;
    *stokes_k = stokes_k.scale(0.0);
    *path_pdf_k = 0.0;

    let (exit, hit_point) = (&mut *exit_event.exit, exit_event.hit_point);
    if exit.enabled {
        // Leaving the gem back into air -- never true simultaneously with
        // `entering_anisotropic` (that flag requires `!inside_gem`).
        let is_exit_event = inside_gem && !entering_anisotropic;
        let (transmitted, r_unpol_k) = compute_channel_transmission(&ChannelTransmissionInputs {
            n1k,
            n2k,
            cos_i,
            cos_t_k,
            r_unpol,
            entering_anisotropic,
            entry_mode_azimuth2,
            incident_stokes_k: original_stokes_k,
        });
        *path_pdf_k = prefix_path_pdf_k * (1.0 - r_unpol_k);
        if is_exit_event && original_stokes_k.intensity() > 0.0 {
            try_split_exit_channel(
                exit,
                hit_point,
                k,
                lambda_k,
                refr_wave_dir_k,
                transmitted.intensity(),
            );
        }
    }
}

/// One channel's share of [`apply_refract_bounce`]'s per-channel loop -- see that
/// function's doc comment for the full rationale (chromatic termination, per-channel
/// path-pdf bookkeeping). Each channel `k` is fully independent of every other.
fn apply_refract_channel(
    bctx: &BounceContext<'_, '_>,
    k: usize,
    ray: BounceRay,
    inside_gem: bool,
    decision: &RefractDecision,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> (Option<Vec3>, bool) {
    let ctx = bctx.ctx;
    let &RefractDecision {
        entering_anisotropic,
        entry_mode_azimuth2,
        final_refr_dir,
        r_unpol,
        ..
    } = decision;
    let (stokes, path_pdf) = (&mut *state.stokes, &mut *state.path_pdf);
    // Captured before any mutation below, so the split branch (which needs the
    // original incident value after `stokes[k]` has already been zeroed) can still
    // compute k's own transmission -- see this function's `else` branch.
    let original_stokes_k = stokes[k];

    let Some(geometry) = compute_channel_refraction_geometry(bctx, k, ray, decision) else {
        stokes[k] = stokes[k].scale(0.0);
        path_pdf[k] = 0.0;
        return (None, false);
    };
    let ChannelRefractionGeometry {
        n1k,
        n2k,
        cos_t_k,
        refr_wave_dir_k,
        final_dir_k,
    } = geometry;

    let direction_matches = final_dir_k.dot(final_refr_dir) >= DIRECTION_MATCH_COS_TOL;

    if direction_matches {
        apply_channel_transmission_match(
            &ChannelTransmitMatchInputs {
                n1k,
                n2k,
                cos_i: bctx.geo.cos_i,
                cos_t_k,
                r_unpol,
                entering_anisotropic,
                entry_mode_azimuth2,
            },
            &mut stokes[k],
            &mut path_pdf[k],
        );
    } else {
        apply_channel_chromatic_termination(
            &ChannelMismatchInputs {
                n1k,
                n2k,
                cos_i: bctx.geo.cos_i,
                cos_t_k,
                r_unpol,
                entering_anisotropic,
                entry_mode_azimuth2,
                inside_gem,
                lambda_k: ctx.lambdas[k],
                refr_wave_dir_k,
                original_stokes_k,
            },
            k,
            &mut stokes[k],
            &mut path_pdf[k],
            exit_event,
        );
    }
    (Some(final_dir_k), direction_matches)
}
