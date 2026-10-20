//! Execution routing above the exact shared scene. Automatic is strictly local.
use super::RenderState;
#[path = "backend_whitted.rs"]
mod whitted;
use crate::settings::RenderBackend;
use glam::Vec3;
use indicatrix::{
    geometry::tool::StoneGeometry,
    optics::{fluorescence::Fluorescence, materials::GemMaterial, raytracer::LightingPreset},
    renderer::gpu_backend::{
        GpuAccumulate, GpuBackend, GpuRouteDecline, GpuSceneRef, GpuWorkLimits,
        scene_decline_with_tools, whitted::WhittedGpuBackend,
    },
};
use indicatrix_web_core::{
    render::{ChunkRange, trace_chunk_abortable},
    scene::OwnedScene,
};
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

#[derive(Clone)]
pub(super) enum TraceBackend {
    Cpu,
    Gpu(Arc<GpuBackend>),
    RayGpu(Arc<whitted::RayGpuSession>),
}

impl TraceBackend {
    pub(super) const fn is_gpu(&self) -> bool {
        matches!(self, Self::Gpu(_) | Self::RayGpu(_))
    }
}

/// The device GPU probe: the backend with its decline reason (or its absence). Acquiring an
/// adapter and compiling the kernels is slow, so one probe is shared by the live render and
/// every picture export of a [`RenderState`](super::RenderState); a failed probe is
/// remembered too.
#[derive(Clone, Default)]
pub(super) struct GpuProbe {
    spectral: ProbeSlot<GpuBackend>,
    whitted: ProbeSlot<WhittedGpuBackend>,
}

/// One shared probe result: the backend with its decline reason, once probed.
type ProbeSlot<B> = Arc<Mutex<Option<(Arc<B>, Option<String>)>>>;

pub(super) struct BackendManager {
    requested: AtomicU8,
    epoch: AtomicU64,
    gpu: GpuProbe,
    report: Mutex<String>,
}

impl BackendManager {
    pub(super) fn new() -> Self {
        Self::sharing(&GpuProbe::default())
    }

    /// A manager with its own request, epoch and report that reuses `probe`, so it never
    /// acquires the GPU a second time.
    pub(super) fn sharing(probe: &GpuProbe) -> Self {
        Self {
            requested: AtomicU8::new(0),
            epoch: AtomicU64::new(0),
            gpu: probe.clone(),
            report: Mutex::new("Preparing local renderer".into()),
        }
    }

    /// The probe this manager fills, to hand to a [`Self::sharing`] manager.
    pub(super) fn probe(&self) -> GpuProbe {
        self.gpu.clone()
    }

    pub(super) fn select(&self, requested: RenderBackend) {
        self.requested.store(
            u8::try_from(requested.index()).unwrap_or(0),
            Ordering::Release,
        );
    }

    pub(super) fn requested(&self) -> RenderBackend {
        RenderBackend::from_index(i32::from(self.requested.load(Ordering::Acquire)))
            .unwrap_or_default()
    }

    pub(super) fn report(&self) -> String {
        self.report
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn begin(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Release);
        self.set_report(epoch, "Preparing renderer".into());
    }

    fn set_report(&self, epoch: u64, report: String) {
        let mut current = self.report.lock().unwrap_or_else(PoisonError::into_inner);
        if self.epoch.load(Ordering::Acquire) == epoch {
            *current = report;
        }
    }

    pub(super) fn remote_report(&self, epoch: u64, label: String) {
        self.set_report(epoch, label);
    }

    pub(super) fn cpu_fallback(&self, epoch: u64, reason: &str) {
        self.set_report(epoch, format!("Device CPU · {reason}"));
    }

    /// Called only by worker-owned scene preparation, never under the pool control lock.
    pub(super) fn prepare(&self, scene: &OwnedScene, epoch: u64) -> Result<TraceBackend, String> {
        let requested = self.requested();
        if scene.engine().is_ray() {
            return self.prepare_ray(scene, epoch);
        }
        if requested == RenderBackend::Remote {
            return Err("Configure and authenticate a remote endpoint before remote rendering is available.".into());
        }
        if requested == RenderBackend::DeviceCpu {
            self.set_report(epoch, "Device CPU".into());
            return Ok(TraceBackend::Cpu);
        }
        let frame = scene.frame_scene();
        let lighting = frame
            .environment
            .lighting_preset()
            .unwrap_or(indicatrix::optics::raytracer::LightingPreset::Daylight);
        let geometry = StoneGeometry {
            planes: scene.planes(),
            tools: scene.tools(),
        };
        if let Some(reason) =
            cpu_only_reason(scene.material(), geometry, scene.fluorescence(), lighting)
        {
            self.cpu_fallback(epoch, &reason);
            return Ok(TraceBackend::Cpu);
        }
        let input = gpu_scene(scene);
        if let Some(reason) = GpuWorkLimits::MOBILE.decline_reason(&input, 1) {
            self.cpu_fallback(epoch, reason);
            return Ok(TraceBackend::Cpu);
        }
        // One cached probe per process, including failed probes. Device-loss recovery
        // uses the upstream bounded cooldown rather than rebuilding every UI tick.
        // The lock covers only the (slow, once-per-process) probe itself; the answer is
        // cloned out so that routing, recovery and reporting never hold it.
        let (backend, decline) = self
            .gpu
            .spectral
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(|| {
                let (backend, decline) = GpuBackend::acquire_with_diagnostics();
                backend.set_work_limits(GpuWorkLimits::MOBILE);
                (Arc::new(backend), decline)
            })
            .clone();
        if let Some(reason) = &decline {
            self.cpu_fallback(epoch, reason);
            return Ok(TraceBackend::Cpu);
        }
        // A restarted epoch must still give a lost device its bounded recovery
        // opportunity; otherwise CPU routing would prevent any future GPU request.
        let _recovered = backend.try_recover();
        let Some(label) = backend.adapter_label() else {
            self.cpu_fallback(
                epoch,
                &backend
                    .last_lost_reason()
                    .unwrap_or_else(|| "no usable device GPU adapter".into()),
            );
            return Ok(TraceBackend::Cpu);
        };
        self.set_report(epoch, format!("Device GPU · {label}"));
        Ok(TraceBackend::Gpu(backend))
    }

    /// Whole-frame GPU work uses one planner partition. Every one-sample dispatch
    /// is accumulated into a private transaction; decline/cancel discards that
    /// transaction before CPU fallback or retry, so samples are never counted twice.
    pub(super) fn trace(
        &self,
        backend: &TraceBackend,
        scene: &OwnedScene,
        range: ChunkRange,
        cancel: &AtomicBool,
        aborted: &dyn Fn() -> bool,
        epoch: u64,
    ) -> Option<Vec<Vec3>> {
        if let TraceBackend::RayGpu(gpu) = backend {
            return self.trace_ray(gpu, scene, range, cancel, aborted, epoch);
        }
        let TraceBackend::Gpu(gpu) = backend else {
            return trace_chunk_abortable(scene, range, aborted);
        };
        if range.first_pixel != 0 || range.stride != 1 {
            return None;
        }
        let mut sum =
            vec![Vec3::ZERO; (scene.width() as usize).checked_mul(scene.height() as usize)?];
        let input = gpu_scene(scene);
        for sample in range.sample_offset..range.sample_offset.checked_add(range.spp)? {
            if aborted() {
                return None;
            }
            // The stone's concave tools travel with the call; the plane-only entry point
            // would draw the stone flat.
            match gpu.try_accumulate_geom_cancellable(
                &input,
                scene.tools(),
                sample,
                1,
                &mut sum,
                cancel,
            ) {
                GpuAccumulate::Done => {}
                GpuAccumulate::Cancelled => return None,
                GpuAccumulate::Declined => {
                    self.cpu_fallback(
                        epoch,
                        &gpu.last_lost_reason().unwrap_or_else(|| {
                            "GPU dispatch declined; restarting this sample range on CPU".into()
                        }),
                    );
                    return trace_chunk_abortable(scene, range, aborted);
                }
            }
            // Small duty gap between bounded submissions leaves the phone compositor
            // and other GPU clients room. This never reduces requested samples.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Some(sum)
    }
}

/// Why the stone must be traced on the CPU, `None` when the device GPU may take it.
///
/// Concave stones route to the GPU up to its tool limit (`GPU_MAX_TOOL_PRIMITIVES`); above
/// it, with fluorescence or a UV lamp, or with a material the kernels cannot render, the
/// exact CPU tracer takes over and the report says why. A pure function of the scene.
fn cpu_only_reason(
    material: &GemMaterial,
    geometry: StoneGeometry<'_>,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
) -> Option<String> {
    scene_decline_with_tools(material, geometry, fluorescence, lighting).map(
        |decline| match decline {
            GpuRouteDecline::Material => {
                "this material or zoning configuration requires CPU tracing".to_owned()
            }
            GpuRouteDecline::TooManyTools { .. } => {
                format!("{decline}; exact CPU tracing is used")
            }
            GpuRouteDecline::Fluorescence | GpuRouteDecline::UvLamp => {
                "fluorescence/UV requires CPU tracing".to_owned()
            }
        },
    )
}

fn gpu_scene(scene: &OwnedScene) -> GpuSceneRef<'_> {
    let frame = scene.frame_scene();
    GpuSceneRef {
        camera: frame.camera,
        width: frame.width,
        height: frame.height,
        planes: frame.planes,
        facet_finishes: frame.facet_finishes,
        material: frame.material,
        max_bounces: frame.max_bounces,
        environment: frame.environment,
    }
}

impl RenderState {
    /// Set before starting a new epoch. A backend switch restarts accumulation.
    pub fn select_backend(&self, requested: RenderBackend) {
        if requested == RenderBackend::Remote && self.shared.backend.requested() != requested {
            // Remote has cancel/retry semantics, not a resumable local pause.
            self.shared
                .update_control(|control| control.user_paused = false);
        }
        self.shared.backend.select(requested);
    }
    #[must_use]
    pub fn backend_report(&self) -> String {
        self.shared.backend.report()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::{
        GpuFacetPlane, cuts::StandardGemCuts, tool::GPU_MAX_TOOL_PRIMITIVES, tool::ToolPrimitive,
    };

    fn planes() -> Vec<GpuFacetPlane> {
        StandardGemCuts::standard_round_brilliant()
    }

    fn tools(count: usize) -> Vec<ToolPrimitive> {
        (0..count)
            .map(|i| ToolPrimitive::ball(glam::Vec3::new(0.01 * i as f32, 0.3, 0.0), 0.05))
            .collect()
    }

    fn reason(tools: &[ToolPrimitive], lighting: LightingPreset) -> Option<String> {
        let planes = planes();
        cpu_only_reason(
            &GemMaterial::diamond(),
            StoneGeometry {
                planes: &planes,
                tools,
            },
            Fluorescence::none(),
            lighting,
        )
    }

    #[test]
    fn a_concave_stone_within_the_tool_limit_is_offered_to_the_gpu() {
        assert_eq!(reason(&[], LightingPreset::Daylight), None);
        assert_eq!(reason(&tools(1), LightingPreset::Daylight), None);
        assert_eq!(
            reason(&tools(GPU_MAX_TOOL_PRIMITIVES), LightingPreset::Daylight),
            None
        );
    }

    #[test]
    fn a_stone_beyond_the_limit_falls_back_to_the_cpu_with_its_reason() {
        let over = reason(
            &tools(GPU_MAX_TOOL_PRIMITIVES + 1),
            LightingPreset::Daylight,
        )
        .expect("declined");
        assert!(
            over.contains(&(GPU_MAX_TOOL_PRIMITIVES + 1).to_string()),
            "{over}"
        );
        assert!(
            over.contains(&GPU_MAX_TOOL_PRIMITIVES.to_string()),
            "{over}"
        );
        assert!(over.contains("CPU"), "{over}");
        assert!(!over.contains("concave tools require exact CPU tracing"));
    }

    #[test]
    fn uv_lamps_still_stay_on_the_cpu_with_or_without_tools() {
        for count in [0, 2] {
            let text = reason(&tools(count), LightingPreset::UvLamp365).expect("declined");
            assert!(text.contains("UV"), "{text}");
        }
    }
}
