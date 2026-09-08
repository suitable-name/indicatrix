// GPU self-determinism self-test kernel, driven by `renderer::gpu::determinism_check` --
// not a physics kernel.
//
// Demonstrates the pattern any real GPU raytracer kernel must use: each thread owns
// one output slot and accumulates into it with a strictly sequential, in-thread loop --
// no `atomicAdd`, no cross-thread reduction. Float addition is not associative, so
// `atomicAdd`'s term order depends on GPU scheduling and isn't bit-reproducible; a
// per-thread-only loop always sums in the same order every run, so it is.
//
// The per-sample term reuses the bit-exact RNG (`rng_equivalence.wgsl`) just to give
// the kernel realistic float work; determinism here doesn't depend on the RNG itself.

struct Params {
    num_pixels: u32,
    num_samples: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> out_sums: array<f32>;

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x = x * 0x85ebca6bu;
    x = x ^ (x >> 13u);
    x = x * 0xc2b2ae35u;
    x = x ^ (x >> 16u);
    return x;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x;
    if (pixel >= params.num_pixels) {
        return;
    }

    var sum: f32 = 0.0;
    for (var s: u32 = 0u; s < params.num_samples; s = s + 1u) {
        let seed = hash_u32((pixel * 0x9e3779b9u) ^ (s * 0x85ebca6bu));
        let v = f32(hash_u32(seed)) / 4294967295.0;
        // Sequential per-thread accumulation -- see file header.
        sum = sum + v;
    }
    out_sums[pixel] = sum;
}
