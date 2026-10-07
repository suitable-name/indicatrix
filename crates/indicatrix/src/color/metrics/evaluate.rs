//! [`evaluate_gem_optical_metrics`]: the main analytical-ray-fan grid loop
//! that fires every ray this module's other files classify, and aggregates
//! their results into one pose's [`GemOpticalMetrics`].
//!
//! The same pass can also measure the face-up tone ([`evaluate_gem_optical_metrics_with_tone`]):
//! every returned ray's Fresnel weight and internal path length go into a fixed histogram
//! (see the private `tone` module), turned into a CIELAB colour under the environment's
//! lighting preset at the end. The plain entry points run the identical code with the tone
//! switched off, so their metrics are bit-for-bit unchanged.

use super::{
    camera::camera_view_basis,
    classify::{ApertureSampleContext, RayClassification, classify_aperture_sample},
    fan::{FanGeometry, GRID_DISC_RADIUS_SQ},
    lighting::ExitLighting,
    ray_trace::StoneArena,
    scintillation::{
        TemporalPoseContext, cell_temporal_variance, combine_scintillation_pct,
        spatial_scintillation_pct, temporal_scintillation_pct,
    },
    tone::{FaceUpTone, ToneAccumulator, ToneIlluminant},
    types::GemOpticalMetrics,
};
use crate::{
    geometry::{plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, build_plane_soa},
    },
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
    stone: StoneArena<'a>,
    fan: FanGeometry,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'a>,
) -> GridEvalSetup<'a> {
    let nd = material.dispersion.evaluate(589.3).max(1.1);
    // Clamped defensively like `nd` above so `trace_wavelength`'s entry refraction
    // never hits the pathological `EntryBlocked` case for these two indices either.
    let n_f = material.dispersion.evaluate(486.1).max(1.001);
    let n_c = material.dispersion.evaluate(656.3).max(1.001);

    let grid_size = 18;

    // Matches the real render camera's frame exactly (see `camera_view_basis`).
    let (cam_forward, cam_right, cam_up) = camera_view_basis(cam_yaw, cam_pitch);

    // The illumination is read from the same radiance the tracer lights the image with
    // (see `ExitLighting`), so the metrics describe the scene on screen.
    let lighting = ExitLighting::new(environment);

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
        stone,
        nd,
        cam_yaw,
        cam_pitch,
        fan,
        lighting: lighting.clone(),
    };

    // Shared context for the per-aperture-sample classification (see
    // `ApertureSampleContext`'s doc): identical across every grid cell and sample.
    let aperture_ctx = ApertureSampleContext {
        stone,
        nd,
        n_f,
        n_c,
        cam_forward,
        cam_right,
        cam_up,
        fan,
        lighting,
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
/// Rays are fired from (`cam_yaw`, `cam_pitch`) through the 3D cutting instructions' facet
/// geometry, dynamically accounting for:
/// 1. Gemstone refractive index n(λ) from Sellmeier / Cauchy equations
/// 2. Snell's law refraction at inclined crown & girdle facet entry points
/// 3. Total Internal Reflection (TIR) vs bottom leakage (Windowing) on pavilion facets
/// 4. The scene's illumination (`environment`: the radiance the tracer lights the image
///    with, relative to its ambient level) vs head-shadow extinction
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
    environment: EnvironmentSource<'_>,
) -> GemOpticalMetrics {
    evaluate_gem_optical_metrics_geom(
        StoneGeometry::planes_only(planes),
        material,
        cam_yaw,
        cam_pitch,
        environment,
    )
}

/// [`evaluate_gem_optical_metrics`] plus the [`FaceUpTone`] of the returned light.
///
/// Taken at the same pose, from the same grid pass (the metrics are bit-identical to the
/// plain entry point). The tone is taken under the lighting preset of `environment` (see
/// [`ToneIlluminant::for_environment`]).
#[must_use]
pub fn evaluate_gem_optical_metrics_with_tone(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'_>,
) -> (GemOpticalMetrics, FaceUpTone) {
    evaluate_gem_optical_metrics_with_tone_geom(
        StoneGeometry::planes_only(planes),
        material,
        cam_yaw,
        cam_pitch,
        environment,
    )
}

/// [`evaluate_gem_optical_metrics_with_tone`] for a stone with tools.
#[must_use]
pub fn evaluate_gem_optical_metrics_with_tone_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'_>,
) -> (GemOpticalMetrics, FaceUpTone) {
    if geom.planes.is_empty() {
        return (placeholder_metrics(), FaceUpTone::NONE);
    }
    let half_width = FanGeometry::for_planes(geom.planes).half_width();
    let mut tone = ToneAccumulator::new(half_width);
    let metrics = evaluate_pose(
        geom,
        material,
        cam_yaw,
        cam_pitch,
        environment,
        Some(&mut tone),
    );
    let illuminant = ToneIlluminant::for_environment(environment);
    (metrics, tone.finish(material, illuminant))
}

/// Display defaults for "no geometry loaded", not measurements.
const fn placeholder_metrics() -> GemOpticalMetrics {
    GemOpticalMetrics {
        brilliance_pct: 85.0,
        fire_index: 25.0,
        scintillation_pct: 75.0,
        windowing_pct: 5.0,
        extinction_pct: 5.0,
    }
}

/// [`evaluate_gem_optical_metrics`] for a stone with tools.
///
/// Every ray goes through the one stone intersector, so the tools are subtracted from
/// the polyhedron `geom.planes` define; with no tools the result is
/// [`evaluate_gem_optical_metrics`]'s, bit for bit. The fan origins sit outside the
/// stone because the fan is scaled to `P` alone (a cavity only removes material, so the
/// polyhedron's girdle width bounds the stone). The traced ray follows the same
/// refract-then-bounce loop as the planar case, so a path that leaves through a groove
/// wall and would re-enter across the cavity is scored as having left: a v1 simplification
/// shared with the tracer's exit split.
#[must_use]
pub fn evaluate_gem_optical_metrics_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'_>,
) -> GemOpticalMetrics {
    evaluate_pose(geom, material, cam_yaw, cam_pitch, environment, None)
}

/// The one grid pass behind every entry point; `tone`, when present, also collects the
/// returned rays' Fresnel weights and internal paths (one `if let` per returned ray).
fn evaluate_pose(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'_>,
    mut tone: Option<&mut ToneAccumulator>,
) -> GemOpticalMetrics {
    let planes = geom.planes;
    if planes.is_empty() {
        // No facet geometry to trace: fall back to neutral placeholder values rather
        // than a formula.
        return placeholder_metrics();
    }

    let mut acc = MetricsAccumulators::default();
    // Checked once here (not once per ray), gated behind `diag_fire_debug` in the hot
    // loop below so a normal run pays no cost for the env lookup or extra additions.
    let diag_fire_debug = std::env::var("DIAG_FIRE_DEBUG").is_ok();

    // SIMD slab arena, built once per evaluation: every ray this function fires (grid,
    // sub-aperture, temporal sub-poses, F/C lines) intersects the same solid.
    let plane_soa = build_plane_soa(planes);
    // The fan is scaled to the stone once per evaluation (the measurement itself is
    // reused while the design is unchanged -- see `FanGeometry`).
    let fan = FanGeometry::for_planes(planes);
    let stone = StoneArena {
        plane_soa: &plane_soa,
        tools: geom.tools,
    };
    let setup = build_grid_eval_setup(stone, fan, material, cam_yaw, cam_pitch, environment);

    for ix in 0..setup.grid_size {
        for iz in 0..setup.grid_size {
            let u = ((ix as f32 + 0.5) / (setup.grid_size as f32)).mul_add(2.0, -1.0);
            let v = ((iz as f32 + 0.5) / (setup.grid_size as f32)).mul_add(2.0, -1.0);
            if v.mul_add(v, u * u) > GRID_DISC_RADIUS_SQ {
                continue; // Stay within the fan's disc
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
                if let Some(t) = tone.as_deref_mut() {
                    t.count_total();
                }

                match classification {
                    RayClassification::EntryBlocked => {}
                    RayClassification::Windowed => acc.windowed_rays += 1,
                    RayClassification::Extinct => acc.extinct_rays += 1,
                    RayClassification::Returned(ray) => {
                        acc.returned_rays += 1;
                        cell_returned += 1;
                        if let Some(t) = tone.as_deref_mut() {
                            t.offer(ray.transmittance, ray.path_len);
                        }

                        if let Some((angle_deg, weight)) = ray.fire {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
        optics::raytracer::LightingPreset,
    };
    use glam::Vec3;

    fn bits(m: GemOpticalMetrics) -> [u32; 5] {
        [
            m.brilliance_pct.to_bits(),
            m.fire_index.to_bits(),
            m.scintillation_pct.to_bits(),
            m.windowing_pct.to_bits(),
            m.extinction_pct.to_bits(),
        ]
    }

    use crate::optics::materials::body_color::BODY_COLOR_PRESETS;
    use std::f32::consts::FRAC_PI_2;

    fn tone_bits(t: FaceUpTone) -> [u32; 7] {
        [
            t.l_star.to_bits(),
            t.chroma.to_bits(),
            t.hue_deg.to_bits(),
            u32::from(t.srgb[0]),
            u32::from(t.srgb[1]),
            t.mean_path_units.to_bits(),
            t.returned_fraction.to_bits(),
        ]
    }

    fn blue_quartz() -> GemMaterial {
        GemMaterial::by_name("Quartz")
            .expect("Quartz resolves")
            .with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb)
    }

    fn table_up_tone(
        planes: &[GpuFacetPlane],
        material: &GemMaterial,
        preset: LightingPreset,
    ) -> FaceUpTone {
        evaluate_gem_optical_metrics_with_tone(
            planes,
            material,
            0.0,
            FRAC_PI_2,
            preset.studio(1.0, 0.85, 0.95),
        )
        .1
    }

    #[test]
    fn tone_pass_leaves_every_metric_bit_identical() {
        let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let materials = [
            GemMaterial::diamond(),
            GemMaterial::sapphire().with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb),
        ];
        for planes in [
            StandardGemCuts::standard_round_brilliant(),
            StandardGemCuts::emerald_cut(),
        ] {
            for material in &materials {
                for (yaw, pitch) in [(0.0, FRAC_PI_2), (0.6, 0.45)] {
                    let plain =
                        evaluate_gem_optical_metrics(&planes, material, yaw, pitch, environment);
                    let (with, _) = evaluate_gem_optical_metrics_with_tone(
                        &planes,
                        material,
                        yaw,
                        pitch,
                        environment,
                    );
                    assert_eq!(bits(plain), bits(with));
                }
            }
        }
    }

    #[test]
    fn a_colourless_stone_is_neutral_under_every_preset() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        for preset in LightingPreset::ALL {
            // UV lamps are out of scope: no ray returns table-up, so the tone is all zeros.
            if matches!(
                preset,
                LightingPreset::UvLamp365 | LightingPreset::UvLamp395
            ) {
                continue;
            }
            let environment = preset.studio(1.0, 0.85, 0.95);
            let (metrics, tone) = evaluate_gem_optical_metrics_with_tone(
                &planes,
                &material,
                0.0,
                FRAC_PI_2,
                environment,
            );
            assert!(tone.l_star > 99.5, "{preset:?}: {tone:?}");
            assert!(tone.chroma < 0.5, "{preset:?}: {tone:?}");
            assert!(tone.srgb.iter().all(|&c| c >= 253), "{preset:?}: {tone:?}");
            assert!(
                (tone.returned_fraction - metrics.brilliance_pct / 100.0).abs() < 1e-4,
                "{preset:?}"
            );
            assert!(tone.mean_path_units > 0.0, "{preset:?}");
        }
    }

    #[test]
    fn the_preset_light_changes_a_coloured_stones_tone() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = blue_quartz();
        let day = table_up_tone(&planes, &material, LightingPreset::Daylight);
        let lamp = table_up_tone(&planes, &material, LightingPreset::Incandescent);
        assert!(
            day.l_star.to_bits() != lamp.l_star.to_bits()
                || day.chroma.to_bits() != lamp.chroma.to_bits()
        );
        assert!(
            lamp.chroma < day.chroma,
            "chroma: Incandescent {} vs Daylight {}",
            lamp.chroma,
            day.chroma
        );
    }

    #[test]
    fn the_same_preset_gives_the_same_bits_and_the_tent_uses_its_planckian_light() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = blue_quartz();
        let tent = table_up_tone(&planes, &material, LightingPreset::LightTent);
        let tent_again = table_up_tone(&planes, &material, LightingPreset::LightTent);
        let day = table_up_tone(&planes, &material, LightingPreset::Daylight);
        assert_eq!(tone_bits(tent), tone_bits(tent_again));
        assert_ne!(tone_bits(tent), tone_bits(day));
    }

    #[test]
    fn a_coloured_stone_is_darker_and_more_saturated_than_colourless() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let tone = table_up_tone(&planes, &blue_quartz(), LightingPreset::Daylight);
        assert!(tone.l_star < 95.0, "{tone:?}");
        assert!(tone.chroma > 5.0, "{tone:?}");
        assert!(tone.srgb[2] > tone.srgb[0], "{tone:?}");
    }

    #[test]
    fn a_larger_stone_is_darker() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let small = table_up_tone(&planes, &blue_quartz(), LightingPreset::Daylight);
        let big = table_up_tone(
            &planes,
            &blue_quartz().with_absorption_path_scale(2.0),
            LightingPreset::Daylight,
        );
        assert!(big.l_star < small.l_star, "{small:?} vs {big:?}");
        assert_eq!(
            big.mean_path_units.to_bits(),
            small.mean_path_units.to_bits()
        );
    }

    #[test]
    fn a_pale_colour_gains_chroma_with_size_then_loses_it() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let [r, g, b] = BODY_COLOR_PRESETS[1].absorption_rgb;
        let pale = GemMaterial::by_name("Quartz")
            .expect("Quartz resolves")
            .with_body_color([r * 0.1, g * 0.1, b * 0.1]);
        let tones: Vec<FaceUpTone> = [1.0f32, 4.0, 16.0, 64.0]
            .iter()
            .map(|&s| {
                table_up_tone(
                    &planes,
                    &pale.clone().with_absorption_path_scale(s),
                    LightingPreset::Daylight,
                )
            })
            .collect();
        assert!(tones[1].chroma > tones[0].chroma, "{tones:?}");
        assert!(tones[3].chroma < tones[2].chroma, "{tones:?}");
        for pair in tones.windows(2) {
            assert!(pair[1].l_star < pair[0].l_star, "{tones:?}");
        }
    }

    #[test]
    fn no_returned_ray_is_black() {
        let (_, tone) = evaluate_gem_optical_metrics_with_tone(
            &[],
            &blue_quartz(),
            0.0,
            FRAC_PI_2,
            LightingPreset::Daylight.studio(1.0, 0.85, 0.95),
        );
        assert_eq!(tone, FaceUpTone::NONE);
    }

    #[test]
    fn the_tone_is_deterministic() {
        let planes = StandardGemCuts::emerald_cut();
        let material = blue_quartz();
        let a = table_up_tone(&planes, &material, LightingPreset::RingLights);
        let b = table_up_tone(&planes, &material, LightingPreset::RingLights);
        assert_eq!(tone_bits(a), tone_bits(b));
        assert_eq!(a.srgb, b.srgb);
    }

    #[test]
    fn metrics_on_a_planar_design_are_unchanged_by_the_geometry_entry_point() {
        let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let material = GemMaterial::diamond();
        for planes in [
            StandardGemCuts::standard_round_brilliant(),
            StandardGemCuts::emerald_cut(),
        ] {
            let old = evaluate_gem_optical_metrics(&planes, &material, 0.6, 0.45, environment);
            let new = evaluate_gem_optical_metrics_geom(
                StoneGeometry::planes_only(&planes),
                &material,
                0.6,
                0.45,
                environment,
            );
            assert_eq!(bits(old), bits(new));
        }
    }

    #[test]
    fn metrics_of_a_grooved_stone_are_finite_and_differ_from_the_planar_stone() {
        let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let material = GemMaterial::diamond();
        let planes = StandardGemCuts::standard_round_brilliant();
        let tools = [ToolPrimitive::cylinder(
            Vec3::new(0.0, 0.44, 0.0),
            Vec3::X,
            0.3,
            2.0,
        )];
        let planar = evaluate_gem_optical_metrics(&planes, &material, 0.6, 1.2, environment);
        let grooved = evaluate_gem_optical_metrics_geom(
            StoneGeometry {
                planes: &planes,
                tools: &tools,
            },
            &material,
            0.6,
            1.2,
            environment,
        );
        for v in bits(grooved) {
            assert!(f32::from_bits(v).is_finite());
        }
        assert_ne!(bits(planar), bits(grooved));
    }
}
