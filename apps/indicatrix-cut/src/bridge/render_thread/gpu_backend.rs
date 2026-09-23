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
    /// Frosted girdle: `&[]` when `RenderContext::girdle_frosted` is off, which both
    /// backends treat as all-polished.
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
    /// Set once a joined GPU-thread panic is observed (see [`hybrid_frame`])
    /// and never cleared -- `GpuBackend` itself only retires its OWN `lost` flag on a
    /// cleanly-reported [`indicatrix::renderer::gpu::GpuFrameError::DeviceLost`], which
    /// a raw thread panic never reaches (the panic unwinds past the normal return path
    /// entirely). Without this, a panic observed only through `hybrid_frame`'s joined
    /// thread left the single-engine path at [`accumulate_frame_samples`] free to call
    /// [`Self::try_accumulate`] again next frame, straight back into the wgpu state
    /// that produced the panic.
    gpu_retired: bool,
    /// True once a post-loss re-acquire attempt (see
    /// [`Self::try_accumulate`]'s own doc comment) has ALSO failed -- permanently
    /// falls back to the CPU tracer for the rest of this session, mirroring
    /// [`Self::gpu_retired`]'s "never cleared" policy for a different trigger
    /// (`GpuBackend::is_lost` rather than a joined thread panic).
    cpu_fallback: bool,
    /// User-facing status text for the viewport's own status pill
    /// (`ui/components/gem_viewport.slint`'s `ViewportModel.gpu_status_text`) --
    /// `None` while healthy or on a CPU-only build (nothing to report), `Some` while
    /// transiently re-acquiring or once [`Self::cpu_fallback`] is permanent. The
    /// render loop polls [`Self::status_message`] once per frame and only pushes a
    /// Slint property write when it actually changed -- see `spawn_render_thread`'s
    /// own `last_gpu_status` local.
    status_message: Option<String>,
}

impl ViewportGpu {
    pub(super) fn acquire() -> Self {
        Self {
            gpu: GpuBackend::acquire(),
            guides: crate::bridge::frame_cache::guide_pass::GuideCache::new(),
            applied_guide_key: None,
            gpu_retired: false,
            cpu_fallback: false,
            status_message: None,
        }
    }

    /// Permanently stops this session's GPU frames after a joined GPU-thread panic --
    /// mirrors the policy `GpuBackend::try_accumulate_cancellable`
    /// already applies to its own `lost` flag (`Declined` forever, never retried), just
    /// tracked here too since a raw thread panic never reaches that code path.
    fn retire(&mut self) {
        self.gpu_retired = true;
        self.status_message = Some("CPU fallback (GPU render thread panicked)".to_string());
    }

    /// The viewport's current GPU status text, if there is anything to show -- see
    /// [`Self::status_message`]'s own doc comment.
    pub(super) fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    /// Invalidates the cached guide-buffer key so the NEXT GPU frame recopies its guide
    /// buffers unconditionally, even if the pose/geometry key it would compute matches
    /// the one last applied.
    ///
    /// Must be called whenever something OTHER than [`Self::try_accumulate`] has just
    /// written `depth`/`normal`/`facet_id` -- a CPU scanline frame
    /// (`render_frame_scanlines`, taking any of [`accumulate_frame_samples`]'s non-GPU
    /// branches) or a resize/dirty reallocation
    /// (`frame_helpers::update_accumulation_state`, called from the render loop) -- since
    /// either leaves guide data in the buffers that does NOT match
    /// `self.applied_guide_key`'s cached pose/geometry association, which
    /// [`Self::try_accumulate`]'s `self.applied_guide_key.as_ref() != Some(&key)` check
    /// would otherwise treat as already up to date and skip recopying, serving a stale
    /// depth/normal/facet-id triple to the denoiser on the next GPU frame.
    pub(super) const fn invalidate_guide_cache(&mut self) {
        self.applied_guide_key = None;
    }

    /// Accumulates one frame's samples on the GPU and refreshes the guide buffers.
    ///
    /// Returns `false` without touching `out` if the GPU declines OR this backend was
    /// already [`Self::retire`]d/has permanently fallen back to the CPU tracer, in
    /// which case the caller must run the CPU path for this frame.
    ///
    /// # Self-healing a lost device
    ///
    /// `GpuBackend::try_accumulate` (crate-level) correctly stops dispatching into a
    /// lost device, but nothing above it on its own tells the user or tries to recover --
    /// left unhandled, the viewport would just stop updating forever, with no crash
    /// (this app installs no `tracing` subscriber, so even the crate's own
    /// `tracing::warn!` goes nowhere). So a decline is checked against
    /// [`GpuBackend::is_lost`]: an ORDINARY decline
    /// (unsupported material/environment, or no adapter at all -- neither sets `lost`)
    /// still just falls back to the CPU for this one frame, no status change, exactly
    /// as before. A decline caused by a genuine loss drops the poisoned backend and
    /// re-acquires a fresh [`GpuBackend`] ONCE, retrying THIS SAME frame against it
    /// (see [`Self::reacquire_after_loss`]) -- if that also fails, this session gives
    /// up on the GPU for good ([`Self::cpu_fallback`]). Either way
    /// [`Self::status_message`] carries a user-visible reason for the viewport's own
    /// status pill, and clears back to `None` the moment a GPU frame succeeds again.
    fn try_accumulate(&mut self, frame: &BackendFrame<'_>, out: &mut FrameOutputs<'_>) -> bool {
        if self.gpu_retired || self.cpu_fallback {
            return false;
        }
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
        if self
            .gpu
            .try_accumulate(&scene, frame.sample_offset, frame.spp, out.accum)
        {
            self.status_message = None;
            self.copy_guides(frame, out);
            return true;
        }
        if !self.gpu.is_lost() {
            // An ordinary per-call decline -- not a loss, nothing to heal.
            return false;
        }
        self.reacquire_after_loss(&scene, frame, out)
    }

    /// The self-healing path [`Self::try_accumulate`] takes once [`GpuBackend::is_lost`]
    /// confirms this frame's decline was a genuine device loss rather than an ordinary
    /// one. Split out purely to keep `try_accumulate` under clippy's function-length
    /// limit; see that function's own doc comment for the full sequence.
    fn reacquire_after_loss(
        &mut self,
        scene: &GpuSceneRef<'_>,
        frame: &BackendFrame<'_>,
        out: &mut FrameOutputs<'_>,
    ) -> bool {
        let reason = self
            .gpu
            .last_lost_reason()
            .unwrap_or_else(|| "unknown reason".to_string());
        tracing::warn!(%reason, "GPU renderer lost; dropping it and re-acquiring once");
        self.status_message = Some("GPU renderer lost \u{2014} reacquiring\u{2026}".to_string());
        self.gpu = GpuBackend::acquire();
        if self
            .gpu
            .try_accumulate(scene, frame.sample_offset, frame.spp, out.accum)
        {
            tracing::info!(%reason, "GPU renderer re-acquired successfully after loss");
            self.status_message = None;
            self.copy_guides(frame, out);
            return true;
        }
        tracing::warn!(
            %reason,
            "GPU re-acquire failed (or was lost again immediately); falling back to the \
             CPU tracer for the rest of this session"
        );
        self.cpu_fallback = true;
        self.status_message = Some(format!("CPU fallback (GPU lost: {reason})"));
        false
    }

    /// Copies the guide buffers for `frame`'s pose/geometry into `out` if they are not
    /// already the ones last applied -- see [`Self::applied_guide_key`]'s own doc
    /// comment. Split out of [`Self::try_accumulate`] so both the first GPU attempt and
    /// the post-re-acquire retry in [`Self::reacquire_after_loss`] share one copy of
    /// this logic instead of two copies that could drift.
    fn copy_guides(&mut self, frame: &BackendFrame<'_>, out: &mut FrameOutputs<'_>) {
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
/// adapter exists, or the device is otherwise unavailable. An HDR-mapped environment is
/// not a special case here either, since the GPU megakernel renders HDR maps directly
/// (see `docs/history/indicatrix-cut.md` for the CPU-only HDR path). Both
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
        // This CPU frame just wrote `outputs`' guide buffers directly --
        // the GPU's cached `applied_guide_key` no longer describes what they hold.
        backend.invalidate_guide_cache();
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
    // The GPU declined above, so this CPU retrace is what actually wrote
    // `outputs`' guide buffers this frame.
    backend.invalidate_guide_cache();
    hybrid.observe_cpu_only(frame.spp, start.elapsed());
}

/// One hybrid viewport frame: the GPU traces the lower `gpu_share` sample range on a
/// scoped thread (real guide buffers), while the CPU traces the upper range into
/// `hybrid`'s scratch with throwaway guide buffers (both backends derive identical
/// guide data, so discarding the CPU copy loses nothing). Summed into the real
/// accumulation only after both engines join, so they never write one buffer
/// concurrently. If the GPU declines mid-session, its share is retraced on the CPU and
/// hybrid stops offering it work.
///
/// # A joined GPU-thread panic retires the backend, not just the hybrid split
///
/// `hybrid.gpu_dead = true` alone only stops THIS function from offering the GPU
/// another share -- `accumulate_frame_samples`'s single-engine path
/// (`backend.try_accumulate`, reached whenever hybrid pacing is still measuring, or for
/// `LocalComputeTarget::Gpu`) does not consult `hybrid.gpu_dead` at all, so it would
/// call straight back into the SAME `GpuFrameRenderer` a panic just unwound out of
/// mid-dispatch -- exactly the "queued caller then panics with 'is still mapped'"
/// pattern `crates/indicatrix`'s own half of this handling describes. A DECLINE (the GPU
/// returning `false` normally) is harmless to retry; a PANIC means the renderer's
/// staging buffers may be left mapped, so only that case calls [`ViewportGpu::retire`],
/// which makes every later [`ViewportGpu::try_accumulate`] call -- hybrid or
/// single-engine alike -- decline without touching the renderer again.
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
    let (gpu_ok, gpu_time, cpu_time, gpu_panicked) = {
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
            match gpu_task.join() {
                Ok((gpu_ok, gpu_time)) => (gpu_ok, gpu_time, cpu_time, false),
                // The GPU-thread panicked rather than returning normally -- see this
                // function's own doc comment ("A joined GPU-thread panic..."). Distinct
                // from a plain decline: `cpu_time` fills the unmeasured `gpu_time` slot
                // (never observed into `hybrid`'s rate, since `gpu_panicked` skips that
                // below) purely so the tuple stays total.
                Err(_) => (false, cpu_time, cpu_time, true),
            }
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
    if gpu_panicked {
        tracing::error!(
            "GPU render thread panicked mid-frame; retiring the GPU backend for the \
             rest of this session"
        );
        backend.retire();
    }
    render_frame_scanlines(frame, gpu_share, frame.sample_offset + gpu_share, outputs);
    // This CPU retrace of the GPU's declined/panicked share just wrote
    // `outputs`' guide buffers directly -- note the GPU share above (when `gpu_ok`)
    // already refreshed `applied_guide_key` itself inside `try_accumulate`, so this
    // call is reached only on the path that did NOT.
    backend.invalidate_guide_cache();
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

/// Hardware tests driving the app's OWN [`ViewportGpu::
/// try_accumulate`] directly -- `indicatrix`'s own
/// `renderer::gpu_backend::tests::viewport_frames_with_camera_changes_never_poison_the_backend`
/// covers the crate-level `GpuBackend` entry point this wraps, but not this wrapper's
/// own self-healing/guide-buffer logic. Reaches `ViewportGpu`'s private fields
/// directly, which is why these live here rather than in `indicatrix`'s own test
/// module. Each test acquires its OWN `ViewportGpu` and prints a note and returns (a
/// clean skip, not a failure) when no GPU adapter is available.
#[cfg(all(test, feature = "gpu"))]
mod gpu_hardware_tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts, optics::raytracer::LightingPreset,
        renderer::gpu_backend::GpuPipelineKind,
    };

    /// Drives `ViewportGpu::try_accumulate` across 12 simulated viewport frames, each
    /// with a DIFFERENT camera pose (as `on_camera_orbit` produces on every drag
    /// `moved` event) and an advancing `sample_offset` (as the render loop's
    /// `accum_samples` does), for the given pipeline kind. Every call must succeed
    /// (`true`), and `status_message()` must stay `None` throughout -- any `Some` means
    /// a turn was declared `DeviceLost` and the self-healing path in
    /// `ViewportGpu::try_accumulate`/`reacquire_after_loss` had to react, which is
    /// exactly the poisoning this bug report describes.
    fn assert_twelve_rotated_frames_never_lose_the_device(pipeline_kind: GpuPipelineKind) {
        let mut viewport = ViewportGpu::acquire();
        if viewport.gpu.adapter_label().is_none() {
            println!(
                "skipping assert_twelve_rotated_frames_never_lose_the_device({pipeline_kind:?}): \
                 no GPU adapter"
            );
            return;
        }
        viewport.gpu.set_pipeline_kind(pipeline_kind);

        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
        let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
        let (width, height) = (480u32, 360u32);
        let spp = 2u32;
        let pixel_count = (width * height) as usize;
        let mut accum = vec![Vec3::ZERO; pixel_count];
        let mut depth = vec![1.0e6; pixel_count];
        let mut normal = vec![Vec3::ZERO; pixel_count];
        let mut facet_id = vec![-1i32; pixel_count];

        for frame in 0..12u32 {
            #[allow(
                clippy::cast_precision_loss,
                reason = "frame index is tiny; precision loss is not a concern in this test"
            )]
            let yaw = (frame as f32).mul_add(0.29, 0.1);
            #[allow(
                clippy::cast_precision_loss,
                reason = "frame index is tiny; precision loss is not a concern in this test"
            )]
            let pitch = (frame as f32).mul_add(0.13, 0.2).clamp(-1.4, 1.4);
            let camera = Camera::new(yaw, pitch, 5.0, 18.0);
            let backend_frame = BackendFrame {
                width,
                height,
                yaw,
                pitch,
                distance: 5.0,
                camera: &camera,
                planes: &planes,
                facet_finishes: &[],
                material: &material,
                max_bounces: 4,
                environment,
                spp,
                sample_offset: frame * spp,
            };
            let ok = viewport.try_accumulate(
                &backend_frame,
                &mut FrameOutputs {
                    accum: &mut accum,
                    depth: &mut depth,
                    normal: &mut normal,
                    facet_id: &mut facet_id,
                },
            );
            assert!(
                ok,
                "frame {frame} ({pipeline_kind:?}, yaw={yaw}, pitch={pitch}) declined -- \
                 status: {:?}",
                viewport.status_message()
            );
            assert_eq!(
                viewport.status_message(),
                None,
                "frame {frame} ({pipeline_kind:?}) left a self-healing status behind -- \
                 the device was lost at some point during this run"
            );
        }
        assert!(
            accum.iter().any(|v| v.length_squared() > 0.0),
            "a lit studio-rig scene traced over 12 frames must leave SOME nonzero radiance"
        );
    }

    #[test]
    fn twelve_rotated_frames_never_lose_the_device_megakernel() {
        assert_twelve_rotated_frames_never_lose_the_device(GpuPipelineKind::Megakernel);
    }

    #[test]
    fn twelve_rotated_frames_never_lose_the_device_wavefront() {
        assert_twelve_rotated_frames_never_lose_the_device(GpuPipelineKind::Wavefront);
    }
}
