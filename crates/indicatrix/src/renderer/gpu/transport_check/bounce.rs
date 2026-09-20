//! Frosted-bounce and cosine-hemisphere: `cosine_weighted_hemisphere` (the frosted-facet
//! direction sampler) and `apply_frosted_bounce` (the full frosted-facet bounce
//! dispatch).

use crate::{
    optics::{
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{
            BounceRefractionGeometry,
            EnvironmentSource,
            LightingPreset,
            RayMaterialContext,
            apply_frosted_bounce,
            build_plane_soa,
            cosine_weighted_hemisphere,
            // `NeeContext` stays `pub(crate)` inside `scattering` rather than being added
            // to `raytracer::mod`'s re-export hub (a file this task does not own) --
            // reached here via its own module's full path instead.
            scattering::NeeContext,
        },
    },
    renderer::gpu::compute,
};
use glam::Vec3;

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::cosine_weighted_hemisphere -- the frosted-bounce
// direction sampler (Malley's method). Compared against calling the REAL CPU function
// directly (now `pub(crate)` -- see that function's own doc comment).
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CosineHemisphereCase {
    n: [f32; 3],
    u1: f32,
    u2: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

/// ULP budget for `cosine_weighted_hemisphere` (one `sqrt`, one `sin_cos` pair, an
/// orthonormal-basis construction, and a final `normalize_or_zero`) -- comparable
/// single-function budget to this file's other geometric-construction checks
/// (`EIGEN_POLARIZATION_ULP_BUDGET`, `SIGNED_PSI_ULP_BUDGET`, both 32).
const COSINE_HEMISPHERE_ULP_BUDGET: u32 = 48;
/// See `crate::renderer::gpu::ulp::within_tolerance`'s doc comment for the general
/// rationale: a hemisphere direction's individual x/y/z components legitimately cross
/// zero (e.g. `n` axis-aligned with a component of the sampled direction landing near
/// zero purely from `theta`'s position on the unit circle), where ULP distance alone is
/// a poor metric.
const COSINE_HEMISPHERE_ABS_FLOOR: f32 = 1e-5;

fn build_cosine_hemisphere_cases() -> Vec<CosineHemisphereCase> {
    let mut cases = Vec::new();
    let dirs = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(0.3, 0.9, 0.1).normalize(),
        Vec3::new(-0.95, 0.1, 0.05).normalize(),
        Vec3::new(0.9, 0.3, 0.1).normalize(),
    ];
    let steps = 12;
    for &n in &dirs {
        for i in 0..=steps {
            // Keep u1 strictly inside (0, 1) for the dense sweep -- the adversarial
            // block below covers the boundary (pole/equator) deliberately.
            let u1 = (i as f32 / steps as f32).mul_add(0.998, 0.001);
            for j in 0..=steps {
                let u2 = j as f32 / steps as f32;
                cases.push(CosineHemisphereCase {
                    n: n.to_array(),
                    u1,
                    u2,
                    _pad0: 0.0,
                    _pad1: 0.0,
                    _pad2: 0.0,
                });
            }
        }
    }
    // Adversarial: the hemisphere pole (u1 -> 0, r -> 0), the hemisphere BOUNDARY/
    // equator (u1 -> 1, the (1-u1).sqrt() term -> 0 -- explicitly called out in the
    // task brief as a required adversarial point), and theta wraparound (u2 near 0 and
    // near 1, where sin_cos(2*PI*u2) crosses the branch cut).
    let adversarial_u = [
        (0.0f32, 0.0f32),
        (0.0, 0.5),
        (1.0 - 1e-6, 0.0),
        (1.0 - 1e-6, 0.999_999),
        (0.5, 0.0),
        (0.5, 1.0 - 1e-6),
        (1e-7, 0.5),
        (0.999_999, 0.5),
        (1.0, 0.25),
    ];
    for &n in &dirs {
        for &(u1, u2) in &adversarial_u {
            cases.push(CosineHemisphereCase {
                n: n.to_array(),
                u1,
                u2,
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            });
        }
    }
    // Adversarial: n.x straddling +-0.9, `frosted_orthonormal_basis`'s own branch
    // boundary for which axis it picks as the seed vector `a`.
    for &nx in &[0.900_000_1f32, 0.899_999_9, 0.9, -0.900_000_1, -0.899_999_9] {
        let n = Vec3::new(nx, f32::mul_add(nx, -nx, 1.0).max(0.0).sqrt(), 0.0).normalize();
        cases.push(CosineHemisphereCase {
            n: n.to_array(),
            u1: 0.37,
            u2: 0.61,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        });
    }
    cases
}

fn cpu_cosine_hemisphere(c: &CosineHemisphereCase) -> [f32; 3] {
    cosine_weighted_hemisphere(c.u1, c.u2, Vec3::from_array(c.n)).to_array()
}

#[must_use]
pub fn run_cosine_hemisphere(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<CosineHemisphereCase> {
    let cases = build_cosine_hemisphere_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "cosine hemisphere in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "cosine hemisphere out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "cosine_hemisphere_main",
        SHADER_SRC,
        "cosine_hemisphere_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "cosine hemisphere bind group",
        &pipeline,
        &[(26, &in_buf), (27, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut acc = UlpAccumulator::new(
        "cosine_weighted_hemisphere",
        COSINE_HEMISPHERE_ULP_BUDGET,
        COSINE_HEMISPHERE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_cosine_hemisphere(case);
        for (c_idx, comp) in ["x", "y", "z"].iter().enumerate() {
            acc.record(case, comp, cpu[c_idx], gpu_out[idx * 3 + c_idx]);
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// GPU port: optics::raytracer::apply_frosted_bounce -- the full frosted-facet
// bounce dispatch (TIR-forced / reflect / transmit branch selection, the broadband
// hero-only r_unpol split, Stokes depolarization, path_pdf scaling). Compared against
// calling the REAL CPU function directly (now `pub(crate)`), never a reimplementation --
// see that function's own doc comment for the achromatic-by-design physics this pins.
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrostedBounceCase {
    is_anisotropic: u32,
    sin2_t: f32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    inside_gem: u32,
    is_extraordinary: u32,
    rng_seed: u32,
    bounce: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    normal: [f32; 3],
    _pad3: f32,
    stokes_in: [[f32; 4]; 8],
    path_pdf_in: [f32; 8],
}

const _: () = assert!(size_of::<FrostedBounceCase>() == 224);

/// ULP budget for the full `apply_frosted_bounce` dispatch: an `fma`-built Fresnel
/// `r_s`/`r_p`/`r_unpol` triple (the same shape already exercised, at the same 16-ULP
/// budget, by `FRESNEL_REFLECTION_ULP_BUDGET`/`FRESNEL_TRANSMISSION_ULP_BUDGET` above),
/// plus a `cosine_weighted_hemisphere` call (its own budget: `COSINE_HEMISPHERE_ULP_BUDGET`
/// = 48) and a handful of divisions accumulating into `stokes`/`path_pdf`. Set to the
/// larger of the two constituent budgets plus headroom for the additional divisions,
/// matching this file's existing convention of leaving headroom above the constituent
/// pieces rather than pinning to their sum exactly.
const FROSTED_BOUNCE_ULP_BUDGET: u32 = 64;
const FROSTED_BOUNCE_ABS_FLOOR: f32 = 1e-5;

/// Distinct-per-channel `stokes`/`path_pdf` test data, shared by [`build_frosted_bounce_dense_cases`]
/// and [`build_frosted_bounce_adversarial_cases`] -- a cross-channel indexing bug (e.g. a
/// swapped loop index) cannot hide behind every channel carrying the same input.
fn frosted_bounce_varied_inputs() -> ([[f32; 4]; 8], [f32; 8]) {
    let varied_stokes: [[f32; 4]; 8] = std::array::from_fn(|k| {
        [
            (k as f32).mul_add(0.1, 1.0),
            0.05 * k as f32,
            -0.03 * k as f32,
            0.02 * k as f32,
        ]
    });
    let varied_pdf: [f32; 8] = std::array::from_fn(|k| 0.05f32.mul_add(k as f32, 0.5));
    (varied_stokes, varied_pdf)
}

/// Dense sweep: index ratios spanning real gem/air interfaces (both entering and
/// exiting), incidence angle from grazing to normal, both `inside_gem` states, both
/// `is_anisotropic` states, both `is_extraordinary` states, and several
/// (`rng_seed`, `bounce`) pairs so both the reflect and transmit RNG branches are naturally
/// exercised across the bank. Split out of [`build_frosted_bounce_cases`] purely to keep
/// that function (and this one) under clippy's function-length lint.
fn build_frosted_bounce_dense_cases(
    uniform_stokes: [[f32; 4]; 8],
    uniform_pdf: [f32; 8],
) -> Vec<FrostedBounceCase> {
    let mut cases = Vec::new();
    let n_pairs = [
        (1.0f32, 1.5f32),
        (1.5, 1.0),
        (1.0, 1.77),
        (1.77, 1.0),
        (1.0, 2.42),
    ];
    let cos_vals = [0.0f32, 0.02, 0.05, 0.2, 0.5, 0.8, 0.95, 0.999];
    let seeds_bounces = [
        (1u32, 0u32),
        (7, 1),
        (99, 2),
        (12345, 3),
        (0xDEAD_BEEFu32, 0),
    ];
    for &(n1, n2) in &n_pairs {
        for &cos_i in &cos_vals {
            let eta = n1 / n2;
            let sin2_t = eta * eta * f32::mul_add(cos_i, -cos_i, 1.0);
            for inside_gem in [false, true] {
                for is_anisotropic in [false, true] {
                    for is_extraordinary in [false, true] {
                        for &(rng_seed, bounce) in &seeds_bounces {
                            cases.push(FrostedBounceCase {
                                is_anisotropic: u32::from(is_anisotropic),
                                sin2_t,
                                n1,
                                n2,
                                cos_i,
                                inside_gem: u32::from(inside_gem),
                                is_extraordinary: u32::from(is_extraordinary),
                                rng_seed,
                                bounce,
                                _pad0: 0,
                                _pad1: 0,
                                _pad2: 0,
                                normal: Vec3::Y.to_array(),
                                _pad3: 0.0,
                                stokes_in: uniform_stokes,
                                path_pdf_in: uniform_pdf,
                            });
                        }
                    }
                }
            }
        }
    }
    cases
}

/// Adversarial cases: the TIR-forced / partial-transmit boundary (`sin2_t` straddling
/// exactly 1.0 -- the "hemisphere boundary" the task brief calls out), grazing incidence
/// (`cos_i` essentially 0), normal incidence (`cos_i` essentially 1), and a
/// zero-intensity edge case (`StokesVector::intensity()`'s own `.max(0.0)` clamp). Split
/// out of [`build_frosted_bounce_cases`] for the same function-length reason as
/// [`build_frosted_bounce_dense_cases`].
fn build_frosted_bounce_adversarial_cases(
    uniform_stokes: [[f32; 4]; 8],
    uniform_pdf: [f32; 8],
    varied_stokes: [[f32; 4]; 8],
    varied_pdf: [f32; 8],
) -> Vec<FrostedBounceCase> {
    let mut cases = Vec::new();
    let adversarial_sin2_t = [0.999_9f32, 0.999_99, 1.0, 1.000_01, 1.0001, 1.5, 5.0];
    for &sin2_t in &adversarial_sin2_t {
        for &(stokes_in, path_pdf_in) in
            &[(uniform_stokes, uniform_pdf), (varied_stokes, varied_pdf)]
        {
            cases.push(FrostedBounceCase {
                is_anisotropic: 1,
                sin2_t,
                n1: 1.5,
                n2: 1.0,
                cos_i: 0.6,
                inside_gem: 1,
                is_extraordinary: 0,
                rng_seed: 42,
                bounce: 0,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
                normal: Vec3::Y.to_array(),
                _pad3: 0.0,
                stokes_in,
                path_pdf_in,
            });
        }
    }
    for &cos_i in &[0.0f32, 1e-6, 1e-4, 1.0 - 1e-6, 0.999_999] {
        let eta = 1.0f32 / 1.5;
        let sin2_t = eta * eta * f32::mul_add(cos_i, -cos_i, 1.0);
        cases.push(FrostedBounceCase {
            is_anisotropic: 0,
            sin2_t,
            n1: 1.0,
            n2: 1.5,
            cos_i,
            inside_gem: 0,
            is_extraordinary: 0,
            rng_seed: 314,
            bounce: 2,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            normal: Vec3::Y.to_array(),
            _pad3: 0.0,
            stokes_in: varied_stokes,
            path_pdf_in: varied_pdf,
        });
    }
    cases.push(FrostedBounceCase {
        is_anisotropic: 0,
        sin2_t: 0.3,
        n1: 1.5,
        n2: 1.0,
        cos_i: 0.7,
        inside_gem: 1,
        is_extraordinary: 0,
        rng_seed: 7,
        bounce: 1,
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
        normal: Vec3::Y.to_array(),
        _pad3: 0.0,
        stokes_in: [[0.0, 0.0, 0.0, 0.0]; 8],
        path_pdf_in: uniform_pdf,
    });
    cases
}

fn build_frosted_bounce_cases() -> Vec<FrostedBounceCase> {
    let uniform_stokes = [[1.0f32, 0.0, 0.0, 0.0]; 8];
    let uniform_pdf = [1.0f32; 8];
    let (varied_stokes, varied_pdf) = frosted_bounce_varied_inputs();

    let mut cases = build_frosted_bounce_dense_cases(uniform_stokes, uniform_pdf);
    cases.extend(build_frosted_bounce_adversarial_cases(
        uniform_stokes,
        uniform_pdf,
        varied_stokes,
        varied_pdf,
    ));
    cases
}

/// `(new_dir, new_inside_gem, has_extraordinary_update, extraordinary_update,
/// stokes_out, path_pdf_out)`, encoded the same way the WGSL kernel's flat output
/// buffer is -- see [`cpu_frosted_bounce`]'s call site for how these fields are compared.
type FrostedBounceCpuResult = ([f32; 3], u32, u32, u32, [[f32; 4]; 8], [f32; 8]);

fn cpu_frosted_bounce(case: &FrostedBounceCase, material: &GemMaterial) -> FrostedBounceCpuResult {
    let geo = BounceRefractionGeometry {
        cos_i: case.cos_i,
        n1: case.n1,
        n2: case.n2,
        sin2_t: case.sin2_t,
        ..Default::default()
    };
    let mat_ctx = RayMaterialContext {
        material,
        lambdas: [550.0; 8],
        hero_idx: 0,
        c_axis: Vec3::Y,
        is_anisotropic: case.is_anisotropic != 0,
        enable_internal_mode_coupling: true,
    };
    let mut stokes: [StokesVector; 8] = std::array::from_fn(|k| {
        let s = case.stokes_in[k];
        StokesVector::new(s[0], s[1], s[2], s[3])
    });
    let mut path_pdf = case.path_pdf_in;
    let normal = Vec3::from_array(case.normal);
    // This check pins `apply_frosted_bounce`'s base reflect/transmit/TIR physics against
    // the WGSL port, which predates finding G8's NEE addition -- `enabled: false` keeps
    // this comparison exactly as it was (no RNG draw, no `radiance` touch; see
    // `NeeContext::enabled`'s doc comment), leaving a dedicated frosted-NEE Tier 2/3
    // check as future work once the WGSL side ports that machinery too.
    let plane_soa = build_plane_soa(&[]);
    let nee = NeeContext {
        environment: EnvironmentSource::Studio {
            preset: LightingPreset::Daylight,
            exposure: 1.0,
            light_yaw: 0.0,
            light_pitch: 0.0,
            backdrop: 0.0,
        },
        plane_soa: &plane_soa,
        enabled: false,
    };
    let mut radiance = [0.0f32; 8];
    let (new_dir, new_inside_gem, extraordinary_update, _pending_light_mis) = apply_frosted_bounce(
        &mat_ctx,
        &geo,
        normal,
        case.inside_gem != 0,
        case.is_extraordinary != 0,
        case.rng_seed,
        case.bounce,
        &mut stokes,
        &mut path_pdf,
        nee,
        &mat_ctx.lambdas,
        &mut radiance,
    );
    let stokes_out: [[f32; 4]; 8] = std::array::from_fn(|k| stokes[k].to_vec4().to_array());
    (
        new_dir.to_array(),
        u32::from(new_inside_gem),
        u32::from(extraordinary_update.is_some()),
        u32::from(extraordinary_update.unwrap_or(false)),
        stokes_out,
        path_pdf,
    )
}

#[must_use]
pub fn run_frosted_bounce(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<FrostedBounceCase> {
    let material = GemMaterial::new_custom("frosted_bounce test", 1.8, 0.02, 0.059, [0.0; 3]);
    let cases = build_frosted_bounce_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "frosted bounce in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "frosted bounce out",
        total * 46,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "frosted_bounce_main",
        SHADER_SRC,
        "frosted_bounce_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "frosted bounce bind group",
        &pipeline,
        &[(28, &in_buf), (29, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 46);

    let mut acc = UlpAccumulator::new(
        "apply_frosted_bounce",
        FROSTED_BOUNCE_ULP_BUDGET,
        FROSTED_BOUNCE_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let (cpu_dir, cpu_inside, cpu_has_ex, cpu_ex, cpu_stokes, cpu_pdf) =
            cpu_frosted_bounce(case, &material);
        let base = idx * 46;
        for (c_idx, comp) in ["dir_x", "dir_y", "dir_z"].iter().enumerate() {
            acc.record(case, comp, cpu_dir[c_idx], gpu_out[base + c_idx]);
        }
        acc.record(case, "new_inside_gem", cpu_inside as f32, gpu_out[base + 3]);
        acc.record(
            case,
            "has_extraordinary_update",
            cpu_has_ex as f32,
            gpu_out[base + 4],
        );
        acc.record(
            case,
            "extraordinary_update",
            cpu_ex as f32,
            gpu_out[base + 5],
        );
        for k in 0..8 {
            for (c_idx, comp) in ["i", "q", "u", "v"].iter().enumerate() {
                acc.record(
                    case,
                    comp,
                    cpu_stokes[k][c_idx],
                    gpu_out[base + 6 + k * 4 + c_idx],
                );
            }
        }
        for k in 0..8 {
            acc.record(case, "path_pdf", cpu_pdf[k], gpu_out[base + 38 + k]);
        }
    }
    acc.finish()
}
