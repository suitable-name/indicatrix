//! Scintillation: the spatial (per-pose grid contrast) and temporal
//! (per-cell, across-pose flicker) terms, combined into `scintillation_pct`.

use super::{
    camera::camera_view_basis,
    fan::FanGeometry,
    lighting::ExitLighting,
    ray_trace::{RayFate, StoneArena, trace_wavelength},
    visibility::ray_is_visibly_returned,
};
use crate::optics::raytracer::Ray;

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
pub(super) struct TemporalPoseContext<'a> {
    pub(super) stone: StoneArena<'a>,
    pub(super) nd: f32,
    pub(super) cam_yaw: f32,
    pub(super) cam_pitch: f32,
    /// Where the fan's rays start, scaled to the stone.
    pub(super) fan: FanGeometry,
    /// The illumination an exit direction is judged against.
    pub(super) lighting: ExitLighting<'a>,
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
        origin: ctx.fan.origin(forward, right, up, u, v),
        dir: forward,
    };
    let Some(hit_rec) = ctx.stone.intersect(ray) else {
        return false;
    };
    let hit_point = ray.origin + hit_rec.t * ray.dir;
    let n_entry = hit_rec.normal;
    let cos_i = (-ray.dir).dot(n_entry).clamp(0.0, 1.0);

    match trace_wavelength(hit_point, ray.dir, n_entry, cos_i, ctx.stone, ctx.nd) {
        RayFate::ExitedUpward(exit) => ray_is_visibly_returned(exit.dir, forward, &ctx.lighting),
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
pub(super) fn cell_temporal_variance(ctx: &TemporalPoseContext, u: f32, v: f32) -> f32 {
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
pub(super) fn spatial_scintillation_pct(
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
pub(super) fn temporal_scintillation_pct(temporal_variance_sum: f32, cell_count: u32) -> f32 {
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
pub(super) fn combine_scintillation_pct(spatial_pct: f32, temporal_pct: f32) -> f32 {
    SCINT_TEMPORAL_WEIGHT
        .mul_add(temporal_pct, SCINT_SPATIAL_WEIGHT * spatial_pct)
        .clamp(0.0, 100.0)
}
