//! GPU-side sample reduction and CPU readback: [`GpuReduceParams`]/`reduce_xyz_main`'s
//! dispatch (the "why a second dispatch" story in [`super::GpuFrameRenderer::dispatch_chunk`]'s
//! doc comment), the persistent [`StagingSlot`]/[`PixelXyzOutput`] buffers it reads
//! into and out of, and [`super::GpuFrameRenderer::drain_pending_chunk`], which blocks on
//! one chunk's readback and sums it into the caller's accumulation buffer.

use crate::renderer::gpu::compute;

use super::{
    FLOATS_PER_TUPLE, WORKGROUP_SIZE, renderer::GpuFrameRenderer, scene_buffers::TransportOutputs,
};

/// `reduce_xyz.wgsl`'s uniform param struct (binding 0 there). `_pad0`/`_pad1` reproduce
/// WGSL's implicit padding of a two-`u32` uniform struct up to a 16-byte block, mirroring
/// `renderer::buffers`' own layout convention (see that module's doc comment) rather than
/// relying on `wgpu`'s own struct-size rounding for a uniform buffer.
///
/// `pub(crate)` (not private): `layout_check::run_reduce_params` builds a
/// sample instance to echo through `phase2_layout_echo.wgsl`, from outside this module
/// tree.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuReduceParams {
    pub(crate) num_pixels: u32,
    pub(crate) num_samples: u32,
    pub(crate) _pad0: u32,
    pub(crate) _pad1: u32,
}

/// One double-buffered slot's GPU-reduced per-pixel XYZ output -- `reduce_xyz_main`'s
/// destination buffer (see `reduce_xyz.wgsl`'s header comment). Alternated
/// in lockstep with [`TransportOutputs`]/[`StagingSlot`] by
/// [`GpuFrameRenderer::dispatch_chunk`]: `reduce_xyz_main` sums a chunk's `out_xyz`
/// (`pixels * spp * 3` floats) down to `pixels * 3` floats here, and THIS buffer -- not
/// `out_xyz` -- is what the chunk's readback copy actually reads from.
pub struct PixelXyzOutput {
    pub(super) buffer: wgpu::Buffer,
    /// Capacity in PIXELS (3 floats each) -- distinct from `TransportOutputs::capacity`,
    /// which counts (pixel, sample) tuples.
    pub(super) capacity: usize,
}

impl PixelXyzOutput {
    pub(crate) fn new(device: &wgpu::Device, capacity: usize) -> Self {
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        Self {
            buffer: compute::zeroed_buffer::<f32>(
                device,
                "transport out pixel xyz (GPU-reduced)",
                capacity * 3,
                usage,
            ),
            capacity,
        }
    }
}

/// One persistent staging buffer backing one of [`GpuFrameRenderer::staging`]'s two
/// double-buffered slots -- reused across every chunk of every frame, and reallocated
/// only when a chunk needs more capacity than it currently has (see
/// [`GpuFrameRenderer::ensure_staging_capacity`]), mirroring how [`TransportOutputs`] is
/// grown-never-shrunk across frames rather than reallocated per chunk.
pub struct StagingSlot {
    pub(super) buffer: wgpu::Buffer,
    /// Capacity in PIXELS, not (pixel, sample) tuples -- `capacity * FLOATS_PER_TUPLE` is
    /// the buffer's actual float count. [`GpuFrameRenderer::dispatch_chunk`]'s
    /// readback copy reads from [`PixelXyzOutput`] (already GPU-reduced to one XYZ
    /// triple per pixel) rather than [`TransportOutputs::xyz`], so this is a
    /// pixel-scale quantity -- matching [`PixelXyzOutput::capacity`]'s own convention,
    /// not [`TransportOutputs`]'s tuple-scale `capacity`.
    pub(super) capacity: usize,
}

impl StagingSlot {
    pub(crate) fn new(device: &wgpu::Device, label: &str, capacity: usize) -> Self {
        let byte_len = (capacity * FLOATS_PER_TUPLE * size_of::<f32>()) as wgpu::BufferAddress;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { buffer, capacity }
    }
}

/// Whether a persistent [`StagingSlot`] currently sized for `current_capacity` tuples
/// must be reallocated to serve a chunk needing `required` tuples.
///
/// A pure predicate (no GPU access) so the "grow, never shrink" growth policy is directly
/// unit-testable. A slot already big enough (even strictly larger than `required`) is
/// deliberately left alone, matching [`GpuFrameRenderer::ensure_capacity`]'s identical
/// policy for `outputs`.
pub const fn staging_needs_growth(current_capacity: usize, required: usize) -> bool {
    current_capacity < required
}

/// Like `compute::copy_to_staging`, but copies into an ALREADY-ALLOCATED `staging`
/// buffer instead of creating a fresh one every call.
///
/// [`GpuFrameRenderer::dispatch_chunk`] reuses one of its two persistent [`StagingSlot`]s
/// across every chunk of a frame rather than allocating a fresh buffer per chunk.
/// `staging` must already be sized for at least `count` `T`s -- guaranteed by
/// [`GpuFrameRenderer::ensure_staging_capacity`], called once per `accumulate` call before
/// any chunk dispatches -- and must carry `COPY_DST | MAP_READ`.
pub fn copy_to_existing_staging<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    staging: &wgpu::Buffer,
    count: usize,
) -> wgpu::SubmissionIndex {
    let byte_len = (count * size_of::<T>()) as wgpu::BufferAddress;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("indicatrix gpu pipelined readback copy encoder (persistent staging)"),
    });
    encoder.copy_buffer_to_buffer(source, 0, staging, 0, byte_len);
    queue.submit(Some(encoder.finish()))
}

impl GpuFrameRenderer {
    /// Submits `reduce_xyz_main`'s dispatch for one chunk -- the second half of
    /// `Self::dispatch_chunk`'s "Why a second dispatch" doc comment, split into its own
    /// method purely to keep `dispatch_chunk` under clippy's function-length limit. Sums
    /// `outputs.xyz()`'s `pixels_this_chunk * num_samples` triples down into
    /// `pixel_output.buffer`'s `pixels_this_chunk` triples.
    pub(super) fn dispatch_reduce(
        &self,
        num_samples: u32,
        staging_slot: usize,
        pixels_this_chunk: usize,
        outputs: &TransportOutputs,
        pixel_output: &PixelXyzOutput,
    ) {
        let reduce_params_buf = self.reduce_params_buffers[staging_slot]
            .as_ref()
            .expect("ensure_reduce_params_buffers just populated both slots");
        let reduce_params = GpuReduceParams {
            num_pixels: pixels_this_chunk as u32,
            num_samples,
            _pad0: 0,
            _pad1: 0,
        };
        self.ctx
            .queue
            .write_buffer(reduce_params_buf, 0, bytemuck::bytes_of(&reduce_params));
        let reduce_bind_group = compute::bind_buffers(
            &self.ctx.device,
            "reduce xyz bind group",
            &self.reduce_pipeline,
            &[
                (0, reduce_params_buf),
                (1, outputs.xyz()),
                (2, &pixel_output.buffer),
            ],
        );
        let reduce_workgroups = (pixels_this_chunk as u32).div_ceil(WORKGROUP_SIZE as u32);
        let _ = compute::dispatch(
            &self.ctx.device,
            &self.ctx.queue,
            &self.reduce_pipeline,
            &reduce_bind_group,
            (reduce_workgroups, 1, 1),
        );
    }

    /// Blocks until `chunk`'s readback copy has completed, reads its XYZ, updates the
    /// per-tuple timing EMA (see [`GpuFrameRenderer::ns_per_tuple_ema`]), and -- when
    /// `accum` is `Some` -- sums each pixel's `spp` samples into it. Split out of
    /// `Self::accumulate_via_pipeline` so its dispatch loop stays readable.
    ///
    /// `accum: None` is `AccumulateOutcome::Cancelled`'s drain-then-discard path: the
    /// chunk is still waited on and read back (so no dangling GPU/mapping state leaks
    /// into the next call), but its result is thrown away rather than summed. The EMA is
    /// still updated either way -- a cancelled chunk's timing is a real measurement too.
    ///
    /// # Errors
    ///
    /// [`super::GpuFrameError::DeviceLost`] if the underlying `compute::finish_map_read_into`
    /// reports a timed-out or failed wait.
    pub(super) fn drain_pending_chunk(
        &mut self,
        chunk: super::PendingChunk,
        spp: u32,
        accum: Option<&mut [glam::Vec3]>,
    ) -> Result<(), super::GpuFrameError> {
        let staging_buffer = &self.staging[chunk.staging_slot]
            .as_ref()
            .expect("dispatch_chunk populated this staging slot")
            .buffer;
        // Reads into self.xyz_scratch (reused across every chunk of every frame) rather
        // than allocating a fresh Vec here -- see that field's own doc comment.
        // staging_buffer/self.xyz_scratch/self.ctx.device are three disjoint
        // fields of `self`, so borrowing them independently here is fine even though
        // this function takes `&mut self`.
        compute::finish_map_read_into(
            &self.ctx.device,
            staging_buffer,
            chunk.copy_index,
            &chunk.rx,
            &mut self.xyz_scratch,
        )
        .map_err(|e| super::GpuFrameError::DeviceLost(e.to_string()))?;

        let tuples_measured = chunk.pixels_this_chunk * spp as usize;
        if tuples_measured > 0 {
            let elapsed_ns = chunk.submitted_at.elapsed().as_nanos() as f64;
            let measured_ns_per_tuple = elapsed_ns / tuples_measured as f64;
            self.ns_per_tuple_ema =
                Some(self.ns_per_tuple_ema.map_or(measured_ns_per_tuple, |prev| {
                    super::CHUNK_TIMING_EMA_ALPHA.mul_add(measured_ns_per_tuple - prev, prev)
                }));
        }

        // self.xyz_scratch already holds `reduce_xyz_main`'s GPU-summed per-pixel
        // triples (see dispatch_chunk's "Why a second dispatch" doc comment) -- added
        // directly, no CPU-side per-sample summation loop needed any more.
        if let Some(accum) = accum {
            let xyz = &self.xyz_scratch;
            for local_pixel in 0..chunk.pixels_this_chunk {
                let base = local_pixel * 3;
                accum[chunk.first_pixel + local_pixel] +=
                    glam::Vec3::new(xyz[base], xyz[base + 1], xyz[base + 2]);
            }
        }
        Ok(())
    }
}
