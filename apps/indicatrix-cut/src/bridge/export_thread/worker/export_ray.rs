//! Deterministic local shares for stills, tilt frames and remote retries.

use crate::bridge::export_thread::batch::ExportCtx;
use glam::Vec3;
use indicatrix::{
    optics::raytracer::add_finite_sample,
    renderer::{frame_scene::FrameScene, whitted::whitted_tap_xyz},
};
use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Reject unsupported physics before a final-picture request can cross the wire.
/// The local frame preparation repeats this guard for direct accumulation callers.
pub(super) fn refusal(
    scene: &crate::bridge::export_thread::scene_snapshot::SceneSnapshot,
) -> Option<String> {
    use indicatrix::renderer::whitted::WhittedDecline;
    if !scene.engine.is_ray() {
        return None;
    }
    if !scene.fluorescence.is_empty() {
        return Some(WhittedDecline::Fluorescence.to_string());
    }
    if scene.env_map.is_none() && scene.lighting_preset.is_uv_lamp() {
        return Some(WhittedDecline::UvLamp.to_string());
    }
    None
}

/// Each pixel sums its assigned absolute taps in order; claims are disjoint.
/// Preparation belongs to the whole export frame, never a claim or pixel.
pub(in crate::bridge::export_thread) fn accumulate(
    ctx: &ExportCtx<'_>,
    first: u32,
    count: u32,
    sums: &mut [Vec3],
    cancel: &AtomicBool,
) -> bool {
    const CHUNK: usize = 256;
    if cancel.load(Ordering::Acquire) {
        return true;
    }
    let (prep, soa) = ctx
        .ray
        .expect("Ray-Traced export preparation was validated once");
    let scene = FrameScene {
        camera: ctx.camera,
        width: ctx.width,
        height: ctx.height,
        planes: &ctx.scene.active_planes,
        facet_finishes: &ctx.scene.facet_finishes,
        material: &ctx.scene.material,
        max_bounces: ctx.scene.max_bounces,
        environment: ctx.gpu_scene.environment,
    };
    if !ctx.gpu_retired.load(Ordering::Relaxed)
        && let Some((gpu, uploaded)) = ctx.ray_gpu
    {
        use indicatrix::renderer::gpu_backend::GpuAccumulate;
        match gpu.try_accumulate_prepared_cancellable(uploaded, first, count, sums, &|| {
            cancel.load(Ordering::Acquire)
        }) {
            GpuAccumulate::Done => return false,
            GpuAccumulate::Cancelled => return true,
            GpuAccumulate::Declined => {
                // Transactional: CPU retries this entire unchanged claim. A stale/lost
                // device snapshot is retired for this frame, avoiding repeated probes.
                ctx.gpu_retired.store(true, Ordering::Relaxed);
            }
        }
    }
    let end = first.saturating_add(count).min(u32::from(prep.taps));
    // Pixels are claimed in disjoint chunks from one atomic counter; every pixel's sum
    // depends only on its index, so the result is bit-identical for any thread count.
    let chunks = Mutex::new(
        sums.chunks_mut(CHUNK)
            .enumerate()
            .map(|(index, chunk)| Some((index * CHUNK, chunk)))
            .collect::<Vec<_>>(),
    );
    let chunk_count = chunks.lock().unwrap_or_else(PoisonError::into_inner).len();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    std::thread::scope(|scope| {
        for _ in 0..threads.min(chunk_count.max(1)) {
            let (chunks, next, scene) = (&chunks, &next, &scene);
            scope.spawn(move || {
                loop {
                    let claim = next.fetch_add(1, Ordering::Relaxed);
                    if claim >= chunk_count {
                        break;
                    }
                    let (base, chunk) = chunks.lock().unwrap_or_else(PoisonError::into_inner)
                        [claim]
                        .take()
                        .expect("an atomic chunk index is claimed exactly once");
                    for (offset, sum) in chunk.iter_mut().enumerate() {
                        let pixel = u32::try_from(base + offset)
                            .expect("validated export dimensions fit u32");
                        let mut contribution = Vec3::ZERO;
                        for tap in first..end {
                            if cancel.load(Ordering::Relaxed) {
                                break;
                            }
                            add_finite_sample(
                                &mut contribution,
                                whitted_tap_xyz(
                                    scene,
                                    &ctx.scene.tools,
                                    soa,
                                    prep,
                                    pixel,
                                    tap,
                                    None,
                                ),
                            );
                        }
                        *sum += contribution;
                    }
                }
            });
        }
    });
    cancel.load(Ordering::Acquire)
}
