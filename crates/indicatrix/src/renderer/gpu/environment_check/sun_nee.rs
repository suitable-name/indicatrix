//! Tier 2 check of the analytic sun's next-event-estimation functions: the WGSL
//! `daylight_sun_cone_direction`, `daylight_sun_nee_pdf` and `daylight_sun_factor` of
//! `shaders/environment.wgsl` (textually pinned equal to the megakernel's copies by
//! `shader_validation_tests`) against the CPU `rig::sun_cone_direction`, `rig::sun_nee_pdf`
//! and `rig::sun_radiance_factor`, dispatched by [`run_sun_nee`].
//!
//! Compared per case: the three components of the drawn direction, the pdf at the draw (the
//! technique's own density, `1 / solid angle` inside the disc), the pdf at an independent
//! probe direction (the other half of the MIS weight: `1 / solid angle` inside, exactly `0`
//! outside) and the sun's radiance factor. The probe offsets and the draws avoid the disc
//! edge itself, where the `dot >= cos` test may legitimately flip between CPU and GPU
//! rounding.

use glam::Vec3;

use crate::{
    optics::raytracer::{
        sun_nee_reference_factor, sun_nee_reference_pdf, sun_nee_reference_sample,
    },
    renderer::gpu::compute,
};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

/// One input case: a key pose, the two uniform randoms of the draw, and a probe direction
/// whose pdf is looked up independently of the draw.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SunNeeCase {
    key_yaw: f32,
    key_pitch: f32,
    u0: f32,
    u1: f32,
    probe: [f32; 3],
    _pad0: f32,
}

const _: () = assert!(size_of::<SunNeeCase>() == 32);

/// ULP budget for the sun NEE draw comparison.
///
/// The draw chains `sin`/`cos` (the azimuth, and the key direction's own pose
/// trig) and a `sqrt`; the pdfs are a constant or exactly zero. See
/// [`super::STUDIO_ENV_ULP_BUDGET`]'s doc comment for the calibration philosophy.
pub const SUN_NEE_ULP_BUDGET: u32 = 256;

/// Absolute-difference floor: direction components near zero carry rounding noise of
/// a few `1e-8` that is not a ULP-meaningful disagreement.
pub const SUN_NEE_ABS_FLOOR: f32 = 1e-6;

/// Builds the sun NEE cases: four poses, a spread of draws (the radial coordinate stays
/// at or below 0.9 so a draw never sits on the disc edge) and probes inside and outside the
/// disc.
#[must_use]
pub fn build_sun_nee_cases() -> Vec<SunNeeCase> {
    let poses = [(0.3f32, 0.6f32), (0.85, 0.95), (-0.5, 1.2), (0.0, 1.5)];
    let u0s = [0.0f32, 0.05, 0.25, 0.5, 0.75, 0.9];
    let u1s = [0.0f32, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9];
    let probe_degrees = [0.0f32, 0.1, 0.2, 0.35, 0.6, 3.0, 90.0, 180.0];
    let mut cases = Vec::new();
    for (key_yaw, key_pitch) in poses {
        let key_dir = crate::optics::studio_rig::StudioRig::new(key_yaw, key_pitch).key_dir;
        let perp = key_dir.any_orthonormal_vector();
        for &u0 in &u0s {
            for &u1 in &u1s {
                for &degrees in &probe_degrees {
                    let (sin_a, cos_a) = degrees.to_radians().sin_cos();
                    let probe: Vec3 = (key_dir * cos_a + perp * sin_a).normalize();
                    cases.push(SunNeeCase {
                        key_yaw,
                        key_pitch,
                        u0,
                        u1,
                        probe: probe.to_array(),
                        _pad0: 0.0,
                    });
                }
            }
        }
    }
    cases
}

/// Runs the sun NEE ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_sun_nee(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<SunNeeCase> {
    let cases = build_sun_nee_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "sun_nee cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "sun_nee out",
        total * 8,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "sun_nee_main", SHADER_SRC, "sun_nee_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "sun_nee bind group",
        &pipeline,
        &[(12, &in_buf), (13, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 8);

    let mut acc = UlpAccumulator::new("daylight_sun_nee", SUN_NEE_ULP_BUDGET, SUN_NEE_ABS_FLOOR);
    for (idx, case) in cases.iter().enumerate() {
        let key_dir =
            crate::optics::studio_rig::StudioRig::new(case.key_yaw, case.key_pitch).key_dir;
        let (dir, pdf) = sun_nee_reference_sample(key_dir, case.u0, case.u1);
        let probe_pdf = sun_nee_reference_pdf(key_dir, Vec3::from_array(case.probe));
        let factor = sun_nee_reference_factor(key_dir);
        let gpu = &gpu_out[idx * 8..idx * 8 + 8];
        acc.record(case, "dir.x", dir.x, gpu[0]);
        acc.record(case, "dir.y", dir.y, gpu[1]);
        acc.record(case, "dir.z", dir.z, gpu[2]);
        acc.record(case, "pdf at the draw", pdf, gpu[3]);
        acc.record(case, "pdf at the probe", probe_pdf, gpu[4]);
        acc.record(case, "sun factor", factor, gpu[5]);
    }
    acc.finish()
}
