//! Per-channel exit-event Fresnel transmission and the bounded exit-splitting probe --
//! see [`super`]'s "Exit-event spectral splitting" doc comment for the estimator these
//! support.

use super::{R_UNPOL_PDF_MAX, R_UNPOL_PDF_MIN, context::ExitSplitCtx};
use crate::optics::{
    polarization::{MuellerMatrix, StokesVector},
    raytracer::{
        camera::Ray, environment::sample_environment_channel, intersect::intersect_polyhedron_soa,
    },
};
use glam::Vec3;

/// Channel k's own Fresnel transmission at a refract/exit interface -- the same
/// per-channel computation [`super::reflect_refract::apply_refract_channel`]'s
/// matching-direction branch runs inline, factored out so the split branch (a channel
/// whose own direction diverges from the hero's) can compute the identical transmitted
/// Stokes state without duplicating the formula. Returns the transmitted state and
/// channel k's own unpolarized reflectance `r_unpol_k` (the matching branch's own
/// `path_pdf[k] *= 1.0 - r_unpol_k` factor; the mismatch branch applies this same
/// factor to `path_pdf[k]` when splitting is enabled -- see [`super`]'s top-of-file
/// doc comment, point 2).
/// The scalar physics inputs [`compute_channel_transmission`] needs -- a direct
/// extraction of `apply_refract_channel`'s own inline matching-branch body, nothing
/// added or bundled further.
pub(super) struct ChannelTransmissionInputs {
    pub(super) n1k: f32,
    pub(super) n2k: f32,
    pub(super) cos_i: f32,
    pub(super) cos_t_k: f32,
    pub(super) r_unpol: f32,
    pub(super) entering_anisotropic: bool,
    pub(super) entry_mode_azimuth2: Option<(f32, f32)>,
    pub(super) incident_stokes_k: StokesVector,
}

pub(super) fn compute_channel_transmission(
    inputs: &ChannelTransmissionInputs,
) -> (StokesVector, f32) {
    let &ChannelTransmissionInputs {
        n1k,
        n2k,
        cos_i,
        cos_t_k,
        r_unpol,
        entering_anisotropic,
        entry_mode_azimuth2,
        incident_stokes_k,
    } = inputs;
    let t_s_k = (2.0 * n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k =
        MuellerMatrix::fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    let incident_k =
        if entering_anisotropic && let Some((cos_2psi_x, sin_2psi_x)) = entry_mode_azimuth2 {
            let i_k = incident_stokes_k.i;
            StokesVector::new(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0)
        } else {
            incident_stokes_k
        };
    let transmitted = incident_k
        .apply_matrix(&trans_matrix_k)
        .scale(1.0 / (1.0 - r_unpol));
    let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k =
        (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
    (transmitted, r_unpol_k)
}

/// Resolves channel `k`'s own radiance contribution when its Snell-refracted direction
/// at an exit bounce (leaving the gem back into air) diverges from the direction the
/// hero-driven shared path actually took. `path_pdf[k]` is not this function's concern:
/// every non-TIR channel at an exit event gets its own local transmit factor folded
/// into `path_pdf[k]` by the caller before this call, whether or not the split below
/// finds an escaping ray. This function is purely the bounded fan-out mechanism for
/// `split_radiance`, called only for a channel that still carries radiance.
///
/// Callers compute channel k's own Fresnel transmission via
/// [`compute_channel_transmission`] and pass in its intensity, already fully resolved.
/// `stokes_k` has already been zeroed by the caller -- only `split_radiance` is touched
/// here. Bounded to exactly one extra intersection test, never recurses into the
/// general per-bounce dispatch:
///   - k's own ray escapes the gem (the common case): its contribution is folded
///     directly into `exit.split_radiance[k]` (`transmitted_intensity * env_spectral`,
///     the same product shape `accumulate_miss_radiance` uses for the shared path).
///   - k's own ray instead re-enters the (necessarily convex) gem: tracing further is
///     unbounded work this design declines, so nothing is added to `split_radiance` --
///     a pure energy-loss truncation, exactly like hitting `max_bounces` or a Russian
///     roulette kill partway through a sub-path, not a density/pdf event. Unbiased for
///     the same reason the `sin2_t_k > 1.0` TIR-mismatch case in
///     `apply_refract_channel` already is: channel `k`'s true contribution through
///     that longer sub-path is captured by other render samples, the ones that draw
///     `k` itself as hero.
pub(super) fn try_split_exit_channel(
    exit: &mut ExitSplitCtx<'_>,
    hit_point: Vec3,
    k: usize,
    lambda_k: f32,
    dir_k: Vec3,
    transmitted_intensity: f32,
) {
    let probe = Ray {
        origin: hit_point + dir_k * 1e-4,
        dir: dir_k,
    };
    if intersect_polyhedron_soa(probe, exit.plane_soa).is_some() {
        // Bounded re-entry: decline to trace further (energy-loss truncation only).
        return;
    }
    let env_spectral = sample_environment_channel(
        exit.environment,
        dir_k,
        lambda_k,
        exit.studio_rig.as_ref(),
        exit.observer,
    );
    // `split_mis_weight` is `1.0` outside a transmit-out event with a live incoming NEE
    // carry -- see `ExitSplitCtx::split_mis_weight`'s doc comment -- so this reproduces
    // the unweighted addition exactly whenever no competing light-sampling technique is
    // in play.
    exit.split_radiance[k] = f32::mul_add(
        transmitted_intensity.max(0.0) * exit.split_mis_weight,
        env_spectral,
        exit.split_radiance[k],
    );
}
