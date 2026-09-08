//! GPU self-determinism self-test.
//!
//! Dispatches `shaders/self_determinism.wgsl` twice against identical input on the same
//! adapter and asserts the two output buffers are byte-for-byte identical.
//!
//! This is the property a future real GPU raytracer's per-pixel accumulation MUST have
//! (see the shader's own doc comment): each thread owns its pixels' accumulation with
//! no `atomicAdd` and no cross-thread reduction, so nothing about GPU scheduling can
//! change the bit pattern of the result from run to run.

use crate::renderer::gpu::compute;

const SHADER_SRC: &str = include_str!("../shaders/self_determinism.wgsl");

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    num_pixels: u32,
    num_samples: u32,
    _pad0: u32,
    _pad1: u32,
}

/// One pixel's disagreement between the two runs.
#[derive(Debug, Clone, Copy)]
pub struct DeterminismMismatch {
    pub pixel: u32,
    pub run1: f32,
    pub run2: f32,
}

#[derive(Debug, Clone)]
pub struct DeterminismCheckResult {
    pub num_pixels: u32,
    pub num_samples: u32,
    pub mismatches: Vec<DeterminismMismatch>,
}

impl DeterminismCheckResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// Runs `shaders/self_determinism.wgsl` once and returns its output buffer.
fn run_once(ctx: &crate::renderer::gpu::GpuContext, num_pixels: u32, num_samples: u32) -> Vec<f32> {
    let params = Params {
        num_pixels,
        num_samples,
        _pad0: 0,
        _pad1: 0,
    };
    let params_buf = compute::upload(
        &ctx.device,
        "self_determinism params",
        std::slice::from_ref(&params),
        wgpu::BufferUsages::UNIFORM,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "self_determinism output",
        num_pixels as usize,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );

    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "self_determinism", SHADER_SRC, "main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "self_determinism bind group",
        &pipeline,
        &[(0, &params_buf), (1, &out_buf)],
    );

    let workgroups = num_pixels.div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );

    compute::readback(&ctx.device, &ctx.queue, &out_buf, num_pixels as usize)
}

/// Runs the self-determinism check: dispatches the accumulation kernel twice against
/// the same `(num_pixels, num_samples)` input and compares every output byte.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run(
    ctx: &crate::renderer::gpu::GpuContext,
    num_pixels: u32,
    num_samples: u32,
) -> DeterminismCheckResult {
    let run1 = run_once(ctx, num_pixels, num_samples);
    let run2 = run_once(ctx, num_pixels, num_samples);

    let mismatches = run1
        .iter()
        .zip(run2.iter())
        .enumerate()
        .filter_map(|(pixel, (&a, &b))| {
            // Bitwise, not `==`: this must be the exact same bit pattern, not merely
            // numerically-equal-modulo-NaN-weirdness (an accumulation that produced NaN
            // deterministically both times should still be reported as "passed" for
            // THIS check, since `NaN != NaN` under `==` would otherwise flag a
            // perfectly reproducible result as a mismatch).
            (a.to_bits() != b.to_bits()).then_some(DeterminismMismatch {
                pixel: pixel as u32,
                run1: a,
                run2: b,
            })
        })
        .take(64)
        .collect();

    DeterminismCheckResult {
        num_pixels,
        num_samples,
        mismatches,
    }
}
