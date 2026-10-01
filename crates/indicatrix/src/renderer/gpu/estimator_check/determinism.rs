//! Determinism: two dispatches of `transport_main` against byte-identical input.

use crate::{
    optics::raytracer::{
        LightingPreset, environment::environment_white_balance, illuminant_temperature_k,
    },
    renderer::buffers::{GpuGemMaterial, GpuTransportParams, transport_env_mode},
};

use super::{
    all_polished_finishes, camera_params_for, dispatch_transport, round_brilliant_planes,
    test_camera, tier3_material,
};

/// Outcome of the determinism check.
#[derive(Debug, Clone)]
pub struct DeterminismResult {
    /// Total values.
    pub total_values: usize,
    /// Mismatches found.
    pub mismatches: usize,
}

impl DeterminismResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.mismatches == 0
    }
}

/// Two dispatches of `transport_main` against byte-identical input.
///
/// Same material, planes, camera, and params both times -- per-thread pixel ownership
/// and no atomics means GPU scheduling order can never affect the result (see
/// `shaders/spectral_transport.wgsl`'s doc comment), so the two runs' `out_xyz` buffers
/// must be bit-for-bit identical.
#[must_use]
pub fn run_determinism(ctx: &crate::renderer::gpu::GpuContext) -> DeterminismResult {
    let camera = test_camera();
    let (width, height, samples) = (32u32, 32u32, 16u32);
    let planes = round_brilliant_planes();
    let material = tier3_material();
    let gpu_material = GpuGemMaterial::encode(&material);
    let camera_params = camera_params_for(&camera, width, height, samples);
    let temp_k = illuminant_temperature_k(LightingPreset::Daylight);
    // The production scale (identity for the D65 Daylight rig), exactly as the frame
    // path derives it from the studio environment.
    let wb = environment_white_balance(LightingPreset::Daylight.studio(1.0, 0.0, 0.0));
    let params = GpuTransportParams::new(
        width * height,
        10,
        0,
        transport_env_mode::STUDIO_RIG,
        0.0,
        temp_k,
        1.0,
        1.0,
        0.0,
        0.0,
        wb.to_array(),
    );
    let total = (width * height * samples) as usize;

    let finishes = all_polished_finishes(planes.len());
    let run1 = dispatch_transport(
        ctx,
        &camera_params,
        &params,
        &gpu_material,
        &planes,
        &finishes,
        total,
    );
    let run2 = dispatch_transport(
        ctx,
        &camera_params,
        &params,
        &gpu_material,
        &planes,
        &finishes,
        total,
    );

    let mismatches = run1
        .xyz
        .iter()
        .zip(run2.xyz.iter())
        .filter(|(a, b)| a.to_bits() != b.to_bits())
        .count();

    DeterminismResult {
        total_values: run1.xyz.len(),
        mismatches,
    }
}
