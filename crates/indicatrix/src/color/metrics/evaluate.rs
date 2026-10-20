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
    accumulate::{MetricsAccumulators, finish_metrics},
    camera::camera_view_basis,
    classify::{ApertureSampleContext, RayClassification, classify_aperture_sample},
    fan::GRID_DISC_RADIUS_SQ,
    prepared::PreparedEval,
    scintillation::{TemporalPoseContext, cell_temporal_variance},
    tone::{FaceUpTone, ToneAccumulator, ToneIlluminant},
    types::GemOpticalMetrics,
};
use crate::{
    geometry::{plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{materials::GemMaterial, raytracer::EnvironmentSource},
};

/// Which outputs of a pose evaluation the caller reads.
///
/// `brilliance_pct`, `extinction_pct` and `windowing_pct` come from the d-line trace alone;
/// `fire_index` needs two more full traces per returned ray and `scintillation_pct` five
/// more single-ray traces per grid cell (about 65% of an evaluation's traces together). The
/// tilt sweep, the optimiser and the AI environment read only the first three, so they ask
/// for [`MetricsWanted::CONSUMED_ONLY`]; the editor's HUD and sweep rows ask for
/// [`MetricsWanted::ALL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MetricsWanted {
    /// Trace the Fire companions and the temporal scintillation sub-poses.
    pub(super) fire_and_scintillation: bool,
}

impl MetricsWanted {
    /// Every metric, the full path.
    pub(super) const ALL: Self = Self {
        fire_and_scintillation: true,
    };
    /// Brilliance, extinction and windowing only: `fire_index` and `scintillation_pct` are
    /// not computed and carry the neutral placeholder values.
    pub(super) const CONSUMED_ONLY: Self = Self {
        fire_and_scintillation: false,
    };
}

/// Everything the main grid loop needs that is fixed across the whole pose: the sampling
/// grid resolution, the sub-aperture jitter bundle, and the two per-ray contexts
/// ([`TemporalPoseContext`], [`ApertureSampleContext`]). The temporal context exists only
/// on the full path.
struct GridEvalSetup<'p> {
    grid_size: i32,
    aperture_samples: [(f32, f32); 5],
    temporal_ctx: Option<TemporalPoseContext<'p>>,
    aperture_ctx: ApertureSampleContext<'p>,
}

/// Builds [`GridEvalSetup`] for one pose from the pose-independent `prepared` state. Only
/// the camera frame (and the five temporal frames) depend on the pose.
fn build_grid_eval_setup<'p>(
    prepared: &'p PreparedEval<'_>,
    cam_yaw: f32,
    cam_pitch: f32,
    wanted: MetricsWanted,
) -> GridEvalSetup<'p> {
    // Matches the real render camera's frame exactly (see `camera_view_basis`).
    let (cam_forward, cam_right, cam_up) = camera_view_basis(cam_yaw, cam_pitch);
    let stone = prepared.stone();

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
    let temporal_ctx = wanted.fire_and_scintillation.then(|| {
        TemporalPoseContext::new(
            stone,
            prepared.nd,
            cam_yaw,
            cam_pitch,
            prepared.fan,
            &prepared.lighting,
        )
    });

    // Shared context for the per-aperture-sample classification (see
    // `ApertureSampleContext`'s doc): identical across every grid cell and sample.
    let aperture_ctx = ApertureSampleContext {
        stone,
        nd: prepared.nd,
        n_f: prepared.n_f,
        n_c: prepared.n_c,
        cam_forward,
        cam_right,
        cam_up,
        fan: prepared.fan,
        lighting: &prepared.lighting,
        want_fire: wanted.fire_and_scintillation,
    };

    GridEvalSetup {
        grid_size: 18,
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
    with_tone(
        geom,
        material,
        cam_yaw,
        cam_pitch,
        environment,
        MetricsWanted::ALL,
    )
}

/// [`evaluate_gem_optical_metrics_with_tone_geom`] without Fire and Scintillation.
///
/// `brilliance_pct`, `extinction_pct` and `windowing_pct` and the [`FaceUpTone`] are
/// bit-identical to the full path's (same counters, same tone offers, same order); the two
/// companion traces per returned ray and the 900 temporal-scintillation traces are skipped,
/// about a 2-3x saving. `fire_index` and `scintillation_pct` are NOT computed: they carry
/// the neutral "no geometry" placeholders (25 and 75), so a caller that displays them must
/// use the full entry point. For the optimiser's Fast and Full objectives, which read only
/// the three percentages and the tone.
#[must_use]
pub fn evaluate_gem_optical_metrics_with_tone_geom_core(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'_>,
) -> (GemOpticalMetrics, FaceUpTone) {
    with_tone(
        geom,
        material,
        cam_yaw,
        cam_pitch,
        environment,
        MetricsWanted::CONSUMED_ONLY,
    )
}

/// The tone entry points' shared body; `wanted` picks the full or the Fire-less pass.
fn with_tone(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'_>,
    wanted: MetricsWanted,
) -> (GemOpticalMetrics, FaceUpTone) {
    let Some(prepared) = PreparedEval::new(geom, material, environment) else {
        return (placeholder_metrics(), FaceUpTone::NONE);
    };
    let mut tone = ToneAccumulator::new(prepared.fan.half_width());
    let metrics = evaluate_pose(&prepared, cam_yaw, cam_pitch, wanted, Some(&mut tone));
    let illuminant = ToneIlluminant::for_environment(environment);
    (metrics, tone.finish(material, illuminant))
}

/// Display defaults for "no geometry loaded", not measurements. Also what the Fire-less
/// fast path reports for the two metrics it does not compute.
pub(super) const fn placeholder_metrics() -> GemOpticalMetrics {
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
    PreparedEval::new(geom, material, environment).map_or_else(placeholder_metrics, |prepared| {
        evaluate_pose(&prepared, cam_yaw, cam_pitch, MetricsWanted::ALL, None)
    })
}

/// [`evaluate_gem_optical_metrics_geom`] without Fire and Scintillation.
///
/// `brilliance_pct`, `extinction_pct` and `windowing_pct` are bit-identical to the full
/// path's: they are counted before and independently of the Fire companion traces and the
/// temporal-scintillation sub-poses, which are skipped here (about a 2-3x saving per
/// evaluation). `fire_index` and `scintillation_pct` are NOT computed: they carry the
/// neutral placeholder values (25 and 75) the full path reports for "no geometry". For the
/// tilt sweep, the optimiser and the AI environment; anything that displays Fire or
/// Scintillation uses [`evaluate_gem_optical_metrics_geom`].
#[must_use]
pub fn evaluate_gem_optical_metrics_geom_core(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    environment: EnvironmentSource<'_>,
) -> GemOpticalMetrics {
    PreparedEval::new(geom, material, environment).map_or_else(placeholder_metrics, |prepared| {
        evaluate_pose(
            &prepared,
            cam_yaw,
            cam_pitch,
            MetricsWanted::CONSUMED_ONLY,
            None,
        )
    })
}

/// The one grid pass behind every entry point, at one camera pose of an already
/// [`PreparedEval`]. `tone`, when present, also collects the returned rays' Fresnel weights
/// and internal paths (one `if let` per returned ray); `wanted` picks whether Fire and
/// Scintillation are traced (see [`MetricsWanted`]).
pub(super) fn evaluate_pose(
    prepared: &PreparedEval<'_>,
    cam_yaw: f32,
    cam_pitch: f32,
    wanted: MetricsWanted,
    tone: Option<&mut ToneAccumulator>,
) -> GemOpticalMetrics {
    let setup = build_grid_eval_setup(prepared, cam_yaw, cam_pitch, wanted);
    let mut acc = MetricsAccumulators::default();
    // Gated behind `diag_fire_debug` in the hot loop so a normal run pays no cost for the
    // extra additions; the env lookup itself happened once, in `PreparedEval::new`.
    run_grid(&setup, &mut acc, tone, prepared.diag_material.is_some());
    finish_metrics(&acc, prepared.diag_material.as_deref(), wanted)
}

/// Fires every grid cell inside the fan's disc and folds the classified rays into `acc`
/// (and `tone`). The Fire accumulator update is applied here, not inside
/// `classify_aperture_sample`, to preserve the exact `f32::mul_add` chain across cells.
fn run_grid(
    setup: &GridEvalSetup<'_>,
    acc: &mut MetricsAccumulators,
    mut tone: Option<&mut ToneAccumulator>,
    diag_fire_debug: bool,
) {
    for ix in 0..setup.grid_size {
        for iz in 0..setup.grid_size {
            let u = ((ix as f32 + 0.5) / (setup.grid_size as f32)).mul_add(2.0, -1.0);
            let v = ((iz as f32 + 0.5) / (setup.grid_size as f32)).mul_add(2.0, -1.0);
            if v.mul_add(v, u * u) > GRID_DISC_RADIUS_SQ {
                continue; // Stay within the fan's disc
            }

            // Per-cell counters for the Scintillation spatial-contrast measurement:
            // how many aperture samples hit the stone, and how many returned brilliance.
            let (cell_total, cell_returned) =
                sample_cell(setup, acc, tone.as_deref_mut(), diag_fire_debug, u, v);

            if cell_total > 0 {
                let frac = cell_returned as f32 / cell_total as f32;
                acc.cell_fraction_sum += frac;
                acc.cell_fraction_sum_sq = frac.mul_add(frac, acc.cell_fraction_sum_sq);
                acc.cell_count += 1;
                if let Some(temporal) = &setup.temporal_ctx {
                    acc.temporal_variance_sum += cell_temporal_variance(temporal, u, v);
                }
            }
        }
    }
}

/// Classifies the cell's sub-aperture bundle at `(u, v)`, updating the ray counters, the
/// tone offers and the Fire sum; returns how many samples hit the stone and how many of
/// those were visibly returned.
fn sample_cell(
    setup: &GridEvalSetup<'_>,
    acc: &mut MetricsAccumulators,
    mut tone: Option<&mut ToneAccumulator>,
    diag_fire_debug: bool,
    u: f32,
    v: f32,
) -> (u32, u32) {
    let mut cell_total = 0u32;
    let mut cell_returned = 0u32;

    for &(dx_sub, dz_sub) in &setup.aperture_samples {
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
            RayClassification::EntryBlocked | RayClassification::Invalid => {}
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
    (cell_total, cell_returned)
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

    /// The Fire-less pass is the full pass minus two derived metrics: the three counters it
    /// still reports, and the face-up tone, are bit-identical; Fire and Scintillation carry
    /// the "not computed" placeholders. Planar and grooved stones, two poses each.
    #[test]
    fn the_core_path_matches_the_full_path_on_every_consumed_output() {
        let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        let material = GemMaterial::diamond();
        let planes = StandardGemCuts::standard_round_brilliant();
        let tools = [ToolPrimitive::cylinder(
            Vec3::new(0.0, 0.44, 0.0),
            Vec3::X,
            0.3,
            2.0,
        )];
        for geom in [
            StoneGeometry::planes_only(&planes),
            StoneGeometry {
                planes: &planes,
                tools: &tools,
            },
        ] {
            for (yaw, pitch) in [(0.0, FRAC_PI_2), (0.6, 0.45)] {
                let full =
                    evaluate_gem_optical_metrics_geom(geom, &material, yaw, pitch, environment);
                let core = evaluate_gem_optical_metrics_geom_core(
                    geom,
                    &material,
                    yaw,
                    pitch,
                    environment,
                );
                assert_eq!(full.brilliance_pct.to_bits(), core.brilliance_pct.to_bits());
                assert_eq!(full.extinction_pct.to_bits(), core.extinction_pct.to_bits());
                assert_eq!(full.windowing_pct.to_bits(), core.windowing_pct.to_bits());
                let placeholder = placeholder_metrics();
                assert_eq!(core.fire_index.to_bits(), placeholder.fire_index.to_bits());
                assert_eq!(
                    core.scintillation_pct.to_bits(),
                    placeholder.scintillation_pct.to_bits()
                );
                let (_, full_tone) = evaluate_gem_optical_metrics_with_tone_geom(
                    geom,
                    &material,
                    yaw,
                    pitch,
                    environment,
                );
                let (_, core_tone) = evaluate_gem_optical_metrics_with_tone_geom_core(
                    geom,
                    &material,
                    yaw,
                    pitch,
                    environment,
                );
                assert_eq!(tone_bits(full_tone), tone_bits(core_tone));
            }
        }
    }
}
