use super::{
    TimeoutCache, TimeoutRead,
    downsample::downsample_preview,
    emitter::{
        ClientPoll, EmitterAccum, PendingDelta, effective_cadence_ms, emit_tick,
        poll_for_client_message, wait_for_tracer_to_stop,
    },
    is_stream_timeout, sizing,
    sizing::next_batch_size,
    tracer::SharedState,
};
use crate::render_core;
use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::{
    SceneState,
    messages::{
        PreviewConfig, RenderRequest, StreamConfig, StreamEvent, TransferMode, read_stream_event,
    },
    radiance,
};
use std::{
    io::ErrorKind,
    sync::{Arc, Mutex},
    time::Duration,
};

fn tiny_scene() -> SceneState {
    SceneState {
        width: 4,
        height: 4,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
    }
}

#[test]
fn coalesced_deltas_sum_identically_to_un_coalesced_ones() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;

    let a = render_core::trace_samples(&scene, 0, 3, 1);
    let b = render_core::trace_samples(&scene, 3, 5, 1);

    let mut pending = PendingDelta::new(pixel_count);
    pending.add(0, 3, &a);
    pending.add(3, 5, &b);
    let mut spare = vec![Vec3::ZERO; pixel_count];
    let (first_sample, samples) = pending.swap_with(&mut spare).unwrap();
    assert_eq!(first_sample, 0);
    assert_eq!(samples, 8);
    let coalesced = spare;

    let direct = render_core::trace_samples(&scene, 0, 8, 1);
    for (c, d) in coalesced.iter().zip(&direct) {
        let diff = (*c - *d).abs();
        let scale = c.abs().max(d.abs()).max(Vec3::splat(1e-6));
        assert!((diff / scale).max_element() < 1e-3, "c={c:?} d={d:?}");
    }
}

#[test]
fn pending_delta_swap_returns_none_when_empty() {
    let mut pending = PendingDelta::new(16);
    let mut spare = vec![Vec3::ZERO; 16];
    assert!(pending.swap_with(&mut spare).is_none());
}

#[test]
fn pending_delta_is_empty_again_immediately_after_a_swap() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let a = render_core::trace_samples(&scene, 0, 2, 1);

    let mut pending = PendingDelta::new(pixel_count);
    pending.add(0, 2, &a);
    let mut spare_a = vec![Vec3::ZERO; pixel_count];
    assert!(pending.swap_with(&mut spare_a).is_some());
    let mut spare_b = vec![Vec3::ZERO; pixel_count];
    assert!(pending.swap_with(&mut spare_b).is_none());
}

#[test]
fn next_batch_size_grows_when_well_under_budget() {
    let next = next_batch_size(4, Duration::from_millis(10));
    assert!(next > 4, "expected growth from 4, got {next}");
}

#[test]
fn next_batch_size_shrinks_when_over_budget() {
    let next = next_batch_size(100, Duration::from_millis(400));
    assert!(next < 100, "expected shrink from 100, got {next}");
    assert!(next >= 1);
}

#[test]
fn next_batch_size_never_returns_zero() {
    assert!(next_batch_size(0, Duration::from_millis(1)) >= 1);
    assert!(next_batch_size(1, Duration::from_secs(10)) >= 1);
}

#[test]
fn next_batch_size_never_exceeds_the_absolute_cap_even_under_adversarial_timing() {
    // Every call looks maximally favorable (near-zero elapsed) -- exactly what would
    // make the relative 4x-per-step clamp alone compound unboundedly across calls.
    let mut batch = 1;
    for _ in 0..40 {
        batch = next_batch_size(batch, Duration::from_nanos(1));
        assert!(
            batch <= sizing::MAX_SUBBATCH,
            "batch size {batch} exceeded the absolute cap of {}",
            sizing::MAX_SUBBATCH
        );
    }
    // Also reaches the cap, so this isn't passing vacuously.
    assert_eq!(batch, sizing::MAX_SUBBATCH);
}

#[test]
#[ignore = "manual repro: prints wall-clock batch growth against the real GPU tracer"]
fn repro_batch_growth_runaway_against_real_gpu_tracer() {
    use indicatrix::renderer::gpu_backend::GpuBackend;
    let gpu = GpuBackend::acquire();
    assert!(
        gpu.adapter_label().is_some(),
        "this probe requires a real adapter -- run probe_gpu_adapter first"
    );

    let scene = SceneState {
        width: 800,
        height: 600,
        max_bounces: 6,
        ..tiny_scene()
    };
    let total_samples: u32 = 65_536; // MAX_SAMPLES_PER_REQUEST
    // Skip one untimed warm-up dispatch: the first GPU call pays adapter/pipeline
    // warm-up not representative of steady-state throughput.
    let _ = render_core::trace_samples_with_gpu(&gpu, &scene, 0, 1, 0);
    let mut produced: u32 = 0;
    let mut batch_size: u32 = 1;
    let mut step = 0;
    let wall_start = std::time::Instant::now();
    while produced < total_samples {
        let this_batch = batch_size.min(total_samples - produced);
        let start = std::time::Instant::now();
        let _ = render_core::trace_samples_with_gpu(&gpu, &scene, produced, this_batch, 0);
        let elapsed = start.elapsed();
        eprintln!(
            "step {step}: batch={this_batch} elapsed={elapsed:?} produced_after={} wall_total={:?}",
            produced + this_batch,
            wall_start.elapsed()
        );
        produced += this_batch;
        batch_size = next_batch_size(batch_size, elapsed);
        step += 1;
        if step > 30 {
            break;
        }
    }
}

#[test]
#[ignore = "manual repro: prints wall-clock batch growth against the real GPU tracer, cheap scene"]
fn repro_batch_growth_runaway_against_real_gpu_tracer_cheap_scene() {
    use indicatrix::renderer::gpu_backend::GpuBackend;
    let gpu = GpuBackend::acquire();
    assert!(
        gpu.adapter_label().is_some(),
        "this probe requires a real adapter -- run probe_gpu_adapter first"
    );

    // Small resolution, low bounce count: makes per-sample GPU cost tiny relative to
    // fixed per-dispatch overhead, to see whether that dominance makes batches overshoot.
    let scene = SceneState {
        width: 64,
        height: 48,
        max_bounces: 2,
        ..tiny_scene()
    };
    let total_samples: u32 = 65_536;
    let _ = render_core::trace_samples_with_gpu(&gpu, &scene, 0, 1, 0);
    let mut produced: u32 = 0;
    let mut batch_size: u32 = 1;
    let mut step = 0;
    let wall_start = std::time::Instant::now();
    while produced < total_samples {
        let this_batch = batch_size.min(total_samples - produced);
        let start = std::time::Instant::now();
        let _ = render_core::trace_samples_with_gpu(&gpu, &scene, produced, this_batch, 0);
        let elapsed = start.elapsed();
        eprintln!(
            "step {step}: batch={this_batch} elapsed={elapsed:?} produced_after={} wall_total={:?}",
            produced + this_batch,
            wall_start.elapsed()
        );
        produced += this_batch;
        batch_size = next_batch_size(batch_size, elapsed);
        step += 1;
        if step > 30 {
            break;
        }
    }
}

#[test]
#[ignore = "manual repro: prints wall-clock batch growth against the real CPU tracer"]
fn repro_batch_growth_runaway_against_real_tracer() {
    // Mirrors `run_tracer`'s loop against a scene closer to real usage than this file's
    // other tiny fixtures, to see whether thread-spawn/dispatch overhead alone (no GPU)
    // makes early batches look "free" and compounds into one giant batch.
    let scene = SceneState {
        width: 64,
        height: 48,
        max_bounces: 2,
        ..tiny_scene()
    };
    let total_samples: u32 = 65_536; // MAX_SAMPLES_PER_REQUEST
    let mut produced: u32 = 0;
    let mut batch_size: u32 = 1;
    let mut step = 0;
    while produced < total_samples {
        let this_batch = batch_size.min(total_samples - produced);
        let start = std::time::Instant::now();
        let _ = render_core::trace_samples(&scene, produced, this_batch, 0);
        let elapsed = start.elapsed();
        eprintln!(
            "step {step}: batch={this_batch} elapsed={elapsed:?} produced_after={}",
            produced + this_batch
        );
        produced += this_batch;
        batch_size = next_batch_size(batch_size, elapsed);
        step += 1;
        if step > 30 {
            break;
        }
    }
}

#[test]
fn downsample_preview_produces_the_requested_dimensions() {
    let buf = vec![Vec3::ONE; 8 * 8];
    let out = downsample_preview(&buf, 8, 8, 2, 2);
    assert_eq!(out.len(), 4);
}

#[test]
fn downsample_preview_of_a_uniform_buffer_preserves_the_value() {
    let buf = vec![Vec3::new(2.0, 4.0, 6.0); 8 * 8];
    let out = downsample_preview(&buf, 8, 8, 2, 2);
    for v in out {
        assert!((v - Vec3::new(2.0, 4.0, 6.0)).length() < 1e-5);
    }
}

#[test]
fn effective_cadence_ms_is_zero_for_fewer_than_two_emissions() {
    assert_eq!(effective_cadence_ms(Duration::from_secs(1), 0), 0);
    assert_eq!(effective_cadence_ms(Duration::from_secs(1), 1), 0);
}

#[test]
fn effective_cadence_ms_averages_the_interval() {
    // 3 emissions over 2 seconds -> 2 intervals -> 1000ms average.
    assert_eq!(effective_cadence_ms(Duration::from_secs(2), 3), 1000);
}

// ---- `emit_tick`: no zero-sample PREVIEW ---------------------------------------

fn stream_config_with_preview() -> StreamConfig {
    StreamConfig {
        transfer_mode: TransferMode::FinalOnly,
        cadence_ms: 250,
        preview: Some(PreviewConfig {
            width: 2,
            height: 2,
        }),
    }
}

fn render_request_with_preview(scene: SceneState) -> RenderRequest {
    RenderRequest {
        request_id: 1,
        scene,
        first_sample: 0,
        samples: 8,
        stream: stream_config_with_preview(),
    }
}

fn shared_state(pixel_count: usize, samples_done: u32) -> SharedState {
    SharedState {
        pending_delta: PendingDelta::new(pixel_count),
        samples_done,
        finished: false,
        panicked: false,
    }
}

/// Reads every `StreamEvent` `emit_tick` wrote into `out`, in order.
fn decode_events(out: &[u8]) -> Vec<StreamEvent> {
    let mut cursor = out;
    let mut events = Vec::new();
    while !cursor.is_empty() {
        let (event, _payload) = read_stream_event(&mut cursor).expect("well-formed StreamEvent");
        events.push(event);
    }
    events
}

/// `run_stream` makes the first cadence tick "due" before any sample lands, so
/// `emit_tick` must not build a full-resolution PREVIEW of all-zeros at that point
/// (pure waste -- ~99.5 MB at 4K). Pins: no `StreamEvent::Preview` before
/// `samples_done > 0`; `StreamEvent::Progress` still goes out at 0/N.
#[test]
fn emit_tick_skips_the_preview_before_any_sample_has_landed() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = render_request_with_preview(scene);
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 0)));
    let mut emitter = EmitterAccum::new(pixel_count);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    let events = decode_events(&out);
    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Preview(_))),
        "must not emit a PREVIEW before samples_done > 0: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Progress(_))),
        "PROGRESS must still be emitted at 0/N: {events:?}"
    );
}

/// Counterpart: once a sample has landed, PREVIEW resumes. `samples_done` is 4, the
/// emitter's `running_total` is pre-seeded (standing in for earlier ticks), and this
/// tick's `pending_delta` is empty -- the state between two cadence ticks with nothing
/// new since the last one.
#[test]
fn emit_tick_resumes_the_preview_once_a_sample_has_landed() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = render_request_with_preview(scene);
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 4)));
    let mut emitter = EmitterAccum::from_running_total(vec![Vec3::ONE; pixel_count]);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    let events = decode_events(&out);
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Preview(_))),
        "PREVIEW must resume once samples_done > 0: {events:?}"
    );
}

// ---- `emit_tick`: skip a redundant full-scale PREVIEW --------------------------

/// Builds a `SharedState` whose `pending_delta` already has `contribution` folded in --
/// `shared_state` (above) always starts empty, which can't exercise `emit_tick`'s
/// `FRAME`-delta path. Pair with a fresh `EmitterAccum::new` so `emit_tick`'s own
/// `swap_and_fold` folds `contribution` in for the first time, producing
/// `emitter.running_total() == contribution` the same way production code does.
fn shared_state_with_pending_frame(
    pixel_count: usize,
    samples_done: u32,
    contribution: &[Vec3],
) -> SharedState {
    let mut pending_delta = PendingDelta::new(pixel_count);
    pending_delta.add(0, samples_done, contribution);
    SharedState {
        pending_delta,
        samples_done,
        finished: false,
        panicked: false,
    }
}

fn stream_config_with_preview_at(
    transfer_mode: TransferMode,
    width: u32,
    height: u32,
) -> StreamConfig {
    StreamConfig {
        transfer_mode,
        cadence_ms: 250,
        preview: Some(PreviewConfig { width, height }),
    }
}

/// A `PREVIEW` configured at exactly the frame's own resolution is redundant with a
/// `FRAME` delta sent in the same tick -- a client's `Accumulator` already reconstructs
/// the cumulative image by summing `FRAME` deltas, so sending both doubles bandwidth for
/// no new information. Pins: under `LiveProgressive`, with a pending delta and a
/// full-scale `PREVIEW` configured, only `FRAME` and `PROGRESS` go out.
#[test]
fn emit_tick_skips_a_full_scale_preview_when_a_frame_delta_is_also_sent() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = RenderRequest {
        request_id: 1,
        scene: scene.clone(),
        first_sample: 0,
        samples: 8,
        stream: stream_config_with_preview_at(
            TransferMode::LiveProgressive,
            scene.width,
            scene.height,
        ),
    };
    let state = Arc::new(Mutex::new(shared_state_with_pending_frame(
        pixel_count,
        4,
        &vec![Vec3::ONE; pixel_count],
    )));
    let mut emitter = EmitterAccum::new(pixel_count);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    let events = decode_events(&out);
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Frame(_))),
        "expected a FRAME delta: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Preview(_))),
        "a full-scale PREVIEW alongside a FRAME delta is redundant and must be skipped: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Progress(_))),
        "PROGRESS must still be emitted: {events:?}"
    );
}

/// Counterpart: a full-scale `PREVIEW` is not redundant under `FinalOnly`, since
/// `emit_tick` never sends a `FRAME` under that mode (`emit_final` sends the only
/// `FRAME`, once tracing completes) -- `PREVIEW` is the only progressive image a client
/// gets meanwhile, and must still go out even at matching resolution.
#[test]
fn emit_tick_still_sends_a_full_scale_preview_under_final_only() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = RenderRequest {
        request_id: 1,
        scene: scene.clone(),
        first_sample: 0,
        samples: 8,
        stream: stream_config_with_preview_at(TransferMode::FinalOnly, scene.width, scene.height),
    };
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 4)));
    let mut emitter = EmitterAccum::from_running_total(vec![Vec3::ONE; pixel_count]);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    let events = decode_events(&out);
    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Frame(_))),
        "FinalOnly must never send a FRAME from emit_tick: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Preview(_))),
        "a full-scale PREVIEW is still the only progressive image under FinalOnly and must \
         not be skipped: {events:?}"
    );
}

/// A genuinely downscaled `PREVIEW` is never skipped even alongside a `FRAME` delta --
/// only an exact full-scale match is redundant; a smaller image is cheaper and still
/// the only progressive image at that reduced size.
#[test]
fn emit_tick_still_sends_a_downscaled_preview_alongside_a_frame_delta() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = RenderRequest {
        request_id: 1,
        scene,
        first_sample: 0,
        samples: 8,
        stream: stream_config_with_preview_at(TransferMode::LiveProgressive, 2, 2),
    };
    let state = Arc::new(Mutex::new(shared_state_with_pending_frame(
        pixel_count,
        4,
        &vec![Vec3::ONE; pixel_count],
    )));
    let mut emitter = EmitterAccum::new(pixel_count);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    let events = decode_events(&out);
    assert!(events.iter().any(|e| matches!(e, StreamEvent::Frame(_))));
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Preview(_))),
        "a downscaled PREVIEW must never be skipped: {events:?}"
    );
}

// ---- S1: double-buffer swap correctness -----------------------------------------

/// Decodes the payload of the first event in `out` matching `pred`, at `width x height`.
/// Panics if no matching event is found or it carries no payload -- every caller here
/// only ever looks for `Frame`/`Preview`, both of which always do.
fn decode_payload(
    out: &[u8],
    width: u32,
    height: u32,
    pred: impl Fn(&StreamEvent) -> bool,
) -> Vec<Vec3> {
    let mut cursor = out;
    while !cursor.is_empty() {
        let (event, payload) = read_stream_event(&mut cursor).expect("well-formed StreamEvent");
        if pred(&event) {
            let bytes = payload.expect("FRAME/PREVIEW must carry a payload");
            return radiance::decode(&bytes, width, height).expect("well-formed radiance payload");
        }
    }
    panic!("no event matched the given predicate");
}

/// The `FRAME` `emit_tick` writes is exactly what was pending in `state.pending_delta`,
/// bit-for-bit (`radiance::encode`/`decode` and `PendingDelta::swap_with` are pure
/// reinterpretation/pointer-swap, no arithmetic) -- not a value the swap could have
/// silently corrupted or duplicated.
#[test]
fn emit_tick_frame_delta_equals_exactly_what_the_tracer_had_pending() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let contribution = render_core::trace_samples(&scene, 0, 4, 1);
    let request = RenderRequest {
        request_id: 7,
        scene: scene.clone(),
        first_sample: 0,
        samples: 8,
        // Downscaled, not full-scale, so the redundancy skip doesn't apply and this
        // tick writes both a FRAME and a PREVIEW to check.
        stream: stream_config_with_preview_at(TransferMode::LiveProgressive, 2, 2),
    };
    let state = Arc::new(Mutex::new(shared_state_with_pending_frame(
        pixel_count,
        4,
        &contribution,
    )));
    let mut emitter = EmitterAccum::new(pixel_count);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    let frame = decode_payload(&out, scene.width, scene.height, |e| {
        matches!(e, StreamEvent::Frame(_))
    });
    assert_eq!(
        frame, contribution,
        "the FRAME delta must equal exactly what the tracer had folded into pending_delta"
    );
}

/// `PREVIEW` is the downscaled cumulative sum -- here, the first delta ever folded in,
/// so just `contribution` downsampled -- built from `EmitterAccum::running_total`,
/// never from `state`.
#[test]
fn emit_tick_preview_equals_the_downscaled_cumulative_sum() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let contribution = render_core::trace_samples(&scene, 0, 4, 1);
    let request = RenderRequest {
        request_id: 8,
        scene: scene.clone(),
        first_sample: 0,
        samples: 8,
        stream: stream_config_with_preview_at(TransferMode::LiveProgressive, 2, 2),
    };
    let state = Arc::new(Mutex::new(shared_state_with_pending_frame(
        pixel_count,
        4,
        &contribution,
    )));
    let mut emitter = EmitterAccum::new(pixel_count);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    let preview = decode_payload(&out, 2, 2, |e| matches!(e, StreamEvent::Preview(_)));
    let expected = downsample_preview(&contribution, scene.width, scene.height, 2, 2);
    assert_eq!(
        preview, expected,
        "PREVIEW must equal the downscaled cumulative running total"
    );
}

/// Once `emit_tick` has swapped a delta out of `state.pending_delta`, anything the
/// tracer adds afterward lands in fresh, separate memory -- it can never corrupt the
/// `FRAME`/`PREVIEW` already written, nor merge into a completed tick's
/// `EmitterAccum::running_total`.
#[test]
fn emit_tick_swap_leaves_the_emitter_copy_independent_of_the_shared_pending_delta() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let first = render_core::trace_samples(&scene, 0, 4, 1);
    let request = RenderRequest {
        request_id: 9,
        scene: scene.clone(),
        first_sample: 0,
        samples: 8,
        stream: stream_config_with_preview_at(TransferMode::LiveProgressive, 2, 2),
    };
    let state = Arc::new(Mutex::new(shared_state_with_pending_frame(
        pixel_count,
        4,
        &first,
    )));
    let mut emitter = EmitterAccum::new(pixel_count);
    let mut emission_count: u32 = 0;
    let mut out = Vec::new();

    emit_tick(
        &mut out,
        &request,
        &state,
        &mut emitter,
        &mut emission_count,
    )
    .unwrap();

    // Simulate the tracer producing a second contribution right after the swap, as
    // `run_tracer`'s loop does between cadence ticks.
    let second = render_core::trace_samples(&scene, 4, 4, 1);
    {
        let mut guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.pending_delta.add(4, 4, &second);
    }

    let frame = decode_payload(&out, scene.width, scene.height, |e| {
        matches!(e, StreamEvent::Frame(_))
    });
    assert_eq!(
        frame, first,
        "the FRAME already written must be unaffected by data added to pending_delta \
         after the swap that produced it"
    );
    assert_eq!(
        emitter.running_total(),
        &first[..],
        "EmitterAccum::running_total must be genuinely separate memory from \
         state.pending_delta's buffer, not an alias of it"
    );
}

// ---- `WriteZero` is a stream timeout too -------------------------------------

#[test]
fn is_stream_timeout_recognizes_would_block_timed_out_and_write_zero() {
    for kind in [
        ErrorKind::WouldBlock,
        ErrorKind::TimedOut,
        ErrorKind::WriteZero,
    ] {
        assert!(
            is_stream_timeout(&std::io::Error::new(kind, "scripted")),
            "expected {kind:?} to be recognized as a stream timeout"
        );
    }
}

#[test]
fn is_stream_timeout_rejects_unrelated_error_kinds() {
    for kind in [
        ErrorKind::ConnectionReset,
        ErrorKind::UnexpectedEof,
        ErrorKind::Other,
    ] {
        assert!(
            !is_stream_timeout(&std::io::Error::new(kind, "scripted")),
            "expected {kind:?} to NOT be recognized as a stream timeout"
        );
    }
}

/// A `Read + TimeoutRead` double whose very first (and only) read returns one scripted
/// `io::Error`, used to pin `poll_for_client_message`'s classification of that error.
struct ErroringRead(ErrorKind);

impl std::io::Read for ErroringRead {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(self.0, "scripted"))
    }
}

impl TimeoutRead for ErroringRead {
    fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// A real TLS stream can surface a socket timeout as `WriteZero` even from a read call
/// (`rustls::Stream::read`'s `complete_io()` can need to flush outgoing bytes too).
/// Pins that `poll_for_client_message` tolerates it like `WouldBlock`/`TimedOut`, not
/// as a fatal protocol error.
#[test]
fn poll_for_client_message_tolerates_a_write_zero_on_the_first_byte() {
    let mut stream = ErroringRead(ErrorKind::WriteZero);
    let mut timeouts = TimeoutCache::new();
    let result = poll_for_client_message(&mut stream, 1, Duration::from_millis(1), &mut timeouts);
    assert!(matches!(result, Ok(ClientPoll::Pending)));
}

// ---- HEARTBEAT_INTERVAL: cadence-independent liveness backstop -------------------
//
// `run_stream`'s main loop applies the same gating logic tested here, but that requires
// a real tracer thread to observe "produces nothing for a while" against.
// `wait_for_tracer_to_stop` is the same gate with none of that machinery: a plain
// `Write`, a `SharedState` driven directly, and an injectable interval.

/// Builds a minimal `RenderRequest` for the tests below -- `wait_for_tracer_to_stop`
/// only ever reads `request_id` off it.
fn heartbeat_test_request() -> RenderRequest {
    render_request_with_preview(tiny_scene())
}

/// Spawns a thread that flips `state.finished` after `after` real wall time, letting
/// `wait_for_tracer_to_stop`'s unbounded `while !finished` loop terminate -- standing
/// in for the tracer noticing `cancel` and exiting. Always tens of milliseconds here,
/// far under the real 2s `HEARTBEAT_INTERVAL`, since these tests inject a tiny interval
/// instead to stay fast.
fn finish_after(state: &Arc<Mutex<SharedState>>, after: Duration) -> std::thread::JoinHandle<()> {
    let state = Arc::clone(state);
    std::thread::spawn(move || {
        std::thread::sleep(after);
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finished = true;
    })
}

/// A tracer that produces nothing for well over two heartbeat intervals must still see
/// at least two `StreamEvent::Progress` heartbeats -- what a wide-`cadence` cancellation
/// wind-down (or a slow calibration probe, long GPU sub-batch, hybrid CPU-only tail)
/// needs from this function.
#[test]
fn wait_for_tracer_to_stop_heartbeats_at_least_twice_when_the_tracer_produces_nothing() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 0)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    let finisher = finish_after(&state, Duration::from_millis(70));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_millis(2), // heartbeat_bound: tiny, so this test stays fast
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    let events = decode_events(&out);
    let progress_count = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Progress(_)))
        .count();
    assert!(
        progress_count >= 2,
        "expected at least 2 heartbeats over > 2 heartbeat intervals, got {progress_count}: {events:?}"
    );
}

/// The counterpart: while `heartbeat_bound` hasn't elapsed yet, no heartbeat goes out
/// at all -- a tick that already proved liveness this recently must suppress the extra
/// write, not double up on it.
#[test]
fn wait_for_tracer_to_stop_suppresses_the_heartbeat_within_the_interval() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 0)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    // Finishes well before the bound below could possibly elapse.
    let finisher = finish_after(&state, Duration::from_millis(30));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_secs(10),
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    assert!(
        out.is_empty(),
        "no heartbeat should have been written within the interval: {:?}",
        decode_events(&out)
    );
}

/// The heartbeat's payload is a live read of `samples_done` at the moment it fires --
/// pins that it is never stale.
#[test]
fn wait_for_tracer_to_stop_heartbeat_carries_the_latest_samples_done() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 42)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    let finisher = finish_after(&state, Duration::from_millis(60));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_millis(2),
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    let events = decode_events(&out);
    let last_progress = events.iter().rev().find_map(|e| match e {
        StreamEvent::Progress(p) => Some(p.samples_done),
        _ => None,
    });
    assert_eq!(
        last_progress,
        Some(42),
        "heartbeat must carry the current samples_done: {events:?}"
    );
}
