//! Batched preview tracing, including deterministic tap ranges.
use super::{
    AssertUnwindSafe, AtomicBool, BatchItem, Instant, Ordering, QueuedBatch, StageJob, TraceEnv,
    Vec3, assets, catch_unwind, check_item, render_core,
};
use indicatrix::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG, FacetFinish},
    renderer::gpu_backend::{GpuAccumulate, GpuBatchItem, GpuSceneRef},
};
/// The tracer thread's body for one batch.
pub(super) fn trace_batch(env: &TraceEnv, batch: &QueuedBatch) {
    let request_id = batch.request.request_id;
    let cancel: &AtomicBool = &batch.cancel;
    let started = Instant::now();

    let mut gpu_items: Vec<(usize, &BatchItem)> = Vec::new();
    let mut cpu_items: Vec<(usize, &BatchItem)> = Vec::new();
    for (index, item) in batch.request.items.iter().enumerate() {
        match check_item(item) {
            Err(reason) => env.fail(request_id, item.item_id, reason),
            Ok(()) if render_core::scene_uses_gpu(&item.scene) => gpu_items.push((index, item)),
            Ok(()) => cpu_items.push((index, item)),
        }
    }

    if !cancel.load(Ordering::Relaxed) {
        let declined = trace_on_gpu(env, request_id, &gpu_items, cancel);
        cpu_items.extend(declined);
        cpu_items.sort_by_key(|(index, _)| *index);
        trace_on_cpu(env, request_id, &cpu_items, cancel);
    }

    let cancelled = cancel.load(Ordering::Relaxed);
    let _ = env.stage.send(StageJob::End {
        request_id,
        cancelled,
    });
    tracing::info!(
        request_id,
        items = batch.request.items.len(),
        gpu_items = gpu_items.len(),
        cancelled,
        traced_in = ?started.elapsed(),
        "worker: traced a preview batch"
    );
}

/// Traces `items` in ONE `GpuBackend::try_accumulate_batch_cancellable` call and hands
/// every finished item to the encoder. Returns the items the GPU declined, for the CPU.
///
/// Delivery happens after the call returns, because the call holds every item's output
/// buffer for its whole duration; the encoder overlaps the NEXT batch's tracing instead.
fn trace_on_gpu<'a>(
    env: &TraceEnv,
    request_id: u32,
    items: &[(usize, &'a BatchItem)],
    cancel: &AtomicBool,
) -> Vec<(usize, &'a BatchItem)> {
    if items.is_empty() {
        return Vec::new();
    }
    let cameras: Vec<Camera> = items
        .iter()
        .map(|(_, item)| {
            Camera::new(
                item.scene.yaw,
                item.scene.pitch,
                item.scene.distance,
                DEFAULT_FOV_DEG,
            )
        })
        .collect();
    let finishes: Vec<Vec<FacetFinish>> = items
        .iter()
        .map(|(_, item)| render_core::resolve_facet_finishes(&item.scene))
        .collect();
    let mut outs: Vec<Vec<Vec3>> = items
        .iter()
        .map(|(_, item)| vec![Vec3::ZERO; item.scene.width as usize * item.scene.height as usize])
        .collect();
    let mut outcomes: Vec<Option<GpuAccumulate>> = vec![None; items.len()];

    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut batch: Vec<GpuBatchItem<'_>> = items
            .iter()
            .zip(&cameras)
            .zip(&finishes)
            .zip(outs.iter_mut())
            .map(|(((&(_, item), camera), finish), out)| GpuBatchItem {
                scene: GpuSceneRef {
                    camera,
                    width: item.scene.width,
                    height: item.scene.height,
                    planes: &item.scene.planes,
                    facet_finishes: finish,
                    material: &item.scene.material,
                    max_bounces: item.scene.max_bounces,
                    environment: assets::environment_source(&item.scene, None),
                },
                tools: &item.scene.tools,
                first_sample: item.first_sample,
                samples: item.samples,
                out: out.as_mut_slice(),
            })
            .collect();
        env.gpu
            .try_accumulate_batch_cancellable(&mut batch, cancel, &mut |index, outcome| {
                if let Some(slot) = outcomes.get_mut(index) {
                    *slot = Some(outcome);
                }
                if outcome == GpuAccumulate::Done
                    && let Some(&(_, item)) = items.get(index)
                {
                    env.traced(request_id, item);
                }
            });
    }));
    let panicked = result.is_err();

    let mut declined = Vec::new();
    for ((&(index, item), accum), outcome) in items.iter().zip(outs).zip(outcomes) {
        match outcome {
            Some(GpuAccumulate::Done) => env.picture(request_id, item, accum),
            Some(GpuAccumulate::Declined) => declined.push((index, item)),
            None if panicked => env.fail(
                request_id,
                item.item_id,
                "the GPU tracer panicked while tracing this batch",
            ),
            // The backend never reported this item (it stopped early without a cancel):
            // give it to the CPU rather than lose it.
            None if !cancel.load(Ordering::Relaxed) => declined.push((index, item)),
            Some(GpuAccumulate::Cancelled) | None => {}
        }
    }
    declined
}

/// Traces `items` one by one on the CPU, handing each finished item to the encoder.
fn trace_on_cpu(
    env: &TraceEnv,
    request_id: u32,
    items: &[(usize, &BatchItem)],
    cancel: &AtomicBool,
) {
    for &(_, item) in items {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let traced = catch_unwind(AssertUnwindSafe(|| {
            if item.scene.engine.is_ray() {
                let mut out =
                    vec![Vec3::ZERO; item.scene.width as usize * item.scene.height as usize];
                return render_core::whitted::trace(
                    &item.scene,
                    item.first_sample,
                    item.samples,
                    env.threads,
                    env.prefer_ray_gpu,
                    cancel,
                    &mut out,
                )
                .then_some(out);
            }
            render_core::trace_samples_with_gpu_cancellable(
                &env.cpu,
                &item.scene,
                item.first_sample,
                item.samples,
                env.threads,
                cancel,
            )
        }));
        match traced {
            Ok(Some(accum)) => {
                env.traced(request_id, item);
                env.picture(request_id, item, accum);
            }
            // Cancelled mid-trace: nothing was added, and nothing is owed.
            Ok(None) if !item.scene.engine.is_ray() || cancel.load(Ordering::Acquire) => return,
            Ok(None) => env.fail(
                request_id,
                item.item_id,
                "the renderer could not trace this item's complete sample range",
            ),
            Err(_) => env.fail(
                request_id,
                item.item_id,
                "tracing panicked on a scene that passed validation",
            ),
        }
    }
}
