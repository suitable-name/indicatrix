//! Deterministic worker tracing. Preparation is reused across a request's tap batches.
use glam::Vec3;
use indicatrix::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG, FacetFinish, add_finite_sample, build_plane_soa},
    renderer::{
        frame_scene::FrameScene,
        gpu_backend::{
            GpuAccumulate,
            whitted::{WhittedGpuBackend, WhittedGpuPrepared},
        },
        whitted::{WhittedPrep, whitted_tap_xyz},
    },
    simd::PlanesSoA32,
};
use indicatrix_net::SceneState;
use std::{
    cell::RefCell,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

struct Prepared {
    camera: Camera,
    finishes: Vec<FacetFinish>,
    planes: PlanesSoA32,
    prep: WhittedPrep<'static>,
    /// Frozen uploads belong to this immutable scene, never an address-based key.
    gpu: Mutex<Option<WhittedGpuPrepared>>,
}
thread_local! {
    // A request has one tracer thread; no unbounded process-wide scene/HDR cache.
    static PREPARED: RefCell<Option<(SceneState, Arc<Prepared>)>> = const { RefCell::new(None) };
}
static GPU: LazyLock<WhittedGpuBackend> = LazyLock::new(WhittedGpuBackend::acquire);

fn frame<'a>(
    scene: &'a SceneState,
    camera: &'a Camera,
    finishes: &'a [FacetFinish],
    environment: indicatrix::optics::raytracer::EnvironmentSource<'a>,
) -> FrameScene<'a> {
    FrameScene {
        camera,
        width: scene.width,
        height: scene.height,
        planes: &scene.planes,
        facet_finishes: finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment,
    }
}
fn prepared(scene: &SceneState) -> Option<Arc<Prepared>> {
    PREPARED.with(|slot| {
        if let Some((key, prep)) = slot.borrow().as_ref()
            && key == scene
        {
            return Some(Arc::clone(prep));
        }
        let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG);
        let finishes = super::resolve_facet_finishes(scene);
        let hdr = crate::assets::resolved_hdr_map(scene);
        let environment = crate::assets::environment_source(scene, hdr.as_deref());
        let prep = WhittedPrep::new(
            &frame(scene, &camera, &finishes, environment),
            &scene.fluorescence,
            scene.engine.ray,
        )
        .ok()?
        .into_owned_with_hdr(hdr.clone());
        let value = Arc::new(Prepared {
            camera,
            finishes,
            planes: build_plane_soa(&scene.planes),
            prep,
            gpu: Mutex::new(None),
        });
        *slot.borrow_mut() = Some((scene.clone(), Arc::clone(&value)));
        Some(value)
    })
}

/// GPU-first or CPU-only with explicit intent, independent of the spectral adapter.
/// Cancellation never exposes partial sums; every retried range starts at zero.
#[expect(
    clippy::redundant_pub_crate,
    reason = "called from jobs::run and render_cmd outside render_core, so it cannot narrow to pub(super)"
)]
pub(crate) fn trace(
    scene: &SceneState,
    first: u32,
    samples: u32,
    threads: usize,
    prefer_gpu: bool,
    cancel: &AtomicBool,
    out: &mut [Vec3],
) -> bool {
    if cancel.load(Ordering::Acquire) {
        return false;
    }
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    if pixels == 0
        || pixels != out.len() as u64
        || !indicatrix::renderer::engine::AA_TAP_CHOICES.contains(&scene.engine.ray.aa_taps)
    {
        return false;
    }
    let Some(end) = first
        .checked_add(samples)
        .filter(|end| *end <= u32::from(scene.engine.ray.aa_taps))
    else {
        return false;
    };
    if samples == 0 {
        return true;
    }
    let Some(prep) = prepared(scene) else {
        return false;
    };
    let hdr = crate::assets::resolved_hdr_map(scene);
    let frame = frame(
        scene,
        &prep.camera,
        &prep.finishes,
        crate::assets::environment_source(scene, hdr.as_deref()),
    );
    if prefer_gpu {
        match trace_prepared_gpu(&prep, &frame, scene, (first, samples), out, cancel) {
            GpuAccumulate::Done => return true,
            GpuAccumulate::Cancelled => return false,
            GpuAccumulate::Declined => {}
        }
    }
    let Some(count) =
        super::THREAD_PERMITS.acquire(super::effective_thread_count(threads).max(1), cancel)
    else {
        return false;
    };
    let _permits = super::PermitGuard(count);
    let mut scratch = vec![Vec3::ZERO; out.len()];
    let rows = Mutex::new(
        scratch
            .chunks_mut(scene.width as usize)
            .map(Some)
            .collect::<Vec<_>>(),
    );
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..count {
            let (rows, next, frame, prep) = (&rows, &next, &frame, &prep);
            scope.spawn(move || {
                loop {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let y = next.fetch_add(1, Ordering::Relaxed);
                    if y >= scene.height as usize {
                        break;
                    }
                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("a row is claimed once");
                    for (x, sum) in row.iter_mut().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        for tap in first..end {
                            add_finite_sample(
                                sum,
                                whitted_tap_xyz(
                                    frame,
                                    &scene.tools,
                                    &prep.planes,
                                    &prep.prep,
                                    u32::try_from(y * scene.width as usize + x)
                                        .expect("validated frame fits u32"),
                                    tap,
                                    None,
                                ),
                            );
                        }
                    }
                }
            });
        }
    });
    if cancel.load(Ordering::Acquire) {
        return false;
    }
    for (dst, src) in out.iter_mut().zip(scratch) {
        *dst += src;
    }
    true
}

/// Reuse uploads for every tap batch of this scene. A device-generation rejection
/// drops the snapshot; the next call may prepare again after backend recovery.
fn trace_prepared_gpu(
    prepared: &Prepared,
    frame: &FrameScene<'_>,
    scene: &SceneState,
    (first, samples): (u32, u32),
    out: &mut [Vec3],
    cancel: &AtomicBool,
) -> GpuAccumulate {
    // A panic while preparing/executing may leave GPU resources mapped. Retire this
    // scene's GPU cache on poison instead of reusing it through into_inner().
    let Ok(mut uploaded) = prepared.gpu.lock() else {
        return GpuAccumulate::Declined;
    };
    let cancelled = || cancel.load(Ordering::Acquire);
    if uploaded.is_none() {
        match GPU.prepare_cancellable(frame, &scene.tools, &prepared.prep, &cancelled) {
            Ok(snapshot) => *uploaded = Some(snapshot),
            Err(outcome) => return outcome,
        }
    }
    let outcome = GPU.try_accumulate_prepared_cancellable(
        uploaded
            .as_ref()
            .expect("successful GPU preparation was cached"),
        first,
        samples,
        out,
        &cancelled,
    );
    if outcome == GpuAccumulate::Declined {
        *uploaded = None;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::renderer::engine::RenderEngine;

    fn scene() -> SceneState {
        let mut scene = super::super::tests::tiny_scene();
        scene.engine.engine = RenderEngine::RayTraced;
        scene.engine.ray.aa_taps = 4;
        scene
    }

    #[test]
    fn preparation_reuses_equal_scenes_and_replaces_changed_contents() {
        let mut scene = scene();
        let original = prepared(&scene).unwrap();
        assert!(Arc::ptr_eq(&original, &prepared(&scene).unwrap()));
        // Reuse the SceneState allocation but change a frozen input.
        scene.exposure += 0.5;
        let changed = prepared(&scene).unwrap();
        assert!(!Arc::ptr_eq(&original, &changed));
        assert!(Arc::ptr_eq(&changed, &prepared(&scene).unwrap()));
    }

    #[test]
    fn a_poisoned_upload_cache_retries_the_entire_range_on_cpu_without_gpu_access() {
        let scene = scene();
        let cancel = AtomicBool::new(false);
        let mut expected = vec![Vec3::ONE; (scene.width * scene.height) as usize];
        assert!(trace(&scene, 0, 4, 1, false, &cancel, &mut expected));
        let cached = prepared(&scene).unwrap();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _upload = cached.gpu.lock().unwrap();
            panic!("simulate a GPU call unwinding while holding its scene cache");
        }));
        assert!(poisoned.is_err());
        assert!(cached.gpu.is_poisoned());
        let mut actual = vec![Vec3::ONE; expected.len()];
        // The poison check must precede touching the lazy GPU backend.
        assert!(trace(&scene, 0, 4, 2, true, &cancel, &mut actual));
        assert_eq!(actual, expected);
        assert!(cached.gpu.is_poisoned());
    }
}
