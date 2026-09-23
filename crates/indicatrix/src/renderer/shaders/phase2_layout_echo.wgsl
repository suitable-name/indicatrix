// Phase 2 GPU struct-layout self-test kernel, driven by
// `renderer::gpu::layout_check::run_transport_params` -- not a physics kernel.
//
// Echoes every field of `GpuTransportParams` to an independent output buffer so
// `layout_check` can compare raw bytes (padding included) and confirm the
// hand-written `#[repr(C)]` offsets in `renderer::buffers` match WGSL's layout.
// Same mechanism as `layout_echo.wgsl`/`phase1_layout_echo.wgsl`.

struct GpuTransportParams {
    num_pixels: u32,
    max_bounces: u32,
    sample_offset: u32,
    env_mode: u32,
    l0: f32,
    studio_temp_k: f32,
    studio_spot_mult: f32,
    studio_exposure: f32,
    studio_light_yaw: f32,
    studio_light_pitch: f32,
    // Echoed explicitly: vec3<f32> alignment padding would hide an offset bug here,
    // and `write_debug_buffers` is not an always-zero pad value.
    pixel_offset: u32,
    write_debug_buffers: u32,
    white_balance: vec3<f32>,
    // Reused from the trailing `_pad2`; echoed explicitly for the same reason as above.
    studio_use_d65: u32,
    studio_model: u32,
    backdrop: f32,
    _pad_backdrop_0: u32,
    _pad_backdrop_1: u32,
}

@group(0) @binding(0) var<storage, read> in_params: GpuTransportParams;
@group(0) @binding(1) var<storage, read_write> out_params: GpuTransportParams;

@compute @workgroup_size(1)
fn echo_transport_params() {
    out_params.num_pixels = in_params.num_pixels;
    out_params.max_bounces = in_params.max_bounces;
    out_params.sample_offset = in_params.sample_offset;
    out_params.env_mode = in_params.env_mode;
    out_params.l0 = in_params.l0;
    out_params.studio_temp_k = in_params.studio_temp_k;
    out_params.studio_spot_mult = in_params.studio_spot_mult;
    out_params.studio_exposure = in_params.studio_exposure;
    out_params.studio_light_yaw = in_params.studio_light_yaw;
    out_params.studio_light_pitch = in_params.studio_light_pitch;
    out_params.pixel_offset = in_params.pixel_offset;
    out_params.write_debug_buffers = in_params.write_debug_buffers;
    out_params.white_balance = in_params.white_balance;
    out_params.studio_use_d65 = in_params.studio_use_d65;
    out_params.studio_model = in_params.studio_model;
    out_params.backdrop = in_params.backdrop;
    out_params._pad_backdrop_0 = in_params._pad_backdrop_0;
    out_params._pad_backdrop_1 = in_params._pad_backdrop_1;
}

// `renderer::gpu::layout_check` echoes these four small uniform structs
// (`reduce_xyz.wgsl`'s `GpuReduceParams`, `wavefront_transport.wgsl`'s
// `WavefrontParams`, `spectral_transport.wgsl`'s `HdrEnvDims`/`GpuDistDims`) in addition
// to `GpuTransportParams`, so their layout is verified by raw byte comparison rather
// than by eye. Mirrors this file's own `GpuTransportParams` echo, one dedicated
// binding pair and entry point per struct, same mechanism as `phase1_layout_echo.wgsl`.

struct GpuReduceParams {
    num_pixels: u32,
    num_samples: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(2) var<storage, read> in_reduce_params: GpuReduceParams;
@group(0) @binding(3) var<storage, read_write> out_reduce_params: GpuReduceParams;

@compute @workgroup_size(1)
fn echo_reduce_params() {
    out_reduce_params.num_pixels = in_reduce_params.num_pixels;
    out_reduce_params.num_samples = in_reduce_params.num_samples;
    out_reduce_params._pad0 = in_reduce_params._pad0;
    out_reduce_params._pad1 = in_reduce_params._pad1;
}

struct GpuWavefrontParams {
    chunk_rays: u32,
    active_count: u32,
    bounce: u32,
    workgroup_count: u32,
}

@group(0) @binding(4) var<storage, read> in_wavefront_params: GpuWavefrontParams;
@group(0) @binding(5) var<storage, read_write> out_wavefront_params: GpuWavefrontParams;

@compute @workgroup_size(1)
fn echo_wavefront_params() {
    out_wavefront_params.chunk_rays = in_wavefront_params.chunk_rays;
    out_wavefront_params.active_count = in_wavefront_params.active_count;
    out_wavefront_params.bounce = in_wavefront_params.bounce;
    out_wavefront_params.workgroup_count = in_wavefront_params.workgroup_count;
}

struct GpuHdrEnvDims {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(6) var<storage, read> in_hdr_env_dims: GpuHdrEnvDims;
@group(0) @binding(7) var<storage, read_write> out_hdr_env_dims: GpuHdrEnvDims;

@compute @workgroup_size(1)
fn echo_hdr_env_dims() {
    out_hdr_env_dims.width = in_hdr_env_dims.width;
    out_hdr_env_dims.height = in_hdr_env_dims.height;
    out_hdr_env_dims._pad0 = in_hdr_env_dims._pad0;
    out_hdr_env_dims._pad1 = in_hdr_env_dims._pad1;
}

struct GpuDistDims {
    width: u32,
    height: u32,
    marginal_func_int: f32,
    _pad0: f32,
}

@group(0) @binding(8) var<storage, read> in_dist_dims: GpuDistDims;
@group(0) @binding(9) var<storage, read_write> out_dist_dims: GpuDistDims;

@compute @workgroup_size(1)
fn echo_dist_dims() {
    out_dist_dims.width = in_dist_dims.width;
    out_dist_dims.height = in_dist_dims.height;
    out_dist_dims.marginal_func_int = in_dist_dims.marginal_func_int;
    out_dist_dims._pad0 = in_dist_dims._pad0;
}
