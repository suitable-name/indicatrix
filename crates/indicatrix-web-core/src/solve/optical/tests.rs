//! The metrics and tilt handlers against direct calls of the functions the desktop calls,
//! on the built-in template designs -- bit for bit.

use std::sync::atomic::{AtomicBool, Ordering};

use indicatrix::{
    color::metrics::{
        GemOpticalMetrics, PROFILE_AZIMUTHS_DEG, evaluate_full_axis_profile_at_azimuth,
        evaluate_gem_optical_metrics,
    },
    optics::materials::GemMaterial,
    render_setup::{MaterialOverrides, apply_material_overrides, resolve_material_with_override},
};
use indicatrix_cut_core::{
    Design, FreshDesignSpec, MaterialSelection, PreformSpec, templates::TEMPLATES,
};
use indicatrix_editor::{EditorSession, solve_policy::design_to_gpu_planes_from_solved};

use super::*;
use crate::{
    scene::{CameraSpec, FinishSpec, LightingSpec, planes_to_data},
    solve::SolveRequest,
};
use indicatrix::{optics::raytracer::LightingPreset, render_setup::Backdrop};

fn template_design(template_index: i32) -> Design {
    EditorSession::from_template(
        FreshDesignSpec {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material: MaterialSelection::none(),
            preform: PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        },
        template_index,
    )
    .design
}

/// The frame the browser app renders for `design`: solved planes, the given material and
/// overrides, and a camera/light pose.
fn scene_of(design: &Design, material: MaterialSpec) -> (SceneSpec, Vec<GpuFacetPlane>) {
    let solved = design.solve().expect("every template solves");
    let planes = design_to_gpu_planes_from_solved(design, &solved);
    let spec = SceneSpec {
        planes: planes_to_data(&planes),
        finishes: FinishSpec::FrostedGirdle,
        material,
        camera: CameraSpec {
            yaw: 0.61,
            pitch: 0.47,
            distance: 2.4,
        },
        lighting: LightingSpec::new(LightingPreset::LightTent, 1.7, 0.84, 0.93, Backdrop::White),
        max_bounces: 24,
        width: 1600,
        height: 1200,
        hdr_id: None,
    };
    (spec, planes)
}

fn metric_bits(m: &GemOpticalMetrics) -> [u32; 5] {
    [
        m.brilliance_pct.to_bits(),
        m.fire_index.to_bits(),
        m.scintillation_pct.to_bits(),
        m.windowing_pct.to_bits(),
        m.extinction_pct.to_bits(),
    ]
}

fn result_bits(m: &MetricsResultData) -> [u32; 5] {
    [
        m.brilliance_pct.to_bits(),
        m.fire_index.to_bits(),
        m.scintillation_pct.to_bits(),
        m.windowing_pct.to_bits(),
        m.extinction_pct.to_bits(),
    ]
}

fn metrics_of(response: SolveResponse) -> MetricsResultData {
    match response {
        SolveResponse::Metrics(m) => m,
        other => panic!("expected metrics, got {other:?}"),
    }
}

fn no_hooks<R>(f: impl FnOnce(&SolveHooks<'_>) -> R) -> R {
    let cancel = AtomicBool::new(false);
    f(&SolveHooks {
        cancel: &cancel,
        on_progress: &|_| {},
        on_sweep: &|_| {},
    })
}

/// The metrics are `evaluate_gem_optical_metrics` for the frame's pose with the material
/// the desktop renders (`resolve_material_with_override` + `apply_material_overrides`),
/// on every template and with the render-time overrides on.
#[test]
fn metrics_equal_the_desktop_call_on_every_template() {
    let overrides = MaterialOverridesSpec {
        inclusion_sigma_s: 1.2,
        c_axis_override: None,
        edge_rounding_radius: 0.01,
        stone_width_mm: 0.0,
    };
    for (i, template) in TEMPLATES.iter().enumerate() {
        let design = template_design(i as i32 + 1);
        for name in ["Diamond", "Sapphire"] {
            let material_spec = MaterialSpec {
                overrides,
                ..MaterialSpec::catalogue(name)
            };
            let (spec, planes) = scene_of(&design, material_spec);
            let got = metrics_of(run_metrics(&mut None, &MetricsParams::from_scene(&spec)));

            let base =
                resolve_material_with_override(&GemMaterial::all_materials(), &[], None, name)
                    .expect("built-in");
            let material = apply_material_overrides(
                base,
                &MaterialOverrides {
                    inclusion_sigma_s: 1.2,
                    c_axis_override: None,
                    edge_rounding_radius: 0.01,
                    stone_width_mm: 0.0,
                },
                None,
            );
            let direct = evaluate_gem_optical_metrics(
                &planes,
                &material,
                0.61,
                0.47,
                LightingPreset::LightTent.studio(1.0, 0.84, 0.93),
            );
            assert_eq!(
                result_bits(&got),
                metric_bits(&direct),
                "{} / {name}",
                template.name
            );
        }
    }
}

/// A cache the Worker keeps answers a repeated pose without changing the number, and a
/// moved camera or light computes a fresh one.
#[test]
fn a_cached_pose_and_a_moved_pose_both_match_the_direct_call() {
    let design = template_design(1);
    let (spec, planes) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    let params = MetricsParams::from_scene(&spec);
    let material = GemMaterial::diamond();
    let mut cache = None;
    let first = metrics_of(run_metrics(&mut cache, &params));
    let again = metrics_of(run_metrics(&mut cache, &params));
    assert_eq!(result_bits(&first), result_bits(&again));

    let moved = MetricsParams {
        pitch: 0.9,
        light_yaw: 0.2,
        ..params
    };
    let got = metrics_of(run_metrics(&mut cache, &moved));
    let direct = evaluate_gem_optical_metrics(
        &planes,
        &material,
        0.61,
        0.9,
        LightingPreset::LightTent.studio(1.0, 0.2, 0.93),
    );
    assert_eq!(result_bits(&got), metric_bits(&direct));
    assert_ne!(result_bits(&got), result_bits(&first));
}

#[test]
fn metrics_params_ignore_everything_that_does_not_change_the_numbers() {
    let design = template_design(1);
    let (spec, _) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    let mut other = spec.clone();
    other.lighting.exposure = 0.5;
    other.lighting.backdrop_index = 0;
    other.width = 320;
    other.max_bounces = 4;
    other.finishes = FinishSpec::AllPolished;
    other.camera.distance = 9.0;
    assert_eq!(
        MetricsParams::from_scene(&spec),
        MetricsParams::from_scene(&other)
    );
    other.camera.yaw += 0.01;
    assert_ne!(
        MetricsParams::from_scene(&spec),
        MetricsParams::from_scene(&other)
    );
    // The render-time overrides change the pose metrics but not the tilt sweep.
    let mut overridden = spec.clone();
    overridden.material.overrides.edge_rounding_radius = 0.02;
    assert_ne!(
        MetricsParams::from_scene(&spec),
        MetricsParams::from_scene(&overridden)
    );
    assert_eq!(
        TiltParams::from_scene(&spec),
        TiltParams::from_scene(&overridden)
    );
    // ...and neither depends on the other's inputs: the camera is not a tilt input.
    assert_eq!(
        TiltParams::from_scene(&spec),
        TiltParams::from_scene(&other)
    );
}

/// The lighting preset is an input of both requests: changing it changes the params (so
/// a cached result is not reused), and the metrics then equal the direct call under the
/// new preset's rig.
#[test]
fn the_lighting_preset_is_an_input_of_both_requests() {
    let design = template_design(1);
    let (spec, planes) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    let mut other = spec.clone();
    other.lighting.preset_index = LightingPreset::RingLights.index();
    assert_ne!(spec.lighting.preset_index, other.lighting.preset_index);
    assert_ne!(
        MetricsParams::from_scene(&spec),
        MetricsParams::from_scene(&other)
    );
    assert_ne!(
        TiltParams::from_scene(&spec),
        TiltParams::from_scene(&other)
    );

    let mut cache = None;
    let first = metrics_of(run_metrics(&mut cache, &MetricsParams::from_scene(&spec)));
    let second = metrics_of(run_metrics(&mut cache, &MetricsParams::from_scene(&other)));
    let direct = evaluate_gem_optical_metrics(
        &planes,
        &GemMaterial::diamond(),
        0.61,
        0.47,
        LightingPreset::RingLights.studio(1.0, 0.84, 0.93),
    );
    assert_eq!(result_bits(&second), metric_bits(&direct));
    assert_ne!(result_bits(&first), result_bits(&second));
}

/// A synthetic 8 x 4 map: `bright` on the top row (row 0 is the north pole) and a dim
/// grey elsewhere.
fn map_bright_on(row: usize, bright: f32) -> EnvironmentMap {
    let mut pixels = vec![[0.05; 3]; 8 * 4];
    pixels[row * 8..(row + 1) * 8].fill([bright; 3]);
    EnvironmentMap::from_rgb(8, 4, pixels).expect("a consistent buffer")
}

/// A held map the request names is what the metrics are scored under: the desktop's call
/// with `EnvironmentSource::HdrMap`, bit for bit, and not the preset's number.
#[test]
fn metrics_under_a_held_map_equal_the_desktop_call_under_that_map() {
    let design = template_design(1);
    let (mut spec, planes) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    spec.hdr_id = Some(9);
    let params = MetricsParams::from_scene(&spec);
    let map = map_bright_on(0, 12.0);
    let got = metrics_of(run_metrics_with_map(&mut None, &params, Some((9, &map))));
    assert_eq!(got.scored_under, ScoredUnder::HdrMap(9));
    let direct = evaluate_gem_optical_metrics(
        &planes,
        &GemMaterial::diamond(),
        0.61,
        0.47,
        EnvironmentSource::HdrMap(&map),
    );
    assert_eq!(result_bits(&got), metric_bits(&direct));

    let preset = metrics_of(run_metrics(&mut None, &params));
    assert_eq!(
        preset.scored_under,
        ScoredUnder::Preset(params.preset_index)
    );
    assert_ne!(result_bits(&got), result_bits(&preset));
}

/// The Worker's cache is keyed on the map: the same pose under the preset, under another
/// map, and under the first map again each give their own numbers (the first and last
/// equal), so a new map never serves another's metrics.
#[test]
fn the_metrics_cache_tells_one_map_from_another_and_from_the_preset() {
    let design = template_design(1);
    let (mut spec, _) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    spec.hdr_id = Some(9);
    let params = MetricsParams::from_scene(&spec);
    let above = map_bright_on(0, 12.0);
    let below = map_bright_on(3, 12.0);
    let mut cache = None;
    let mut score = |params: &MetricsParams, held: Option<(u64, &EnvironmentMap)>| {
        metrics_of(run_metrics_with_map(&mut cache, params, held))
    };
    let first = score(&params, Some((9, &above)));
    let other_map = score(&params, Some((9, &below)));
    let preset = score(&params, None);
    let again = score(&params, Some((9, &above)));
    assert_eq!(result_bits(&first), result_bits(&again));
    assert_ne!(result_bits(&first), result_bits(&other_map));
    assert_ne!(result_bits(&first), result_bits(&preset));
    assert_ne!(result_bits(&other_map), result_bits(&preset));
}

/// The HDR map is an input of the metrics (a new map recomputes them) and not of the tilt
/// sweep, which the desktop scores under the preset only.
#[test]
fn the_hdr_map_is_an_input_of_the_metrics_but_not_of_the_tilt_sweep() {
    let design = template_design(1);
    let (spec, _) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    let with_map = |id| SceneSpec {
        hdr_id: Some(id),
        ..spec.clone()
    };
    assert_eq!(MetricsParams::from_scene(&spec).hdr_id, None);
    assert_eq!(MetricsParams::from_scene(&with_map(3)).hdr_id, Some(3));
    assert_ne!(
        MetricsParams::from_scene(&spec),
        MetricsParams::from_scene(&with_map(3))
    );
    assert_ne!(
        MetricsParams::from_scene(&with_map(3)),
        MetricsParams::from_scene(&with_map(4))
    );
    assert_eq!(
        TiltParams::from_scene(&spec),
        TiltParams::from_scene(&with_map(3))
    );
}

/// What the HUD and the Tilt dialog say they were scored under.
#[test]
fn the_notes_say_what_the_numbers_were_scored_under() {
    let tent = LightingPreset::LightTent.index();
    // The preset the viewport shows needs no note.
    assert_eq!(ScoredUnder::Preset(tent).hud_note(false), "");
    assert_eq!(
        ScoredUnder::HdrMap(9).hud_note(true),
        "Scored under the HDR map"
    );
    assert_eq!(
        ScoredUnder::Preset(tent).hud_note(true),
        "Scored under Light tent + black cards, map not loaded"
    );
    assert_eq!(
        tilt_lighting_note(tent, false),
        "Scored under the Light tent + black cards rig at the current light."
    );
    assert!(
        tilt_lighting_note(tent, true).contains("the loaded HDR map is not used for this sweep")
    );
}

#[test]
fn an_unknown_material_or_an_empty_stone_is_an_analysis_failure() {
    let design = template_design(1);
    let (spec, _) = scene_of(&design, MaterialSpec::catalogue("Unobtainium"));
    for response in [
        run_metrics(&mut None, &MetricsParams::from_scene(&spec)),
        no_hooks(|hooks| run_tilt(&TiltParams::from_scene(&spec), hooks)),
    ] {
        assert!(matches!(
            response,
            SolveResponse::AnalysisFailed {
                missing_anchor: false,
                ..
            }
        ));
    }
    let (mut empty, _) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    empty.planes.clear();
    assert!(matches!(
        run_metrics(&mut None, &MetricsParams::from_scene(&empty)),
        SolveResponse::AnalysisFailed { .. }
    ));
}

/// `handle_solve` routes both requests without a design (the message's TOML is empty).
#[test]
fn the_requests_need_no_design() {
    let design = template_design(1);
    let (spec, _) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    let response = crate::solve::handle_solve(
        "",
        &SolveRequest::Metrics {
            params: MetricsParams::from_scene(&spec),
        },
    );
    assert!(matches!(response, SolveResponse::Metrics(_)));
}

/// The tilt sweep is the desktop dialog's four `evaluate_full_axis_profile_at_azimuth`
/// calls, under the bare resolved material: the request's overrides change nothing.
#[test]
fn the_tilt_sweep_equals_the_four_direct_axis_profiles() {
    let template = TEMPLATES
        .iter()
        .position(|t| t.name == "Simple Teaching Design")
        .expect("the template exists");
    let design = template_design(template as i32 + 1);
    let with_overrides = MaterialSpec {
        overrides: MaterialOverridesSpec {
            inclusion_sigma_s: 2.0,
            c_axis_override: None,
            edge_rounding_radius: 0.02,
            stone_width_mm: 0.0,
        },
        ..MaterialSpec::catalogue("Sapphire")
    };
    let (spec, planes) = scene_of(&design, with_overrides);

    // The request as a caller that forgot to drop the overrides would send it.
    let params = TiltParams {
        material: spec.material.clone(),
        ..TiltParams::from_scene(&spec)
    };
    assert_ne!(params, TiltParams::from_scene(&spec));
    let reports = std::cell::RefCell::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let got = run_tilt(
        &params,
        &SolveHooks {
            cancel: &cancel,
            on_progress: &|_| {},
            on_sweep: &|p| reports.borrow_mut().push(p),
        },
    );
    let SolveResponse::TiltCurves(result) = got else {
        panic!("expected tilt curves, got {got:?}");
    };

    let bare = resolve_material_with_override(&GemMaterial::all_materials(), &[], None, "Sapphire")
        .expect("built-in");
    assert_eq!(result.axes.len(), PROFILE_AZIMUTHS_DEG.len());
    for (axis, &azimuth) in result.axes.iter().zip(&PROFILE_AZIMUTHS_DEG) {
        let (b, e, w) = evaluate_full_axis_profile_at_azimuth(
            &planes,
            &bare,
            azimuth,
            LightingPreset::LightTent.studio(1.0, 0.84, 0.93),
        );
        assert_eq!(axis.azimuth_deg.to_bits(), azimuth.to_bits());
        let profile = axis.to_profile().expect("181 points each");
        for (name, got, want) in [
            ("brilliance", &profile.brilliance, &b),
            ("extinction", &profile.extinction, &e),
            ("windowing", &profile.windowing, &w),
        ] {
            let got: Vec<u32> = got.iter().map(|v| v.to_bits()).collect();
            let want: Vec<u32> = want.iter().map(|v| v.to_bits()).collect();
            assert_eq!(got, want, "axis {azimuth} {name}");
        }
    }

    // Progress: one report per evaluation, in order, over all 724.
    let reports = reports.into_inner();
    assert_eq!(reports.len(), 724);
    assert!(reports.windows(2).all(|w| w[1].done == w[0].done + 1));
    assert_eq!((reports[0].axis, reports[723].axis), (0, 3));
    assert!(!cancel.load(Ordering::Relaxed));
}

#[test]
fn a_set_cancel_flag_stops_the_sweep_early() {
    let design = template_design(1);
    let (spec, _) = scene_of(&design, MaterialSpec::catalogue("Diamond"));
    let cancel = AtomicBool::new(false);
    let seen = std::cell::Cell::new(0usize);
    let got = run_tilt(
        &TiltParams::from_scene(&spec),
        &SolveHooks {
            cancel: &cancel,
            on_progress: &|_| {},
            on_sweep: &|_| {
                seen.set(seen.get() + 1);
                if seen.get() == 3 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        },
    );
    assert_eq!(got, SolveResponse::Cancelled);
    assert_eq!(seen.get(), 3, "no evaluation is started after the cancel");
}

#[test]
fn the_status_line_names_the_axis_and_the_points() {
    let text = sweep_status(SweepProgress {
        axis: 1,
        done: 300,
        total: 724,
    });
    assert_eq!(
        text,
        "Tilt sweep: axis 2 of 4 (45\u{b0}) \u{2014} 300 of 724 points"
    );
    let half = sweep_fraction(SweepProgress {
        axis: 2,
        done: 362,
        total: 724,
    });
    assert!((half - 0.5).abs() < 1e-6);
}
