//! The analysis Worker's metrics and tilt jobs score a concave stone WITH its tools, as the
//! desktop does (`StoneGeometry { planes, tools }`), and a flat stone exactly as before.
//! The stone is `Design::concave_fixture()` (a groove and a dimple).

use indicatrix::{
    color::metrics::{
        AxisProfile, GemOpticalMetrics, evaluate_all_axes_profiles_geom,
        evaluate_gem_optical_metrics_geom,
    },
    render_setup::Backdrop,
};
use indicatrix_cut_core::Design;
use indicatrix_editor::solve_policy::design_to_gpu_planes_from_solved;

use super::*;
use crate::{
    protocol::{FromWorker, PROTOCOL_VERSION, ToWorker, WorkerRole},
    scene::{CameraSpec, FinishSpec, LightingSpec, planes_to_data},
    solve::{design_from_toml, design_to_toml, handle_solve},
    worker::WorkerHandler,
};

const YAW: f32 = 0.61;
const PITCH: f32 = 0.47;
const LIGHT_YAW: f32 = 0.84;
const LIGHT_PITCH: f32 = 0.93;

struct Stone {
    design: Design,
    spec: SceneSpec,
    gpu: Vec<GpuFacetPlane>,
    tools: Vec<ToolPrimitive>,
}

/// The fixture as the browser would render it: solved planes, the tools the desktop's
/// scoring resolves (`Design::tools_for_scoring`), and a frame around them.
fn stone() -> Stone {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let gpu = design_to_gpu_planes_from_solved(&design, &solved);
    let tools = design.tools_for_scoring(&design.planes_from_solved(&solved));
    assert!(!tools.is_empty(), "the fixture's tiers resolve to tools");
    let spec = SceneSpec {
        planes: planes_to_data(&gpu),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: YAW,
            pitch: PITCH,
            distance: 2.4,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.7,
            LIGHT_YAW,
            LIGHT_PITCH,
            Backdrop::White,
            16.0,
        ),
        max_bounces: 8,
        width: 64,
        height: 48,
        hdr_id: None,
    };
    Stone {
        design,
        spec,
        gpu,
        tools,
    }
}

fn environment() -> EnvironmentSource<'static> {
    LightingPreset::LightTent.studio(1.0, LIGHT_YAW, LIGHT_PITCH)
}

fn direct_metrics(stone: &Stone, tools: &[ToolPrimitive]) -> [u32; 5] {
    let material =
        resolve_scene_material(&MaterialSpec::catalogue("Diamond"), &stone.gpu).expect("built-in");
    let m: GemOpticalMetrics = evaluate_gem_optical_metrics_geom(
        StoneGeometry {
            planes: &stone.gpu,
            tools,
        },
        &material,
        YAW,
        PITCH,
        environment(),
    );
    [
        m.brilliance_pct.to_bits(),
        m.fire_index.to_bits(),
        m.scintillation_pct.to_bits(),
        m.windowing_pct.to_bits(),
        m.extinction_pct.to_bits(),
    ]
}

fn answered_metrics(request: &SolveRequest) -> [u32; 5] {
    match handle_solve("", request) {
        SolveResponse::Metrics(m) => [
            m.brilliance_pct.to_bits(),
            m.fire_index.to_bits(),
            m.scintillation_pct.to_bits(),
            m.windowing_pct.to_bits(),
            m.extinction_pct.to_bits(),
        ],
        other => panic!("expected metrics, got {other:?}"),
    }
}

fn profile_bits(profile: &AxisProfile) -> Vec<u32> {
    [
        &profile.brilliance[..],
        &profile.extinction[..],
        &profile.windowing[..],
    ]
    .concat()
    .iter()
    .map(|v| v.to_bits())
    .collect()
}

/// The HUD's numbers of a concave stone are the carved stone's, bit for bit the direct
/// `StoneGeometry` call, and not the flat stone's.
#[test]
fn metrics_score_the_stone_with_its_tools() {
    let s = stone();
    let params = MetricsParams::from_scene(&s.spec);

    let carved = answered_metrics(&SolveRequest::metrics(params.clone(), &s.tools));
    assert_eq!(carved, direct_metrics(&s, &s.tools));

    let flat = answered_metrics(&SolveRequest::metrics(params, &[]));
    assert_eq!(flat, direct_metrics(&s, &[]), "no tools: the planar score");
    assert_ne!(
        carved, flat,
        "the groove and dimple must change the numbers"
    );
}

/// A flat design keeps the planar request, byte for byte, and the planar answer.
#[test]
fn a_stone_without_tools_keeps_the_planar_requests() {
    let s = stone();
    let metrics = MetricsParams::from_scene(&s.spec);
    let tilt = TiltParams::from_scene(&s.spec);

    assert_eq!(
        SolveRequest::metrics(metrics.clone(), &[]),
        SolveRequest::Metrics {
            params: metrics.clone()
        }
    );
    assert_eq!(
        SolveRequest::tilt(tilt.clone(), &[]),
        SolveRequest::Tilt {
            params: tilt.clone()
        }
    );
    assert_eq!(
        MetricsJob::from_scene(&s.spec, &[]).request(),
        SolveRequest::Metrics { params: metrics }
    );
    assert_eq!(
        TiltJob::from_scene(&s.spec, &[]).request(),
        SolveRequest::Tilt { params: tilt }
    );

    let with_tools = MetricsJob::from_scene(&s.spec, &s.tools).request();
    assert!(
        matches!(&with_tools, SolveRequest::MetricsWithTools { tools, .. } if *tools == s.tools),
        "{with_tools:?}"
    );
    assert!(matches!(
        TiltJob::from_scene(&s.spec, &s.tools).request(),
        SolveRequest::TiltWithTools { .. }
    ));
}

/// The page compares whole jobs: the same params with other tools are another job, so a
/// concave edit alone recomputes the HUD and stales the tilt curves.
#[test]
fn a_change_of_tools_alone_is_another_job() {
    let s = stone();
    let mut fewer = s.tools.clone();
    fewer.pop();
    assert_eq!(
        MetricsJob::from_scene(&s.spec, &s.tools),
        MetricsJob::from_scene(&s.spec, &s.tools)
    );
    assert_ne!(
        MetricsJob::from_scene(&s.spec, &s.tools),
        MetricsJob::from_scene(&s.spec, &fewer)
    );
    assert_ne!(
        MetricsJob::from_scene(&s.spec, &s.tools),
        MetricsJob::from_scene(&s.spec, &[])
    );
    assert_ne!(
        TiltJob::from_scene(&s.spec, &s.tools),
        TiltJob::from_scene(&s.spec, &[])
    );
}

/// The Tilt sweep of a concave stone is `evaluate_all_axes_profiles_geom` with the tools
/// (bit for bit), and differs from the flat stone's sweep.
#[test]
fn the_tilt_sweep_scores_the_stone_with_its_tools() {
    let s = stone();
    let params = TiltParams::from_scene(&s.spec);

    let SolveResponse::TiltCurves(carved) =
        handle_solve("", &SolveRequest::tilt(params.clone(), &s.tools))
    else {
        panic!("expected tilt curves");
    };
    let bare =
        resolve_scene_material(&tilt_material_spec(&params.material), &s.gpu).expect("built-in");
    let direct = evaluate_all_axes_profiles_geom(
        StoneGeometry {
            planes: &s.gpu,
            tools: &s.tools,
        },
        &bare,
        environment(),
    );
    assert_eq!(carved.axes.len(), direct.len());
    for (axis, want) in carved.axes.iter().zip(&direct) {
        let got = axis.to_profile().expect("181 points each");
        assert_eq!(profile_bits(&got), profile_bits(want));
    }

    let SolveResponse::TiltCurves(flat) = handle_solve("", &SolveRequest::tilt(params, &[])) else {
        panic!("expected tilt curves");
    };
    assert_ne!(carved, flat, "the tools must change the curves");
}

/// A tool list that does not validate is a failed analysis, never a flat answer; more
/// than the limit likewise.
#[test]
fn a_bad_tool_list_fails_the_analysis() {
    let s = stone();
    let mut broken = s.tools[0];
    broken.origin[3] = f32::NAN;
    let too_many = vec![s.tools[0]; indicatrix::geometry::tool::MAX_TOOL_PRIMITIVES + 1];
    for tools in [vec![broken], too_many] {
        for request in [
            SolveRequest::metrics(MetricsParams::from_scene(&s.spec), &tools),
            SolveRequest::tilt(TiltParams::from_scene(&s.spec), &tools),
        ] {
            let response = handle_solve("", &request);
            assert!(
                matches!(
                    response,
                    SolveResponse::AnalysisFailed {
                        missing_anchor: false,
                        ..
                    }
                ),
                "{response:?}"
            );
        }
    }
}

fn solve_message(job_id: u64, request: SolveRequest) -> ToWorker {
    ToWorker::Solve {
        job_id,
        design_toml: String::new(),
        request,
    }
}

fn metrics_reply(reply: Option<FromWorker>) -> MetricsResultData {
    match reply {
        Some(FromWorker::SolveResult {
            response: SolveResponse::Metrics(m),
            ..
        }) => m,
        other => panic!("expected a metrics result, got {other:?}"),
    }
}

/// The analysis Worker answers `MetricsWithTools` from its cache like `Metrics`, and the
/// cache key includes the tools: the flat pose is not served for the carved stone.
#[test]
fn the_analysis_worker_scores_the_tools_and_keys_its_cache_on_them() {
    let s = stone();
    let params = MetricsParams::from_scene(&s.spec);
    let mut worker = WorkerHandler::new();
    let clock = || 0.0;
    let ready = worker.handle(
        ToWorker::Init {
            protocol_version: PROTOCOL_VERSION,
            role: WorkerRole::Solve,
            worker_index: 0,
        },
        &clock,
    );
    assert!(matches!(ready, Some(FromWorker::Ready { .. })), "{ready:?}");

    let flat = metrics_reply(worker.handle(
        solve_message(1, SolveRequest::metrics(params.clone(), &[])),
        &clock,
    ));
    let carved = metrics_reply(worker.handle(
        solve_message(2, SolveRequest::metrics(params.clone(), &s.tools)),
        &clock,
    ));
    assert_ne!(
        flat, carved,
        "a cached flat pose must not answer the carved one"
    );
    assert_eq!(
        [
            carved.brilliance_pct.to_bits(),
            carved.fire_index.to_bits(),
            carved.scintillation_pct.to_bits(),
            carved.windowing_pct.to_bits(),
            carved.extinction_pct.to_bits(),
        ],
        direct_metrics(&s, &s.tools)
    );

    // Asked again, the carved pose is served from the cache, unchanged; the flat one too.
    let again = metrics_reply(worker.handle(
        solve_message(3, SolveRequest::metrics(params.clone(), &s.tools)),
        &clock,
    ));
    assert_eq!(again, carved);
    let flat_again =
        metrics_reply(worker.handle(solve_message(4, SolveRequest::metrics(params, &[])), &clock));
    assert_eq!(flat_again, flat);
}

/// Optimize and Retarget carry no separate tools: they get the design as its native file
/// and score it through the shared engines (`Design::tools_for_scoring`). The file keeps
/// the concave tiers, so the Worker resolves exactly the tools the page does.
#[test]
fn optimize_and_retarget_see_the_tools_through_the_design_file() {
    let s = stone();
    let toml = design_to_toml(&s.design).expect("the fixture serialises");
    let received = design_from_toml(&toml).expect("and reads back");
    assert_eq!(received.concave_tiers.len(), s.design.concave_tiers.len());

    let solved = received.solve().expect("the received design solves");
    let tools = received.tools_for_scoring(&received.planes_from_solved(&solved));
    assert_eq!(tools, s.tools, "the Worker places the page's tools");
}
