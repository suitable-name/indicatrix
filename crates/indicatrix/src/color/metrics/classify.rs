//! Per-aperture-sample ray classification: brilliance/windowing/extinction
//! bucketing at the d-line, plus the Fire (F-line/C-line) measurement and its
//! bifurcation gate.

use glam::Vec3;

use super::{
    fan::FanGeometry,
    lighting::ExitLighting,
    ray_trace::{RayFate, trace_wavelength},
    visibility::ray_is_visibly_returned,
};
use crate::optics::raytracer::{Ray, intersect_polyhedron_soa};

/// Per-evaluation context threaded through [`classify_aperture_sample`]: everything
/// needed to fire and classify one grid-cell aperture-sample ray at the fixed camera
/// pose, bundled to keep that function's argument count within clippy's
/// `too_many_arguments` lint -- identical across all
/// `grid_size * grid_size * aperture_samples.len()` calls per invocation. Mirrors
/// [`super::scintillation::TemporalPoseContext`]'s reason for existing, one level up.
pub(super) struct ApertureSampleContext<'a> {
    pub(super) plane_soa: &'a crate::simd::PlanesSoA32,
    pub(super) nd: f32,
    pub(super) n_f: f32,
    pub(super) n_c: f32,
    pub(super) cam_forward: Vec3,
    pub(super) cam_right: Vec3,
    pub(super) cam_up: Vec3,
    /// Where the fan's rays start, scaled to the stone.
    pub(super) fan: FanGeometry,
    /// The illumination an exit direction is judged against.
    pub(super) lighting: ExitLighting<'a>,
}

/// How one grid-cell aperture-sample ray was classified. `Returned` carries the
/// qualifying Fire sample's raw `(angle_deg, weight)` pair, if any -- deliberately not
/// folded into the Fire accumulator here, so the caller applies the same `f32::mul_add`
/// chain against its own running total in a fixed iteration order (folding it in here
/// instead would change which intermediate values a fused multiply-add rounds against,
/// perturbing the bit-exact result).
pub(super) enum RayClassification {
    /// Not classified into any bucket. See `RayFate::EntryBlocked` doc.
    EntryBlocked,
    /// Leaked out through the pavilion bottom.
    Windowed,
    /// Trapped internally, absorbed, or exited but not visibly returned to the
    /// observer (head-shadowed or unlit).
    Extinct,
    /// Exited upward and was visibly returned to the observer. Carries the Fire
    /// `(angle_deg, weight)` pair if the F-line/C-line companion traces also both
    /// exited upward through the same facet after the same number of bounces (the F/C
    /// bifurcation gate -- see the comment on the gate itself, below).
    Returned(Option<(f32, f32)>),
}

/// Fires a single grid-cell aperture-sample ray at screen-space coordinates `(u, v)`
/// (in `[-1, 1]`) with sub-aperture jitter `(dx_sub, dz_sub)`, refracts and traces it
/// through the stone at the d-line (and, if it qualifies, replays the same entry
/// point/direction at the F-line and C-line indices for the Fire measurement), and
/// classifies the result. Returns `None` if the ray misses the stone geometry entirely.
/// See [`RayClassification`]'s doc for why the Fire accumulator is threaded through the
/// caller rather than updated in here.
pub(super) fn classify_aperture_sample(
    ctx: &ApertureSampleContext,
    u: f32,
    v: f32,
    dx_sub: f32,
    dz_sub: f32,
) -> Option<RayClassification> {
    let ray_dir = (ctx.cam_forward + dx_sub * ctx.cam_right + dz_sub * ctx.cam_up).normalize();
    let ray_origin = ctx
        .fan
        .origin(ctx.cam_forward, ctx.cam_right, ctx.cam_up, u, v);
    let ray = Ray {
        origin: ray_origin,
        dir: ray_dir,
    };

    let hit = intersect_polyhedron_soa(ray, ctx.plane_soa);
    let hit_rec = hit?;

    let hit_point_entry = ray.origin + hit_rec.t * ray.dir;
    let n_entry = hit_rec.normal;

    // Refract from air (1.0) into gemstone (nd) using Snell's Law
    let cos_i = (-ray.dir).dot(n_entry).clamp(0.0, 1.0);

    match trace_wavelength(
        hit_point_entry,
        ray.dir,
        n_entry,
        cos_i,
        ctx.plane_soa,
        ctx.nd,
    ) {
        RayFate::EntryBlocked => Some(RayClassification::EntryBlocked),
        RayFate::Leaked => Some(RayClassification::Windowed),
        // The d-line's own exit cosine is deliberately not used as a Fire weight: it
        // was tried and made no material difference to the emerald-cut ordering
        // problem, since it isn't tied to the diagnosed mechanism (F/C exit-angle
        // divergence, not the d-line ray's own exit angle).
        RayFate::ExitedUpward(exit_d) => {
            let exit_dir = exit_d.dir;
            let transmittance = exit_d.transmittance;
            // Directional Extinction Analysis: head-shadow cone vs. the environment's
            // light sources (see `ray_is_visibly_returned`).
            if !ray_is_visibly_returned(exit_dir, ctx.cam_forward, &ctx.lighting) {
                return Some(RayClassification::Extinct);
            }

            // Fire: replay the same entry point/direction at the hydrogen F-line and
            // C-line indices, for rays that pass the same illumination test as
            // brilliance. An unweighted mean over surviving rays would let a
            // badly-leaking cut win via a handful of near-critical-angle survivors, so
            // each ray's angular contribution is weighted by transmittance (energy
            // actually delivered) and normalized by TOTAL incident rays, not just the
            // qualifying count (see fire_index below). Considered only if BOTH F-line
            // and C-line companion traces also exit upward -- a ray whose F or C image
            // is lost to TIR or pavilion leakage contributes no Fire sample.
            let fate_f = trace_wavelength(
                hit_point_entry,
                ray.dir,
                n_entry,
                cos_i,
                ctx.plane_soa,
                ctx.n_f,
            );
            let fate_c = trace_wavelength(
                hit_point_entry,
                ray.dir,
                n_entry,
                cos_i,
                ctx.plane_soa,
                ctx.n_c,
            );
            let Some(fire) = (match (fate_f, fate_c) {
                (RayFate::ExitedUpward(exit_f), RayFate::ExitedUpward(exit_c)) => {
                    // F/C bifurcation gate: when the F-line and C-line traces exit
                    // through a different facet or bounce count, their critical angles
                    // straddled a TIR threshold at different points -- physically
                    // disjoint paths, so acos(dir_f . dir_c) would measure unrelated
                    // exit directions, not dispersion. These bifurcated pairs carried
                    // 45-98% of the weighted Fire sum wherever a step cut wrongly
                    // out-scored a brilliant -- a sampling artifact, not a real optical
                    // effect. Rejected a capped-credit alternative (a small fixed
                    // separation instead of discarding): it put Quartz above Sapphire
                    // on the emerald cut, when strict-discard matches independently
                    // measured SRB reference readings.
                    if exit_f.facet_idx != exit_c.facet_idx || exit_f.bounces != exit_c.bounces {
                        None
                    } else {
                        let transmittance_f = exit_f.transmittance;
                        let transmittance_c = exit_c.transmittance;
                        let exit_cos_f = exit_f.exit_cos_theta;
                        let exit_cos_c = exit_c.exit_cos_theta;
                        let cos_sep = exit_f.dir.dot(exit_c.dir).clamp(-1.0, 1.0);
                        let angle_deg = cos_sep.acos().to_degrees();
                        // Weight by the PRODUCT of all three wavelengths' transmittances,
                        // not just the d-line's: near a critical angle, angular
                        // sensitivity to a small index change diverges at almost the same
                        // rate the d-line's Fresnel transmittance vanishes, so
                        // angle * transmittance_d alone plateaus at a nonzero residual.
                        // Multiplying in transmittance_f/_c breaks that near-cancellation,
                        // so a ray only registers strongly when all three wavelengths are
                        // comfortably transmitted.
                        //
                        // Transmittance alone is still not enough: acos(dir_f . dir_c)
                        // has a near-singularity at grazing exit, which low-index stones
                        // hit more often, and transmittance falls off too slowly to
                        // outrun it. The missing factor is radiance: light leaving at
                        // grazing incidence is smeared across a larger solid angle before
                        // reaching a fixed-aperture observer, so the F-line/C-line exit
                        // cosines are multiplied in too -- at grazing, exit_cos -> 0 and
                        // cancels the acos divergence.
                        let energy_weight = transmittance * transmittance_f * transmittance_c;
                        let radiance_weight = exit_cos_f.max(0.0) * exit_cos_c.max(0.0);
                        let weight = energy_weight * radiance_weight;
                        Some((angle_deg, weight))
                    }
                }
                _ => None,
            }) else {
                return Some(RayClassification::Returned(None));
            };
            Some(RayClassification::Returned(Some(fire)))
        }
        RayFate::Absorbed => Some(RayClassification::Extinct),
    }
}
