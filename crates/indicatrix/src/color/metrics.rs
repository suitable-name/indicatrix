use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Ray, build_plane_soa, intersect_polyhedron_soa},
    },
};
use glam::Vec3;

/// Builds the observer-PoV view basis (forward, right, up) from camera yaw/pitch.
///
/// Uses the exact same convention as `Camera::new` in `optics::raytracer` (same
/// `world_up` fallback threshold and axis), so that gemological metrics are evaluated
/// against the same frame that is actually rendered.
#[must_use]
pub fn camera_view_basis(cam_yaw: f32, cam_pitch: f32) -> (Vec3, Vec3, Vec3) {
    let cos_cp = cam_pitch.cos();
    let sin_cp = cam_pitch.sin();
    let cos_cy = cam_yaw.cos();
    let sin_cy = cam_yaw.sin();
    let cam_forward = Vec3::new(-cos_cp * sin_cy, -sin_cp, -cos_cp * cos_cy).normalize();
    let world_up = if cos_cp.abs() < 1e-4 {
        Vec3::new(0.0, 0.0, -1.0)
    } else {
        Vec3::Y
    };
    let cam_right = cam_forward.cross(world_up).normalize();
    let cam_up = cam_right.cross(cam_forward).normalize();
    (cam_forward, cam_right, cam_up)
}

#[derive(Debug, Clone, Copy)]
pub struct GemOpticalMetrics {
    pub brilliance_pct: f32,
    pub fire_index: f32,
    pub scintillation_pct: f32,
    pub windowing_pct: f32,
    pub extinction_pct: f32,
}

/// 19 tilt ELEVATION sample points (camera pitch, not tilt-from-table-up) in exact 5°
/// steps across 0° to 90°.
///
/// A different, independently-valid parameterisation from [`TILT_ANGLES_DEG`]'s
/// `-90..=+90°` tilt-away-from-table-up domain: `0°` here means edge-on/profile
/// (camera pitch 0), `90°` means table-up/face-up (camera pitch 90) -- the reverse of
/// where those two poses sit in `TILT_ANGLES_DEG`. Do not "unify" the two domains --
/// [`evaluate_angular_profile_at_azimuth`] is an independently-used elevation sweep at
/// a single fixed azimuth, not a coarser draft of the full-axis function.
pub const PROFILE_ANGLES_DEG: [f32; 19] = [
    0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0,
    80.0, 85.0, 90.0,
];

/// Outcome of refracting an incident ray into the gemstone at a known entry point/normal
/// (for one specific refractive index) and following it through up to 10 internal
/// bounces. Factored out so the exact same entry-refraction-then-bounce logic can be
/// replayed at the d-line index (windowing/extinction/brilliance classification) and
/// independently at the F-line and C-line indices (Fire measurement), from the
/// identical physical entry point and incident direction.
#[derive(Debug, Clone, Copy)]
enum RayFate {
    /// Entry refraction has no real solution at this index (only possible for a
    /// pathological index < 1); not classified into any bucket. Unreachable for the
    /// d-line index (clamped to >= 1.1), kept for defensive symmetry with the F/C traces.
    EntryBlocked,
    /// Leaked out through the pavilion bottom (`n_out.y < -0.05`): windowing.
    Leaked,
    /// Exited back out through the upper hemisphere/crown: direction, the fraction of
    /// incident intensity transmitted along this exact path (Fresnel transmittance at
    /// entry times exit, TIR bounces in between being lossless), and the cosine of the
    /// exit angle from the exit facet's own normal (the projected-area radiometric
    /// factor used to weight Fire, distinct from transmittance). Also carries the exit
    /// facet index and internal bounce count -- see `ExitPath`'s doc.
    ExitedUpward(ExitPath),
    /// Trapped internally: exhausted its bounce budget, exited sideways through the
    /// girdle, or hit no further facet.
    Absorbed,
}

/// The physical exit path of a ray that escaped upward through the crown: exit
/// direction, Fresnel entry*exit transmittance and exit-facet-normal cosine (see
/// `RayFate::ExitedUpward`'s doc), plus which facet it exited through and how many
/// internal TIR bounces it took to get there.
///
/// The facet index and bounce count let the Fire measurement tell whether a ray's
/// F-line and C-line companion traces exited via the SAME physical path. Near a
/// critical angle, F and C can straddle the TIR threshold at different bounces --
/// `acos(dir_f . dir_c)` between such unrelated exit directions measures nothing
/// physically meaningful, yet can dominate the weighted Fire sum -- see the bifurcation
/// gate in `evaluate_gem_optical_metrics`.
#[derive(Debug, Clone, Copy)]
struct ExitPath {
    dir: Vec3,
    transmittance: f32,
    exit_cos_theta: f32,
    facet_idx: usize,
    bounces: u32,
}

/// Unpolarized Fresnel transmittance at a dielectric interface, given the cosines of the
/// incident and transmitted angles on either side (`n1` -> `n2`). Averages the s- and
/// p-polarized reflectances (`Rs`, `Rp`) into a single scalar reflectance `R`, then
/// returns `T = 1 - R`, clamped to [0, 1]. Used to weight each ray's contribution to Fire
/// by the energy it actually delivers, rather than counting every surviving ray equally
/// regardless of how much of its light made it through the entry and exit interfaces.
fn fresnel_transmittance(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> f32 {
    let denom_s = n2.mul_add(cos_t, n1 * cos_i);
    let denom_p = n2.mul_add(cos_i, n1 * cos_t);
    // Denominators vanish only at grazing incidence, where reflectance already tends
    // to 1 (T -> 0); zero transmittance there is safe (no 0/0 NaN) and physically correct.
    if denom_s.abs() < 1e-6 || denom_p.abs() < 1e-6 {
        return 0.0;
    }
    let rs = (n2.mul_add(-cos_t, n1 * cos_i) / denom_s).powi(2);
    let rp = (n2.mul_add(-cos_i, n1 * cos_t) / denom_p).powi(2);
    let r = f32::midpoint(rs, rp);
    (1.0 - r).clamp(0.0, 1.0)
}

/// Refracts `incoming_dir` into the gem at `entry_point`/`n_entry` using Snell's law for
/// `index`, then follows up to 10 internal bounces (TIR vs refract-out) exactly as the
/// original single-wavelength trace did. This is the physical core shared by the d-line,
/// F-line, and C-line traces -- identical control flow, parameterized only by which
/// refractive index the light is carrying.
fn trace_wavelength(
    entry_point: Vec3,
    incoming_dir: Vec3,
    n_entry: Vec3,
    cos_i: f32,
    plane_soa: &crate::simd::PlanesSoA32,
    index: f32,
) -> RayFate {
    let sin2_t = (1.0 / (index * index)) * cos_i.mul_add(-cos_i, 1.0);
    if sin2_t > 1.0 {
        return RayFate::EntryBlocked;
    }
    let cos_t = (1.0 - sin2_t).sqrt();
    let mut curr_dir =
        ((1.0 / index) * incoming_dir + (1.0 / index).mul_add(cos_i, -cos_t) * n_entry).normalize();
    let mut hit_point = entry_point + curr_dir * 1e-4;
    let sin_crit = 1.0 / index;
    // Entry transmittance (air -> gem): computed once here since cos_i/cos_t at entry
    // don't change across bounces; combined with the exit transmittance below to give
    // this path's total energy weight.
    let entry_transmittance = fresnel_transmittance(1.0, index, cos_i, cos_t);

    let mut leaked = false;
    let mut exited_upwards = false;
    let mut exit_dir = Vec3::ZERO;
    let mut exit_transmittance = 0.0f32;
    let mut exit_cos_theta = 0.0f32;
    let mut exit_facet_idx = usize::MAX;
    let mut exit_bounces = 0u32;

    for bounce in 0..10 {
        let inside_ray = Ray {
            origin: hit_point,
            dir: curr_dir,
        };
        let next_hit = intersect_polyhedron_soa(inside_ray, plane_soa);
        let Some(next_rec) = next_hit else { break };
        let next_point = inside_ray.origin + next_rec.t * inside_ray.dir;
        let n_out = next_rec.normal; // outward-pointing facet normal

        let cos_theta = curr_dir.dot(n_out).clamp(0.0, 1.0);
        let sin_theta = cos_theta.mul_add(-cos_theta, 1.0).max(0.0).sqrt();

        if sin_theta < sin_crit {
            // Refracts out of the stone (TIR failed)
            let sin2_out = (index * index) * cos_theta.mul_add(-cos_theta, 1.0);
            if sin2_out <= 1.0 {
                let cos_out = (1.0 - sin2_out).sqrt();
                let out_dir =
                    (index * curr_dir + index.mul_add(-cos_theta, cos_out) * n_out).normalize();
                if n_out.y < -0.05 {
                    leaked = true; // Leaks out through pavilion bottom -> windowing.
                } else if out_dir.y > 0.05 {
                    // Exits back toward upper hemisphere / crown.
                    exited_upwards = true;
                    exit_dir = out_dir;
                    exit_transmittance = fresnel_transmittance(index, 1.0, cos_theta, cos_out);
                    exit_cos_theta = cos_out;
                    exit_facet_idx = next_rec.facet_idx;
                    exit_bounces = bounce;
                }
                break;
            }
        }

        // Total Internal Reflection (TIR)
        curr_dir = (curr_dir - 2.0 * cos_theta * n_out).normalize();
        hit_point = next_point + curr_dir * 1e-4;
    }

    if leaked {
        RayFate::Leaked
    } else if exited_upwards {
        RayFate::ExitedUpward(ExitPath {
            dir: exit_dir,
            transmittance: entry_transmittance * exit_transmittance,
            exit_cos_theta,
            facet_idx: exit_facet_idx,
            bounces: exit_bounces,
        })
    } else {
        RayFate::Absorbed
    }
}

/// Whether a ray that exited the gem in direction `exit_dir` is visibly returned to an
/// observer at `cam_forward` under the given key/fill/overhead-ring illumination -- not
/// lost to the observer's own head-shadow, and actually collected by at least one light
/// source. The same test used for `brilliance_pct`/`extinction_pct` classification in
/// the main loop below, factored out so the Scintillation temporal sub-poses (see
/// `cell_returned_at_yaw_offset`) apply an identical definition of "returned".
#[must_use]
fn ray_is_visibly_returned(
    exit_dir: Vec3,
    cam_forward: Vec3,
    key_dir: Vec3,
    fill_dir: Vec3,
    sin_lp: f32,
) -> bool {
    // 1. Head-shadow cone (angle < 16 deg from viewing vector)
    let is_head_shadow = (-exit_dir).dot(cam_forward) > 0.96;

    // 2. Light collection from Key, Fill, or Overhead ring illumination
    let key_dot = exit_dir.dot(key_dir).max(0.0);
    let fill_dot = exit_dir.dot(fill_dir).max(0.0);
    let ring_dot = sin_lp.mul_add(-0.8, exit_dir.y).abs() < 0.35;
    let is_illuminated = (key_dot > 0.70) || (fill_dot > 0.75) || (ring_dot && exit_dir.y > 0.2);

    !is_head_shadow && is_illuminated
}

/// Small camera-yaw offsets (degrees), sampled around the primary viewing azimuth, used
/// to measure Scintillation's TEMPORAL component: how much a given grid cell's light
/// return *changes* as the stone is gently rotated, distinct from the static spatial
/// contrast measured at a single fixed pose. Five poses (odd count -- see
/// `cell_returned_at_yaw_offset`'s doc for why), spanning +/-3 deg, comparable in scale
/// to a hand gently tilting a stone for inspection, not a full turn.
const SCINT_TEMPORAL_YAW_OFFSETS_DEG: [f32; 5] = [-3.0, -1.5, 0.0, 1.5, 3.0];

/// Weight of the spatial (per-pose, per-grid-cell contrast) term in the combined
/// `scintillation_pct`. See `combine_scintillation_pct` for the full weighting rationale.
const SCINT_SPATIAL_WEIGHT: f32 = 0.6;
/// Weight of the temporal (per-cell, across-pose flicker) term in the combined
/// `scintillation_pct`. See `combine_scintillation_pct` for the full weighting rationale.
const SCINT_TEMPORAL_WEIGHT: f32 = 0.4;

/// Per-evaluation context threaded through the Scintillation temporal sub-poses
/// (`cell_returned_at_yaw_offset`): everything needed to fire and classify one extra
/// ray at a rotated camera azimuth, bundled to keep that function's argument count
/// within clippy's `too_many_arguments` lint -- identical across all
/// `grid_size * grid_size * SCINT_TEMPORAL_YAW_OFFSETS_DEG.len()` calls per invocation.
struct TemporalPoseContext<'a> {
    plane_soa: &'a crate::simd::PlanesSoA32,
    nd: f32,
    cam_yaw: f32,
    cam_pitch: f32,
    key_dir: Vec3,
    fill_dir: Vec3,
    sin_lp: f32,
}

/// Fires a single, non-jittered ray at grid cell `(u, v)` (same screen-space coordinates
/// as the main grid loop, in `[-1, 1]`) from the camera pose rotated by `yaw_offset_deg`
/// around `ctx.cam_yaw`, refracts it at the d-line only, and reports whether it is
/// visibly returned to an observer at that pose (`ray_is_visibly_returned` above).
/// Independent of the main grid loop's per-cell state: temporal modulation asks whether
/// the SAME screen-space cell's return status flips as the viewpoint rotates, a
/// different question from the spatial sub-aperture contrast; no F/C companion trace is
/// needed since only whether light returns, not its color separation, matters here.
///
/// Odd offset counts (see [`SCINT_TEMPORAL_YAW_OFFSETS_DEG`]) avoid the temporal term
/// ever reaching exactly 100%: an even sample count lets a cell that flips exactly half
/// the time hit the maximum Bernoulli variance (p=0.5, p*(1-p)=0.25) exactly, whereas
/// 5 (odd) samples cap the achievable variance at 0.24 (k=2 or 3 of 5), strictly below
/// 0.25.
#[must_use]
fn cell_returned_at_yaw_offset(
    ctx: &TemporalPoseContext,
    yaw_offset_deg: f32,
    u: f32,
    v: f32,
) -> bool {
    let (forward, right, up) =
        camera_view_basis(ctx.cam_yaw + yaw_offset_deg.to_radians(), ctx.cam_pitch);
    let ray = Ray {
        origin: -forward * 2.5 + (u * 0.95) * right + (v * 0.95) * up,
        dir: forward,
    };
    let Some(hit_rec) = intersect_polyhedron_soa(ray, ctx.plane_soa) else {
        return false;
    };
    let hit_point = ray.origin + hit_rec.t * ray.dir;
    let n_entry = hit_rec.normal;
    let cos_i = (-ray.dir).dot(n_entry).clamp(0.0, 1.0);

    match trace_wavelength(hit_point, ray.dir, n_entry, cos_i, ctx.plane_soa, ctx.nd) {
        RayFate::ExitedUpward(exit) => {
            ray_is_visibly_returned(exit.dir, forward, ctx.key_dir, ctx.fill_dir, ctx.sin_lp)
        }
        RayFate::EntryBlocked | RayFate::Leaked | RayFate::Absorbed => false,
    }
}

/// Scintillation TEMPORAL contribution of one grid cell `(u, v)`: does its return status
/// flip as the camera nudges through the small azimuth offsets in
/// [`SCINT_TEMPORAL_YAW_OFFSETS_DEG`]? Computed via independent single-ray samples (see
/// `cell_returned_at_yaw_offset`), reduced to the Bernoulli variance `p * (1 - p)` of the
/// fraction `p` of offsets at which the cell returned light: 0 when never flipping
/// (bright or dark, not sparkling), up to 0.24 when it flips close to half the time.
#[must_use]
fn cell_temporal_variance(ctx: &TemporalPoseContext, u: f32, v: f32) -> f32 {
    let mut temporal_returned = 0u32;
    for &yaw_offset_deg in &SCINT_TEMPORAL_YAW_OFFSETS_DEG {
        if cell_returned_at_yaw_offset(ctx, yaw_offset_deg, u, v) {
            temporal_returned += 1;
        }
    }
    let temporal_p = temporal_returned as f32 / SCINT_TEMPORAL_YAW_OFFSETS_DEG.len() as f32;
    temporal_p.mul_add(-temporal_p, temporal_p)
}

/// Scintillation SPATIAL term: coefficient of variation (std-dev / mean) of per-cell
/// light-return fraction across the 18x18 grid, mapped into a 0-100 percentage. CV = 0
/// means every visited cell returns light at the same rate (no contrast, no sparkle);
/// CV climbs as bright and dark cells diverge, and routinely lands well above 1.0 for
/// these sparse-bright-cell distributions -- a straight `(cv * 100.0).clamp(0.0, 100.0)`
/// was measured to saturate at exactly 100% for 19 of 26 material/cut combinations,
/// collapsing the metric's ability to discriminate among most stones.
///
/// Instead, squash CV through `cv / (1 + cv)`, a monotone bijection from [0, inf) to
/// [0, 1) that never reaches 100% however large CV gets. A stone with no visited cells
/// reports 0.
#[must_use]
fn spatial_scintillation_pct(
    cell_fraction_sum: f32,
    cell_fraction_sum_sq: f32,
    cell_count: u32,
) -> f32 {
    if cell_count == 0 {
        return 0.0;
    }
    let mean_frac = cell_fraction_sum / cell_count as f32;
    if mean_frac <= 1e-4 {
        return 0.0;
    }
    let variance = mean_frac
        .mul_add(-mean_frac, cell_fraction_sum_sq / cell_count as f32)
        .max(0.0);
    let std_dev = variance.sqrt();
    let coefficient_of_variation = std_dev / mean_frac;
    (coefficient_of_variation / (1.0 + coefficient_of_variation) * 100.0).clamp(0.0, 100.0)
}

/// Scintillation TEMPORAL term: mean per-cell Bernoulli variance across the same
/// visited-cell set as the spatial term, normalized by its own achievable maximum (0.24,
/// not the continuous 0.25 -- see `cell_returned_at_yaw_offset`) into a 0-100
/// percentage. Needs no separate saturating squash: it is bounded by construction, and
/// averaging many cells makes the whole grid landing on that per-cell ceiling
/// simultaneously vanishingly unlikely in practice.
#[must_use]
fn temporal_scintillation_pct(temporal_variance_sum: f32, cell_count: u32) -> f32 {
    if cell_count == 0 {
        return 0.0;
    }
    ((temporal_variance_sum / cell_count as f32) / 0.24 * 100.0).clamp(0.0, 100.0)
}

/// Combines the Scintillation spatial and temporal terms (both already 0-100
/// percentages) into the final displayed `scintillation_pct`.
///
/// The reference gemological definition of scintillation is spatial AND temporal
/// modulation together: a static bright/dark pattern is not "sparkle", and uniform
/// flicker with no spatial contrast has nothing to flicker between. Weighted 60/40
/// toward the spatial term, since it is measured from a much larger effective sample (5
/// sub-aperture rays x 18x18 grid) and validated as non-saturating and broadly
/// discriminating, while the temporal term is measured more coarsely (5 single-ray
/// azimuth samples per cell, no sub-aperture averaging) to keep added cost modest. Not
/// strong enough to make SRB out-scintillate the emerald cut unconditionally (the
/// spatial term's larger dynamic range still dominates when the two cuts return very
/// different light), but decisive at roughly comparable brilliance.
#[must_use]
fn combine_scintillation_pct(spatial_pct: f32, temporal_pct: f32) -> f32 {
    SCINT_TEMPORAL_WEIGHT
        .mul_add(temporal_pct, SCINT_SPATIAL_WEIGHT * spatial_pct)
        .clamp(0.0, 100.0)
}

/// Degrees-of-angular-separation -> display-scale multiplier for `fire_index`.
///
/// The measured quantity is the mean angle (degrees) between a ray's F-line and C-line
/// exit directions, over rays that exit upward through the crown and pass the same
/// illumination test as `brilliance_pct`. That mean separation typically lands in the
/// 0.4-10 deg range depending on material/cut/angle (multiple TIR bounces each add
/// their own chromatic walk-off) -- physically real, but not yet a legible UI number.
/// This constant rescales it into a range comparable to the old closed-form `fire_index`
/// values (which topped out around 80 for diamond); it is a *display* scale, not a fit.
///
/// Calibrated at 275 (Diamond, standard round brilliant, yaw 0.0/pitch 0.45/light
/// 0.85-0.95, post F/C-bifurcation-gate weighted-mean separation ~0.0716 deg) to land
/// `fire_index` ~= 19.7; every built-in material on both cuts stays comfortably under
/// 100 at this scale. This scale, together with the bifurcation gate, fixes the
/// emerald-cut Fire ordering -- see `evaluate_gem_optical_metrics`'s doc comment.
const FIRE_DEGREES_TO_DISPLAY_SCALE: f32 = 275.0;

/// Per-evaluation context threaded through [`classify_aperture_sample`]: everything
/// needed to fire and classify one grid-cell aperture-sample ray at the fixed camera
/// pose, bundled to keep that function's argument count within clippy's
/// `too_many_arguments` lint -- identical across all
/// `grid_size * grid_size * aperture_samples.len()` calls per invocation. Mirrors
/// [`TemporalPoseContext`]'s reason for existing, one level up.
struct ApertureSampleContext<'a> {
    plane_soa: &'a crate::simd::PlanesSoA32,
    nd: f32,
    n_f: f32,
    n_c: f32,
    cam_forward: Vec3,
    cam_right: Vec3,
    cam_up: Vec3,
    key_dir: Vec3,
    fill_dir: Vec3,
    sin_lp: f32,
}

/// How one grid-cell aperture-sample ray was classified. `Returned` carries the
/// qualifying Fire sample's raw `(angle_deg, weight)` pair, if any -- deliberately not
/// folded into the Fire accumulator here, so the caller applies the same `f32::mul_add`
/// chain against its own running total in a fixed iteration order (folding it in here
/// instead would change which intermediate values a fused multiply-add rounds against,
/// perturbing the bit-exact result).
enum RayClassification {
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
fn classify_aperture_sample(
    ctx: &ApertureSampleContext,
    u: f32,
    v: f32,
    dx_sub: f32,
    dz_sub: f32,
) -> Option<RayClassification> {
    let ray_dir = (ctx.cam_forward + dx_sub * ctx.cam_right + dz_sub * ctx.cam_up).normalize();
    let ray_origin = -ctx.cam_forward * 2.5 + (u * 0.95) * ctx.cam_right + (v * 0.95) * ctx.cam_up;
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
            // Directional Extinction Analysis: head-shadow cone vs. Key/Fill/
            // Overhead-ring illumination collection (see `ray_is_visibly_returned`).
            if !ray_is_visibly_returned(
                exit_dir,
                ctx.cam_forward,
                ctx.key_dir,
                ctx.fill_dir,
                ctx.sin_lp,
            ) {
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

/// Aggregate accumulators threaded through `evaluate_gem_optical_metrics`'s main grid
/// loop, bundled into one `#[derive(Default)]` struct to keep the function under
/// clippy's line-count budget without losing per-field rationale, which lives on the
/// fields below.
#[derive(Default)]
struct MetricsAccumulators {
    total_rays: u32,
    windowed_rays: u32,
    extinct_rays: u32,
    returned_rays: u32,
    /// ENERGY-WEIGHTED sum of per-ray F-line/C-line exit angular separations (degrees),
    /// over rays that exit upward through the crown at the d-line, pass the same
    /// illumination test as brilliance, and whose F-line/C-line companion traces also
    /// both exit upward. Each contribution is weighted by that ray's own Fresnel
    /// entry*exit transmittance, and the sum is normalized by TOTAL incident rays
    /// (`n_total`) rather than the count of qualifying rays -- so a stone that returns
    /// little light cannot score highly on a small, wide-angle survivor population.
    fire_energy_weighted_sum_deg: f32,
    /// Diagnostic-only accumulators for the `DIAG_FIRE_DEBUG` eprintln block, gated
    /// behind `diag_fire_debug` in the hot loop so a normal run pays no extra cost.
    dbg_fire_qualifying: u32,
    dbg_fire_angle_sum_unweighted: f32,
    dbg_fire_transmittance_sum: f32,
    /// Scintillation accumulators: per-grid-cell fraction of aperture samples that
    /// returned illuminated brilliance, aggregated into a coefficient of variation
    /// across the 18x18 grid at the end.
    cell_fraction_sum: f32,
    cell_fraction_sum_sq: f32,
    cell_count: u32,
    /// Scintillation TEMPORAL accumulator: per visited cell, the Bernoulli variance
    /// (max 0.24 at this sample count -- see `cell_returned_at_yaw_offset`) of that
    /// cell's return status across `SCINT_TEMPORAL_YAW_OFFSETS_DEG`, summed and
    /// averaged by `cell_count` after the loop. A cell returning light identically at
    /// every offset contributes 0; one that flips contributes up to 0.24.
    temporal_variance_sum: f32,
}

/// Prints the `DIAG_FIRE_DEBUG` Fire diagnostic line. Pure formatting over
/// already-computed values, extracted to keep the `eprintln!`'s argument list out of
/// the main function's line count.
fn log_fire_diagnostics(
    material_name: &str,
    n_total: f32,
    fire_index: f32,
    acc: &MetricsAccumulators,
) {
    let mean_angle_unweighted = if acc.dbg_fire_qualifying > 0 {
        acc.dbg_fire_angle_sum_unweighted / acc.dbg_fire_qualifying as f32
    } else {
        0.0
    };
    let mean_transmittance = if acc.dbg_fire_qualifying > 0 {
        acc.dbg_fire_transmittance_sum / acc.dbg_fire_qualifying as f32
    } else {
        0.0
    };
    eprintln!(
        "DIAG material={material_name} n_total={n_total} returned_rays={} fire_qualifying={} mean_angle_unweighted={mean_angle_unweighted:.4} mean_transmittance={mean_transmittance:.4} weighted_sum={:.4} fire_index={fire_index:.4}",
        acc.returned_rays, acc.dbg_fire_qualifying, acc.fire_energy_weighted_sum_deg
    );
}

/// Prints the `DIAG_FIRE_DEBUG` Scintillation diagnostic line. Same rationale as
/// [`log_fire_diagnostics`].
fn log_scintillation_diagnostics(
    material_name: &str,
    spatial_scint_pct: f32,
    temporal_pct: f32,
    scintillation_pct: f32,
) {
    eprintln!(
        "DIAG-SCINT material={material_name} spatial={spatial_scint_pct:.4} temporal={temporal_pct:.4} combined={scintillation_pct:.4}"
    );
}

/// Everything the main grid loop in [`evaluate_gem_optical_metrics`] needs that is
/// fixed across the whole evaluation: the sampling grid resolution, the sub-aperture
/// jitter bundle, and the two shared per-ray contexts ([`TemporalPoseContext`],
/// [`ApertureSampleContext`]).
struct GridEvalSetup<'a> {
    grid_size: i32,
    aperture_samples: [(f32, f32); 5],
    temporal_ctx: TemporalPoseContext<'a>,
    aperture_ctx: ApertureSampleContext<'a>,
}

/// Builds [`GridEvalSetup`]. A pure setup extraction: every value is computed exactly
/// once, unconditionally, with no accumulator or loop state involved.
fn build_grid_eval_setup<'a>(
    plane_soa: &'a crate::simd::PlanesSoA32,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> GridEvalSetup<'a> {
    let nd = material.dispersion.evaluate(589.3).max(1.1);
    // Clamped defensively like `nd` above so `trace_wavelength`'s entry refraction
    // never hits the pathological `EntryBlocked` case for these two indices either.
    let n_f = material.dispersion.evaluate(486.1).max(1.001);
    let n_c = material.dispersion.evaluate(656.3).max(1.001);

    let grid_size = 18;

    // Matches the real render camera's frame exactly (see `camera_view_basis`).
    let (cam_forward, cam_right, cam_up) = camera_view_basis(cam_yaw, cam_pitch);

    // Key/Fill Light Direction from the shared `StudioRig` -- the same construction
    // `sample_studio_environment` uses to light the image these metrics describe, so
    // the two can never silently drift apart. `rig.ring_dirs` is not consulted: the
    // ring/annulus test below is a deliberately coarser approximation that only needs
    // `sin_light_pitch` (see `ray_is_visibly_returned`).
    let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
    let key_dir = rig.key_dir;
    let fill_dir = rig.fill_dir;
    let sin_lp = rig.sin_light_pitch;

    // 5-point angular sub-aperture bundle (standard GIA 0° to 6° observer eye cone)
    let aperture_samples = [
        (0.0f32, 0.0f32),
        (0.08, 0.0),
        (-0.08, 0.0),
        (0.0, 0.08),
        (0.0, -0.08),
    ];

    // Shared context for the Scintillation temporal sub-poses (see
    // `TemporalPoseContext`'s doc): identical across every grid cell and offset sample.
    let temporal_ctx = TemporalPoseContext {
        plane_soa,
        nd,
        cam_yaw,
        cam_pitch,
        key_dir,
        fill_dir,
        sin_lp,
    };

    // Shared context for the per-aperture-sample classification (see
    // `ApertureSampleContext`'s doc): identical across every grid cell and sample.
    let aperture_ctx = ApertureSampleContext {
        plane_soa,
        nd,
        n_f,
        n_c,
        cam_forward,
        cam_right,
        cam_up,
        key_dir,
        fill_dir,
        sin_lp,
    };

    GridEvalSetup {
        grid_size,
        aperture_samples,
        temporal_ctx,
        aperture_ctx,
    }
}

/// Evaluates true GIA / AGSL optical gemological metrics by firing an analytical grid
/// of rays with viewing aperture cone from the observer's **Point of View (`PoV`)**.
///
/// Rays are fired from (`cam_yaw`, `cam_pitch`) through the 3D cutting schedule facet
/// geometry, dynamically accounting for:
/// 1. Gemstone refractive index n(λ) from Sellmeier / Cauchy equations
/// 2. Snell's law refraction at inclined crown & girdle facet entry points
/// 3. Total Internal Reflection (TIR) vs bottom leakage (Windowing) on pavilion facets
/// 4. Light source illumination alignment (`light_yaw`, `light_pitch`) vs head-shadow extinction
/// 5. Fire: the angular separation between the F-line and C-line images of each ray that
///    is actually visibly returned (same illumination test as brilliance), so a
///    high-leakage cut with a few stray near-critical-angle rays cannot outscore a
///    well-performing one on angle alone
/// 6. Scintillation: the spatial contrast (coefficient of variation) of light return
///    across the 18x18 sampling grid over the stone's face
///
/// ## Decision record: Fire ordering on step (emerald) cuts, and the F/C bifurcation artifact
///
/// When F-line/C-line traces exit via a different facet or bounce count, their critical
/// angles straddled a TIR threshold at different points, so `acos(dir_f . dir_c)`
/// measured unrelated exit directions, not dispersion -- these bifurcated pairs carried
/// 45-98% of the weighted Fire sum wherever a step cut wrongly out-scored a brilliant.
/// Fixed by the F/C gate below: a pair contributes only when both traces share the same
/// exit facet and bounce count. Rejected a capped-credit alternative (a small fixed
/// angle for a bifurcated pair): it put Quartz above Sapphire on the emerald cut, when
/// Sapphire's true dispersion is higher.
///
/// ## Investigated and closed: Quartz vs. Topaz ordering on the emerald cut is noise, not a defect
///
/// Quartz measures a higher `fire_index` than Topaz at the canonical pose despite
/// Topaz's larger F-C dispersion. Sweeping camera pitch showed the gap swinging sign
/// with no consistent direction, and quadrupling `grid_size` collapsed it near zero --
/// discrete-grid quantization noise, not a material-ordering bug. Separately, Topaz's
/// higher base index gives it lower Fresnel transmittance, legitimately offsetting its
/// dispersion edge since Fire is transmittance-weighted by design. If revisited, raise
/// `grid_size` rather than retune the F/C gate or `FIRE_DEGREES_TO_DISPLAY_SCALE`.
#[must_use]
pub fn evaluate_gem_optical_metrics(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> GemOpticalMetrics {
    if planes.is_empty() {
        // No facet geometry to trace: fall back to neutral placeholder values rather
        // than a formula. Display defaults for "no geometry loaded", not measurements.
        return GemOpticalMetrics {
            brilliance_pct: 85.0,
            fire_index: 25.0,
            scintillation_pct: 75.0,
            windowing_pct: 5.0,
            extinction_pct: 5.0,
        };
    }

    let mut acc = MetricsAccumulators::default();
    // Checked once here (not once per ray), gated behind `diag_fire_debug` in the hot
    // loop below so a normal run pays no cost for the env lookup or extra additions.
    let diag_fire_debug = std::env::var("DIAG_FIRE_DEBUG").is_ok();

    // SIMD slab arena, built once per evaluation: every ray this function fires (grid,
    // sub-aperture, temporal sub-poses, F/C lines) intersects the same solid.
    let plane_soa = build_plane_soa(planes);
    let setup = build_grid_eval_setup(
        &plane_soa,
        material,
        cam_yaw,
        cam_pitch,
        light_yaw,
        light_pitch,
    );

    for ix in 0..setup.grid_size {
        for iz in 0..setup.grid_size {
            let u = ((ix as f32 + 0.5) / (setup.grid_size as f32)).mul_add(2.0, -1.0);
            let v = ((iz as f32 + 0.5) / (setup.grid_size as f32)).mul_add(2.0, -1.0);
            if v.mul_add(v, u * u) > 0.70 {
                continue; // Stay within gem perimeter
            }

            // Per-cell counters for the Scintillation spatial-contrast measurement:
            // how many aperture samples hit the stone, and how many returned brilliance.
            let mut cell_total = 0u32;
            let mut cell_returned = 0u32;

            for &(dx_sub, dz_sub) in &setup.aperture_samples {
                // See `classify_aperture_sample`'s doc -- the Fire accumulator update
                // below is applied here, not inside the helper, to preserve the exact
                // `f32::mul_add` chain across grid cells.
                let Some(classification) =
                    classify_aperture_sample(&setup.aperture_ctx, u, v, dx_sub, dz_sub)
                else {
                    continue;
                };

                acc.total_rays += 1;
                cell_total += 1;

                match classification {
                    RayClassification::EntryBlocked => {}
                    RayClassification::Windowed => acc.windowed_rays += 1,
                    RayClassification::Extinct => acc.extinct_rays += 1,
                    RayClassification::Returned(fire) => {
                        acc.returned_rays += 1;
                        cell_returned += 1;

                        if let Some((angle_deg, weight)) = fire {
                            acc.fire_energy_weighted_sum_deg =
                                f32::mul_add(angle_deg, weight, acc.fire_energy_weighted_sum_deg);
                            if diag_fire_debug {
                                acc.dbg_fire_qualifying += 1;
                                acc.dbg_fire_angle_sum_unweighted += angle_deg;
                                acc.dbg_fire_transmittance_sum += weight;
                            }
                        }
                    }
                }
            }

            if cell_total > 0 {
                let frac = cell_returned as f32 / cell_total as f32;
                acc.cell_fraction_sum += frac;
                acc.cell_fraction_sum_sq = frac.mul_add(frac, acc.cell_fraction_sum_sq);
                acc.cell_count += 1;
                acc.temporal_variance_sum += cell_temporal_variance(&setup.temporal_ctx, u, v);
            }
        }
    }

    let n_total = acc.total_rays.max(1) as f32;
    let windowing_pct = (acc.windowed_rays as f32 / n_total * 100.0).clamp(0.0, 100.0);
    let extinction_pct = (acc.extinct_rays as f32 / n_total * 100.0).clamp(0.0, 100.0);
    let brilliance_pct = (acc.returned_rays as f32 / n_total * 100.0).clamp(0.0, 100.0);

    // Fire: energy-weighted F-line/C-line angular separation, normalized by TOTAL
    // incident rays (n_total), not the count of rays that happened to qualify -- the
    // same convention as brilliance_pct/windowing_pct/extinction_pct above, closing the
    // loophole where a shrinking denominator lets a few wide-angle survivors from a
    // badly-leaking cut inflate the average. See
    // `MetricsAccumulators::fire_energy_weighted_sum_deg`'s doc. Naturally floors at 0.1
    // when the weighted sum is zero -- no separate branch needed.
    let fire_index =
        (acc.fire_energy_weighted_sum_deg / n_total * FIRE_DEGREES_TO_DISPLAY_SCALE).max(0.1);
    if diag_fire_debug {
        log_fire_diagnostics(&material.name, n_total, fire_index, &acc);
    }

    let spatial_scint_pct = spatial_scintillation_pct(
        acc.cell_fraction_sum,
        acc.cell_fraction_sum_sq,
        acc.cell_count,
    );
    let temporal_pct = temporal_scintillation_pct(acc.temporal_variance_sum, acc.cell_count);
    let scintillation_pct = combine_scintillation_pct(spatial_scint_pct, temporal_pct);
    if diag_fire_debug {
        log_scintillation_diagnostics(
            &material.name,
            spatial_scint_pct,
            temporal_pct,
            scintillation_pct,
        );
    }

    GemOpticalMetrics {
        brilliance_pct,
        fire_index,
        scintillation_pct,
        windowing_pct,
        extinction_pct,
    }
}

/// Camera azimuths the Tilt Performance dialog sweeps a full tilt-elevation profile at,
/// in degrees -- see [`evaluate_angular_profile_at_azimuth`].
///
/// `0.0` looks straight down whatever direction `RenderContext::yaw == 0.0` frames (for
/// an elongated outline -- marquise, emerald cut, pear -- conventionally the table's
/// long axis). Each further entry rotates the viewpoint another 45° around the
/// table-normal axis, so `90.0` looks down the perpendicular ("width") axis and `45.0`/
/// `135.0` bisect the two -- tilting toward a non-round stone's long vs. short axis
/// windows very differently, information a single azimuth-0 sweep hides.
pub const PROFILE_AZIMUTHS_DEG: [f32; 4] = [0.0, 45.0, 90.0, 135.0];

/// Calculates a 19-point angular profile of (Brilliance %, Extinction %, Windowing %)
/// at an explicit camera azimuth.
///
/// Sampled over `PoV` tilt elevation in exact 5° steps
/// (see [`PROFILE_ANGLES_DEG`]). `cam_yaw` is in radians, matching
/// `evaluate_gem_optical_metrics`'s own convention -- see [`PROFILE_AZIMUTHS_DEG`] for
/// the four-azimuth convention the Tilt Performance dialog's axis switcher uses.
/// [`evaluate_angular_profile`] is just this function called at `cam_yaw: 0.0`.
#[must_use]
pub fn evaluate_angular_profile_at_azimuth(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cam_yaw: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> ([f32; 19], [f32; 19], [f32; 19]) {
    sample_elevation_sweep(
        planes,
        material,
        cam_yaw,
        &PROFILE_ANGLES_DEG,
        light_yaw,
        light_pitch,
    )
}

/// Shared per-angle sampling loop behind [`evaluate_angular_profile_at_azimuth`] (called
/// with `&PROFILE_ANGLES_DEG`, 19 points / 5° steps) and
/// [`evaluate_full_axis_profile_at_azimuth`] (called with `&HALF_AXIS_PITCH_DEG`, 90
/// points / 1° steps -- a different grid needing its own const array rather than a finer
/// `PROFILE_ANGLES_DEG`). Factored out so both call sites run the textually identical
/// sequence of floating-point operations rather than two hand-copies that could drift
/// apart. Const-generic over `N` so one function body serves both grids without a
/// `Vec`-based version paying an allocation per sweep.
fn sample_elevation_sweep<const N: usize>(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cam_yaw: f32,
    angles_deg: &[f32; N],
    light_yaw: f32,
    light_pitch: f32,
) -> ([f32; N], [f32; N], [f32; N]) {
    let mut brilliance_curve = [0.0f32; N];
    let mut extinction_curve = [0.0f32; N];
    let mut windowing_curve = [0.0f32; N];

    for (i, &deg) in angles_deg.iter().enumerate() {
        let cam_pitch_rad = deg.to_radians();
        let m = evaluate_gem_optical_metrics(
            planes,
            material,
            cam_yaw,
            cam_pitch_rad,
            light_yaw,
            light_pitch,
        );
        brilliance_curve[i] = m.brilliance_pct;
        extinction_curve[i] = m.extinction_pct;
        windowing_curve[i] = m.windowing_pct;
    }

    (brilliance_curve, extinction_curve, windowing_curve)
}

/// Pitch (camera-elevation) values `0..=89` degrees, ascending -- the per-half grid
/// [`evaluate_full_axis_profile_at_azimuth`] sweeps at each of the two azimuths it
/// combines. Excludes `90.0`: pitch `90` (table-up/face-up, the shared point -- see that
/// function's doc comment) is evaluated exactly once by the caller and stitched into
/// both halves, rather than swept twice only to throw one copy away.
const fn build_half_axis_pitch_deg() -> [f32; 90] {
    let mut out = [0.0f32; 90];
    let mut i = 0;
    while i < 90 {
        // out[i] = pitch i degrees: out[0] = 0.0, out[89] = 89.0.
        out[i] = i as f32;
        i += 1;
    }
    out
}
const HALF_AXIS_PITCH_DEG: [f32; 90] = build_half_axis_pitch_deg();

const fn build_tilt_angles_deg() -> [f32; 181] {
    let mut out = [0.0f32; 181];
    let mut i = 0;
    while i < 181 {
        // i=0 -> -90.0, i=90 -> 0.0, i=180 -> 90.0.
        out[i] = i as f32 - 90.0;
        i += 1;
    }
    out
}

/// 181 full-axis tilt sample points in exact 1° steps, `-90..=90` inclusive.
///
/// The VALUE is tilt AWAY FROM TABLE-UP, in degrees -- NOT camera elevation/pitch (that
/// is what [`PROFILE_ANGLES_DEG`] is). `TILT_ANGLES_DEG[90] == 0.0` is the shared
/// table-up/face-up pole (camera pitch 90°) every axis's curve passes through; `[0]`
/// and `[180]` are both edge-on/profile (camera pitch 0°), reached from the two
/// opposite azimuths of the axis pair -- see [`evaluate_full_axis_profile_at_azimuth`]'s
/// doc comment for the full geometry and the pose-to-index formula.
///
/// Table-up, not edge-on, is shared at the centre: an earlier version shared edge-on
/// instead, which is approached from two physically distinct azimuths, making the
/// merged curve discontinuous exactly at the "shared" point. Table-up is the one pose
/// in this sweep that is actually azimuth-independent (see the next function's doc
/// comment), which is what makes sharing it correct.
///
/// # Why 1° and not coarser (measured)
///
/// Reconstruction error against the true 1°-resolution curve decays roughly linearly
/// with step size (measured against round-brilliant diamond down to 2°/91 pts: 3-5
/// percentage points), since the curve carries genuine high-frequency structure
/// (individual facets flipping in and out of a fixed light as the stone tilts). That
/// error is disqualifying here since these curves back hard-threshold catalogue filters
/// (e.g. "windowing never exceeds 20% within ±45°"), where it can flip a design in or
/// out of the result set. 1° is also the natural ceiling: the graph canvas is a few
/// hundred pixels wide, so finer sampling would only resolve grid-sampling noise --
/// don't "improve" this to 0.5° later.
pub const TILT_ANGLES_DEG: [f32; 181] = build_tilt_angles_deg();

/// Merges one axis's independently-swept positive-azimuth and negative-azimuth
/// (`positive_azimuth + 180°`) pitch-0..89 halves, plus the shared table-up (pitch-90)
/// pole evaluated once, into the single 181-point `TILT_ANGLES_DEG`-indexed output
/// array -- see [`evaluate_full_axis_profile_at_azimuth`]'s doc comment for the
/// geometry and tilt-to-pose formula. `positive_pitch_sweep`/`negative_pitch_sweep` are
/// each indexed by [`HALF_AXIS_PITCH_DEG`] (pitch `i` at index `i`, i.e. ascending
/// pitch = descending `|tilt|`, since `pitch = 90 - |tilt|`).
fn merge_full_axis_halves(
    table_up_pole: f32,
    positive_pitch_sweep: &[f32; 90],
    negative_pitch_sweep: &[f32; 90],
) -> [f32; 181] {
    let mut out = [0.0f32; 181];
    // [0..90] = tilt -90..=-1: negative_pitch_sweep laid down directly (pitch = 90 +
    // tilt for tilt < 0, so out[k] is exactly negative_pitch_sweep[k]).
    out[0..90].copy_from_slice(negative_pitch_sweep);
    out[90] = table_up_pole; // tilt = 0.0: the shared table-up (pitch 90) pole.
    // [91..181] = tilt 1..=90: positive_pitch_sweep laid down reversed (pitch = 90 -
    // tilt, descending as tilt increases, ending at pitch 0/edge-on at tilt 90).
    for i in 0..90 {
        out[91 + i] = positive_pitch_sweep[89 - i];
    }
    out
}

/// Calculates a full-axis, 181-point angular profile spanning the ENTIRE `-90°..=+90°`
/// TILT range for one axis of [`PROFILE_AZIMUTHS_DEG`].
///
/// Tilt is measured AWAY FROM TABLE-UP -- see [`TILT_ANGLES_DEG`]. Returns
/// (Brilliance %, Extinction %, Windowing %) at 1° resolution. A different
/// parameterisation of the same [`evaluate_gem_optical_metrics`] machinery, not a
/// replacement for [`evaluate_angular_profile_at_azimuth`] (which sweeps camera
/// ELEVATION at a single fixed azimuth): this one sweeps TILT AWAY FROM TABLE-UP,
/// switching between two opposite azimuths partway through. Do not "unify"
/// the two -- they answer different questions.
///
/// # The tilt-to-pose formula
///
/// For tilt `t` (`-90..=+90°`) on the axis whose positive azimuth is `A =
/// positive_azimuth_deg`:
///
/// ```text
/// cam_pitch = (90 - |t|).to_radians()
/// cam_yaw   = A          when t >= 0
///             A + 180    when t <  0
/// ```
///
/// `t = 0` -> `cam_pitch = 90°`: table-up. `t = ±90` -> `cam_pitch = 0°`: edge-on,
/// approached from the two opposite azimuths. This is the relationship the catalogue's
/// performance filters are phrased against (e.g. "windowing never exceeds 20% within
/// ±45° [of table-up]").
///
/// # Why the negative half is a real sweep, not a mirror
///
/// [`evaluate_gem_optical_metrics`] takes a FIXED `light_yaw`/`light_pitch`. Tilting
/// toward vs. away from the light gives genuinely different brilliance/extinction/
/// windowing, even for a symmetric round brilliant, and an asymmetric outline (pear,
/// heart, half-moon) is not 2-fold symmetric geometrically either. So
/// `positive_azimuth_deg + 180°` is independently raytraced at every pitch, never
/// derived by reflecting the positive half.
///
/// # Why table-up (not edge-on) is the point that's genuinely shared
///
/// `Camera::new` computes `origin = (d*cos(pitch)*sin(yaw), d*sin(pitch),
/// d*cos(pitch)*cos(yaw))`. At `pitch == 0°` (edge-on), `origin` depends on `yaw` -- the
/// two azimuths put the camera in different places. At `pitch == 90°` (table-up),
/// `origin ≈ (0, d, 0)` for every yaw -- one single pose, and `Camera::new`'s
/// `world_up` fallback switches exactly at this pole too, keeping `right`/`up` from
/// spinning with `yaw` there. Table-up is therefore the one pose in this sweep that is
/// actually azimuth-independent, which is what makes evaluating it once and sharing it
/// between both halves correct rather than merely convenient (an earlier version
/// shared edge-on instead -- see [`TILT_ANGLES_DEG`]'s doc comment for why that was a
/// genuine bug).
///
/// Net cost: `90 + 1 + 90 = 181` raytrace evaluations per axis (the shared table-up
/// point is evaluated once, not twice).
#[must_use]
pub fn evaluate_full_axis_profile_at_azimuth(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    positive_azimuth_deg: f32,
    light_yaw: f32,
    light_pitch: f32,
) -> ([f32; 181], [f32; 181], [f32; 181]) {
    // The shared table-up (pitch 90) pole -- see this function's doc comment for why
    // evaluating it once is sound. Evaluated at the positive azimuth by convention
    // (azimuth is provably irrelevant here, but a concrete choice is still needed).
    let table_up_pole = evaluate_gem_optical_metrics(
        planes,
        material,
        positive_azimuth_deg.to_radians(),
        90.0f32.to_radians(),
        light_yaw,
        light_pitch,
    );
    let (positive_brilliance, positive_extinction, positive_windowing) = sample_elevation_sweep(
        planes,
        material,
        positive_azimuth_deg.to_radians(),
        &HALF_AXIS_PITCH_DEG,
        light_yaw,
        light_pitch,
    );
    let negative_azimuth_deg = positive_azimuth_deg + 180.0;
    let (negative_brilliance, negative_extinction, negative_windowing) = sample_elevation_sweep(
        planes,
        material,
        negative_azimuth_deg.to_radians(),
        &HALF_AXIS_PITCH_DEG,
        light_yaw,
        light_pitch,
    );

    (
        merge_full_axis_halves(
            table_up_pole.brilliance_pct,
            &positive_brilliance,
            &negative_brilliance,
        ),
        merge_full_axis_halves(
            table_up_pole.extinction_pct,
            &positive_extinction,
            &negative_extinction,
        ),
        merge_full_axis_halves(
            table_up_pole.windowing_pct,
            &positive_windowing,
            &negative_windowing,
        ),
    )
}

/// Calculates a 19-point angular profile of (Brilliance %, Extinction %, Windowing %)
/// at the canonical (0°) camera azimuth.
///
/// See [`evaluate_angular_profile_at_azimuth`] for the general form this delegates to,
/// and [`PROFILE_AZIMUTHS_DEG`] for what the other three azimuths mean. Sampled over
/// `PoV` tilt elevation angles in exact 5° steps: [0°, 5°, 10°, 15°, 20°, 25°, 30°,
/// 35°, 40°, 45°, 50°, 55°, 60°, 65°, 70°, 75°, 80°, 85°, 90°].
#[must_use]
pub fn evaluate_angular_profile(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    light_yaw: f32,
    light_pitch: f32,
) -> ([f32; 19], [f32; 19], [f32; 19]) {
    evaluate_angular_profile_at_azimuth(planes, material, 0.0, light_yaw, light_pitch)
}
