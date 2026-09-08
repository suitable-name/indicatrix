//! The GPU backend wrapper and frame dispatch: `ViewportGpu` (the shared `GpuBackend`
//! plus the guide buffers it cannot produce itself), the hybrid CPU+GPU pacing state,
//! and the per-frame dispatch that picks between them.

use super::scanline::render_frame_scanlines;
use crate::settings::model::LocalComputeTarget;
use glam::Vec3;
use indicatrix::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
    renderer::gpu_backend::{GpuBackend, GpuSceneRef},
};

/// One frame's scene, as both backends need it. Bundled so [`ViewportGpu::try_accumulate`]
/// and the CPU path take the same description rather than a dozen loose parameters.
#[derive(Clone, Copy)]
pub(super) struct BackendFrame<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    /// Pose, carried separately from `camera` because the guide-buffer cache keys on
    /// it (see `bridge::guide_pass`) and cannot recover it from a built `Camera`.
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) camera: &'a Camera,
    pub(super) planes: &'a [GpuFacetPlane],
    /// Frosted girdle: `&[]` when `RenderContext::girdle_frosted` is off, reproducing
    /// the pre-existing all-polished behaviour on both backends.
    pub(super) facet_finishes: &'a [FacetFinish],
    pub(super) material: &'a GemMaterial,
    pub(super) max_bounces: u32,
    pub(super) environment: EnvironmentSource<'a>,
    pub(super) spp: u32,
    /// Samples already in the accumulation buffer. Seeds each sample's jitter/RNG on
    /// both backends, so this must keep advancing across frames or samples repeat.
    pub(super) sample_offset: u32,
}

/// The four per-pixel buffers a frame writes: the radiance running sum, plus the three
/// first-hit guide buffers the A-Trous denoiser keys on.
pub(super) struct FrameOutputs<'a> {
    pub(super) accum: &'a mut [Vec3],
    pub(super) depth: &'a mut [f32],
    pub(super) normal: &'a mut [Vec3],
    pub(super) facet_id: &'a mut [i32],
}

/// The viewport's GPU backend: the shared [`GpuBackend`] plus the guide buffers it
/// cannot produce.
///
/// The megakernel returns radiance only, with no first-hit depth/normal/facet-id, so
/// the A-Trous denoiser has nothing to key on -- the same gap a remote worker's `FRAME`
/// payload has, solved the same way: `bridge::guide_pass`'s local primary-ray prepass,
/// cached on pose plus geometry. When the `gpu` feature is off, `GpuBackend` always
/// declines and the guide cache is never consulted.
pub(super) struct ViewportGpu {
    gpu: GpuBackend,
    guides: crate::bridge::frame_cache::guide_pass::GuideCache,
    /// Key of the guide buffers currently copied into the render loop's buffers, so an
    /// unchanged pose copies nothing rather than memcpying ~10 MB every frame.
    applied_guide_key: Option<crate::bridge::frame_cache::guide_pass::GuideKey>,
}

impl ViewportGpu {
    pub(super) fn acquire() -> Self {
        Self {
            gpu: GpuBackend::acquire(),
            guides: crate::bridge::frame_cache::guide_pass::GuideCache::new(),
            applied_guide_key: None,
        }
    }

    /// Accumulates one frame's samples on the GPU and refreshes the guide buffers.
    ///
    /// Returns `false` without touching `out` if the GPU declines, in which case the
    /// caller must run the CPU path for this frame.
    fn try_accumulate(&mut self, frame: &BackendFrame<'_>, out: &mut FrameOutputs<'_>) -> bool {
        let scene = GpuSceneRef {
            camera: frame.camera,
            width: frame.width,
            height: frame.height,
            planes: frame.planes,
            facet_finishes: frame.facet_finishes,
            material: frame.material,
            max_bounces: frame.max_bounces,
            environment: frame.environment,
        };
        if !self
            .gpu
            .try_accumulate(&scene, frame.sample_offset, frame.spp, out.accum)
        {
            return false;
        }

        let key = crate::bridge::frame_cache::guide_pass::GuideCache::key_for(
            frame.width,
            frame.height,
            frame.yaw,
            frame.pitch,
            frame.distance,
            frame.planes,
        );
        if self.applied_guide_key.as_ref() != Some(&key) {
            let guides = self.guides.ensure(
                frame.width,
                frame.height,
                frame.yaw,
                frame.pitch,
                frame.distance,
                frame.planes,
            );
            out.depth.copy_from_slice(&guides.depth);
            out.normal.copy_from_slice(&guides.normal);
            out.facet_id.copy_from_slice(&guides.facet_id);
            self.applied_guide_key = Some(key);
        }

        true
    }
}

/// Accumulates one frame's `spp` samples into `outputs`, per `local_compute_target`:
///
/// - [`LocalComputeTarget::Cpu`] never touches `backend` -- straight to the CPU
///   scanline tracer, every frame.
/// - [`LocalComputeTarget::CpuGpu`] is this function's original behaviour: once both
///   engines' throughputs are known, hybrid pacing splits each frame's samples between
///   them and traces CONCURRENTLY over disjoint sample ranges, falling back to the
///   single-engine path while hybrid pacing is still measuring.
/// - [`LocalComputeTarget::Gpu`] skips the hybrid split -- the GPU is always offered
///   the frame's FULL `spp`, still falling through to the CPU tracer on a per-frame
///   decline exactly like `CpuGpu`'s single-engine path.
///
/// The CPU fallback is not exceptional: it runs whenever the `gpu` feature is off, no
/// adapter exists, or the device is otherwise unavailable -- an HDR-mapped environment
/// is no longer a special case, since the GPU megakernel renders HDR maps directly (see
/// `docs/history/indicatrix-cut.md` for the CPU-only HDR path this replaced). Both
/// backends add into the same buffer with the same sample-counter
/// meaning, so switching between them mid-render (including a live
/// `local_compute_target` change) continues a correct running average.
pub(super) fn accumulate_frame_samples(
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    local_compute_target: LocalComputeTarget,
) {
    // `Cpu`: never dispatch to the GPU backend -- straight to the CPU tracer.
    if local_compute_target == LocalComputeTarget::Cpu {
        let start = std::time::Instant::now();
        render_frame_scanlines(frame, frame.spp, frame.sample_offset + frame.spp, outputs);
        hybrid.observe_cpu_only(frame.spp, start.elapsed());
        return;
    }

    // Hybrid CPU+GPU: once both engines' throughputs are known, each frame's samples
    // split between them and trace CONCURRENTLY over disjoint sample ranges, so the
    // running average stays correct regardless of the split. Only for `CpuGpu` -- `Gpu`
    // always offers the backend the FULL spp instead.
    if local_compute_target == LocalComputeTarget::CpuGpu
        && let Some(gpu_share) = hybrid.gpu_share(frame.spp)
    {
        let cpu_share = frame.spp - gpu_share;
        if gpu_share > 0 && cpu_share > 0 {
            hybrid_frame(backend, frame, outputs, hybrid, gpu_share);
            return;
        }
    }

    // Single-engine path: GPU-first with per-frame CPU fallback -- runs while hybrid
    // pacing is still measuring (`CpuGpu`), and always for `Gpu`.
    let start = std::time::Instant::now();
    if backend.try_accumulate(frame, outputs) {
        hybrid.observe_gpu_only(frame.spp, start.elapsed());
        return;
    }
    let start = std::time::Instant::now();
    render_frame_scanlines(frame, frame.spp, frame.sample_offset + frame.spp, outputs);
    hybrid.observe_cpu_only(frame.spp, start.elapsed());
}

/// One hybrid viewport frame: the GPU traces the lower `gpu_share` sample range on a
/// scoped thread (real guide buffers), while the CPU traces the upper range into
/// `hybrid`'s scratch with throwaway guide buffers (both backends derive identical
/// guide data, so discarding the CPU copy loses nothing). Summed into the real
/// accumulation only after both engines join, so they never write one buffer
/// concurrently. If the GPU declines mid-session, its share is retraced on the CPU and
/// hybrid stops offering it work.
fn hybrid_frame(
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    gpu_share: u32,
) {
    let cpu_share = frame.spp - gpu_share;
    let pixel_count = (frame.width as usize) * (frame.height as usize);
    hybrid.reset_scratch(pixel_count);
    let (gpu_ok, gpu_time, cpu_time) = {
        let cpu_scratch = &mut hybrid.cpu_scratch;
        let cpu_depth = &mut hybrid.scratch_depth;
        let cpu_normal = &mut hybrid.scratch_normal;
        let cpu_facet = &mut hybrid.scratch_facet;
        std::thread::scope(|scope| {
            let gpu_task = scope.spawn(|| {
                let start = std::time::Instant::now();
                let gpu_frame = BackendFrame {
                    spp: gpu_share,
                    ..*frame
                };
                let ok = backend.try_accumulate(&gpu_frame, outputs);
                (ok, start.elapsed())
            });
            let start = std::time::Instant::now();
            render_frame_scanlines(
                frame,
                cpu_share,
                frame.sample_offset + frame.spp,
                &mut FrameOutputs {
                    accum: cpu_scratch,
                    depth: cpu_depth,
                    normal: cpu_normal,
                    facet_id: cpu_facet,
                },
            );
            let cpu_time = start.elapsed();
            let (gpu_ok, gpu_time) = gpu_task.join().unwrap_or((false, cpu_time));
            (gpu_ok, gpu_time, cpu_time)
        })
    };
    for (px, extra) in outputs.accum.iter_mut().zip(&hybrid.cpu_scratch) {
        *px += *extra;
    }
    if gpu_ok {
        hybrid.observe(gpu_share, gpu_time, cpu_share, cpu_time);
        return;
    }
    hybrid.gpu_dead = true;
    render_frame_scanlines(frame, gpu_share, frame.sample_offset + gpu_share, outputs);
}

/// Adaptive pacing state for the viewport's hybrid CPU+GPU accumulation:
/// exponentially-smoothed samples-per-second per engine, measured from actual work
/// done, deciding how the NEXT frame's samples split.
///
/// The live preview is a progressive stochastic estimate that resets on every
/// interaction, so (unlike the deterministic library-level `HybridSplit`) an adaptive
/// timing-based split is appropriate: every sample is a valid estimate over its own
/// disjoint range regardless of the split.
pub(super) struct HybridPacing {
    /// Smoothed samples-per-second per engine; `None` until first measured.
    gpu_rate: Option<f64>,
    cpu_rate: Option<f64>,
    /// True while `cpu_rate` holds only the pessimistic seed from
    /// [`Self::observe_gpu_only`] rather than a real measurement -- the split then
    /// always leaves the CPU at least one sample so its true rate gets measured.
    cpu_rate_seeded: bool,
    /// The GPU declined mid-session; hybrid stops offering it work.
    gpu_dead: bool,
    /// Radiance scratch for the concurrent CPU share, plus throwaway guide buffers
    /// (see [`hybrid_frame`]); owned here so steady state does no per-frame allocation.
    cpu_scratch: Vec<Vec3>,
    scratch_depth: Vec<f32>,
    scratch_normal: Vec<Vec3>,
    scratch_facet: Vec<i32>,
}

impl HybridPacing {
    pub(super) const fn new() -> Self {
        Self {
            gpu_rate: None,
            cpu_rate: None,
            cpu_rate_seeded: false,
            gpu_dead: false,
            cpu_scratch: Vec::new(),
            scratch_depth: Vec::new(),
            scratch_normal: Vec::new(),
            scratch_facet: Vec::new(),
        }
    }

    /// The GPU's sample share for an `spp`-sample frame, once both engines have rates;
    /// `None` keeps the single-engine path (also the initial per-engine measurement).
    fn gpu_share(&self, spp: u32) -> Option<u32> {
        if self.gpu_dead || spp < 2 {
            return None;
        }
        let (gpu, cpu) = (self.gpu_rate?, self.cpu_rate?);
        let frac = gpu / (gpu + cpu);
        let share = (f64::from(spp) * frac).round() as u32;
        // While the CPU rate is only a seed, force it a real slice to measure.
        let cap = if self.cpu_rate_seeded { spp - 1 } else { spp };
        Some(share.min(cap))
    }

    fn reset_scratch(&mut self, pixel_count: usize) {
        self.cpu_scratch.clear();
        self.cpu_scratch.resize(pixel_count, Vec3::ZERO);
        self.scratch_depth.clear();
        self.scratch_depth.resize(pixel_count, 0.0);
        self.scratch_normal.clear();
        self.scratch_normal.resize(pixel_count, Vec3::ZERO);
        self.scratch_facet.clear();
        self.scratch_facet.resize(pixel_count, -1);
    }

    fn observe(
        &mut self,
        gpu_spp: u32,
        gpu_time: std::time::Duration,
        cpu_spp: u32,
        cpu_time: std::time::Duration,
    ) {
        Self::blend(&mut self.gpu_rate, gpu_spp, gpu_time);
        Self::blend(&mut self.cpu_rate, cpu_spp, cpu_time);
        self.cpu_rate_seeded = false;
    }

    fn observe_gpu_only(&mut self, spp: u32, elapsed: std::time::Duration) {
        Self::blend(&mut self.gpu_rate, spp, elapsed);
        // Seed a CPU estimate so hybrid engages at all; `cpu_rate_seeded` guarantees
        // the next hybrid frame measures the real rate.
        if self.cpu_rate.is_none()
            && let Some(gpu) = self.gpu_rate
        {
            self.cpu_rate = Some(gpu / 16.0);
            self.cpu_rate_seeded = true;
        }
    }

    fn observe_cpu_only(&mut self, spp: u32, elapsed: std::time::Duration) {
        Self::blend(&mut self.cpu_rate, spp, elapsed);
        self.cpu_rate_seeded = false;
    }

    /// Exponential smoothing damps frame-to-frame timing noise while tracking real
    /// shifts (thermal throttling, other machine load).
    fn blend(slot: &mut Option<f64>, spp: u32, elapsed: std::time::Duration) {
        if spp == 0 {
            return;
        }
        let rate = f64::from(spp) / elapsed.as_secs_f64().max(1e-9);
        *slot = Some(slot.map_or(rate, |prev| prev.mul_add(0.7, rate * 0.3)));
    }
}
