//! [`apply_partial_fresnel_bounce`], the top-level per-bounce dispatch: draws the
//! reflect-vs-transmit decision for the shared hero-driven path and routes to whichever
//! closed-form uniaxial path, or scalar-Fresnel fallback, applies.

use super::{
    R_UNPOL_SELECT_MAX, R_UNPOL_SELECT_MIN,
    context::{
        BounceContext, BounceRay, BounceState, ExitEvent, PathModeState, RayMaterialContext,
        RngDraw, UniaxialBounceContext,
    },
    geometry::{BounceRefractionGeometry, poynting_dir_for_mode},
    reflect_refract::{RefractSelection, apply_partial_reflect_bounce, apply_refract_bounce},
    uniaxial_entry::{apply_uniaxial_entry_bounce, entry_eigenmode_selection},
    uniaxial_internal::apply_uniaxial_internal_bounce,
};
use crate::optics::{
    polarization::StokesVector,
    raytracer::sampling::{BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, hash_u32},
};
use glam::Vec3;

/// [`apply_partial_fresnel_bounce`]'s own full result tuple: the new wave-normal `k`
/// and Poynting direction `S`, the new `inside_gem` state, the `is_extraordinary`
/// update (if any), and (mirroring [`super::reflect_refract::RefractBounceOutcome`]) a
/// reserved fifth slot.
/// Named purely so [`try_dispatch_uniaxial_bounce`]'s `Option`-wrapped return type
/// doesn't trip clippy's `type_complexity` lint.
type FresnelBounceResult = (Vec3, Vec3, bool, Option<bool>, Option<f32>);

/// Dispatches [`apply_partial_fresnel_bounce`]'s two closed-form uniaxial paths --
/// air->crystal entry and internal/exit -- split out purely to keep that function's
/// own body under the workspace line-count lint. Returns `Some` with the full result
/// tuple when one of the closed-form paths applies; `None` when the caller must fall
/// through to the general (biaxial/isotropic) machinery instead.
///
/// A uniaxial air->crystal entry whose wave normal is NOT parallel to the optic axis
/// (`k_hat.cross(c_axis).length_squared() > 1e-6`) takes the closed-form
/// `apply_uniaxial_entry_bounce` path in full, self-contained, and returns directly.
///
/// Any uniaxial internal event (o<->e coupled partial reflection or
/// uniaxial->isotropic exit transmission) that is not already hero-forced past
/// critical angle (that case is `apply_tir_bounce`'s own uniaxial branch, dispatched
/// from a different call site in `transport::dispatch_bounce`) takes the closed-form
/// `apply_uniaxial_internal_bounce` path the same way, under the SAME non-degenerate
/// guard -- see that function's own doc comment.
///
/// Both closed-form paths share this one degeneracy guard (the entry branch
/// used to special-case it internally via its own now-removed isotropic-Fresnel
/// fallback, reached by no split draw the general path below takes, which diverged
/// from the GPU's `uniaxial_nondegenerate`-gated 50/50 draw at this exact limit). The
/// remaining fallthrough case (`None`, `apply_partial_fresnel_bounce`'s general
/// `entry_eigenmode_selection`/scalar-Fresnel machinery) is reached by a biaxial
/// material, an isotropic one, or this SAME degenerate wave-normal-parallel-to-optic-axis
/// case for EITHER a uniaxial entry or a uniaxial internal bounce (`k_hat.cross(c_axis)`
/// ~ 0): the closed-form ordinary D-direction `k x c_axis` vanishes exactly there,
/// singularizing `internal_solve`'s boundary system (its own `flux_inc` floors to the
/// `1e-12` guard instead of the true nonzero value, which very nearly zeroed EVERY
/// uniaxial internal bounce along a c-axis-aligned ray -- caught by
/// `test_spectral_raytrace_colored_gem`'s ruby render going black) and, for entry,
/// `apply_uniaxial_entry_bounce`'s own boundary-match system the same way. At this
/// exact limit BOTH eigenmodes truly collapse to a single isotropic response at `n_o`
/// (`effective_extraordinary_index(n_o, n_e, theta_c=0) == n_o` exactly), so falling
/// through to the plain scalar isotropic-at-`n_o` machinery is not an approximation --
/// it is the exact physics, and (now) the SAME code path a degenerate entry and
/// a degenerate internal bounce both take, rather than two independently-maintained
/// isotropic special cases.
fn try_dispatch_uniaxial_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    entering_anisotropic: bool,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> Option<FresnelBounceResult> {
    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, .. } = ray;
    let PathModeState {
        inside_gem,
        is_extraordinary,
        ..
    } = mode_state;

    // Guarded by the SAME non-degenerate test (`k_hat` vs the optic axis, not
    // parallel) the internal-bounce branch below already uses -- a degenerate entry
    // (wave normal parallel to `c_axis`) singularizes `apply_uniaxial_entry_bounce`'s
    // own closed-form boundary-match system, so it declines (`None`) here and falls
    // through to `apply_partial_fresnel_bounce`'s general scalar-Fresnel path instead,
    // matching the GPU's `uniaxial_nondegenerate` gate (`08_bounce_step.wgsl`) exactly:
    // both sides now draw a plain (or polarization-weighted) ordinary/extraordinary
    // split through the same code path at this exact limit, rather than the CPU alone
    // taking a separate hand-rolled isotropic-Fresnel fallback no split draw ever
    // reached.
    if entering_anisotropic
        && let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        let (new_k, new_s, new_inside_gem, is_extraordinary_update) =
            apply_uniaxial_entry_bounce(&ubctx, ray, rng, state, exit_event.exit);
        return Some((new_k, new_s, new_inside_gem, is_extraordinary_update, None));
    }
    if inside_gem
        && let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        return Some(apply_uniaxial_internal_bounce(
            &ubctx,
            is_extraordinary,
            ray,
            rng,
            state,
            exit_event,
        ));
    }
    None
}

/// The inputs [`resolve_entry_mode_selection`] reads, bundled to keep its signature
/// short.
struct EntryModeSelectionInputs<'m, 'b> {
    ctx: &'b RayMaterialContext<'m>,
    geo: &'b BounceRefractionGeometry,
    entering_anisotropic: bool,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    hero_stokes: StokesVector,
    is_extraordinary: bool,
    rng_seed: u32,
    bounce: u32,
}

/// The polarization-weighted eigenmode selection [`apply_partial_fresnel_bounce`]
/// makes at an anisotropic entry -- split out purely to keep that function's own body
/// under the workspace line-count lint. Returns `(use_extraordinary,
/// entry_mode_azimuth2)`.
///
/// Mirrors `apply_refract_bounce`'s old `split_rand < 0.5` exactly when
/// `entry_selection` is `None` (`p_o == 0.5`: `1.0 - p_o == 0.5` too, same hash,
/// same threshold, same sense -- extraordinary iff `mode_split_rand < 0.5`);
/// weighted by polarization otherwise (extraordinary drawn with probability
/// `1 - p_o`, ordinary with probability `p_o`, as `p_o`'s own doc comment defines
/// it). For a non-entering bounce (internal TIR/reflect/refract, or any
/// isotropic-material refraction) this keeps whatever mode the path already
/// carries -- exactly the `entering_anisotropic` guard's else branch.
///
/// The chosen mode's own doubled polarization azimuth is for `apply_refract_channel`'s
/// eigenmode projection -- `None` when `entry_selection` itself was `None`, in which
/// case that projection is skipped entirely (see its own doc comment). Extraordinary
/// is perpendicular to ordinary (`psi_e == psi_o + 90 deg`), so its doubled azimuth
/// is simply the negation of the ordinary one (`cos(2*psi_o + 180deg) ==
/// -cos(2*psi_o)`, likewise for `sin`).
fn resolve_entry_mode_selection(
    inputs: &EntryModeSelectionInputs<'_, '_>,
) -> (bool, Option<(f32, f32)>) {
    let &EntryModeSelectionInputs {
        ctx,
        geo,
        entering_anisotropic,
        current_plane_normal,
        k_hat,
        hero_stokes,
        is_extraordinary,
        rng_seed,
        bounce,
    } = inputs;
    let entry_selection = if entering_anisotropic && !geo.is_biaxial {
        entry_eigenmode_selection(ctx.c_axis, current_plane_normal, k_hat, hero_stokes)
    } else {
        None
    };
    let p_o = entry_selection.map_or(0.5, |(p, ..)| p);
    let mode_split_rand = (hash_u32(rng_seed ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))
        as f32)
        / 4_294_967_295.0;
    let use_extraordinary = if entering_anisotropic {
        mode_split_rand < (1.0 - p_o)
    } else {
        is_extraordinary
    };
    let entry_mode_azimuth2 = entry_selection.map(|(_, cos_2psi_o, sin_2psi_o)| {
        if use_extraordinary {
            (-cos_2psi_o, -sin_2psi_o)
        } else {
            (cos_2psi_o, sin_2psi_o)
        }
    });
    (use_extraordinary, entry_mode_azimuth2)
}

/// The reflect-vs-transmit SELECTION probability [`apply_partial_fresnel_bounce`]
/// draws against -- split out purely to keep that function's own body under the
/// workspace line-count lint.
///
/// Use the SELECTED mode's own index for n2 at THIS interface -- `geo.n2` (mode
/// B/extraordinary) unchanged when extraordinary is selected, not entering an
/// anisotropic material at all, or the material is biaxial (`geo.n_o_hero` is a
/// uniaxial-only quantity, not mode A -- see `entry_eigenmode_selection`'s doc
/// comment), `geo.n_o_hero` when the ordinary mode is selected instead. `geo.n1`/
/// `geo.cos_i` are unaffected: `n1 == 1.0` (air) on every path this function can
/// reach (`entering_anisotropic` requires `!inside_gem`).
///
/// `sin2_t` is bit-identical to the earlier `(1.0 - geo.sin2_t).sqrt()` whenever `n2
/// == geo.n2` (every case that isn't a fresh ordinary-mode selection): reuses
/// `geo.sin2_t` itself rather than re-deriving an algebraically-equivalent value from
/// `n1`/`n2`/`cos_i`, since a DIFFERENT sequence of floating-point operations
/// computing "the same" mathematical quantity is not guaranteed (and, empirically, is
/// not) bit-identical to `geo.sin2_t`'s own `eta * eta * cos_i.mul_add(-cos_i, 1.0)`.
/// Only genuinely recomputed (at `n_o_hero` instead of `geo.n2`, via the exact same
/// expression shape `compute_bounce_refraction_geometry` uses) when the ordinary
/// mode was actually selected.
///
/// The reflect/transmit SELECTION probability's clamp (`min`/`max`) is distinct from
/// the per-channel `r_unpol_k` clamps elsewhere in this file (which only scale
/// `path_pdf`, never divide `stokes` directly). `[0.02, 0.98]` rather than
/// `[1e-4, 1-1e-4]` caps the `1/r_unpol`/`1/(1-r_unpol)` divisions
/// (`apply_partial_reflect_bounce`/`apply_refract_channel`) at 50x instead of
/// 10,000x at grazing incidence, where the unclamped raw value genuinely approaches 0
/// or 1 -- still unbiased (the contribution is weighted by the TRUE `R`/`T` divided by
/// this same `p`), just far less firefly-prone.
fn compute_entry_reflect_probability(
    geo: &BounceRefractionGeometry,
    entering_anisotropic: bool,
    use_extraordinary: bool,
    min: f32,
    max: f32,
) -> f32 {
    let ordinary_selected = entering_anisotropic && !geo.is_biaxial && !use_extraordinary;
    let n2 = if ordinary_selected {
        geo.n_o_hero
    } else {
        geo.n2
    };
    let sin2_t = if ordinary_selected {
        let eta = geo.n1 / n2;
        eta * eta * geo.cos_i.mul_add(-geo.cos_i, 1.0)
    } else {
        geo.sin2_t
    };
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_s =
        f32::mul_add(n2, -cos_t, geo.n1 * geo.cos_i) / f32::mul_add(n2, cos_t, geo.n1 * geo.cos_i);
    let r_p =
        f32::mul_add(geo.n1, -cos_t, n2 * geo.cos_i) / f32::mul_add(geo.n1, cos_t, n2 * geo.cos_i);

    let r_unpol_raw = 0.5 * r_p.mul_add(r_p, r_s * r_s);
    r_unpol_raw.clamp(min, max)
}

/// Partial Fresnel Reflection & Refraction via Stokes-Mueller Polarized Wave Transport:
/// decides reflect vs. transmit for the shared hero-driven path from the hero's own
/// `r_unpol` (a well-mixed hash of `(rng_seed, bounce)`, replacing an earlier
/// deterministic `(rng_seed + bounce*7919) % 1000` arithmetic progression that could
/// correlate with the Russian-roulette draw; this hash is decorrelated from it via a
/// distinct salt), then applies whichever event was sampled via
/// [`apply_partial_reflect_bounce`] or [`apply_refract_bounce`]. A direct extraction of
/// the pre-extraction inline branch dispatch: the same floating-point operations, in
/// the same order, driven by the same random draw. Returns the new wave normal `k'`,
/// the new Poynting direction `S'` (reflection's `k'` is re-converted to `S'` via
/// [`poynting_dir_for_mode`] here, using the mode still in effect -- reflection alone
/// never changes `is_extraordinary`, see `maybe_apply_internal_mode_coupling`'s own doc
/// comment for the SEPARATE relabeling step that may reassign it for the NEXT bounce),
/// the new `inside_gem` state, and (mirroring [`super::reflect_refract::RefractBounceOutcome`])
/// the `is_extraordinary` update, if any.
///
/// At an air->crystal entry into an anisotropic material, which eigenmode
/// (ordinary/mode-A vs extraordinary/mode-B) this path's single geometric transmission
/// event represents is decided HERE -- before the reflect-vs-transmit draw below,
/// rather than inside `apply_refract_bounce`'s own transmit branch.
/// Two reasons this has to happen up here:
///   - The SELECTION itself is weighted by the incident polarization's projection onto
///     each eigenmode's own axis ([`entry_eigenmode_selection`]), not a blanket 50/50 --
///     and that weighting has to happen exactly once per bounce, shared by both
///     branches below: a beam already aligned with the ordinary axis should be MORE
///     likely to reflect at the ordinary index too, not just more likely to transmit
///     as ordinary conditional on transmitting.
///   - The REFLECT branch's own Fresnel coefficients should be evaluated at the SAME
///     mode's index the transmit branch (`apply_refract_channel`) uses for this
///     channel. For a uniaxial material they are ([`compute_entry_reflect_probability`]
///     substitutes `n_o_hero` when the ordinary mode is selected), so `R + T == 1`
///     for whichever mode this draw selects; always
///     using mode B (extraordinary)'s index for the reflect/transmit decision would
///     instead leave `R + T != 1` by `O(delta_n / n)` whenever the ordinary mode is
///     selected.
///
/// A biaxial material has no uniaxial "ordinary" eigenmode to weight against, so both
/// the selection and the index correction are gated on `!geo.is_biaxial`: a biaxial
/// entry always uses a blanket 50/50 split. Mode B's index (`geo.n2`) drives the
/// reflect/transmit decision while transmission uses the index of the mode the split
/// selects, so energy is conserved only on average there: with `R_A`/`R_B` the
/// unpolarized Fresnel reflectances at the two modes' indices, `R + T` is
/// `R_B + (1 - R_sel)`, whose expectation over the 50/50 selection is
/// `1 + 0.5 * (R_B - R_A)`, not 1.
///
/// Non-degenerate uniaxial entries and internal events do not reach the code below
/// whenever a closed-form path applies: [`try_dispatch_uniaxial_bounce`] takes them
/// first.
pub(in crate::optics::raytracer) fn apply_partial_fresnel_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> (Vec3, Vec3, bool, Option<bool>, Option<f32>) {
    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, normal } = ray;
    let PathModeState {
        current_plane_normal,
        inside_gem,
        is_extraordinary,
    } = mode_state;
    let RngDraw { rng_seed, bounce } = rng;

    let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
    if let Some(result) = try_dispatch_uniaxial_bounce(
        bctx,
        ray,
        mode_state,
        entering_anisotropic,
        rng,
        state,
        exit_event,
    ) {
        return result;
    }

    let (use_extraordinary, entry_mode_azimuth2) =
        resolve_entry_mode_selection(&EntryModeSelectionInputs {
            ctx,
            geo,
            entering_anisotropic,
            current_plane_normal,
            k_hat,
            hero_stokes: state.stokes[ctx.hero_idx],
            is_extraordinary,
            rng_seed,
            bounce,
        });

    let r_unpol = compute_entry_reflect_probability(
        geo,
        entering_anisotropic,
        use_extraordinary,
        R_UNPOL_SELECT_MIN,
        R_UNPOL_SELECT_MAX,
    );
    let rng_bounce =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM)) as f32) / 4_294_967_295.0;

    if rng_bounce < r_unpol {
        let new_k =
            apply_partial_reflect_bounce(geo, r_unpol, k_hat, normal, state.stokes, state.path_pdf);
        let new_s = poynting_dir_for_mode(ctx, geo, new_k, inside_gem, is_extraordinary);
        (new_k, new_s, inside_gem, None, None)
    } else {
        let selection = RefractSelection {
            use_extraordinary,
            entry_mode_azimuth2,
        };
        let outcome =
            apply_refract_bounce(bctx, r_unpol, ray, inside_gem, selection, state, exit_event);
        (
            outcome.new_k,
            outcome.new_s,
            !inside_gem,
            outcome.is_extraordinary_update,
            None,
        )
    }
}
