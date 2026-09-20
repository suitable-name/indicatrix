//! The mandatory GPU struct-layout self-test.
//!
//! Uploads a populated [`GpuGemMaterial`], has `shaders/layout_echo.wgsl` echo every
//! field through to an independent output buffer, and compares the two buffers' raw
//! bytes.
//!
//! See `renderer::buffers`' module doc comment for why this exists: a hand-derived
//! `#[repr(C)]` offset can look right and still disagree with what WGSL actually
//! computes (this is exactly the bug [`DispersionParams`](crate::renderer::buffers::DispersionParams)
//! had). This test is the mechanical, permanent guard against that bug class -- it does
//! not trust the `offset_of!` comments in `renderer::buffers`, it proves them against
//! the GPU itself.

use crate::{
    geometry::GpuFacetPlane,
    renderer::{
        buffers::{
            DispersionParams, GpuAbsorptionBand, GpuCameraParams, GpuGemMaterial, GpuHitRecord,
            GpuRay, GpuTransportParams, MAX_ABSORPTION_BANDS,
        },
        gpu::compute,
    },
};

const SHADER_SRC: &str = include_str!("../shaders/layout_echo.wgsl");
const PHASE1_SHADER_SRC: &str = include_str!("../shaders/phase1_layout_echo.wgsl");
const PHASE2_SHADER_SRC: &str = include_str!("../shaders/phase2_layout_echo.wgsl");

/// One byte-level disagreement between the input and echoed-output buffers.
#[derive(Debug, Clone, Copy)]
pub struct ByteMismatch {
    pub offset: usize,
    pub expected: u8,
    pub actual: u8,
}

/// Result of [`run`]: either every byte echoed back exactly, or the first N mismatches
/// (capped so a total layout mismatch doesn't dump 320 individual diagnostics).
#[derive(Debug, Clone)]
pub struct LayoutCheckResult {
    pub input_bytes: Vec<u8>,
    pub output_bytes: Vec<u8>,
    pub mismatches: Vec<ByteMismatch>,
}

impl LayoutCheckResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// Builds a [`GpuGemMaterial`] with distinct, non-zero, non-symmetric values in every
/// field.
///
/// Distinct values are essential here: if two fields held the same value, a
/// bug that swapped their offsets would echo back byte-identical, defeating the whole
/// point of the test. Padding fields are left zeroed (via
/// [`bytemuck::Zeroable::zeroed`]) and never touched again, so any padding byte that
/// comes back non-zero after the echo is itself evidence of a layout bug (a real field
/// landed where padding should be).
#[must_use]
pub fn sample_material() -> GpuGemMaterial {
    let mut m = GpuGemMaterial::zeroed_material();
    m.dispersion.model_type = crate::renderer::buffers::dispersion_model_type::SELLMEIER3;
    m.dispersion.param_a = [1.431_349, 0.650_547, 5.341_402, 0.0];
    m.dispersion.param_b = [0.00528, 0.01424, 325.015, 0.0];
    m.dispersion.param_c = [11.0, 12.0, 13.0, 14.0];
    m.dispersion.c_axis_and_birefringence = [0.1, 0.2, 0.3, -0.0081];
    m.dispersion.is_anisotropic = 1;
    m.dispersion.biaxial_delta_beta_alpha = 0.001_951;
    m.dispersion.has_biaxial_delta = 1;

    m.crystal_system = crate::renderer::buffers::crystal_system::TRIGONAL;
    m.optical_character = crate::renderer::buffers::optical_character::UNIAXIAL_NEGATIVE;
    m.is_pleochroic = 1;
    m.o_ray_band_count = 2;
    m.e_ray_band_count = 3;

    for (i, band) in m.o_ray_bands.iter_mut().enumerate() {
        *band = GpuAbsorptionBand {
            center_nm: 400.0 + i as f32,
            width_nm: 20.0 + i as f32,
            peak: (i as f32).mul_add(0.1, 1.0),
            // Alternating discriminant: proves the echo round-trips BOTH `shape`
            // values, not just whichever one happens to be the zeroed default.
            shape: (i % 2) as u32,
        };
    }
    for (i, band) in m.e_ray_bands.iter_mut().enumerate() {
        *band = GpuAbsorptionBand {
            center_nm: 500.0 + i as f32,
            width_nm: 30.0 + i as f32,
            peak: (i as f32).mul_add(0.1, 2.0),
            shape: ((i + 1) % 2) as u32,
        };
    }
    // Distinct, non-zero, non-symmetric values for the new
    // scattering fields, same rationale as every other field here.
    m.scattering_sigma_s = 0.734;
    m.scattering_g = 0.417;
    m.edge_rounding_radius = 0.0512;

    // Distinct, non-zero, non-symmetric values for the beta-ray fields, same rationale
    // as every other field here.
    m.has_beta_ray = 1;
    m.beta_ray_band_count = 2;
    for (i, band) in m.beta_ray_bands.iter_mut().enumerate() {
        *band = GpuAbsorptionBand {
            center_nm: 600.0 + i as f32,
            width_nm: 40.0 + i as f32,
            peak: (i as f32).mul_add(0.1, 3.0),
            shape: (i % 2) as u32,
        };
    }

    // A distinct, non-zero, non-1.0 value -- same rationale as every other field here
    // (and specifically != 1.0, since 1.0 is this field's semantically-meaningful
    // default and would not distinguish "echoed correctly" from "defaulted to
    // zeroed-then-reset").
    m.absorption_path_scale = 2.375;

    // Distinct, non-zero, non-symmetric values for the extraordinary-dispersion fields,
    // same rationale as every other field here.
    m.has_extraordinary_dispersion = 1;
    m.extraordinary_model_type = crate::renderer::buffers::dispersion_model_type::SELLMEIER3;
    m.extraordinary_param_a = [0.288_518, 1.095_099, 1.156_625, 0.0];
    m.extraordinary_param_b = [0.0, 0.010_210, 100.0, 0.0];
    m
}

/// Trivial helper kept local to this module: `GpuGemMaterial` is `bytemuck::Zeroable`,
/// but spelling `<GpuGemMaterial as bytemuck::Zeroable>::zeroed()` at every call site
/// above would be noisier than this one-line wrapper.
trait ZeroedMaterial {
    fn zeroed_material() -> Self;
}
impl ZeroedMaterial for GpuGemMaterial {
    fn zeroed_material() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// Runs the struct-echo self-test against a live GPU.
///
/// # Panics
///
/// Panics on any `wgpu` API misuse (shader compile failure, bind group mismatch) --
/// these would be bugs in this crate's shader/Rust struct correspondence at the
/// WGSL-syntax level, not a condition this function needs to recover from gracefully
/// (unlike "no GPU available", which is
/// [`crate::renderer::gpu::GpuContext::acquire`]'s job to report cleanly before this
/// function is ever called).
#[must_use]
pub fn run(ctx: &crate::renderer::gpu::GpuContext) -> LayoutCheckResult {
    let material = sample_material();
    let input_bytes = bytemuck::bytes_of(&material).to_vec();

    let pipeline = compute::create_compute_pipeline(&ctx.device, "layout_echo", SHADER_SRC, "main");

    let input_buf = compute::upload(
        &ctx.device,
        "layout_echo input",
        std::slice::from_ref(&material),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let output_buf = compute::zeroed_buffer::<GpuGemMaterial>(
        &ctx.device,
        "layout_echo output",
        1,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
    );

    let bind_group = compute::bind_buffers(
        &ctx.device,
        "layout_echo bind group",
        &pipeline,
        &[(0, &input_buf), (1, &output_buf)],
    );

    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));

    let output: Vec<GpuGemMaterial> = compute::readback(&ctx.device, &ctx.queue, &output_buf, 1);
    let output_bytes = bytemuck::bytes_of(&output[0]).to_vec();

    let mismatches = diff_bytes(&input_bytes, &output_bytes);

    LayoutCheckResult {
        input_bytes,
        output_bytes,
        mismatches,
    }
}

/// Shared byte-diff step for every echo test in this module (the original
/// [`GpuGemMaterial`] one and the four Phase-1 ones below): first 32 mismatching byte
/// offsets, capped so a total layout mismatch doesn't dump hundreds of individual
/// diagnostics.
fn diff_bytes(input_bytes: &[u8], output_bytes: &[u8]) -> Vec<ByteMismatch> {
    input_bytes
        .iter()
        .zip(output_bytes.iter())
        .enumerate()
        .filter_map(|(offset, (&expected, &actual))| {
            (expected != actual).then_some(ByteMismatch {
                offset,
                expected,
                actual,
            })
        })
        .take(32)
        .collect()
}

/// Generic single-struct echo test, shared by the four Phase-1 struct-layout self-tests
/// below ([`run_facet_plane`], [`run_camera_params`], [`run_ray`],
/// [`run_hit_record`]). Uploads `sample` to a `read`-only storage binding, dispatches
/// `entry_point` from `PHASE1_SHADER_SRC` (which copies every named field into a
/// separate `read_write` storage binding), and diffs the raw bytes -- exactly
/// [`run`]'s mechanism, generalized over `T` since all four Phase-1 structs share the
/// same "one instance, one dispatch, compare bytes" shape.
///
/// # Panics
///
/// See [`run`]'s doc comment for the same rationale (a `wgpu` API-misuse panic here is
/// a bug in this crate's shader/Rust correspondence, not a runtime condition to recover
/// from).
fn run_echo<T: bytemuck::Pod + bytemuck::Zeroable>(
    ctx: &crate::renderer::gpu::GpuContext,
    entry_point: &str,
    in_binding: u32,
    out_binding: u32,
    sample: T,
) -> LayoutCheckResult {
    let input_bytes = bytemuck::bytes_of(&sample).to_vec();

    let pipeline =
        compute::create_compute_pipeline(&ctx.device, entry_point, PHASE1_SHADER_SRC, entry_point);

    let input_buf = compute::upload(
        &ctx.device,
        "phase1 layout_echo input",
        std::slice::from_ref(&sample),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let output_buf = compute::zeroed_buffer::<T>(
        &ctx.device,
        "phase1 layout_echo output",
        1,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
    );

    // Each of the four Phase-1 structs occupies its own fixed pair of binding slots in
    // `PHASE1_SHADER_SRC` (see that file's own doc comment) -- `wgpu`'s auto-inferred
    // bind group layout (`layout: None`) is scoped to exactly the bindings the chosen
    // `entry_point` references, so the bind group here must name the SAME slots that
    // entry point declared, not always (0, 1).
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "phase1 layout_echo bind group",
        &pipeline,
        &[(in_binding, &input_buf), (out_binding, &output_buf)],
    );

    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));

    let output: Vec<T> = compute::readback(&ctx.device, &ctx.queue, &output_buf, 1);
    let output_bytes = bytemuck::bytes_of(&output[0]).to_vec();
    let mismatches = diff_bytes(&input_bytes, &output_bytes);

    LayoutCheckResult {
        input_bytes,
        output_bytes,
        mismatches,
    }
}

/// Runs the Phase-1 [`GpuFacetPlane`] struct-echo self-test (`shaders/
/// phase1_layout_echo.wgsl`'s `echo_plane` entry point, bindings 0/1). See [`run_echo`].
#[must_use]
pub fn run_facet_plane(ctx: &crate::renderer::gpu::GpuContext) -> LayoutCheckResult {
    let sample = GpuFacetPlane {
        normal: [0.267_261, 0.534_522, 0.801_784],
        d: -0.732_051,
    };
    run_echo(ctx, "echo_plane", 0, 1, sample)
}

/// Runs the Phase-1 [`GpuCameraParams`] struct-echo self-test (`echo_camera` entry
/// point, bindings 2/3). See [`run_echo`].
#[must_use]
pub fn run_camera_params(ctx: &crate::renderer::gpu::GpuContext) -> LayoutCheckResult {
    let sample = GpuCameraParams {
        origin: [1.1, 2.2, 3.3],
        fov_tan: 0.4663,
        forward: [4.4, 5.5, 6.6],
        width: 800.0,
        right: [7.7, 8.8, 9.9],
        height: 600.0,
        up: [10.1, 11.2, 12.3],
        num_samples: 64,
    };
    run_echo(ctx, "echo_camera", 2, 3, sample)
}

/// Runs the Phase-1 [`GpuRay`] struct-echo self-test (`echo_ray` entry point, bindings
/// 4/5). See [`run_echo`].
#[must_use]
pub fn run_ray(ctx: &crate::renderer::gpu::GpuContext) -> LayoutCheckResult {
    let sample = GpuRay::new([0.1, 0.2, 0.3], [0.4, 0.5, 0.6]);
    run_echo(ctx, "echo_ray", 4, 5, sample)
}

/// Runs the Phase-1 [`GpuHitRecord`] struct-echo self-test (`echo_hit` entry point,
/// bindings 6/7). See [`run_echo`].
#[must_use]
pub fn run_hit_record(ctx: &crate::renderer::gpu::GpuContext) -> LayoutCheckResult {
    let sample = GpuHitRecord::hit(12.75, 23, [0.301_511, 0.492_386, 0.816_497]);
    run_echo(ctx, "echo_hit", 6, 7, sample)
}

/// GPU port (frosted girdle finish): the Tier 1 struct-layout echo test for
/// `renderer::buffers::facet_finish`'s `array<u32>` upload.
///
/// Uses `echo_facet_finishes` (bindings 8/9 of `PHASE1_SHADER_SRC`) -- see that
/// module's own doc comment for why a separate storage buffer, parallel to `planes`,
/// was chosen over widening `GpuFacetPlane`. A bare `u32` has no vec3/vec4 alignment
/// pitfall of its own (see this module's doc comment), but this still proves what the
/// GPU actually did with the uploaded bytes -- including that a MULTI-element
/// `array<u32>` storage buffer round-trips index-for-index, not just that a single
/// scalar does -- rather than trusting that by inspection alone. This is the exact kind
/// of self-test the `DispersionParams` bug this module's doc comment describes shows is
/// never safe to skip, even for a type this simple.
///
/// # Panics
///
/// See [`run`]'s doc comment for the same rationale.
#[must_use]
pub fn run_facet_finish(ctx: &crate::renderer::gpu::GpuContext) -> LayoutCheckResult {
    let sample: Vec<u32> = vec![0, 1, 0, 1, 1, 0, 1, 0];
    let input_bytes = bytemuck::cast_slice(&sample).to_vec();

    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "echo_facet_finishes",
        PHASE1_SHADER_SRC,
        "echo_facet_finishes",
    );
    let input_buf = compute::upload(
        &ctx.device,
        "facet finish layout_echo input",
        &sample,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let output_buf = compute::zeroed_buffer::<u32>(
        &ctx.device,
        "facet finish layout_echo output",
        sample.len(),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "facet finish layout_echo bind group",
        &pipeline,
        &[(8, &input_buf), (9, &output_buf)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));

    let output: Vec<u32> = compute::readback(&ctx.device, &ctx.queue, &output_buf, sample.len());
    let output_bytes = bytemuck::cast_slice(&output).to_vec();
    let mismatches = diff_bytes(&input_bytes, &output_bytes);

    LayoutCheckResult {
        input_bytes,
        output_bytes,
        mismatches,
    }
}

/// Runs the Phase-2 [`GpuTransportParams`] struct-echo self-test.
///
/// Uses `shaders/phase2_layout_echo.wgsl`'s `echo_transport_params` entry point,
/// bindings 0/1 of its OWN independent bind group -- a separate shader module from
/// `PHASE1_SHADER_SRC`, so [`run_echo`]'s hardcoded shader source can't be reused
/// directly here.
///
/// # Panics
///
/// See [`run`]'s doc comment for the same rationale.
#[must_use]
pub fn run_transport_params(ctx: &crate::renderer::gpu::GpuContext) -> LayoutCheckResult {
    let sample = GpuTransportParams::new(
        1234,
        9,
        4096,
        1,
        2.5,
        6500.0,
        1.6,
        1.1,
        0.4,
        0.35,
        [1.05, 1.0, 0.92],
    )
    // A distinct, non-zero value for `studio_use_d65`, same rationale as every other
    // field here.
    .with_studio_use_d65(true)
    // A distinct, non-zero value for `studio_model` (LightTent).
    .with_studio_model(crate::renderer::buffers::studio_model::LIGHT_TENT)
    // A distinct, non-zero backdrop, same rationale.
    .with_backdrop(0.23);
    let input_bytes = bytemuck::bytes_of(&sample).to_vec();

    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "echo_transport_params",
        PHASE2_SHADER_SRC,
        "echo_transport_params",
    );
    let input_buf = compute::upload(
        &ctx.device,
        "phase2 layout_echo input",
        std::slice::from_ref(&sample),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let output_buf = compute::zeroed_buffer::<GpuTransportParams>(
        &ctx.device,
        "phase2 layout_echo output",
        1,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "phase2 layout_echo bind group",
        &pipeline,
        &[(0, &input_buf), (1, &output_buf)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));

    let output: Vec<GpuTransportParams> =
        compute::readback(&ctx.device, &ctx.queue, &output_buf, 1);
    let output_bytes = bytemuck::bytes_of(&output[0]).to_vec();
    let mismatches = diff_bytes(&input_bytes, &output_bytes);

    LayoutCheckResult {
        input_bytes,
        output_bytes,
        mismatches,
    }
}

/// Human-readable field name for a byte offset within [`GpuGemMaterial`], for
/// diagnostics -- best-effort, only as granular as is useful to point a human at the
/// right struct definition.
#[must_use]
pub const fn field_name_at_offset(offset: usize) -> &'static str {
    match offset {
        0..=15 => "dispersion.model_type (+ implicit pad)",
        16..=31 => "dispersion.param_a",
        32..=47 => "dispersion.param_b",
        48..=63 => "dispersion.param_c",
        64..=79 => "dispersion.c_axis_and_birefringence",
        80..=83 => "dispersion.is_anisotropic",
        84..=87 => "dispersion.biaxial_delta_beta_alpha",
        88..=91 => "dispersion.has_biaxial_delta",
        92..=95 => "dispersion._pad_tail",
        96..=99 => "crystal_system",
        100..=103 => "optical_character",
        104..=107 => "is_pleochroic",
        108..=111 => "o_ray_band_count",
        112..=115 => "e_ray_band_count",
        116..=243 => "o_ray_bands[..]",
        244..=371 => "e_ray_bands[..]",
        372..=375 => "scattering_sigma_s",
        376..=379 => "scattering_g",
        380..=383 => "edge_rounding_radius",
        384..=387 => "has_beta_ray",
        388..=391 => "beta_ray_band_count",
        392..=519 => "beta_ray_bands[..]",
        520..=523 => "absorption_path_scale",
        524..=527 => "has_extraordinary_dispersion",
        528..=531 => "extraordinary_model_type",
        532..=543 => "_pad_before_extraordinary_params",
        544..=559 => "extraordinary_param_a",
        560..=575 => "extraordinary_param_b",
        _ => "out of range (GpuGemMaterial is 576 bytes)",
    }
}

const _: () = assert!(size_of::<DispersionParams>() == 96);
const _: () = assert!(MAX_ABSORPTION_BANDS == 8);
