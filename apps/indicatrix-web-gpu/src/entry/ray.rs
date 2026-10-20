//! The small Ray-Traced kernel, qualified independently from event-wavefront.

use std::{cell::RefCell, rc::Rc};

use glam::Vec3;
use indicatrix::renderer::{
    engine::{EngineSpec, RayOptions, RenderEngine},
    gpu::{
        event_wavefront::compare::compare_frames,
        whitted::{CHUNK_PIXELS, WhittedGpuError, WhittedGpuRenderer},
    },
};
use indicatrix_web_core::{
    gpu_admission,
    protocol::{FromWorker, WorkerRole},
    render::{Accumulator, handle_trace_chunk},
    scene::OwnedScene,
};

use super::{
    CachedScene, DEVICE_LOST, GpuWorkerState, RaceResult, device_lost, now_ms, post, timeout_race,
    trace::Pass,
};

pub(super) fn set_engine(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    scene_id: u64,
    engine: EngineSpec,
) {
    let mut state = state_rc.borrow_mut();
    // The device's real storage limit and the held HDR map's texel count let the classifier
    // refuse an oversized map once here, as the page does (0 / None would never refuse).
    let max_storage_binding = state.context.as_ref().map_or(0, |context| {
        context.device.limits().max_storage_buffer_binding_size
    });
    let hdr_texels = state
        .cached_hdr
        .as_ref()
        .map(|(_, map)| (map.width() as u64).saturating_mul(map.height() as u64));
    let Some(cached) = state
        .cached_scene
        .as_mut()
        .filter(|s| s.scene_id == scene_id)
    else {
        return;
    };
    cached.ray_gpu = None;
    if let Err(error) = cached.scene.set_engine(engine) {
        state.cached_scene = None;
        post(&FromWorker::SceneError {
            scene_id,
            message: error.to_string(),
        });
        return;
    }
    cached.ineligible = if engine.is_ray() {
        gpu_admission::whitted_ineligibility_reason(
            &cached.spec,
            cached.scene.tools().len(),
            max_storage_binding,
            hdr_texels,
            !cached.scene.fluorescence().is_empty(),
        )
    } else {
        gpu_admission::gpu_ineligibility_reason_with_tools(
            &cached.spec,
            cached.scene.tools().len(),
            0,
            None,
            !cached.scene.fluorescence().is_empty(),
        )
    };
}

/// Compilation includes the 16×16, four-tap CPU comparison before it may report success.
#[expect(
    clippy::future_not_send,
    reason = "browser workers are single-threaded"
)]
pub(super) async fn compile(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    class: u32,
    soft_ms: u32,
    hard_ms: u32,
) -> bool {
    let start = now_ms();
    let context = state_rc.borrow().context.clone();
    let attempt = async {
        let context = context.ok_or_else(|| "Ray-Traced device is not initialized".to_owned())?;
        let mut renderer = WhittedGpuRenderer::from_context_async(context)
            .await
            .map_err(|e| e.to_string())?;
        let mut scene = OwnedScene::build(&gpu_admission::self_check_scene(), None)
            .map_err(|e| e.to_string())?;
        scene
            .set_engine(EngineSpec {
                engine: RenderEngine::RayTraced,
                ray: RayOptions {
                    aa_taps: 4,
                    fire: true,
                },
            })
            .map_err(|e| e.to_string())?;
        let prepared = renderer
            .prepare(
                &scene.frame_scene(),
                scene.tools(),
                scene.whitted_prep().map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let actual = renderer
            .trace_chunk_async(&prepared, 0, 256, 0, 4)
            .await
            .map_err(|e| e.to_string())?;
        let reference = handle_trace_chunk(&scene, scene.plane_soa(), 0, 1, 0, 4);
        if actual.len() != reference.len() {
            return Err("Ray-Traced self-check returned an incomplete frame".to_owned());
        }
        let difference = compare_frames(&actual, &reference, 4);
        if !difference.tol_passed(256) {
            return Err(format!(
                "Ray-Traced CPU/GPU self-check failed: {} pixels outside G-D10, max error {}",
                difference.tol_failing_pixels, difference.max_abs_diff
            ));
        }
        if device_lost() {
            return Err("Ray-Traced device was lost during qualification".to_owned());
        }
        Ok(renderer)
    };
    let result = match timeout_race(attempt, f64::from(hard_ms)).await {
        RaceResult::Completed(result) => result,
        RaceResult::TimedOut => Err(format!(
            "Ray-Traced qualification timed out after {hard_ms} ms"
        )),
    };
    let elapsed = now_ms() - start;
    let result = result.and_then(|renderer| {
        if elapsed > f64::from(soft_ms) {
            Err(format!(
                "Ray-Traced qualification took {elapsed:.1} ms (limit {soft_ms})"
            ))
        } else {
            Ok(renderer)
        }
    });
    match result {
        Ok(renderer) => {
            state_rc.borrow_mut().whitted = Some(renderer);
            post(&FromWorker::GpuCompiled {
                kernel: gpu_admission::WHITTED_KERNEL.to_owned(),
                class,
                ms: elapsed,
            });
            true
        }
        Err(reason) => {
            state_rc.borrow_mut().whitted = None;
            post(&FromWorker::GpuCompileFailed {
                kernel: gpu_admission::WHITTED_KERNEL.to_owned(),
                class,
                reason,
                ms: elapsed,
            });
            false
        }
    }
}

fn dropped(pass: Pass) {
    post(&FromWorker::ChunkDropped {
        scene_id: pass.scene_id,
        first_pixel: 0,
        sample_offset: pass.sample_offset,
    });
}

/// Upload the scene once, then trace every chunk of one pass and gather the sums.
#[expect(
    clippy::future_not_send,
    reason = "browser workers are single-threaded"
)]
async fn trace_all_chunks(
    renderer: &mut WhittedGpuRenderer,
    cached: &mut CachedScene,
    pass: Pass,
) -> Result<Vec<Vec3>, WhittedGpuError> {
    if cached.ray_gpu.is_none() {
        cached.ray_gpu = Some(
            renderer.prepare(
                &cached.scene.frame_scene(),
                cached.scene.tools(),
                cached
                    .scene
                    .whitted_prep()
                    .map_err(|e| WhittedGpuError::Declined(e.to_string()))?,
            )?,
        );
    }
    let scene = cached.ray_gpu.as_ref().ok_or_else(|| {
        WhittedGpuError::Declined("Ray-Traced scene uploads unavailable".to_owned())
    })?;
    let mut sums = Vec::with_capacity(scene.pixels as usize);
    let mut first = 0;
    while first < scene.pixels {
        if device_lost() {
            return Err(WhittedGpuError::DeviceLost(
                "device lost during Ray-Traced pass".to_owned(),
            ));
        }
        let pixels = (scene.pixels - first).min(CHUNK_PIXELS);
        sums.extend(
            renderer
                .trace_chunk_async(scene, first, pixels, pass.sample_offset, pass.spp)
                .await?,
        );
        first += pixels;
    }
    Ok(sums)
}

#[expect(
    clippy::future_not_send,
    reason = "browser workers are single-threaded"
)]
pub(super) async fn trace_pass(state_rc: &Rc<RefCell<GpuWorkerState>>, pass: Pass) {
    let inputs = {
        let mut state = state_rc.borrow_mut();
        if state.role != Some(WorkerRole::Gpu)
            || state
                .cached_scene
                .as_ref()
                .is_none_or(|s| s.scene_id != pass.scene_id || s.ineligible.is_some())
        {
            drop(state);
            dropped(pass);
            return;
        }
        match (state.whitted.take(), state.cached_scene.take()) {
            (Some(renderer), Some(scene)) => Some((renderer, scene)),
            (renderer, scene) => {
                state.whitted = renderer;
                state.cached_scene = scene;
                None
            }
        }
    };
    let Some((mut renderer, mut cached)) = inputs else {
        dropped(pass);
        return;
    };
    let started = now_ms();
    let result = trace_all_chunks(&mut renderer, &mut cached, pass).await;
    {
        let mut state = state_rc.borrow_mut();
        state.whitted = Some(renderer);
        state.cached_scene = Some(cached);
    }
    match result {
        Ok(sums) => {
            let sums: Vec<_> = sums.iter().map(Vec3::to_array).collect();
            if Accumulator::full_pass_sums_valid(&sums) && !device_lost() {
                post(&FromWorker::FullPassResult {
                    scene_id: pass.scene_id,
                    pass_id: pass.id,
                    sample_offset: pass.sample_offset,
                    spp: pass.spp,
                    sums,
                    elapsed_ms: now_ms() - started,
                });
            } else {
                DEVICE_LOST.with(|lost| lost.set(true));
                post(&FromWorker::GpuLost {
                    reason: "Ray-Traced returned invalid pixels".to_owned(),
                });
            }
        }
        Err(WhittedGpuError::DeviceLost(reason)) => {
            DEVICE_LOST.with(|lost| lost.set(true));
            post(&FromWorker::GpuLost { reason });
        }
        Err(WhittedGpuError::Declined(reason)) => {
            // A decline is a property of the scene, not of the pass: mark it ineligible so
            // later passes are dropped without a GPU round trip or another Error.
            if let Some(cached) = state_rc
                .borrow_mut()
                .cached_scene
                .as_mut()
                .filter(|s| s.scene_id == pass.scene_id)
            {
                cached.ineligible = Some("Ray-Traced GPU upload declined");
            }
            post(&FromWorker::Error {
                message: format!("Ray-Traced GPU pass refused: {reason}"),
            });
            dropped(pass);
        }
    }
}
