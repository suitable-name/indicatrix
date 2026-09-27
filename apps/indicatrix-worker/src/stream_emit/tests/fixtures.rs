//! Shared test fixtures for `stream_emit`'s tests: scenes, `SharedState`/`RenderRequest`
//! builders, and the `StreamEvent` decoding helpers every topic file in this folder uses.

use crate::stream_emit::{emitter::PendingDelta, tracer::SharedState};
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
};

pub(super) fn tiny_scene() -> SceneState {
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
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
    }
}

/// Reads every `StreamEvent` `emit_tick` wrote into `out`, in order.
pub(super) fn decode_events(out: &[u8]) -> Vec<StreamEvent> {
    let mut cursor = out;
    let mut events = Vec::new();
    while !cursor.is_empty() {
        let (event, _payload) = read_stream_event(&mut cursor).expect("well-formed StreamEvent");
        events.push(event);
    }
    events
}

pub(super) fn stream_config_with_preview() -> StreamConfig {
    StreamConfig {
        transfer_mode: TransferMode::FinalOnly,
        cadence_ms: 250,
        preview: Some(PreviewConfig {
            width: 2,
            height: 2,
        }),
    }
}

pub(super) fn render_request_with_preview(scene: SceneState) -> RenderRequest {
    RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 1,
        scene,
        first_sample: 0,
        samples: 8,
        stream: stream_config_with_preview(),
    }
}

pub(super) fn shared_state(pixel_count: usize, samples_done: u32) -> SharedState {
    SharedState {
        samples_done,
        ..SharedState::new(pixel_count)
    }
}

/// Builds a `SharedState` whose `pending_delta` already has `contribution` folded in --
/// [`shared_state`] always starts empty, which can't exercise `emit_tick`'s `FRAME`-delta
/// path. Pair with a fresh `EmitterAccum::new` so `emit_tick`'s own `swap_and_fold` folds
/// `contribution` in for the first time, producing `emitter.running_total() ==
/// contribution` the same way production code does.
pub(super) fn shared_state_with_pending_frame(
    pixel_count: usize,
    samples_done: u32,
    contribution: &[Vec3],
) -> SharedState {
    let mut pending_delta = PendingDelta::new(pixel_count);
    pending_delta.add(0, samples_done, contribution);
    SharedState {
        pending_delta,
        samples_done,
        ..SharedState::new(0)
    }
}

pub(super) fn stream_config_with_preview_at(
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

/// Decodes the payload of the first event in `out` matching `pred`, at `width x height`.
/// Panics if no matching event is found or it carries no payload -- every caller here
/// only ever looks for `Frame`/`Preview`, both of which always do.
pub(super) fn decode_payload(
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
            return indicatrix_net::radiance::decode(&bytes, width, height)
                .expect("well-formed radiance payload");
        }
    }
    panic!("no event matched the given predicate");
}

/// Builds a minimal `RenderRequest` for the heartbeat tests -- `wait_for_tracer_to_stop`
/// only ever reads `request_id` off it.
pub(super) fn heartbeat_test_request() -> RenderRequest {
    render_request_with_preview(tiny_scene())
}

/// Spawns a thread that flips `state.finished` after `after` real wall time, letting
/// `wait_for_tracer_to_stop`'s unbounded `while !finished` loop terminate -- standing
/// in for the tracer noticing `cancel` and exiting. Always tens of milliseconds here,
/// far under the real 2s `HEARTBEAT_INTERVAL`, since these tests inject a tiny interval
/// instead to stay fast.
pub(super) fn finish_after(
    state: &std::sync::Arc<std::sync::Mutex<SharedState>>,
    after: std::time::Duration,
) -> std::thread::JoinHandle<()> {
    let state = std::sync::Arc::clone(state);
    std::thread::spawn(move || {
        std::thread::sleep(after);
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finished = true;
    })
}
