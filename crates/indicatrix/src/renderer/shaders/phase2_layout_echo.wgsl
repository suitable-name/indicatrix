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
    // and `write_debug_buffers` is no longer an always-zero pad value.
    pixel_offset: u32,
    write_debug_buffers: u32,
    white_balance: vec3<f32>,
    // Reused from the trailing `_pad2`; echoed explicitly for the same reason as above.
    studio_use_d65: u32,
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
}
