//! The phone's small Ray-Traced GPU kernel. No megakernel/material-class gate.
use super::{BackendManager, TraceBackend};
use crate::settings::RenderBackend;
use glam::Vec3;
use indicatrix::{
    geometry::tool::GPU_MAX_TOOL_PRIMITIVES,
    renderer::gpu_backend::{
        GpuAccumulate, GpuWorkLimits,
        whitted::{WhittedGpuBackend, WhittedGpuPrepared},
    },
};
use indicatrix_web_core::{
    render::{ChunkRange, trace_chunk_abortable},
    scene::OwnedScene,
};
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

/// Uploads belong to exactly one immutable scene's `TraceBackend`. Only the adapter
/// probe is shared between epochs/exports; no borrowed scene address identifies a cache.
pub(in crate::render) struct RayGpuSession {
    gpu: Arc<WhittedGpuBackend>,
    upload: Mutex<Upload>,
}

enum Upload {
    Pending,
    Ready(Box<WhittedGpuPrepared>),
    Declined,
}

impl RayGpuSession {
    const fn new(gpu: Arc<WhittedGpuBackend>) -> Self {
        Self {
            gpu,
            upload: Mutex::new(Upload::Pending),
        }
    }

    fn accumulate(
        &self,
        scene: &OwnedScene,
        range: ChunkRange,
        sums: &mut [Vec3],
        stopped: &dyn Fn() -> bool,
    ) -> GpuAccumulate {
        if stopped() {
            return GpuAccumulate::Cancelled;
        }
        // A panic may have interrupted upload/dispatch bookkeeping. Keep the mutex
        // poisoned for this session's lifetime; never recover or reuse its Ready data.
        let Ok(mut upload) = self.upload.lock() else {
            return if stopped() {
                GpuAccumulate::Cancelled
            } else {
                GpuAccumulate::Declined
            };
        };
        if matches!(*upload, Upload::Pending) {
            let Ok(prep) = scene.whitted_prep() else {
                return GpuAccumulate::Declined;
            };
            match self
                .gpu
                .prepare_cancellable(&scene.frame_scene(), scene.tools(), prep, stopped)
            {
                Ok(prepared) => *upload = Upload::Ready(Box::new(prepared)),
                Err(GpuAccumulate::Cancelled) => return GpuAccumulate::Cancelled,
                Err(_) => *upload = Upload::Declined,
            }
        }
        let result = match &*upload {
            Upload::Ready(prepared) => self.gpu.try_accumulate_prepared_cancellable(
                prepared,
                range.sample_offset,
                range.spp,
                sums,
                stopped,
            ),
            Upload::Pending | Upload::Declined => GpuAccumulate::Declined,
        };
        if result == GpuAccumulate::Declined {
            // A lost device's uploads are never reused after recovery. This epoch
            // stays on CPU; the next epoch gets a new session and recovery probe.
            *upload = Upload::Declined;
        }
        result
    }
}

/// Pure admission rules; the prepared scalar-index model supports every material.
fn decline_reason(scene: &OwnedScene) -> Option<String> {
    if let Err(reason) = scene.whitted_prep() {
        return Some(reason.message().to_owned());
    }
    if scene.tools().len() > GPU_MAX_TOOL_PRIMITIVES {
        return Some(format!(
            "Ray-Traced GPU supports at most {GPU_MAX_TOOL_PRIMITIVES} tools; exact CPU tracing is used"
        ));
    }
    if u64::from(scene.width()) * u64::from(scene.height())
        > GpuWorkLimits::WHITTED.max_frame_pixels
    {
        return Some("Ray-Traced frame exceeds the phone GPU memory budget".into());
    }
    None
}

impl BackendManager {
    pub(super) fn prepare_ray(
        &self,
        scene: &OwnedScene,
        epoch: u64,
    ) -> Result<TraceBackend, String> {
        // Prep's refusal is an engine refusal, never an excuse to silently switch
        // physics. Ordinary memory/tool/device limits may use the identical CPU core.
        scene
            .whitted_prep()
            .map_err(|reason| reason.message().to_owned())?;
        let requested = self.requested();
        if requested == RenderBackend::Remote {
            return Err("The Remote backend requires an authenticated remote session; no local work was substituted.".into());
        }
        if requested == RenderBackend::DeviceCpu {
            self.set_report(epoch, "Device CPU · Ray-Traced".into());
            return Ok(TraceBackend::Cpu);
        }
        if let Some(reason) = decline_reason(scene) {
            self.cpu_fallback(epoch, &reason);
            return Ok(TraceBackend::Cpu);
        }
        // This cache is independent of the spectral probe and shared with exports.
        // A phone unable to compile the megakernel may still run this small kernel.
        let (gpu, decline) = self
            .gpu
            .whitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(|| {
                let (gpu, decline) = WhittedGpuBackend::acquire_with_diagnostics();
                gpu.set_work_limits(GpuWorkLimits::WHITTED);
                (Arc::new(gpu), decline)
            })
            .clone();
        if let Some(reason) = decline {
            self.cpu_fallback(epoch, &reason);
            return Ok(TraceBackend::Cpu);
        }
        let _ = gpu.try_recover();
        gpu.adapter_label().map_or_else(
            || {
                self.cpu_fallback(
                    epoch,
                    "Ray-Traced GPU is recovering; exact CPU tracing is used",
                );
                Ok(TraceBackend::Cpu)
            },
            |label| {
                self.set_report(epoch, format!("Device GPU · Ray-Traced · {label}"));
                Ok(TraceBackend::RayGpu(Arc::new(RayGpuSession::new(gpu))))
            },
        )
    }

    pub(super) fn trace_ray(
        &self,
        gpu: &RayGpuSession,
        scene: &OwnedScene,
        range: ChunkRange,
        cancel: &AtomicBool,
        aborted: &dyn Fn() -> bool,
        epoch: u64,
    ) -> Option<Vec<Vec3>> {
        if range.first_pixel != 0 || range.stride != 1 {
            return None;
        }
        let mut sums =
            vec![Vec3::ZERO; (scene.width() as usize).checked_mul(scene.height() as usize)?];
        let stopped = || cancel.load(Ordering::Acquire) || aborted();
        match gpu.accumulate(scene, range, &mut sums, &stopped) {
            GpuAccumulate::Done => Some(sums),
            GpuAccumulate::Cancelled => None,
            GpuAccumulate::Declined => {
                self.cpu_fallback(
                    epoch,
                    "Ray-Traced GPU declined; restarting the full tap range on CPU",
                );
                trace_chunk_abortable(scene, range, &stopped)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        optics::raytracer::LightingPreset,
        renderer::engine::{EngineSpec, RayOptions, RenderEngine},
    };
    use indicatrix_web_core::scene::{
        CameraSpec, FinishSpec, LightingSpec, MaterialSpec, SceneSpec, planes_to_data,
    };

    fn spec(material: &str, width: u32, height: u32) -> SceneSpec {
        SceneSpec {
            width,
            height,
            planes: planes_to_data(
                &indicatrix::geometry::StandardGemCuts::standard_round_brilliant(),
            ),
            finishes: FinishSpec::AllPolished,
            material: MaterialSpec::catalogue(material),
            camera: CameraSpec {
                yaw: 0.6,
                pitch: 0.45,
                distance: 3.0,
            },
            lighting: LightingSpec {
                preset_index: LightingPreset::Daylight.index(),
                exposure: 1.0,
                light_yaw: 0.7,
                light_pitch: 0.8,
                // The phone's own default backdrop, not a hard-coded index.
                backdrop_index: crate::design_props::backdrop_index(
                    crate::settings::LightingSettings::default().backdrop,
                ),
                head_shadow_deg: 16.0,
                surface_glare: 1.0,
            },
            max_bounces: 12,
            hdr_id: None,
        }
    }

    fn scene(material: &str, width: u32, height: u32) -> OwnedScene {
        let mut scene = OwnedScene::build(&spec(material, width, height), None).expect("scene");
        scene
            .set_engine(EngineSpec {
                engine: RenderEngine::RayTraced,
                ray: RayOptions::default(),
            })
            .expect("ray scene");
        scene
    }

    #[test]
    fn every_material_class_uses_the_small_kernel_admission() {
        for material in ["Diamond", "Sapphire", "Peridot"] {
            assert!(
                decline_reason(&scene(material, 64, 64)).is_none(),
                "{material}"
            );
        }
    }

    #[test]
    fn a_large_frame_keeps_its_exact_cpu_fallback() {
        assert!(
            decline_reason(&scene("Diamond", 1025, 1024))
                .expect("budget decline")
                .contains("memory budget")
        );
        assert!(decline_reason(&scene("Diamond", 1024, 1024)).is_none());
    }

    #[test]
    fn a_full_tool_list_routes_and_uv_refuses_the_engine_before_any_gpu_probe() {
        use indicatrix::geometry::tool::ToolPrimitive;
        // The GPU limit is the CPU's own (`GPU_MAX_TOOL_PRIMITIVES == MAX_TOOL_PRIMITIVES`), so
        // no scene that builds can exceed it: the longest valid tool list is not declined for
        // its tools. The over-limit decline itself is covered by the pure routing tests.
        let geometry = indicatrix_web_core::scene::GeometrySceneSpec {
            scene: spec("Diamond", 16, 12),
            tools: vec![
                ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.1);
                GPU_MAX_TOOL_PRIMITIVES
            ],
        };
        let mut built = geometry.build(None).expect("CPU supports the tool list");
        built
            .set_engine(EngineSpec {
                engine: RenderEngine::RayTraced,
                ray: RayOptions::default(),
            })
            .expect("supported CPU engine");
        assert!(
            decline_reason(&built).is_none_or(|reason| !reason.contains("tools")),
            "{:?}",
            decline_reason(&built)
        );
        let mut uv = spec("Diamond", 16, 12);
        uv.lighting.preset_index = LightingPreset::UvLamp365.index();
        let mut built = OwnedScene::build(&uv, None).expect("UV scene");
        assert!(
            built
                .set_engine(EngineSpec {
                    engine: RenderEngine::RayTraced,
                    ray: RayOptions::default()
                })
                .is_err()
        );
        let manager = BackendManager::new();
        assert!(manager.prepare_ray(&built, 1).is_err());
        assert!(manager.gpu.whitted.lock().expect("probe").is_none());
    }

    #[test]
    fn live_and_export_share_both_probes_and_cpu_never_acquires_them() {
        let live = BackendManager::new();
        let export = BackendManager::sharing(&live.probe());
        assert!(Arc::ptr_eq(&live.gpu.whitted, &export.gpu.whitted));
        assert!(Arc::ptr_eq(&live.gpu.spectral, &export.gpu.spectral));
        live.select(RenderBackend::DeviceCpu);
        live.begin(1);
        assert!(matches!(
            live.prepare_ray(&scene("Diamond", 16, 12), 1).expect("CPU"),
            TraceBackend::Cpu
        ));
        assert!(live.gpu.whitted.lock().expect("probe").is_none());
        assert!(live.gpu.spectral.lock().expect("probe").is_none());
        assert!(live.report().contains("Ray-Traced"));
    }

    #[test]
    fn a_poisoned_upload_never_recovers_and_already_cancelled_work_stays_cancelled() {
        let scene = scene("Diamond", 16, 12);
        let session = RayGpuSession::new(Arc::new(WhittedGpuBackend::disabled()));
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _upload = session.upload.lock().unwrap();
                panic!("simulate an interrupted upload");
            }))
            .is_err()
        );
        let range = ChunkRange {
            first_pixel: 0,
            stride: 1,
            sample_offset: 0,
            spp: 1,
        };
        let original = vec![Vec3::ONE; 16 * 12];
        let mut sums = original.clone();
        assert_eq!(
            session.accumulate(&scene, range, &mut sums, &|| true),
            GpuAccumulate::Cancelled
        );
        for _ in 0..2 {
            assert_eq!(
                session.accumulate(&scene, range, &mut sums, &|| false),
                GpuAccumulate::Declined
            );
            assert!(session.upload.is_poisoned());
            assert_eq!(sums, original);
        }
    }

    #[test]
    fn cancelled_upload_can_retry_but_declined_upload_stays_local_to_its_epoch() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RayGpuSession>();
        let scene = scene("Diamond", 16, 12);
        let manager = BackendManager::new();
        manager.begin(1);
        let gpu = Arc::new(WhittedGpuBackend::disabled());
        let epoch = RayGpuSession::new(Arc::clone(&gpu));
        let next_epoch = RayGpuSession::new(gpu);
        let range = ChunkRange {
            first_pixel: 0,
            stride: 1,
            sample_offset: 1,
            spp: 3,
        };
        assert!(
            manager
                .trace_ray(&epoch, &scene, range, &AtomicBool::new(true), &|| false, 1)
                .is_none()
        );
        assert!(matches!(*epoch.upload.lock().unwrap(), Upload::Pending));
        let expected = trace_chunk_abortable(&scene, range, &|| false).unwrap();
        assert_eq!(
            manager
                .trace_ray(&epoch, &scene, range, &AtomicBool::new(false), &|| false, 1)
                .unwrap(),
            expected
        );
        assert!(matches!(*epoch.upload.lock().unwrap(), Upload::Declined));
        assert!(matches!(
            *next_epoch.upload.lock().unwrap(),
            Upload::Pending
        ));
        let next = ChunkRange {
            sample_offset: 4,
            spp: 4,
            ..range
        };
        assert_eq!(
            manager
                .trace_ray(&epoch, &scene, next, &AtomicBool::new(false), &|| false, 1)
                .unwrap(),
            trace_chunk_abortable(&scene, next, &|| false).unwrap()
        );
    }

    #[test]
    fn a_gpu_decline_retraces_the_entire_absolute_tap_range_on_cpu() {
        let scene = scene("Diamond", 16, 12);
        let manager = BackendManager::new();
        manager.begin(1);
        let range = ChunkRange {
            first_pixel: 0,
            stride: 1,
            sample_offset: 1,
            spp: 3,
        };
        let expected = trace_chunk_abortable(&scene, range, &|| false).expect("CPU reference");
        let actual = manager
            .trace_ray(
                &RayGpuSession::new(Arc::new(WhittedGpuBackend::disabled())),
                &scene,
                range,
                &AtomicBool::new(false),
                &|| false,
                1,
            )
            .expect("CPU fallback");
        assert_eq!(actual, expected);
        assert!(manager.report().contains("full tap range"));
        assert!(
            manager
                .trace_ray(
                    &RayGpuSession::new(Arc::new(WhittedGpuBackend::disabled())),
                    &scene,
                    range,
                    &AtomicBool::new(true),
                    &|| true,
                    1
                )
                .is_none()
        );
    }
}
