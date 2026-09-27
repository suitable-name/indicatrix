//! Tests for [`emit_tick`]: no zero-sample `PREVIEW`, skipping a redundant full-scale
//! `PREVIEW` alongside a `FRAME` delta, and (S1) double-buffer swap correctness.

use super::fixtures::{
    decode_events, decode_payload, render_request_with_preview, shared_state,
    shared_state_with_pending_frame, stream_config_with_preview_at, tiny_scene,
};
use crate::{
    render_core,
    stream_emit::{
        downsample::downsample_preview,
        emitter::{EmitterAccum, emit_tick},
    },
};
use glam::Vec3;
use indicatrix_net::messages::{RenderRequest, StreamEvent, TransferMode};
use std::sync::{Arc, Mutex};

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
        intent: indicatrix_net::messages::RequestIntent::Batch,
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
        intent: indicatrix_net::messages::RequestIntent::Batch,
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
        intent: indicatrix_net::messages::RequestIntent::Batch,
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
        intent: indicatrix_net::messages::RequestIntent::Batch,
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
        intent: indicatrix_net::messages::RequestIntent::Batch,
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
        intent: indicatrix_net::messages::RequestIntent::Batch,
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
