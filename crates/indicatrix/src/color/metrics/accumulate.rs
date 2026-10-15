//! The per-pose accumulators of the grid pass and the step that turns them into one pose's
//! [`GemOpticalMetrics`].
//!
//! Split out of `evaluate` (which owns the grid loop that fills [`MetricsAccumulators`]) so
//! each file stays within the house size limit.

use super::{
    evaluate::{MetricsWanted, placeholder_metrics},
    scintillation::{
        combine_scintillation_pct, spatial_scintillation_pct, temporal_scintillation_pct,
    },
    types::GemOpticalMetrics,
};

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
/// Chosen at 275 so a Diamond standard round brilliant at yaw 0.0/pitch 0.45 (weighted-mean
/// separation ~0.0716 deg after the F/C bifurcation gate) read `fire_index` ~= 19.7, under
/// the earlier cone-based illumination test and a fixed 0.8-unit fan. It has not been
/// re-derived for the radiance-based illumination test and girdle-scaled fan described in
/// the module docs, so absolute values are comparable between stones measured by the
/// same build, not with older readings. For the emerald-cut Fire ordering this scale was
/// chosen with, see `evaluate_gem_optical_metrics`'s doc comment.
const FIRE_DEGREES_TO_DISPLAY_SCALE: f32 = 275.0;

/// Aggregate accumulators threaded through the grid pass of `evaluate_pose`, bundled into
/// one `#[derive(Default)]` struct to keep the functions under clippy's line-count budget
/// without losing per-field rationale, which lives on the fields below.
#[derive(Default)]
pub(super) struct MetricsAccumulators {
    pub(super) total_rays: u32,
    pub(super) windowed_rays: u32,
    pub(super) extinct_rays: u32,
    pub(super) returned_rays: u32,
    /// ENERGY-WEIGHTED sum of per-ray F-line/C-line exit angular separations (degrees),
    /// over rays that exit upward through the crown at the d-line, pass the same
    /// illumination test as brilliance, and whose F-line/C-line companion traces also
    /// both exit upward. Each contribution is weighted by that ray's own Fresnel
    /// entry*exit transmittance, and the sum is normalized by TOTAL incident rays
    /// (`n_total`) rather than the count of qualifying rays -- so a stone that returns
    /// little light cannot score highly on a small, wide-angle survivor population.
    pub(super) fire_energy_weighted_sum_deg: f32,
    /// Diagnostic-only accumulators for the `DIAG_FIRE_DEBUG` eprintln block, gated
    /// behind `diag_fire_debug` in the hot loop so a normal run pays no extra cost.
    pub(super) dbg_fire_qualifying: u32,
    pub(super) dbg_fire_angle_sum_unweighted: f32,
    pub(super) dbg_fire_transmittance_sum: f32,
    /// Scintillation accumulators: per-grid-cell fraction of aperture samples that
    /// returned illuminated brilliance, aggregated into a coefficient of variation
    /// across the 18x18 grid at the end.
    pub(super) cell_fraction_sum: f32,
    pub(super) cell_fraction_sum_sq: f32,
    pub(super) cell_count: u32,
    /// Scintillation TEMPORAL accumulator: per visited cell, the Bernoulli variance
    /// (max 0.24 at this sample count -- see `cell_returned_at_yaw_offset`) of that
    /// cell's return status across `SCINT_TEMPORAL_YAW_OFFSETS_DEG`, summed and
    /// averaged by `cell_count` after the loop. A cell returning light identically at
    /// every offset contributes 0; one that flips contributes up to 0.24. Stays 0 on the
    /// Fire-less fast path, which skips the temporal sub-poses.
    pub(super) temporal_variance_sum: f32,
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

/// Turns the finished accumulators into the pose's [`GemOpticalMetrics`]. `diag_material`
/// is the material name when `DIAG_FIRE_DEBUG` is on. With Fire and Scintillation not
/// wanted, those two fields are the [`placeholder_metrics`] values.
pub(super) fn finish_metrics(
    acc: &MetricsAccumulators,
    diag_material: Option<&str>,
    wanted: MetricsWanted,
) -> GemOpticalMetrics {
    let n_total = acc.total_rays.max(1) as f32;
    let windowing_pct = (acc.windowed_rays as f32 / n_total * 100.0).clamp(0.0, 100.0);
    let extinction_pct = (acc.extinct_rays as f32 / n_total * 100.0).clamp(0.0, 100.0);
    let brilliance_pct = (acc.returned_rays as f32 / n_total * 100.0).clamp(0.0, 100.0);

    let (fire_index, scintillation_pct) = if wanted.fire_and_scintillation {
        fire_and_scintillation(acc, n_total, diag_material)
    } else {
        let placeholder = placeholder_metrics();
        (placeholder.fire_index, placeholder.scintillation_pct)
    };

    GemOpticalMetrics {
        brilliance_pct,
        fire_index,
        scintillation_pct,
        windowing_pct,
        extinction_pct,
    }
}

/// The `(fire_index, scintillation_pct)` pair of the finished accumulators; the full
/// path's two derived metrics, with their `DIAG_FIRE_DEBUG` diagnostics.
fn fire_and_scintillation(
    acc: &MetricsAccumulators,
    n_total: f32,
    diag_material: Option<&str>,
) -> (f32, f32) {
    // Fire: energy-weighted F-line/C-line angular separation, normalized by TOTAL
    // incident rays (n_total), not the count of rays that happened to qualify -- the
    // same convention as brilliance_pct/windowing_pct/extinction_pct above, closing the
    // loophole where a shrinking denominator lets a few wide-angle survivors from a
    // badly-leaking cut inflate the average. See
    // `MetricsAccumulators::fire_energy_weighted_sum_deg`'s doc. Naturally floors at 0.1
    // when the weighted sum is zero -- no separate branch needed.
    let fire_index =
        (acc.fire_energy_weighted_sum_deg / n_total * FIRE_DEGREES_TO_DISPLAY_SCALE).max(0.1);
    if let Some(material_name) = diag_material {
        log_fire_diagnostics(material_name, n_total, fire_index, acc);
    }

    let spatial_scint_pct = spatial_scintillation_pct(
        acc.cell_fraction_sum,
        acc.cell_fraction_sum_sq,
        acc.cell_count,
    );
    let temporal_pct = temporal_scintillation_pct(acc.temporal_variance_sum, acc.cell_count);
    let scintillation_pct = combine_scintillation_pct(spatial_scint_pct, temporal_pct);
    if let Some(material_name) = diag_material {
        log_scintillation_diagnostics(
            material_name,
            spatial_scint_pct,
            temporal_pct,
            scintillation_pct,
        );
    }
    (fire_index, scintillation_pct)
}
