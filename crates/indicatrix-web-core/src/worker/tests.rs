//! The Worker state machine, driven natively.

use std::cell::Cell;

use super::*;
use crate::{
    render::handle_trace_chunk,
    scene::{CameraSpec, FinishSpec, LightingSpec, MaterialSpec, planes_to_data},
    solve::{SolveRequest, design_to_toml},
};
use indicatrix::{
    geometry::cuts::StandardGemCuts, optics::raytracer::LightingPreset, render_setup::Backdrop,
};

pub(super) fn spec(hdr_id: Option<u64>) -> SceneSpec {
    SceneSpec {
        planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.2,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.4,
            0.35,
            Backdrop::Grey,
            16.0,
        ),
        max_bounces: 4,
        width: 6,
        height: 4,
        hdr_id,
    }
}

/// A clock that advances 10 ms per reading.
pub(super) fn ticking_clock() -> impl Fn() -> f64 {
    let t = Cell::new(0.0);
    move || {
        t.set(t.get() + 10.0);
        t.get()
    }
}

pub(super) fn init(role: WorkerRole) -> WorkerHandler {
    let mut handler = WorkerHandler::new();
    let reply = handler.handle(
        ToWorker::Init {
            protocol_version: PROTOCOL_VERSION,
            role,
            worker_index: 1,
        },
        &|| 0.0,
    );
    assert_eq!(
        reply,
        Some(FromWorker::Ready {
            role,
            worker_index: 1
        })
    );
    assert_eq!(handler.worker_index(), 1);
    handler
}

/// A tiny valid Radiance file (flat, uncompressed scanlines: width < 8 disables RLE).
pub(super) fn tiny_hdr() -> Vec<u8> {
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n".to_vec();
    for _ in 0..8 {
        bytes.extend_from_slice(&[128, 128, 128, 129]);
    }
    bytes
}

#[test]
fn a_version_mismatch_is_refused() {
    let mut handler = WorkerHandler::new();
    let reply = handler.handle(
        ToWorker::Init {
            protocol_version: PROTOCOL_VERSION + 1,
            role: WorkerRole::Render,
            worker_index: 0,
        },
        &|| 0.0,
    );
    assert!(matches!(reply, Some(FromWorker::Error { .. })));
    assert_eq!(handler.role(), None);
    assert_eq!(
        WorkerHandler::loaded_message(),
        FromWorker::Loaded {
            protocol_version: PROTOCOL_VERSION
        }
    );
}

#[test]
fn a_render_worker_traces_its_cached_scene_and_drops_other_scenes() {
    let mut handler = init(WorkerRole::Render);
    let clock = ticking_clock();
    let chunk = |scene_id| ToWorker::TraceChunk {
        scene_id,
        first_pixel: 1,
        stride: 2,
        sample_offset: 0,
        spp: 1,
    };
    // No scene yet.
    assert_eq!(
        handler.handle(chunk(1), &clock),
        Some(FromWorker::ChunkDropped {
            scene_id: 1,
            first_pixel: 1,
            sample_offset: 0
        })
    );
    assert_eq!(
        handler.handle(
            ToWorker::SetScene {
                scene_id: 1,
                spec: spec(None)
            },
            &clock
        ),
        None
    );
    let Some(FromWorker::ChunkResult {
        sums, elapsed_ms, ..
    }) = handler.handle(chunk(1), &clock)
    else {
        panic!("expected a chunk result");
    };
    assert_eq!(sums.len(), 12, "half of the 6x4 frame");
    assert!((elapsed_ms - 10.0).abs() < 1e-9);
    // The same chunk, traced directly, is bit-identical.
    let scene = OwnedScene::build(&spec(None), None).expect("builds");
    let direct = handle_trace_chunk(&scene, scene.plane_soa(), 1, 2, 0, 1);
    let direct: Vec<[f32; 3]> = direct.into_iter().map(<[f32; 3]>::from).collect();
    assert_eq!(
        sums.iter()
            .flatten()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        direct
            .iter()
            .flatten()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
    // A chunk for another scene is dropped, not traced against this one.
    assert!(matches!(
        handler.handle(chunk(2), &clock),
        Some(FromWorker::ChunkDropped { scene_id: 2, .. })
    ));
    // Solving is not a render worker's job.
    assert!(matches!(
        handler.handle(
            ToWorker::Solve {
                job_id: 1,
                design_toml: String::new(),
                request: SolveRequest::Solve
            },
            &clock
        ),
        Some(FromWorker::Error { .. })
    ));
}

#[test]
fn hdr_scenes_need_their_map_and_clearing_it_drops_them() {
    let mut handler = init(WorkerRole::Render);
    let clock = ticking_clock();
    assert!(matches!(
        handler.handle(
            ToWorker::SetScene {
                scene_id: 1,
                spec: spec(Some(4))
            },
            &clock
        ),
        Some(FromWorker::SceneError { scene_id: 1, .. })
    ));
    assert_eq!(
        handler.handle(
            ToWorker::HdrMap {
                id: 4,
                bytes: tiny_hdr()
            },
            &clock
        ),
        Some(FromWorker::HdrLoaded {
            id: 4,
            width: 4,
            height: 2
        })
    );
    assert_eq!(
        handler.handle(
            ToWorker::SetScene {
                scene_id: 2,
                spec: spec(Some(4))
            },
            &clock
        ),
        None
    );
    let trace = ToWorker::TraceChunk {
        scene_id: 2,
        first_pixel: 0,
        stride: 1,
        sample_offset: 0,
        spp: 1,
    };
    assert!(matches!(
        handler.handle(trace.clone(), &clock),
        Some(FromWorker::ChunkResult { .. })
    ));
    assert_eq!(handler.handle(ToWorker::ClearHdr, &clock), None);
    assert!(matches!(
        handler.handle(trace, &clock),
        Some(FromWorker::ChunkDropped { .. })
    ));
    assert!(matches!(
        handler.handle(
            ToWorker::HdrMap {
                id: 5,
                bytes: b"garbage".to_vec()
            },
            &clock
        ),
        Some(FromWorker::HdrError { id: 5, .. })
    ));
}

/// `Picture` makes exactly what `crate::display` makes on the page, for the cached
/// scene only.
#[test]
fn a_render_worker_denoises_and_encodes_pictures_of_its_scene() {
    let mut handler = init(WorkerRole::Render);
    let clock = ticking_clock();
    assert_eq!(
        handler.handle(
            ToWorker::SetScene {
                scene_id: 3,
                spec: spec(None)
            },
            &clock
        ),
        None
    );
    let sums: Vec<[f32; 3]> = (0..24)
        .map(|i| [i as f32 * 0.1, 0.5, (24 - i) as f32 * 0.05])
        .collect();
    let picture = |scene_id, kind| ToWorker::Picture {
        scene_id,
        sample_count: 4,
        sums: sums.clone(),
        kind,
    };
    let scene = OwnedScene::build(&spec(None), None).expect("builds");
    let sum: Vec<Vec3> = sums.iter().copied().map(Vec3::from_array).collect();
    let frame = DenoiseFrame {
        guide_key: 3,
        width: 6,
        height: 4,
        camera: scene.camera(),
        planes: scene.planes(),
        sample_count: 4,
        sum: &sum,
    };

    let Some(FromWorker::Picture { bytes, kind, .. }) =
        handler.handle(picture(3, PictureKind::DenoisedLive), &clock)
    else {
        panic!("expected a picture");
    };
    assert_eq!(kind, PictureKind::DenoisedLive);
    assert_eq!(bytes, Denoiser::new().denoised_rgba(&frame));

    let png_kind = PictureKind::Png {
        color_space: 1,
        denoise: true,
    };
    let Some(FromWorker::Picture { bytes, .. }) = handler.handle(picture(3, png_kind), &clock)
    else {
        panic!("expected a PNG");
    };
    let mean = Denoiser::new().denoised_mean(&frame);
    let expected = export_png(6, 4, 4, &sum, export_color_space(1), Some(&mean)).expect("encodes");
    assert_eq!(bytes, expected);

    assert!(matches!(
        handler.handle(picture(4, PictureKind::DenoisedLive), &clock),
        Some(FromWorker::PictureFailed { scene_id: 4, .. })
    ));
    let short = ToWorker::Picture {
        scene_id: 3,
        sample_count: 4,
        sums: sums[1..].to_vec(),
        kind: PictureKind::DenoisedLive,
    };
    assert!(matches!(
        handler.handle(short, &clock),
        Some(FromWorker::PictureFailed { .. })
    ));
}

#[test]
fn a_cancel_watch_belongs_to_its_own_job_and_is_used_once() {
    let mut pending = Some((7, "blob:x".to_string()));
    assert_eq!(take_watch(&mut pending, 7).as_deref(), Some("blob:x"));
    assert_eq!(take_watch(&mut pending, 7), None, "consumed");
    // A watch for another job is dropped, not kept for a later one.
    let mut pending = Some((7, "blob:x".to_string()));
    assert_eq!(take_watch(&mut pending, 8), None);
    assert_eq!(pending, None);
    // A worker records the message and answers nothing; the next Solve consumes it.
    let mut handler = init(WorkerRole::Solve);
    let clock = ticking_clock();
    assert_eq!(
        handler.handle(
            ToWorker::WatchCancel {
                job_id: 1,
                url: "blob:y".to_string()
            },
            &clock
        ),
        None
    );
    assert_eq!(handler.cancel_watch, Some((1, "blob:y".to_string())));
    let design = indicatrix_editor::EditorSession::fresh().design;
    let reply = handler.handle(
        ToWorker::Solve {
            job_id: 1,
            design_toml: design_to_toml(&design).expect("encodes"),
            request: SolveRequest::Solve,
        },
        &clock,
    );
    assert!(matches!(
        reply,
        Some(FromWorker::SolveResult {
            response: SolveResponse::Solved(_),
            ..
        })
    ));
    assert_eq!(handler.cancel_watch, None);
}

#[test]
fn the_cancel_poll_is_throttled_and_reports_the_probe() {
    let asked = Cell::new(0);
    let gone = Cell::new(false);
    let probe = |url: &str| {
        assert_eq!(url, "blob:z");
        asked.set(asked.get() + 1);
        if gone.get() {
            CancelProbe::Revoked
        } else {
            CancelProbe::Live
        }
    };
    let watch = CancelWatch::new(Some("blob:z"), &probe, None);
    // The first look asks; the ones within the interval do not.
    assert!(!watch.cancelled(1000.0));
    assert!(!watch.cancelled(1000.0 + CANCEL_POLL_INTERVAL_MS - 1.0));
    assert_eq!(asked.get(), 1);
    // Revoked: the next look after the interval sees it.
    gone.set(true);
    assert!(
        !watch.cancelled(1000.0 + CANCEL_POLL_INTERVAL_MS - 1.0),
        "still throttled"
    );
    assert!(watch.cancelled(1000.0 + CANCEL_POLL_INTERVAL_MS));
    assert_eq!(asked.get(), 2);
}

/// A probe that cannot run (a strict content-security policy refuses the synchronous
/// request) says nothing about the page's wishes: it is not a cancel, and it is not asked
/// again -- only the message mark can still stop the job.
#[test]
fn an_unavailable_probe_is_not_a_cancel_and_is_asked_once() {
    let asked = Cell::new(0);
    let probe = |_: &str| {
        asked.set(asked.get() + 1);
        CancelProbe::Unavailable
    };
    let mark = AtomicU64::new(0);
    let watch = CancelWatch::new(Some("blob:z"), &probe, Some((&mark, 7)));
    for step in 0..5 {
        assert!(
            !watch.cancelled(f64::from(step).mul_add(10.0 * CANCEL_POLL_INTERVAL_MS, 1000.0)),
            "step {step}"
        );
    }
    assert_eq!(asked.get(), 1, "retired after the first refusal");
    // A `Cancel` naming a later job leaves job 7 alone; one naming job 7 stops it.
    mark.fetch_max(7, Ordering::Relaxed);
    assert!(
        !watch.cancelled(1e6),
        "the mark is one more than the job id"
    );
    mark.fetch_max(8, Ordering::Relaxed);
    assert!(watch.cancelled(1e6));
    assert_eq!(asked.get(), 1, "the URL stays retired");
}

/// Without a URL the watch reads only the message mark, and never the clock.
#[test]
fn a_watch_without_a_url_reads_no_clock() {
    let probe = |_: &str| CancelProbe::Revoked;
    let mark = AtomicU64::new(0);
    let watch = CancelWatch::new(None, &probe, Some((&mark, 3)));
    let clock = || -> f64 { panic!("a watch with no URL must not read the clock") };
    assert!(!watch.cancelled_reading(&clock));
    mark.store(4, Ordering::Relaxed);
    assert!(watch.cancelled_reading(&clock));
}

#[test]
fn the_solve_worker_solves_and_skips_cancelled_jobs() {
    let mut handler = init(WorkerRole::Solve);
    let clock = ticking_clock();
    let design = indicatrix_editor::EditorSession::fresh().design;
    let toml = design_to_toml(&design).expect("encodes");
    let solve = |job_id| ToWorker::Solve {
        job_id,
        design_toml: toml.clone(),
        request: SolveRequest::Solve,
    };
    let Some(FromWorker::SolveResult {
        job_id,
        response,
        elapsed_ms,
    }) = handler.handle(solve(1), &clock)
    else {
        panic!("expected a solve result");
    };
    assert_eq!(job_id, 1);
    assert!(matches!(response, SolveResponse::Solved(_)));
    assert!((elapsed_ms - 10.0).abs() < 1e-9);

    assert_eq!(handler.handle(ToWorker::Cancel { job_id: 3 }, &clock), None);
    for cancelled in [2, 3] {
        assert_eq!(
            handler.handle(solve(cancelled), &clock),
            Some(FromWorker::SolveResult {
                job_id: cancelled,
                response: SolveResponse::Cancelled,
                elapsed_ms: 0.0
            })
        );
    }
    assert!(matches!(
        handler.handle(solve(4), &clock),
        Some(FromWorker::SolveResult {
            response: SolveResponse::Solved(_),
            ..
        })
    ));
    // Tracing is not the solve worker's job.
    assert!(matches!(
        handler.handle(
            ToWorker::SetScene {
                scene_id: 1,
                spec: spec(None)
            },
            &clock
        ),
        Some(FromWorker::Error { .. })
    ));
}

#[test]
fn the_solve_worker_answers_metrics_without_a_design_and_keeps_its_cache() {
    use crate::solve::{MetricsParams, run_metrics};
    let mut handler = init(WorkerRole::Solve);
    let clock = ticking_clock();
    let params = MetricsParams::from_scene(&spec(None));
    let job = |job_id| ToWorker::Solve {
        job_id,
        design_toml: String::new(),
        request: SolveRequest::Metrics {
            params: params.clone(),
        },
    };
    let want = run_metrics(&mut None, &params);
    assert!(matches!(want, SolveResponse::Metrics(_)));
    assert!(handler.metrics_cache.is_none());
    for job_id in [1, 2] {
        let Some(FromWorker::SolveResult {
            job_id: got_id,
            response,
            ..
        }) = handler.handle(job(job_id), &clock)
        else {
            panic!("expected a metrics result");
        };
        assert_eq!(got_id, job_id);
        assert_eq!(response, want);
        assert!(handler.metrics_cache.is_some(), "the pose stays cached");
    }
    // A render Worker has no business with it.
    let mut render = init(WorkerRole::Render);
    assert!(matches!(
        render.handle(job(3), &clock),
        Some(FromWorker::Error { .. })
    ));
}

/// A browser that refuses the synchronous request must not end every search at its first
/// look: the sweep runs on, and a `Cancel` that reaches the Worker still stops it.
#[test]
fn a_sweep_whose_probe_is_unavailable_runs_on_until_a_cancel_message_names_it() {
    use crate::solve::TiltParams;
    let mut handler = init(WorkerRole::Solve);
    let now = Cell::new(0.0);
    let clock = || {
        now.set(now.get() + 150.0);
        now.get()
    };
    assert_eq!(
        handler.handle(
            ToWorker::WatchCancel {
                job_id: 1,
                url: "blob:tilt".to_string()
            },
            &clock
        ),
        None
    );
    let mark = Arc::clone(&handler.cancel_mark);
    let emitted = Cell::new(0);
    let asked = Cell::new(0);
    let probe = |_: &str| {
        asked.set(asked.get() + 1);
        CancelProbe::Unavailable
    };
    // A `Cancel` for job 1 reaches the Worker while the third report is being posted.
    let emit = |_: FromWorker| {
        emitted.set(emitted.get() + 1);
        if emitted.get() == 3 {
            mark.fetch_max(2, Ordering::Relaxed);
        }
    };
    let reply = handler.handle_probed(
        ToWorker::Solve {
            job_id: 1,
            design_toml: String::new(),
            request: SolveRequest::Tilt {
                params: TiltParams::from_scene(&spec(None)),
            },
        },
        &clock,
        &emit,
        &probe,
    );
    let Some(FromWorker::SolveResult { response, .. }) = reply else {
        panic!("expected the sweep's result, got {reply:?}");
    };
    assert_eq!(response, SolveResponse::Cancelled);
    assert_eq!(
        asked.get(),
        1,
        "an unavailable probe is asked once, then retired"
    );
    assert!(
        emitted.get() >= 3,
        "the sweep ran past the unavailable probe, not stopped by it"
    );
}

/// A cancel message that arrives while jobs are queued still skips every job up to its id,
/// and the mark is kept as the newest id seen.
#[test]
fn cancel_messages_raise_a_mark_that_only_grows() {
    let mut handler = init(WorkerRole::Solve);
    let clock = ticking_clock();
    assert_eq!(handler.cancel_mark.load(Ordering::Relaxed), 0);
    assert_eq!(handler.handle(ToWorker::Cancel { job_id: 4 }, &clock), None);
    assert_eq!(handler.cancel_mark.load(Ordering::Relaxed), 5);
    assert_eq!(handler.handle(ToWorker::Cancel { job_id: 2 }, &clock), None);
    assert_eq!(
        handler.cancel_mark.load(Ordering::Relaxed),
        5,
        "an older cancel never lowers it"
    );
    assert_eq!(
        handler.handle(ToWorker::Cancel { job_id: u64::MAX }, &clock),
        None
    );
    assert_eq!(handler.cancel_mark.load(Ordering::Relaxed), u64::MAX);
}

/// A chunk traced in row groups stops at the first look that finds its URL revoked,
/// answers `ChunkAborted`, and a watch serves one chunk only.
#[test]
fn a_chunk_whose_cancel_url_is_revoked_is_aborted_and_the_watch_serves_one_chunk() {
    const SPP: u32 = 90;
    assert!(
        crate::render::slice_count(24, SPP) >= 2,
        "the chunk must span several row groups for this test to look between them"
    );
    let mut handler = init(WorkerRole::Render);
    let now = Cell::new(0.0);
    // Every reading is 150 ms later: the first look between row groups is due.
    let clock = || {
        now.set(now.get() + 150.0);
        now.get()
    };
    assert_eq!(
        handler.handle(
            ToWorker::SetScene {
                scene_id: 1,
                spec: spec(None)
            },
            &clock
        ),
        None
    );
    let chunk = |spp| ToWorker::TraceChunk {
        scene_id: 1,
        first_pixel: 0,
        stride: 1,
        sample_offset: 0,
        spp,
    };
    let watch = || ToWorker::WatchCancel {
        job_id: 1,
        url: "blob:chunk".to_string(),
    };
    let asked = Cell::new(0);
    let probe_with = |answer: CancelProbe| {
        let asked = &asked;
        move |url: &str| {
            assert_eq!(url, "blob:chunk");
            asked.set(asked.get() + 1);
            answer
        }
    };

    // Revoked: aborted at the first look, before any tracing.
    assert_eq!(
        handler.handle_probed(watch(), &clock, &|_| {}, &probe_with(CancelProbe::Revoked)),
        None
    );
    assert_eq!(
        handler.handle_probed(
            chunk(SPP),
            &clock,
            &|_| {},
            &probe_with(CancelProbe::Revoked)
        ),
        Some(FromWorker::ChunkAborted {
            scene_id: 1,
            first_pixel: 0,
            sample_offset: 0
        })
    );
    assert_eq!(asked.get(), 1);
    assert_eq!(handler.cancel_watch, None, "the watch is used up");

    // Live: traced in full, the same bits as an unwatched trace.
    assert_eq!(
        handler.handle_probed(watch(), &clock, &|_| {}, &probe_with(CancelProbe::Live)),
        None
    );
    let Some(FromWorker::ChunkResult { sums, .. }) =
        handler.handle_probed(chunk(SPP), &clock, &|_| {}, &probe_with(CancelProbe::Live))
    else {
        panic!("expected a chunk result");
    };
    assert!(asked.get() >= 2, "the live URL was looked at");
    let scene = OwnedScene::build(&spec(None), None).expect("builds");
    let direct = handle_trace_chunk(&scene, scene.plane_soa(), 0, 1, 0, SPP);
    let bits =
        |sums: &[[f32; 3]]| -> Vec<u32> { sums.iter().flatten().map(|v| v.to_bits()).collect() };
    let direct: Vec<[f32; 3]> = direct.into_iter().map(<[f32; 3]>::from).collect();
    assert_eq!(bits(&sums), bits(&direct));

    // No watch now: the next chunk is not asked about anything.
    let before = asked.get();
    assert!(matches!(
        handler.handle_probed(chunk(1), &clock, &|_| {}, &probe_with(CancelProbe::Revoked)),
        Some(FromWorker::ChunkResult { .. })
    ));
    assert_eq!(asked.get(), before);
}

#[test]
fn a_tilt_sweep_streams_progress_and_stops_when_its_cancel_url_is_revoked() {
    use crate::solve::TiltParams;
    let mut handler = init(WorkerRole::Solve);
    // Every clock reading is 150 ms later, so every progress hook may probe and report.
    let now = Cell::new(0.0);
    let clock = || {
        now.set(now.get() + 150.0);
        now.get()
    };
    assert_eq!(
        handler.handle(
            ToWorker::WatchCancel {
                job_id: 1,
                url: "blob:tilt".to_string()
            },
            &clock
        ),
        None
    );
    let emitted = std::cell::RefCell::new(Vec::new());
    let asked = Cell::new(0);
    let probe = |url: &str| {
        assert_eq!(url, "blob:tilt");
        asked.set(asked.get() + 1);
        if asked.get() >= 4 {
            CancelProbe::Revoked
        } else {
            CancelProbe::Live
        }
    };
    let reply = handler.handle_probed(
        ToWorker::Solve {
            job_id: 1,
            design_toml: String::new(),
            request: SolveRequest::Tilt {
                params: TiltParams::from_scene(&spec(None)),
            },
        },
        &clock,
        &|message| emitted.borrow_mut().push(message),
        &probe,
    );
    let Some(FromWorker::SolveResult {
        job_id: 1,
        response,
        ..
    }) = reply
    else {
        panic!("expected the sweep's result, got {reply:?}");
    };
    assert_eq!(response, SolveResponse::Cancelled);
    assert_eq!(
        asked.get(),
        4,
        "the sweep stops at the first look that says so"
    );
    let emitted = emitted.into_inner();
    assert!(emitted.len() >= 2, "{emitted:?}");
    match &emitted[0] {
        FromWorker::Progress {
            job_id: 1,
            message,
            fraction: Some(fraction),
        } => {
            assert!(message.starts_with("Tilt sweep: axis 1 of 4"), "{message}");
            assert!(*fraction < 0.01);
        }
        other => panic!("expected progress, got {other:?}"),
    }
}
