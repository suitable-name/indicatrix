//! The solve Worker's HDR map: the analysis Worker decodes and holds one like a render
//! Worker does, and scores the current-view metrics under it.

use indicatrix::renderer::env_map::environment_from_hdr_bytes;

use super::{
    tests::{init, spec, ticking_clock, tiny_hdr},
    *,
};
use crate::solve::{
    MetricsParams, MetricsResultData, ScoredUnder, run_metrics, run_metrics_with_map,
};

/// A 4 x 2 map lit from above: a bright top row and a dim bottom row (flat scanlines: a
/// width under 8 is not run-length encoded).
fn lit_from_above() -> Vec<u8> {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..4 {
        bytes.extend_from_slice(&[200, 200, 200, 132]);
    }
    for _ in 0..4 {
        bytes.extend_from_slice(&[20, 20, 20, 120]);
    }
    bytes
}

fn metrics_job(job_id: u64, params: &MetricsParams) -> ToWorker {
    ToWorker::Solve {
        job_id,
        design_toml: String::new(),
        request: SolveRequest::Metrics {
            params: params.clone(),
        },
    }
}

/// The metrics a Worker answered a job with.
fn answered(reply: Option<FromWorker>) -> MetricsResultData {
    match reply {
        Some(FromWorker::SolveResult {
            response: SolveResponse::Metrics(metrics),
            ..
        }) => metrics,
        other => panic!("expected metrics, got {other:?}"),
    }
}

/// The metrics inside a direct `run_metrics*` answer.
fn computed(response: SolveResponse) -> MetricsResultData {
    match response {
        SolveResponse::Metrics(metrics) => metrics,
        other => panic!("expected metrics, got {other:?}"),
    }
}

fn bits(m: &MetricsResultData) -> [u32; 5] {
    [
        m.brilliance_pct.to_bits(),
        m.fire_index.to_bits(),
        m.scintillation_pct.to_bits(),
        m.windowing_pct.to_bits(),
        m.extinction_pct.to_bits(),
    ]
}

/// The map is decoded and held by a solve-role Worker; a request that names it is scored
/// under it -- exactly the call a holder of the decoded map makes -- and differs from the
/// preset's score.
#[test]
fn the_analysis_worker_scores_the_metrics_under_the_map_it_holds() {
    let mut handler = init(WorkerRole::Solve);
    let clock = ticking_clock();
    assert_eq!(
        handler.handle(
            ToWorker::HdrMap {
                id: 4,
                bytes: lit_from_above()
            },
            &clock
        ),
        Some(FromWorker::HdrLoaded {
            id: 4,
            width: 4,
            height: 2
        })
    );
    let params = MetricsParams::from_scene(&spec(Some(4)));
    let under_map = answered(handler.handle(metrics_job(1, &params), &clock));
    assert_eq!(under_map.scored_under, ScoredUnder::HdrMap(4));

    let map = environment_from_hdr_bytes(&lit_from_above(), web_hdr_limits()).expect("decodes");
    let direct = run_metrics_with_map(&mut None, &params, Some((4, &map)));
    assert_eq!(SolveResponse::Metrics(under_map), direct);

    let preset = computed(run_metrics(&mut None, &params));
    assert_eq!(
        preset.scored_under,
        ScoredUnder::Preset(params.preset_index)
    );
    assert_ne!(
        bits(&under_map),
        bits(&preset),
        "the map lights the stone differently from the preset's rig"
    );
}

/// A request that names no map, or another one than the Worker holds, is scored under the
/// preset and says so; so does one the Worker could not decode a map for.
#[test]
fn a_request_for_a_map_the_worker_does_not_hold_is_scored_under_the_preset() {
    let mut handler = init(WorkerRole::Solve);
    let clock = ticking_clock();
    let held = handler.handle(
        ToWorker::HdrMap {
            id: 4,
            bytes: lit_from_above(),
        },
        &clock,
    );
    assert!(matches!(held, Some(FromWorker::HdrLoaded { id: 4, .. })));

    let preset_params = MetricsParams::from_scene(&spec(None));
    let want = computed(run_metrics(&mut None, &preset_params));
    for hdr_id in [None, Some(5)] {
        let params = MetricsParams {
            hdr_id,
            ..preset_params.clone()
        };
        let got = answered(handler.handle(metrics_job(1, &params), &clock));
        assert_eq!(got, want, "hdr_id {hdr_id:?}");
        assert_eq!(got.scored_under, ScoredUnder::Preset(params.preset_index));
    }

    // A map that does not decode is reported, and leaves nothing to score under.
    assert!(matches!(
        handler.handle(
            ToWorker::HdrMap {
                id: 6,
                bytes: b"garbage".to_vec()
            },
            &clock
        ),
        Some(FromWorker::HdrError { id: 6, .. })
    ));
    let params = MetricsParams::from_scene(&spec(Some(6)));
    let got = answered(handler.handle(metrics_job(2, &params), &clock));
    assert_eq!(got.scored_under, ScoredUnder::Preset(params.preset_index));
    assert_eq!(bits(&got), bits(&want));
}

/// Clearing the map returns the Worker to the preset, and the cache does not serve the
/// map's numbers for the same pose.
#[test]
fn clearing_the_map_returns_the_metrics_to_the_preset() {
    let mut handler = init(WorkerRole::Solve);
    let clock = ticking_clock();
    let held = handler.handle(
        ToWorker::HdrMap {
            id: 4,
            bytes: lit_from_above(),
        },
        &clock,
    );
    assert!(matches!(held, Some(FromWorker::HdrLoaded { .. })));
    let params = MetricsParams::from_scene(&spec(Some(4)));
    let under_map = answered(handler.handle(metrics_job(1, &params), &clock));
    assert_eq!(under_map.scored_under, ScoredUnder::HdrMap(4));
    // Asked again, it is answered from the cache, unchanged.
    assert_eq!(
        answered(handler.handle(metrics_job(2, &params), &clock)),
        under_map
    );

    assert_eq!(handler.handle(ToWorker::ClearHdr, &clock), None);
    let after = answered(handler.handle(metrics_job(3, &params), &clock));
    assert_eq!(after.scored_under, ScoredUnder::Preset(params.preset_index));
    assert_ne!(bits(&after), bits(&under_map));
    assert_eq!(after, computed(run_metrics(&mut None, &params)));
}

/// A map is for the Workers that use it: the render and solve roles take one, a Worker
/// that has not been given a role does not, and neither does it hold a scene for it.
#[test]
fn only_a_worker_with_a_role_takes_a_map() {
    let clock = ticking_clock();
    for role in [WorkerRole::Render, WorkerRole::Solve] {
        let mut handler = init(role);
        assert!(
            matches!(
                handler.handle(
                    ToWorker::HdrMap {
                        id: 1,
                        bytes: tiny_hdr()
                    },
                    &clock
                ),
                Some(FromWorker::HdrLoaded { id: 1, .. })
            ),
            "{role:?}"
        );
    }
    let mut unassigned = WorkerHandler::new();
    let Some(FromWorker::Error { message }) = unassigned.handle(
        ToWorker::HdrMap {
            id: 1,
            bytes: tiny_hdr(),
        },
        &clock,
    ) else {
        panic!("a Worker with no role must refuse a map");
    };
    assert!(message.contains("HdrMap"), "{message}");
}
