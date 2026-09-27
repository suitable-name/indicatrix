//! Pipeline compilation and bind-group construction: the megakernel's own bind group
//! ([`build_bind_group`]/[`build_chunk_bind_group`]) plus the five wavefront-pipeline
//! pipelines ([`WavefrontPipelines`]) and their four per-round bind groups
//! ([`WavefrontRoundBindGroups`]). See the parent module's doc comment's "Wavefront
//! pipeline" section for the kernel shape these bindings serve.

use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    renderer::{
        buffers::{GpuCameraParams, GpuGemMaterial, GpuTransportParams},
        env_map_gpu::HdrEnvGpuData,
        gpu::{GpuContext, compute},
    },
};

use super::{
    WAVEFRONT_SHADER_SRC,
    scene_buffers::{FrameSceneBuffers, TransportOutputs, WavefrontRayBuffers},
};

/// Bundles the material/scene/output-buffer inputs shared by [`build_bind_group`] and
/// [`super::encode_and_dispatch`] (and, on wasm32, [`super::GpuFrameRenderer::accumulate_async`]'s
/// inlined dispatch) -- purely to keep argument counts within clippy's
/// `too_many_arguments` limit. `total_tuples` stays its own parameter on the two dispatch
/// functions rather than joining this struct: it drives the workgroup count and the
/// output-capacity assert, and differs from `outputs.capacity` whenever a chunk is
/// smaller than its buffers' full capacity.
pub struct TransportDispatchArgs<'a> {
    pub(crate) ctx: &'a GpuContext,
    pub(crate) pipeline: &'a wgpu::ComputePipeline,
    pub(crate) camera_params: &'a GpuCameraParams,
    pub(crate) params: &'a GpuTransportParams,
    pub(crate) material: &'a GpuGemMaterial,
    pub(crate) planes: &'a [GpuFacetPlane],
    pub(crate) facet_finishes: &'a [u32],
    pub(crate) outputs: &'a TransportOutputs,
    /// Bindings 10-14 (`hdr_texels`/`hdr_env_dims`/`dist_func`/`dist_cdf`/`dist_dims`) -- ALWAYS bound, even for
    /// a non-HDR dispatch (pass [`HdrEnvGpuData::dummy`]); see that type's own doc comment
    /// for why the megakernel's bind group layout always includes these two.
    pub(crate) hdr_env: &'a HdrEnvGpuData,
}

/// Uploads the scene inputs and binds all eight buffers, WITHOUT dispatching.
///
/// Shared by every self-test's blocking path ([`super::encode_and_dispatch`]) and wasm32's
/// [`super::GpuFrameRenderer::accumulate_async`] inlined dispatch. Native's pipelined
/// production path (`GpuFrameRenderer::dispatch_chunk`) does NOT call this -- it would
/// re-upload all five buffers every chunk, which [`FrameSceneBuffers`]/
/// [`build_chunk_bind_group`] avoid; see the parent module's doc comment's "Per-frame
/// uploads, persistent staging" section. The local upload buffers are safely dropped when
/// this function returns even before the GPU has consumed them: `wgpu` keeps a resource
/// alive internally as long as any submitted (but not completed) command buffer
/// references it.
pub fn build_bind_group(args: &TransportDispatchArgs<'_>) -> wgpu::BindGroup {
    let camera_buf = compute::upload(
        &args.ctx.device,
        "transport camera",
        std::slice::from_ref(args.camera_params),
        BufferUsages::UNIFORM,
    );
    let params_buf = compute::upload(
        &args.ctx.device,
        "transport params",
        std::slice::from_ref(args.params),
        BufferUsages::UNIFORM,
    );
    let material_buf = compute::upload(
        &args.ctx.device,
        "transport material",
        std::slice::from_ref(args.material),
        BufferUsages::STORAGE,
    );
    let planes_buf = compute::upload(
        &args.ctx.device,
        "transport planes",
        args.planes,
        BufferUsages::STORAGE,
    );
    // A SEPARATE storage buffer, parallel to `planes_buf`, never merged into
    // `GpuFacetPlane` itself -- see `renderer::buffers::facet_finish`'s module doc
    // comment for why.
    let facet_finishes_buf = compute::upload(
        &args.ctx.device,
        "transport facet finishes",
        args.facet_finishes,
        BufferUsages::STORAGE,
    );

    compute::bind_buffers(
        &args.ctx.device,
        "transport bind group",
        args.pipeline,
        &[
            (0, &camera_buf),
            (1, &params_buf),
            (2, &material_buf),
            (3, &planes_buf),
            (4, args.outputs.xyz()),
            (5, args.outputs.radiance()),
            (6, args.outputs.lambdas()),
            (7, args.outputs.path_pdf()),
            (8, &facet_finishes_buf),
            (9, args.outputs.compat()),
            (10, &args.hdr_env.texels),
            (11, &args.hdr_env.dims),
            (12, &args.hdr_env.dist_func),
            (13, &args.hdr_env.dist_cdf),
            (14, &args.hdr_env.dist_dims),
        ],
    )
}

/// Like [`build_bind_group`], but for [`super::GpuFrameRenderer::dispatch_chunk`]'s
/// pipelined per-chunk dispatch: `frame_buffers`' four scene buffers and `params_buf` are
/// all PERSISTENT (see [`FrameSceneBuffers`] and [`super::GpuFrameRenderer::params_buffers`])
/// and already written by the caller -- this function only builds the bind group,
/// uploading nothing itself. `planes_len` is `state.scene.planes.len()` for the CURRENT
/// scene, which can be smaller than `frame_buffers.planes`'s allocated capacity
/// (grown-never-shrunk across frames), so `planes`/`facet_finishes` bind an explicit byte
/// range via [`compute::bind_buffers_sized`] rather than [`compute::bind_buffers`]'s
/// whole-buffer `as_entire_binding` -- otherwise the shader's `arrayLength(&planes)`
/// would see the buffer's stale larger capacity instead of this frame's true plane count.
/// See the parent module's doc comment's "Per-frame uploads, persistent staging" section.
pub fn build_chunk_bind_group(
    ctx: &GpuContext,
    pipeline: &wgpu::ComputePipeline,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
) -> wgpu::BindGroup {
    let planes_bytes = (planes_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (planes_len * size_of::<u32>()) as wgpu::BufferAddress;
    compute::bind_buffers_sized(
        &ctx.device,
        "transport bind group (pipelined production)",
        pipeline,
        &[
            (0, &frame_buffers.camera, None),
            (1, params_buf, None),
            (2, &frame_buffers.material, None),
            (3, &frame_buffers.planes, Some(planes_bytes)),
            (4, outputs.xyz(), None),
            (5, outputs.radiance(), None),
            (6, outputs.lambdas(), None),
            (7, outputs.path_pdf(), None),
            (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
            (9, outputs.compat(), None),
            (10, &frame_buffers.hdr_env.texels, None),
            (11, &frame_buffers.hdr_env.dims, None),
            (12, &frame_buffers.hdr_env.dist_func, None),
            (13, &frame_buffers.hdr_env.dist_cdf, None),
            (14, &frame_buffers.hdr_env.dist_dims, None),
        ],
    )
}

/// The five `wavefront_transport.wgsl` entry points -- see that
/// file's own header comment for the kernel sequence and
/// [`super::GpuFrameRenderer::dispatch_chunk_wavefront`] for how they're driven.
pub struct WavefrontPipelines {
    pub(super) generate: wgpu::ComputePipeline,
    pub(super) bounce: wgpu::ComputePipeline,
    pub(super) compact_scan: wgpu::ComputePipeline,
    pub(super) compact_scatter: wgpu::ComputePipeline,
    pub(super) finalize_survivors: wgpu::ComputePipeline,
}

impl WavefrontPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            generate: compute::create_compute_pipeline(
                device,
                "wavefront_generate",
                WAVEFRONT_SHADER_SRC,
                "wavefront_generate",
            ),
            bounce: compute::create_compute_pipeline(
                device,
                "wavefront_bounce",
                WAVEFRONT_SHADER_SRC,
                "wavefront_bounce",
            ),
            compact_scan: compute::create_compute_pipeline(
                device,
                "wavefront_compact_scan",
                WAVEFRONT_SHADER_SRC,
                "wavefront_compact_scan",
            ),
            compact_scatter: compute::create_compute_pipeline(
                device,
                "wavefront_compact_scatter",
                WAVEFRONT_SHADER_SRC,
                "wavefront_compact_scatter",
            ),
            finalize_survivors: compute::create_compute_pipeline(
                device,
                "wavefront_finalize_survivors",
                WAVEFRONT_SHADER_SRC,
                "wavefront_finalize_survivors",
            ),
        }
    }
}

/// Bytes per (pixel, sample) tuple of [`WavefrontRayBuffers::stokes`], its widest field
/// -- `tuples * 8` elements of `[f32; 4]` (an 8-channel Stokes vector per ray).
/// [`super::GpuFrameRenderer::cap_byte_budget_for_wavefront`] caps a wavefront chunk's
/// tuple count against this so `stokes`'s own buffer request never exceeds the device's
/// `max_storage_buffer_binding_size` -- the same `max_storage_buffer_binding_size` is a
/// PER-BINDING limit (`compute::create_buffer`'s validation is against one buffer at a
/// time), so this constant only needs to track the single widest per-ray field, not a
/// sum over all of them. `WavefrontRayBuffers::pending_light_mis_dir` (binding 34,
/// `wavefront_transport.wgsl`'s light-MIS interior-direction hand-off), added alongside
/// `pending_light_mis`, is `tuples` elements of `[f32; 4]` -- 16 bytes/tuple, well under
/// this constant -- so it does not change the cap.
pub const WAVEFRONT_STOKES_BYTES_PER_TUPLE: usize = 8 * 16;

/// The pure arithmetic [`super::GpuFrameRenderer::cap_byte_budget_for_wavefront`]
/// wraps -- pulled out into a standalone function (taking the device's
/// `max_storage_buffer_binding_size` as a plain `usize` rather than reading
/// `self.ctx.device.limits()`) so `renderer::gpu::frame`'s adapter-free `#[cfg(test)]`
/// module can exercise it with a FAKE limit, no real GPU device required. Caps
/// `byte_budget_pixels` (already the megakernel-sized ceiling) down to however many
/// pixels' worth of `spp` samples fit [`WavefrontRayBuffers::stokes`] (this module's
/// widest per-tuple wavefront buffer, [`WAVEFRONT_STOKES_BYTES_PER_TUPLE`] bytes/tuple)
/// within one storage-buffer binding -- see that function's own doc comment for why
/// only `stokes` needs tracking. `.max(1)` at both intermediate steps: a pathologically
/// tiny `max_binding_bytes` (or `spp == 0`) must still return a usable (if degenerate)
/// cap rather than `0`, which would make every later chunk-sizing division by it panic.
#[must_use]
pub fn wavefront_pixel_cap(max_binding_bytes: usize, spp: u32, byte_budget_pixels: usize) -> usize {
    let tuple_cap = (max_binding_bytes / WAVEFRONT_STOKES_BYTES_PER_TUPLE).max(1);
    let spp_usize = (spp as usize).max(1);
    let pixel_cap = (tuple_cap / spp_usize).max(1);
    byte_budget_pixels.min(pixel_cap)
}

/// Deterministic exclusive prefix sum over `counts` (one entry per compaction
/// workgroup), plus the total. A trivial sequential CPU loop -- see
/// `wavefront_compact_scan`'s own doc comment (`wavefront_transport.wgsl`) for why this
/// stays on the host rather than a GPU-side atomic counter.
pub fn exclusive_prefix_sum(counts: &[u32]) -> (Vec<u32>, u32) {
    let mut offsets = Vec::with_capacity(counts.len());
    let mut running = 0u32;
    for &c in counts {
        offsets.push(running);
        running += c;
    }
    (offsets, running)
}

/// `wavefront_generate`'s binding list -- see `wavefront_transport.wgsl`'s own module
/// doc comment for why this entry point reaches `camera`/`params` (binding 0/1, camera
/// ray generation) but none of the other shared scene bindings.
pub const fn wavefront_generate_bindings<'a>(
    camera: &'a wgpu::Buffer,
    params_buf: &'a wgpu::Buffer,
    wf_params_buf: &'a wgpu::Buffer,
    rays: &'a WavefrontRayBuffers,
) -> [(u32, &'a wgpu::Buffer); 18] {
    [
        (0, camera),
        (1, params_buf),
        (15, wf_params_buf),
        (16, &rays.origin),
        (17, &rays.dir),
        (18, &rays.k),
        (19, &rays.prev_plane_normal),
        (20, &rays.flags),
        (21, &rays.stokes),
        (22, &rays.radiance),
        (23, &rays.path_pdf),
        (24, &rays.split_radiance),
        (25, &rays.compat),
        (26, &rays.pending_light_mis),
        (27, &rays.lambdas),
        (28, &rays.seed),
        (29, &rays.active_in),
        (34, &rays.pending_light_mis_dir),
    ]
}

/// The four bind groups every bounce round (plus the final survivors pass) dispatches
/// through, built ONCE per chunk since the buffers they reference never change across
/// rounds -- see [`super::GpuFrameRenderer::run_wavefront_bounce_rounds`]'s own comment on
/// why `active_ray_indices`/`active_ray_indices_next` are copied between rounds instead of
/// the bind groups being rebuilt.
pub struct WavefrontRoundBindGroups {
    pub(super) bounce: wgpu::BindGroup,
    pub(super) compact_scan: wgpu::BindGroup,
    pub(super) compact_scatter: wgpu::BindGroup,
    pub(super) finalize: wgpu::BindGroup,
}

/// Builds [`WavefrontRoundBindGroups`] -- split out of
/// [`super::GpuFrameRenderer::dispatch_chunk_wavefront`] purely to keep that function under
/// clippy's function-length limit (mirrors [`build_chunk_bind_group`]'s own reason for
/// being a free function rather than inlined into its one call site), and split again
/// into one function per bind group (below) for the same reason.
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per distinct buffer source this chunk's four bind groups draw \
              from (device, pipelines, scene buffers, params, geometry length, transport \
              outputs, wavefront params, ray state) -- bundling them into a context struct \
              here would just re-introduce this exact parameter list one level removed, the \
              same reasoning ChunkFrameState/TransportDispatchArgs/TurnSetup already \
              document elsewhere in this module tree"
)]
pub fn build_wavefront_round_bind_groups(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> WavefrontRoundBindGroups {
    WavefrontRoundBindGroups {
        bounce: wavefront_bounce_bind_group(
            device,
            pipelines,
            frame_buffers,
            params_buf,
            planes_len,
            outputs,
            wf_params_buf,
            rays,
        ),
        compact_scan: wavefront_compact_scan_bind_group(device, pipelines, wf_params_buf, rays),
        compact_scatter: wavefront_compact_scatter_bind_group(
            device,
            pipelines,
            wf_params_buf,
            rays,
        ),
        finalize: wavefront_finalize_bind_group(
            device,
            pipelines,
            params_buf,
            outputs,
            wf_params_buf,
            rays,
        ),
    }
}

/// `wavefront_bounce`'s bind group -- everything `transport_bounce_step`/
/// `transport_finalize_ray` can reach, plus this pipeline's own per-round state.
///
/// Includes `camera` (binding 0): `wavefront_bounce` recomputes this ray's
/// `observer = -transport_generate_ray(ray_idx).dir` fresh every bounce round (mirroring
/// `transport_main`'s own `observer = -gen.dir`, see `wavefront_transport.wgsl`'s
/// `wavefront_bounce`), and `transport_generate_ray` reads `camera` (via
/// `generate_camera_ray`) and `params` (`camera.num_samples`/`params.pixel_offset`/
/// `params.sample_offset`) to reconstruct the ray's pixel/sample indices. Omitting it
/// made the auto-derived pipeline layout (which naga computes from what the entry
/// point's call graph actually reaches, not from this list) require a binding this bind
/// group never provided -- a `wgpu` validation error on every `Wavefront`-pipeline
/// dispatch (the wgpu-side symptom `examples/gpu_equivalence_harness.rs`'s
/// `run_pipeline_equivalence` hit as `DeviceLost`, before `GpuContext::
/// last_uncaptured_error` existed to surface the actual wgpu message).
#[expect(
    clippy::too_many_arguments,
    reason = "see build_wavefront_round_bind_groups' own #[expect] just above"
)]
fn wavefront_bounce_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    let planes_bytes = (planes_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (planes_len * size_of::<u32>()) as wgpu::BufferAddress;
    compute::bind_buffers_sized(
        device,
        "wavefront bounce bind group",
        &pipelines.bounce,
        &[
            (0, &frame_buffers.camera, None),
            (1, params_buf, None),
            (2, &frame_buffers.material, None),
            (3, &frame_buffers.planes, Some(planes_bytes)),
            (4, outputs.xyz(), None),
            (5, outputs.radiance(), None),
            (6, outputs.lambdas(), None),
            (7, outputs.path_pdf(), None),
            (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
            (9, outputs.compat(), None),
            (10, &frame_buffers.hdr_env.texels, None),
            (11, &frame_buffers.hdr_env.dims, None),
            (12, &frame_buffers.hdr_env.dist_func, None),
            (13, &frame_buffers.hdr_env.dist_cdf, None),
            (14, &frame_buffers.hdr_env.dist_dims, None),
            (15, wf_params_buf, None),
            (16, &rays.origin, None),
            (17, &rays.dir, None),
            (18, &rays.k, None),
            (19, &rays.prev_plane_normal, None),
            (20, &rays.flags, None),
            (21, &rays.stokes, None),
            (22, &rays.radiance, None),
            (23, &rays.path_pdf, None),
            (24, &rays.split_radiance, None),
            (25, &rays.compat, None),
            (26, &rays.pending_light_mis, None),
            (27, &rays.lambdas, None),
            (28, &rays.seed, None),
            (29, &rays.active_in, None),
            (34, &rays.pending_light_mis_dir, None),
        ],
    )
}

/// `wavefront_compact_scan`'s bind group -- see `wavefront_transport.wgsl`'s own doc
/// comment for why it touches only `ray_flags`/`active_ray_indices` plus its own
/// per-workgroup scratch, none of the scene bindings.
fn wavefront_compact_scan_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront compact scan bind group",
        &pipelines.compact_scan,
        &[
            (15, wf_params_buf),
            (20, &rays.flags),
            (29, &rays.active_in),
            (31, &rays.local_offset),
            (32, &rays.block_alive_count),
        ],
    )
}

/// `wavefront_compact_scatter`'s bind group -- see `wavefront_compact_scan_bind_group`'s
/// own doc comment.
fn wavefront_compact_scatter_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront compact scatter bind group",
        &pipelines.compact_scatter,
        &[
            (15, wf_params_buf),
            (20, &rays.flags),
            (29, &rays.active_in),
            (30, &rays.active_out),
            (31, &rays.local_offset),
            (33, &rays.block_offset),
        ],
    )
}

/// `wavefront_finalize_survivors`'s bind group -- reaches `params`/the four output
/// buffers (via `transport_finalize_ray`) plus its own per-ray scratch, but none of
/// `camera`/`material`/`planes`/`facet_finishes`/HDR environment (a survivor's own
/// physics already ran inside `wavefront_bounce`; finalizing just integrates and
/// writes out what it already accumulated).
fn wavefront_finalize_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    params_buf: &wgpu::Buffer,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront finalize survivors bind group",
        &pipelines.finalize_survivors,
        &[
            (1, params_buf),
            (4, outputs.xyz()),
            (5, outputs.radiance()),
            (6, outputs.lambdas()),
            (7, outputs.path_pdf()),
            (9, outputs.compat()),
            (15, wf_params_buf),
            (20, &rays.flags),
            (22, &rays.radiance),
            (23, &rays.path_pdf),
            (24, &rays.split_radiance),
            (25, &rays.compat),
            (27, &rays.lambdas),
            (29, &rays.active_in),
        ],
    )
}
