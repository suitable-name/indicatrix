//! [`evaluate_gem_optical_metrics`]: the main analytical-ray-fan grid loop
//! that fires every ray this module's other files classify, and aggregates
//! their results into one pose's [`GemOpticalMetrics`].

use super::{
    camera::camera_view_basis,
    classify::{ApertureSampleContext, RayClassification, classify_aperture_sample},
    scintillation::{
        TemporalPoseContext, cell_temporal_variance, combine_scintillation_pct,
        spatial_scintillation_pct, temporal_scintillation_pct,
    },
    types::GemOpticalMetrics,
};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::build_plane_soa},
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
/// Calibrated at 275 (Diamond, standard round brilliant, yaw 0.0/pitch 0.45/light
/// 0.85-0.95, post F/C-bifurcation-gate weighted-mean separation ~0.0716 deg) to land
/// `fire_index` ~= 19.7; every built-in material on both cuts stays comfortably under
/// 100 at this scale. This scale, together with the bifurcation gate, fixes the
/// emerald-cut Fire ordering -- see `evaluate_gem_optical_metrics`'s doc comment.
const FIRE_DEGREES_TO_DISPLAY_SCALE: f32 = 275.0;

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
