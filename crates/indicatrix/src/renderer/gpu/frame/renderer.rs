//! [`GpuFrameRenderer`] itself: the struct definition plus construction and
//! capacity-management methods (`new`, `set_pipeline_kind`, the `ensure_*` buffer-growth
//! methods, and `abandon_in_flight`). The per-frame accumulate loop and per-chunk
//! dispatch live in sibling modules ([`super::accumulate`]/[`super::dispatch`]) since
//! both need `&mut self`/`&self` access to the fields defined here.

use wgpu::BufferUsages;

use crate::renderer::gpu::{
    GpuContext, MEGAKERNEL_STORAGE_BUFFERS, WAVEFRONT_BOUNCE_TOTAL_BUFFERS,
    WAVEFRONT_STORAGE_BUFFERS,
};

use super::{
    CHUNK_BUDGET_BYTES, GpuFrameError, GpuPipelineKind, REDUCE_SHADER_SRC, SHADER_SRC,
    bind_groups::WavefrontPipelines,
    readback::{GpuReduceParams, PixelXyzOutput, StagingSlot, staging_needs_growth},
    scene_buffers::{FrameSceneBuffers, TransportOutputs},
};
use crate::renderer::gpu::compute;

/// A GPU-backed renderer for arbitrary scenes, owning its device and pipeline across
/// frames.
///
/// Construct once and keep it: [`GpuFrameRenderer::new`] acquires an adapter and
/// compiles the megakernel, both of which take long enough to be worth doing off the
/// frame loop.
pub struct GpuFrameRenderer {
    pub(super) ctx: GpuContext,
    /// The GENERIC (`MATERIAL_CLASS = 0`) pipeline -- built eagerly since every self-test
    /// in `renderer::gpu` dispatches it, and the equivalence checks need it available
    /// without any prior `accumulate` call. See the parent module's doc comment's
    /// "Material-class kernel specialisation" section.
    pub(super) pipeline: wgpu::ComputePipeline,
    /// The three per-class specialised pipelines, built LAZILY (see
    /// [`Self::ensure_specialized_pipeline`](super::specialisation)) on first use of that
    /// class -- `None` until a scene of that class is dispatched, so a session that only
    /// renders one class never pays the others' shader-compile cost.
    pub(super) pipeline_isotropic: Option<wgpu::ComputePipeline>,
    pub(super) pipeline_uniaxial: Option<wgpu::ComputePipeline>,
    pub(super) pipeline_biaxial: Option<wgpu::ComputePipeline>,
    /// `reduce_xyz_main`'s pipeline (see `reduce_xyz.wgsl`'s header comment) -- built
    /// eagerly in [`Self::new`]/`Self::new_async` alongside the GENERIC transport
    /// pipeline, since every production dispatch (`dispatch_chunk` and wasm32's
    /// `accumulate_async`) uses it unconditionally.
    pub(super) reduce_pipeline: wgpu::ComputePipeline,
    pub(super) adapter_label: String,
    /// TWO chunk-output buffer sets, alternated by chunk index so one chunk's dispatch
    /// can be queued into the other slot while the previous chunk's readback is still in
    /// flight -- see the parent module's doc comment's "Overlapped chunk pipeline"
    /// section.
    pub(super) outputs: [Option<TransportOutputs>; 2],
    /// TWO double-buffered [`PixelXyzOutput`]s, alternated in lockstep with `outputs` --
    /// `reduce_xyz_main`'s destination, and what `Self::dispatch_chunk`'s readback copy
    /// reads from instead of `outputs[..].xyz()` directly (see `Self::dispatch_chunk`'s
    /// "Why a second dispatch" doc comment).
    pub(super) pixel_outputs: [Option<PixelXyzOutput>; 2],
    /// TWO persistent staging buffers, alternated by chunk index in lockstep with
    /// `outputs` -- reused across every chunk of every frame and grown (never shrunk)
    /// only when [`Self::ensure_staging_capacity`] finds one too small. See the parent
    /// module's doc comment's "Per-frame uploads, persistent staging" section.
    pub(super) staging: [Option<StagingSlot>; 2],
    /// TWO persistent per-output-slot `GpuTransportParams` uniform buffers, written via
    /// `queue.write_buffer` immediately before that slot's dispatch rather than recreated
    /// every chunk (see `Self::dispatch_chunk`). Safe to overwrite a
    /// slot's buffer two chunks later because `wgpu` executes queue operations in
    /// submission order: the write is submitted strictly after the earlier chunk that
    /// read the same slot, matching `outputs`/`staging`'s own double-buffering depth.
    pub(super) params_buffers: [Option<wgpu::Buffer>; 2],
    /// TWO persistent per-output-slot [`GpuReduceParams`] uniform buffers for
    /// `reduce_xyz_main`, mirroring `params_buffers`' same write-before-dispatch,
    /// two-deep-safe reuse policy.
    pub(super) reduce_params_buffers: [Option<wgpu::Buffer>; 2],
    /// The four scene-input buffers, persistent across every `accumulate` call (not just
    /// within one) -- see [`FrameSceneBuffers::ensure`]. `None` only before this
    /// renderer's very first `accumulate` call.
    pub(super) scene_buffers: Option<FrameSceneBuffers>,
    /// Reusable scratch buffer for `Self::drain_pending_chunk`'s per-chunk GPU-reduced
    /// XYZ readback -- avoids a fresh `Vec` allocation every chunk (see
    /// `compute::finish_map_read_into`). Safe to reuse across chunks: the overlapped
    /// pipeline's pending queue is exactly one chunk deep (see the parent module's doc
    /// comment's "Overlapped chunk pipeline" section), so at most one
    /// `drain_pending_chunk` call is ever using it at a time.
    pub(super) xyz_scratch: Vec<f32>,
    pub(super) chunk_budget_bytes: usize,
    /// Running exponential moving average of nanoseconds-per-(pixel,sample)-tuple,
    /// measured from dispatch submission to readback completion in
    /// `Self::drain_pending_chunk` -- `None` until the first chunk has drained.
    /// `Self::next_chunk_pixels` sizes every later chunk toward `TARGET_CHUNK_MS`
    /// from this estimate.
    pub(super) ns_per_tuple_ema: Option<f64>,
    /// Which kernel `Self::dispatch_chunk` dispatches through --
    /// see [`GpuPipelineKind`]'s own doc comment. `Default::default()` (`Megakernel`)
    /// until [`Self::set_pipeline_kind`] is called.
    pub(super) pipeline_kind: GpuPipelineKind,
    /// The five wavefront-pipeline pipelines, built LAZILY on first use of
    /// [`GpuPipelineKind::Wavefront`] -- mirrors `Self::pipeline_isotropic`/etc.'s own
    /// "never pay a shader-compile cost a session doesn't use" reasoning, at the whole
    /// pipeline granularity rather than per material class.
    pub(super) wavefront_pipelines: Option<WavefrontPipelines>,
    /// Set once, permanently, by [`Self::abandon_in_flight`] -- never cleared.
    /// A `true` here means a previous chunk readback failed (or a previous turn
    /// unwound, see `Self::turn_in_progress`) and this renderer's staging slots were
    /// dropped rather than `unmap()`d, so any state a resumed dispatch would depend on
    /// (persistent buffers, the chunk-timing EMA) is no longer trustworthy. Checked at
    /// the very top of `Self::accumulate_turn`, before touching anything else, so a
    /// poisoned renderer declines every later call instead of ever dispatching into it
    /// again -- mirrors `GpuFrameError::DeviceLost`'s "stop using this renderer for the
    /// rest of the process" contract, enforced one layer up by `GpuBackend::lost`.
    pub(super) poisoned: bool,
    /// `true` for exactly the duration of one `Self::accumulate_turn` call
    /// -- set at entry, cleared just before that call returns. Exists to catch a panic
    /// that unwinds OUT of `accumulate_turn` (a wgpu validation panic this module did
    /// not manage to intercept via `GpuContext::validation_error_seen`, or any other
    /// unexpected panic mid-chunk): the unwind skips the "clear" step, so the NEXT call
    /// into `accumulate_turn` finds this still `true`, knows the previous call never
    /// finished, and poisons the renderer via [`Self::abandon_in_flight`] before doing
    /// anything else -- a `catch_unwind`-free way to detect "the last turn didn't
    /// complete" without needing `std::panic::catch_unwind` (which this crate avoids: it
    /// would need `UnwindSafe` bounds on every `wgpu` type this module touches).
    pub(super) turn_in_progress: bool,
}

impl GpuFrameRenderer {
    /// Acquires a GPU device and compiles `transport_main`.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::Acquire`] if this machine has no usable adapter or device --
    /// an expected outcome, not a bug; the caller should fall back to the CPU tracer.
    pub fn new() -> Result<Self, GpuFrameError> {
        let ctx = GpuContext::acquire().map_err(GpuFrameError::Acquire)?;
        let available = ctx.device.limits().max_storage_buffers_per_shader_stage;
        if available < MEGAKERNEL_STORAGE_BUFFERS {
            return Err(GpuFrameError::DeviceLimits {
                needed: MEGAKERNEL_STORAGE_BUFFERS,
                available,
            });
        }
        let info = ctx.adapter.get_info();
        let adapter_label = format!("{} ({:?})", info.name, info.backend);
        let pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "transport_main",
            SHADER_SRC,
            "transport_main",
        );
        let reduce_pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "reduce_xyz_main",
            REDUCE_SHADER_SRC,
            "reduce_xyz_main",
        );
        Ok(Self {
            ctx,
            pipeline,
            pipeline_isotropic: None,
            pipeline_uniaxial: None,
            pipeline_biaxial: None,
            reduce_pipeline,
            adapter_label,
            outputs: [None, None],
            pixel_outputs: [None, None],
            staging: [None, None],
            params_buffers: [None, None],
            reduce_params_buffers: [None, None],
            scene_buffers: None,
            xyz_scratch: Vec::new(),
            chunk_budget_bytes: CHUNK_BUDGET_BYTES,
            ns_per_tuple_ema: None,
            pipeline_kind: GpuPipelineKind::default(),
            wavefront_pipelines: None,
            poisoned: false,
            turn_in_progress: false,
        })
    }

    /// Overrides the per-dispatch output-buffer budget (default [`CHUNK_BUDGET_BYTES`]).
    ///
    /// Exists so a check can force a frame through many small chunks and confirm the
    /// result is identical to the single-chunk one -- see
    /// [`super::run_chunk_equivalence`]. Lowering it trades more dispatches for less
    /// peak VRAM; it never changes what a frame renders.
    pub const fn set_chunk_budget_bytes(&mut self, bytes: usize) {
        self.chunk_budget_bytes = bytes;
    }

    /// Selects which kernel every LATER chunk dispatches through --
    /// see [`GpuPipelineKind`]'s own doc comment. Takes effect from the next
    /// `dispatch_chunk` call onward; a chunk already dispatched (or in flight in the
    /// overlapped pipeline) is unaffected. Existing signatures
    /// (`Self::accumulate`/`Self::accumulate_cancellable`/etc.) are unchanged --
    /// this is the caller's opt-in, not a new required argument.
    ///
    /// Eagerly compiles the five wavefront pipelines on first switch to
    /// [`GpuPipelineKind::Wavefront`] (a no-op on a later call, or a switch back to
    /// `Megakernel`): `dispatch_chunk`/`dispatch_chunk_wavefront` take `&self`, not
    /// `&mut self` (required by the overlapped-chunk-pipeline borrow pattern
    /// `accumulate_turn` uses), so lazy build-on-first-dispatch the way
    /// `Self::ensure_specialized_pipeline` does for material classes isn't available
    /// here -- this setter is the one `&mut self` call site available to do it instead.
    pub fn set_pipeline_kind(&mut self, kind: GpuPipelineKind) {
        let device_limits = self.ctx.device.limits();
        let available = device_limits.max_storage_buffers_per_shader_stage;
        // A device that granted enough
        // storage-buffer bindings can still fall short of the COMBINED
        // uniform+storage+acceleration-structure count `wavefront_bounce`'s auto-derived
        // pipeline layout needs -- see `WAVEFRONT_BOUNCE_TOTAL_BUFFERS`'s doc comment.
        // Checked here, before `WavefrontPipelines::new` ever calls
        // `Device::create_compute_pipeline`, so a device that genuinely cannot support
        // it declines gracefully (falls back to the megakernel) instead of that call
        // itself failing with "Unable to derive an implicit layout".
        let available_total =
            device_limits.max_buffers_and_acceleration_structures_per_shader_stage;
        if kind == GpuPipelineKind::Wavefront
            && (available < WAVEFRONT_STORAGE_BUFFERS
                || available_total < WAVEFRONT_BOUNCE_TOTAL_BUFFERS)
        {
            tracing::warn!(
                available,
                needed = WAVEFRONT_STORAGE_BUFFERS,
                available_total,
                needed_total = WAVEFRONT_BOUNCE_TOTAL_BUFFERS,
                "device cannot bind enough buffers for the wavefront pipeline; staying on \
                 the megakernel"
            );
            self.pipeline_kind = GpuPipelineKind::Megakernel;
            return;
        }
        self.pipeline_kind = kind;
        if kind == GpuPipelineKind::Wavefront && self.wavefront_pipelines.is_none() {
            self.wavefront_pipelines = Some(WavefrontPipelines::new(&self.ctx.device));
        }
    }

    /// Which kernel `Self::dispatch_chunk` currently dispatches through -- see
    /// [`Self::set_pipeline_kind`].
    #[must_use]
    pub const fn pipeline_kind(&self) -> GpuPipelineKind {
        self.pipeline_kind
    }

    /// Human-readable adapter name and backend, for logging and for telling the user
    /// which device is actually rendering.
    #[must_use]
    pub fn adapter_label(&self) -> &str {
        &self.adapter_label
    }

    /// Drops both persistent staging slots and marks this renderer
    /// permanently unusable.
    ///
    /// A staging buffer `compute::finish_map_read_into` failed to finish reading (a
    /// poll timeout, a `recv_timeout` failure, or a `get_mapped_range` error -- see that
    /// function's `# Errors`) is left in wgpu's `Waiting`/`Active` map state forever:
    /// calling `unmap()` on it from here would itself error (a buffer's map state only
    /// clears via ITS OWN pending callback resolving, not an unrelated call site), and
    /// the next `dispatch_chunk` that reuses the same slot would submit a write into a
    /// still-mapped buffer, which panics with the exact
    /// `"transport out xyz staging (persistent)" ... still mapped` text wgpu produces
    /// for a buffer still mapped when reused. Dropping the buffer instead is legal in
    /// wgpu 30 even mid-map (the
    /// pending callback is simply never delivered) and immediately releases the GPU
    /// allocation.
    ///
    /// Never rebuilds the dropped slots: `self.poisoned = true` here is permanent (no
    /// method ever clears it) exactly because a renderer this was called on has no
    /// trustworthy way to know how much of a chunk's work actually completed before the
    /// failure -- matching [`GpuFrameError::DeviceLost`]'s "stop using this renderer for
    /// the rest of the process" contract enforced one layer up in
    /// `renderer::gpu_backend::GpuBackend`.
    ///
    /// `pub(crate)`: lets this crate's own `#[cfg(all(test, feature =
    /// "gpu"))]` hardware tests simulate "a previous chunk readback already failed"
    /// directly, without needing to actually wedge a real GPU dispatch to reach this
    /// state -- see `renderer::gpu::frame`'s test module.
    pub(crate) fn abandon_in_flight(&mut self) {
        self.staging = [None, None];
        self.poisoned = true;
    }

    /// Grows the cached output buffers to hold at least `tuples`, reusing them when
    /// already large enough (the common case: a progressive render re-dispatches the same
    /// resolution until the camera moves). Grows BOTH double-buffered slots identically,
    /// since a chunk can land in either.
    pub(super) fn ensure_capacity(&mut self, tuples: usize) {
        for slot in &mut self.outputs {
            let big_enough = slot.as_ref().is_some_and(|o| o.capacity >= tuples);
            if !big_enough {
                *slot = Some(TransportOutputs::new_production(&self.ctx.device, tuples));
            }
        }
    }

    /// Grows the persistent staging buffers to hold at least `pixels` pixels' worth of
    /// (GPU-reduced) XYZ floats, reusing them when already large enough. Mirrors
    /// [`Self::ensure_capacity`]'s policy for `outputs` -- grow both slots identically,
    /// never shrink -- via the same pure predicate, [`staging_needs_growth`]. Sized in
    /// PIXELS, not (pixel, sample) tuples, since `Self::dispatch_chunk`'s readback copy
    /// reads from [`PixelXyzOutput`], not `TransportOutputs::xyz`
    /// directly -- see [`StagingSlot`]'s own doc comment.
    pub(super) fn ensure_staging_capacity(&mut self, pixels: usize) {
        for slot in &mut self.staging {
            let current_capacity = slot.as_ref().map_or(0, |s| s.capacity);
            if staging_needs_growth(current_capacity, pixels) {
                *slot = Some(StagingSlot::new(
                    &self.ctx.device,
                    "transport out xyz staging (persistent)",
                    pixels,
                ));
            }
        }
    }

    /// Grows the cached [`PixelXyzOutput`] buffers to hold at least `pixels` pixels,
    /// reusing them when already large enough. Mirrors [`Self::ensure_capacity`]'s policy
    /// for `outputs` exactly, just sized in pixels rather than (pixel, sample) tuples.
    pub(super) fn ensure_pixel_capacity(&mut self, pixels: usize) {
        for slot in &mut self.pixel_outputs {
            let big_enough = slot.as_ref().is_some_and(|o| o.capacity >= pixels);
            if !big_enough {
                *slot = Some(PixelXyzOutput::new(&self.ctx.device, pixels));
            }
        }
    }

    /// Creates both persistent `GpuTransportParams` uniform buffers on first use --
    /// see [`Self::params_buffers`]'s doc comment. Fixed-size (the struct never changes
    /// size), so unlike [`Self::ensure_capacity`] this only ever creates, never grows.
    pub(super) fn ensure_params_buffers(&mut self) {
        for slot in &mut self.params_buffers {
            if slot.is_none() {
                *slot = Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("transport params (persistent)"),
                    size: size_of::<crate::renderer::buffers::GpuTransportParams>()
                        as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
        }
    }

    /// Like [`Self::ensure_params_buffers`], for [`Self::reduce_params_buffers`].
    pub(super) fn ensure_reduce_params_buffers(&mut self) {
        for slot in &mut self.reduce_params_buffers {
            if slot.is_none() {
                *slot = Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("reduce xyz params (persistent)"),
                    size: size_of::<GpuReduceParams>() as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
        }
    }
}
