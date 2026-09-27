// Standalone kernel -- NOT concatenated with `transport_physics.wgsl` by `build.rs` (see
// that file's `generate_transport_shaders`, which only touches `spectral_transport.wgsl`
// and `transport_functions.wgsl`). `renderer::gpu::frame` pulls this in directly via
// `include_str!`, and it assumes no symbol from any other shader file is in scope.
//
// # Why this exists
//
// `transport_main` (in `spectral_transport.wgsl`) writes one XYZ triple per (pixel,
// sample) thread into `out_xyz`. Copying `tuples * 3` floats off the GPU per chunk and
// summing each pixel's `spp` samples on the CPU would cost, at 1080p x 8 spp, ~200 MB
// of readback per progressive pass. `reduce_xyz_main` does that same sum ON the GPU
// instead, one thread per PIXEL, so only `pixels_this_chunk * 3` floats -- the final
// per-pixel sums -- ever cross the PCIe bus.
//
// # Determinism
//
// Each thread sums its pixel's `num_samples` consecutive `out_xyz` tuples in fixed
// ASCENDING sample-index order -- the same order `GpuFrameRenderer::drain_pending_chunk`
// sums them in on the CPU (`for s in 0..spp`). Float addition is not associative,
// so a reduction order is only guaranteed bit-identical to another if the order itself is
// identical; here it is, so `run_chunk_equivalence`'s chunked-vs-whole-frame bit-identity
// and `estimator_check`'s dispatch-determinism checks both continue to hold.
//
// # Non-finite samples: dropped but still counted
//
// A tuple with any NaN or +/-Inf component adds nothing to its pixel's sum, while the
// caller still counts it in the sample count it divides by. This is the rule every render
// backend shares (the CPU twin is `optics::raytracer::add_finite_sample`, used by the
// live scanline path, the export batch tracer, the worker and `gpu::hybrid`), so sums
// from different backends merge by plain addition. Keep the two in lock-step.

struct GpuReduceParams {
    num_pixels: u32,
    num_samples: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> reduce_params: GpuReduceParams;
// The `f32` XYZ triples `transport_finalize_ray` wrote into `out_xyz`, read here as their
// raw bit patterns (`u32`, same 4-byte stride) so the finiteness test below is pure integer
// work on the loaded bits -- see `valid_xyz_bits`.
@group(0) @binding(1) var<storage, read> reduce_in_xyz: array<u32>;
@group(0) @binding(2) var<storage, read_write> out_pixel_xyz: array<f32>;

// IEEE-754 binary32 exponent field. A value is finite exactly when its exponent bits are
// not all ones -- the same test as Rust's `f32::is_finite`.
const F32_EXPONENT_MASK: u32 = 0x7f800000u;

// GPU twin of `optics::raytracer::add_finite_sample`'s `Vec3::is_finite` test: true when
// all three components are finite. Tested on the bit pattern before any float
// reinterpretation, not with `x == x` or a magnitude compare: WGSL lets an implementation
// assume NaN and infinities are absent from float arithmetic, so a float-side test could
// legally be folded away.
fn valid_xyz_bits(bits: vec3<u32>) -> bool {
    let exp = bits & vec3<u32>(F32_EXPONENT_MASK);
    return all(exp != vec3<u32>(F32_EXPONENT_MASK));
}

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
        let bits = vec3<u32>(
            reduce_in_xyz[base + 0u],
            reduce_in_xyz[base + 1u],
            reduce_in_xyz[base + 2u],
        );
        // Dropped but still counted -- see "Non-finite samples" in the header comment.
        if (valid_xyz_bits(bits)) {
            let sample = bitcast<vec3<f32>>(bits);
            sum.x = sum.x + sample.x;
            sum.y = sum.y + sample.y;
            sum.z = sum.z + sample.z;
        }
    }

    out_pixel_xyz[pixel * 3u + 0u] = sum.x;
    out_pixel_xyz[pixel * 3u + 1u] = sum.y;
    out_pixel_xyz[pixel * 3u + 2u] = sum.z;
}
