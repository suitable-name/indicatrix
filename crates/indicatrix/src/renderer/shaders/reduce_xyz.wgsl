// Standalone kernel -- NOT concatenated with `transport_physics.wgsl` by `build.rs` (see
// that file's `generate_transport_shaders`, which only touches `spectral_transport.wgsl`
// and `transport_functions.wgsl`). `renderer::gpu::frame` pulls this in directly via
// `include_str!`, and it assumes no symbol from any other shader file is in scope.
//
// # Why this exists
//
// `transport_main` (in `spectral_transport.wgsl`) writes one XYZ triple per (pixel,
// sample) thread into `out_xyz`. `GpuFrameRenderer::dispatch_chunk`'s production path
// used to copy `tuples * 3` floats off the GPU per chunk and sum each pixel's `spp`
// samples on the CPU -- at 1080p x 8 spp that is ~200 MB of readback per progressive
// pass. `reduce_xyz_main` does that same sum ON the GPU instead, one thread per PIXEL,
// so only `pixels_this_chunk * 3` floats -- the final per-pixel sums -- ever cross the
// PCIe bus.
//
// # Determinism
//
// Each thread sums its pixel's `num_samples` consecutive `out_xyz` tuples in fixed
// ASCENDING sample-index order -- the exact order `GpuFrameRenderer::drain_pending_chunk`
// used to sum them in on the CPU (`for s in 0..spp`). Float addition is not associative,
// so a reduction order is only guaranteed bit-identical to another if the order itself is
// identical; here it is, so `run_chunk_equivalence`'s chunked-vs-whole-frame bit-identity
// and `estimator_check`'s dispatch-determinism checks both continue to hold unchanged.

struct GpuReduceParams {
    num_pixels: u32,
    num_samples: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> reduce_params: GpuReduceParams;
@group(0) @binding(1) var<storage, read> reduce_in_xyz: array<f32>;
@group(0) @binding(2) var<storage, read_write> out_pixel_xyz: array<f32>;

@compute @workgroup_size(64)
fn reduce_xyz_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x;
    if (pixel >= reduce_params.num_pixels) {
        return;
    }

    // Ascending sample-index order -- see this file's header comment on why that
    // matters for determinism/equivalence.
    var sum = vec3<f32>(0.0, 0.0, 0.0);
    let base_tuple = pixel * reduce_params.num_samples;
    for (var s: u32 = 0u; s < reduce_params.num_samples; s = s + 1u) {
        let base = (base_tuple + s) * 3u;
        sum.x = sum.x + reduce_in_xyz[base + 0u];
        sum.y = sum.y + reduce_in_xyz[base + 1u];
        sum.z = sum.z + reduce_in_xyz[base + 2u];
    }

    out_pixel_xyz[pixel * 3u + 0u] = sum.x;
    out_pixel_xyz[pixel * 3u + 1u] = sum.y;
    out_pixel_xyz[pixel * 3u + 2u] = sum.z;
}
