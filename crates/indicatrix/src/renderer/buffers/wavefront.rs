//! The wavefront transport pipeline's (`shaders/wavefront_transport.wgsl`,
//! `renderer::gpu::frame`'s `GpuPipelineKind::Wavefront` path) per-round uniform -- an
//! alternative to the megakernel's [`super::GpuTransportParams`], not a replacement.

use core::mem::offset_of;

/// Mirrors `wavefront_transport.wgsl`'s `WavefrontParams` uniform struct exactly.
///
/// # Layout
///
/// Four 4-byte-aligned `u32`s pack tightly into 16 bytes with no padding -- the same
/// "every field already lands on a WGSL-legal offset" property small uniform structs
/// share throughout this module tree.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuWavefrontParams {
    /// Total rays this chunk dispatched (`pixels_this_chunk * spp`) -- `wavefront_generate`'s
    /// own dispatch bound, and the fixed size every per-ray struct-of-arrays buffer
    /// (`renderer::gpu::frame::WavefrontRayBuffers`) is sized to.
    pub chunk_rays: u32,
    /// How many entries of `active_ray_indices` are live for the CURRENT bounce round --
    /// `wavefront_bounce`/`wavefront_compact_scan`/`wavefront_compact_scatter`/
    /// `wavefront_finalize_survivors`'s own dispatch bound.
    pub active_count: u32,
    /// The current bounce round index -- fed to `transport_bounce_step` exactly as
    /// `transport_main`'s own `for (var bounce...)` loop counter is.
    pub bounce: u32,
    /// `active_count.div_ceil(64)` -- how many `compact_block_alive_count`/
    /// `compact_block_offset` entries this round's compaction actually uses. Not read by
    /// any WGSL kernel today (each compaction kernel derives its own workgroup index from
    /// `@builtin(workgroup_id)` instead); carried here purely so the host-side CPU prefix
    /// sum and the GPU dispatch's own workgroup count agree on ONE source of truth rather
    /// than each recomputing `div_ceil` independently.
    pub workgroup_count: u32,
}

const _: () = {
    assert!(offset_of!(GpuWavefrontParams, chunk_rays) == 0);
    assert!(offset_of!(GpuWavefrontParams, active_count) == 4);
    assert!(offset_of!(GpuWavefrontParams, bounce) == 8);
    assert!(offset_of!(GpuWavefrontParams, workgroup_count) == 12);
    assert!(size_of::<GpuWavefrontParams>() == 16);
};
