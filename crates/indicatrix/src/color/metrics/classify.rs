//! Per-aperture-sample ray classification: brilliance/windowing/extinction
//! bucketing at the d-line, plus the Fire (F-line/C-line) measurement and its
//! bifurcation gate.

use glam::Vec3;

use super::{
    fan::FanGeometry,
    lighting::ExitLighting,
    ray_trace::{EntryRay, RayFate, StoneArena, trace_entry},
    visibility::returned_or_reason,
};
use crate::optics::raytracer::Ray;

/// Per-evaluation context threaded through [`classify_aperture_sample`]: everything
/// needed to fire and classify one grid-cell aperture-sample ray at the fixed camera
/// pose, bundled to keep that function's argument count within clippy's
/// `too_many_arguments` lint -- identical across all
/// `grid_size * grid_size * aperture_samples.len()` calls per invocation. Mirrors
/// [`super::scintillation::TemporalPoseContext`]'s reason for existing, one level up.
pub(super) struct ApertureSampleContext<'a> {
    pub(super) stone: StoneArena<'a>,
    pub(super) nd: f32,
    pub(super) n_f: f32,
    pub(super) n_c: f32,
    pub(super) cam_forward: Vec3,
    pub(super) cam_right: Vec3,
    pub(super) cam_up: Vec3,
    /// Where the fan's rays start, scaled to the stone.
    pub(super) fan: FanGeometry,
    /// The illumination an exit direction is judged against, shared by every pose of a
    /// sweep (built once, see `PreparedEval`).
    pub(super) lighting: &'a ExitLighting<'a>,
    /// Whether returned rays also get the Fire F/C companion traces. Off for the sweep and
    /// the optimiser, which read only brilliance/extinction/windowing: those counters
    /// come from the d-line trace alone, so they are identical either way.
    pub(super) want_fire: bool,
}

/// How one grid-cell aperture-sample ray was classified. `Returned` carries the
/// qualifying Fire sample's raw `(angle_deg, weight)` pair, if any -- deliberately not
/// folded into the Fire accumulator here, so the caller applies the same `f32::mul_add`
/// chain against its own running total in a fixed iteration order (folding it in here
/// instead would change which intermediate values a fused multiply-add rounds against,
/// perturbing the bit-exact result).
pub(super) enum RayClassification {
    /// Non-finite diagnostic path; retained as unknown, never extinction.
    Invalid,
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
    Returned(ReturnedRay),
}

/// What a visibly returned ray hands to the caller's accumulators.
///
/// `fire` is the qualifying Fire `(angle_deg, weight)` pair (see [`RayClassification`]);
/// `transmittance` and `path_len` are the d-line trace's Fresnel weight and total internal
/// path (model units), the two numbers the face-up tone mixes Beer-Lambert terms from.
#[derive(Clone, Copy)]
pub(super) struct ReturnedRay {
    pub(super) fire: Option<(f32, f32)>,
    pub(super) transmittance: f32,
    pub(super) path_len: f32,
}

/// Fires a single grid-cell aperture-sample ray at screen-space coordinates `(u, v)`
/// (in `[-1, 1]`) with sub-aperture jitter `(sub_right, sub_up)`, refracts and traces it
/// through the stone at the d-line (and, if it qualifies, replays the same entry
/// point/direction at the F-line and C-line indices for the Fire measurement), and
/// classifies the result. Returns `None` if the ray misses the stone geometry entirely.
/// See [`RayClassification`]'s doc for why the Fire accumulator is threaded through the
/// caller rather than updated in here.
pub(super) fn classify_aperture_sample(
    ctx: &ApertureSampleContext,
    u: f32,
    v: f32,
    sub_right: f32,
    sub_up: f32,
) -> Option<RayClassification> {
    let ray_dir = (ctx.cam_forward + sub_right * ctx.cam_right + sub_up * ctx.cam_up).normalize();
    let ray_origin = ctx
        .fan
        .origin(ctx.cam_forward, ctx.cam_right, ctx.cam_up, u, v);
    let ray = Ray {
        origin: ray_origin,
        dir: ray_dir,
    };

    // The fan's rays run along the camera axis, so the eye is straight back along it.
    classify_ray(ctx, ray, -ctx.cam_forward, None)
}

/// Shared classification of an already generated camera or fan ray.
///
/// `observer` is the unit direction from the stone towards the eye that the head-shadow cone
/// is centred on. The legacy fan rays are parallel to the camera axis, so they pass
/// `-ctx.cam_forward` (bit-identical to the former `(-exit_dir) . cam_forward` test); the
/// perspective image path passes `-ray.dir`, the eye direction of its own pixel, exactly as
/// the beauty render does (`bounce_body.rs`, `observer`).
pub(super) fn classify_ray(
    ctx: &ApertureSampleContext,
    ray: Ray,
    observer: Vec3,
    mut detail: Option<&mut super::image_types::RayDetail>,
) -> Option<RayClassification> {
    let hit = ctx.stone.intersect(ray);
    let hit_rec = hit?;

    if let Some(d) = detail.as_deref_mut() {
        d.entry_facet = Some(hit_rec.facet_idx);
    }
    let hit_point_entry = ray.origin + hit_rec.t * ray.dir;
    if detail.is_some()
        && (!hit_rec.t.is_finite() || !hit_point_entry.is_finite() || !hit_rec.normal.is_finite())
    {
        if let Some(d) = detail.as_deref_mut() {
            d.reason = super::image_types::ExtinctionReason::Invalid;
        }
        return Some(RayClassification::Invalid);
    }
    let n_entry = hit_rec.normal;

    // Refract from air (1.0) into gemstone (nd) using Snell's Law
    let cos_i = (-ray.dir).dot(n_entry).clamp(0.0, 1.0);

    let mut trace_detail = super::image_types::RayDetail::default();
    let entry = EntryRay {
        point: hit_point_entry,
        dir: ray.dir,
        normal: n_entry,
        cos_i,
        facet: hit_rec.facet_idx,
    };
    let fate = trace_entry(
        entry,
        ctx.stone,
        ctx.nd,
        detail.as_ref().map(|_| &mut trace_detail),
    );
    if let Some(d) = detail.as_deref_mut() {
        d.exit_facet = trace_detail.exit_facet;
        d.bounces = trace_detail.bounces;
        d.reason = trace_detail.reason;
    }
    if trace_detail.reason == super::image_types::ExtinctionReason::Invalid {
        return Some(RayClassification::Invalid);
    }
    match fate {
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
            // light sources (see `returned_or_reason`).
            if let Err(reason) = returned_or_reason(exit_dir, observer, ctx.lighting) {
                if let Some(d) = detail {
                    d.reason = reason;
                }
                return Some(RayClassification::Extinct);
            }

            // The Fire measurement costs two more full traces per returned ray; the sweep
            // and the optimiser never read it, so they switch it off (`want_fire`).
            let fire = if ctx.want_fire {
                fire_sample(ctx, entry, transmittance)
            } else {
                None
            };
            Some(RayClassification::Returned(ReturnedRay {
                fire,
                transmittance,
                path_len: exit_d.path_len,
            }))
        }
        RayFate::Absorbed => Some(RayClassification::Extinct),
    }
}

/// The Fire sample of one visibly returned ray: `(angle_deg, weight)`, or `None` when the
/// F-line/C-line companion traces do not both exit upward through the same facet.
///
/// A pure extraction of the tail of [`classify_aperture_sample`] (same operations, same
/// order), so the Fire-less fast path can skip the two companion traces without touching
/// any arithmetic the d-line counters depend on. `transmittance` is the d-line trace's.
fn fire_sample(
    ctx: &ApertureSampleContext,
    entry: EntryRay,
    transmittance: f32,
) -> Option<(f32, f32)> {
    // Fire: replay the same entry point/direction at the hydrogen F-line and
    // C-line indices, for rays that pass the same illumination test as
    // brilliance. An unweighted mean over surviving rays would let a
    // badly-leaking cut win via a handful of near-critical-angle survivors, so
    // each ray's angular contribution is weighted by transmittance (energy
    // actually delivered) and normalized by TOTAL incident rays, not just the
    // qualifying count (see fire_index below). Considered only if BOTH F-line
    // and C-line companion traces also exit upward -- a ray whose F or C image
    // is lost to TIR or pavilion leakage contributes no Fire sample.
    let fate_f = trace_entry(entry, ctx.stone, ctx.n_f, None);
    let fate_c = trace_entry(entry, ctx.stone, ctx.n_c, None);
    let (RayFate::ExitedUpward(exit_f), RayFate::ExitedUpward(exit_c)) = (fate_f, fate_c) else {
        return None;
    };
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
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    fn signature(value: Option<&RayClassification>) -> (u8, Option<[u32; 4]>) {
        match value {
            None => (0, None),
            Some(RayClassification::EntryBlocked) => (1, None),
            Some(RayClassification::Windowed) => (2, None),
            Some(RayClassification::Extinct) => (3, None),
            Some(RayClassification::Invalid) => (5, None),
            Some(RayClassification::Returned(r)) => (
                4,
                r.fire.map(|(a, w)| {
                    [
                        a.to_bits(),
                        w.to_bits(),
                        r.transmittance.to_bits(),
                        r.path_len.to_bits(),
                    ]
                }),
            ),
        }
    }
    /// The legacy fan (`classify_aperture_sample`) judges the head-shadow cone around the
    /// camera axis. Passing `-ctx.cam_forward` as the observer to the shared `classify_ray`
    /// reproduces it bit for bit, under a studio rig and under lit models with the default
    /// and a wide cone, so every metric score of the legacy callers is unchanged by the
    /// per-pixel observer of the image path.
    #[test]
    fn identical_fan_rays_have_identical_class_and_fire_bits() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let camera = crate::optics::raytracer::Camera::new(0.6, 0.45, 2.4, 42.0);
        let mut extinct = 0;
        let mut returned = 0;
        for (preset, head_shadow) in [
            (LightingPreset::RingLights, 16.0),
            (LightingPreset::LightTent, 16.0),
            (LightingPreset::LightTent, 30.0),
            (LightingPreset::IsoHemisphere, 0.0),
        ] {
            let prepared = super::super::prepared::PreparedEval::new(
                StoneGeometry::planes_only(&planes),
                &material,
                preset.studio(1.0, 0.85, 0.95).with_head_shadow(head_shadow),
            )
            .unwrap();
            let ctx = ApertureSampleContext {
                stone: prepared.stone(),
                nd: prepared.nd,
                n_f: prepared.n_f,
                n_c: prepared.n_c,
                cam_forward: camera.forward,
                cam_right: camera.right,
                cam_up: camera.up,
                fan: prepared.fan,
                lighting: &prepared.lighting,
                want_fire: true,
            };
            for u in [-0.7, 0.0, 0.4] {
                for v in [-0.6, 0.0, 0.5] {
                    for (sub_right, sub_up) in [(0.01, 0.02), (-0.03, 0.0), (0.0, -0.02)] {
                        let ray = Ray {
                            origin: ctx.fan.origin(
                                ctx.cam_forward,
                                ctx.cam_right,
                                ctx.cam_up,
                                u,
                                v,
                            ),
                            dir: (ctx.cam_forward
                                + sub_right * ctx.cam_right
                                + sub_up * ctx.cam_up)
                                .normalize(),
                        };
                        let mut detail = super::super::image_types::RayDetail::default();
                        let legacy = classify_aperture_sample(&ctx, u, v, sub_right, sub_up);
                        extinct += usize::from(matches!(legacy, Some(RayClassification::Extinct)));
                        returned +=
                            usize::from(matches!(legacy, Some(RayClassification::Returned(_))));
                        assert_eq!(
                            signature(legacy.as_ref()),
                            signature(
                                classify_ray(&ctx, ray, -ctx.cam_forward, Some(&mut detail))
                                    .as_ref()
                            )
                        );
                    }
                }
            }
        }
        assert!(
            extinct > 0 && returned > 0,
            "{extinct} extinct, {returned} returned"
        );
    }
}
