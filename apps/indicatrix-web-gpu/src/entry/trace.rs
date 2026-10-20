//! The GPU Worker's `TraceFullPass` handling: takes the renderer and the cached scene out
//! of the shared state, traces one full-frame pass, puts both back and posts the answer.
//!
//! Split out of `lib.rs` (the message handler grew past the lint's line limit); the shared
//! state, `post` and the device flags live in the parent module.

use std::{cell::RefCell, rc::Rc};

use glam::Vec3;
use indicatrix::renderer::gpu::event_wavefront::{EventWavefrontRenderer, EwError};
use indicatrix_web_core::{
    protocol::{FromWorker, WorkerRole},
    render::Accumulator,
};

use super::{CachedScene, DEVICE_LOST, GpuWorkerState, now_ms, post};

/// The pass a `TraceFullPass` message asked for.
#[derive(Clone, Copy)]
pub(super) struct Pass {
    pub(super) scene_id: u64,
    pub(super) id: u64,
    pub(super) sample_offset: u32,
    pub(super) spp: u32,
}

/// `true` for a sum buffer the accumulator may merge: non-empty, every value finite
/// and non-negative.
fn sums_are_valid(sums: &[[f32; 3]]) -> bool {
    Accumulator::full_pass_sums_valid(sums)
}

/// Posts `ChunkDropped` for `pass`: the page hands the pass back to the CPU workers.
fn post_dropped(pass: Pass) {
    post(&FromWorker::ChunkDropped {
        scene_id: pass.scene_id,
        first_pixel: 0,
        sample_offset: pass.sample_offset,
    });
}

/// Takes the renderer and the scene OUT of the shared state: no `RefCell` borrow may live
/// across the `.await` of the trace. `None` after posting why the pass cannot run.
fn take_pass_inputs(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    pass: Pass,
) -> Option<(EventWavefrontRenderer, CachedScene)> {
    let mut state = state_rc.borrow_mut();
    if state.role != Some(WorkerRole::Gpu) {
        drop(state);
        post(&FromWorker::Error {
            message: "TraceFullPass before a successful Init".to_string(),
        });
        return None;
    }
    let scene_ok = state
        .cached_scene
        .as_ref()
        .is_some_and(|c| c.scene_id == pass.scene_id);
    if !scene_ok {
        drop(state);
        post_dropped(pass);
        return None;
    }
    let refusal = state.cached_scene.as_ref().and_then(|c| c.ineligible);
    if let Some(reason) = refusal {
        drop(state);
        post(&FromWorker::Error {
            message: format!("the GPU worker refused the pass: {reason}"),
        });
        post_dropped(pass);
        return None;
    }
    let Some(renderer) = state.renderer.take() else {
        drop(state);
        post(&FromWorker::GpuLost {
            reason: "renderer is not initialized".to_string(),
        });
        return None;
    };
    let Some(cached) = state.cached_scene.take() else {
        state.renderer = Some(renderer);
        return None;
    };
    Some((renderer, cached))
}

/// Posts the answer to a traced pass: its sums when they are valid, `GpuLost` for a lost
/// device or a pass with NaN, infinite or negative sums, and an error plus `ChunkDropped`
/// when only this pass cannot run. `pass_start_ms` is when the trace began.
fn report_pass(pass: Pass, result: Result<(), EwError>, sums: Vec<Vec3>, pass_start_ms: f64) {
    match result {
        Ok(()) => {
            let elapsed_ms = now_ms() - pass_start_ms;
            let sums_arr: Vec<[f32; 3]> = sums.iter().map(Vec3::to_array).collect();
            drop(sums);
            if sums_are_valid(&sums_arr) {
                post(&FromWorker::FullPassResult {
                    scene_id: pass.scene_id,
                    pass_id: pass.id,
                    sample_offset: pass.sample_offset,
                    spp: pass.spp,
                    sums: sums_arr,
                    elapsed_ms,
                });
            } else {
                // A pass with NaN, infinite or negative sums must never reach
                // the accumulator; GpuLost makes the pool hand the pass back
                // to the CPU workers.
                post(&FromWorker::GpuLost {
                    reason: "the GPU pass produced non-finite or negative sums".to_string(),
                });
            }
        }
        Err(
            error @ (EwError::DeviceLost(_) | EwError::ChunkTimeout | EwError::BufferMapping(_)),
        ) => {
            DEVICE_LOST.with(|l| l.set(true));
            post(&FromWorker::GpuLost {
                reason: format!("event wavefront accumulation failed: {error}"),
            });
        }
        Err(error) => {
            // An eligibility error (unsupported material, bad buffer size):
            // the device is fine, only this pass cannot run.
            post(&FromWorker::Error {
                message: format!("event wavefront pass refused: {error}"),
            });
            post_dropped(pass);
        }
    }
}

/// Handles `TraceFullPass`: traces `pass` on the GPU and posts the answer.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn handle_full_pass(state_rc: &Rc<RefCell<GpuWorkerState>>, pass: Pass) {
    let Some((mut renderer, cached)) = take_pass_inputs(state_rc, pass) else {
        return;
    };

    let num_pixels = cached.width as usize * cached.height as usize;
    let mut sums = vec![Vec3::ZERO; num_pixels];
    let pass_start_ms = now_ms();

    let result = {
        let frame_scene = cached.scene.frame_scene();
        renderer
            .accumulate_with_tools(
                &frame_scene,
                cached.scene.tools(),
                pass.sample_offset,
                pass.spp,
                &mut sums,
            )
            .await
    };

    // Put both back before anything else can look at the state.
    {
        let mut state = state_rc.borrow_mut();
        state.renderer = Some(renderer);
        state.cached_scene = Some(cached);
    }

    report_pass(pass, result, sums, pass_start_ms);
}
