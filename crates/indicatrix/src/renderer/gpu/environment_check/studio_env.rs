//! `sample_studio_environment` ULP-budget self-test: [`build_studio_env_cases`]'s dense
//! direction/pose sweep plus adversarial points, dispatched by [`run_studio_env`].

use glam::Vec3;

use crate::{
    optics::raytracer::{LightingModel, LightingPreset, sample_studio_environment_observed},
    renderer::gpu::compute,
};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult, fibonacci_sphere};

/// One input case for the studio env check.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StudioEnvCase {
    dir: [f32; 3],
    lambda_nm: f32,
    temp_k: f32,
    spot_mult: f32,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    model: f32,
    _pad1: f32,
    /// Was `_pad2` -- nonzero for a case built from a preset where [`LightingPreset::uses_d65`]
    /// is true, mirroring
    /// `optics::raytracer::environment::sample_studio_environment_with_rig`'s own
    /// `preset.uses_d65()` branch. See
    /// [`build_studio_env_cases`].
    use_d65: f32,
    /// Unit direction towards the eye for the lit models' head shadow (`[0.0; 3]`
    /// disables it) -- mirrors `sample_studio_environment_observed`'s `observer`.
    observer: [f32; 3],
    _pad2: f32,
}

const _: () = assert!(size_of::<StudioEnvCase>() == 64);

/// ULP budget for `sample_studio_environment`.
///
/// The largest of the four Phase-1 environment budgets: it chains `sin`/`cos` (twice,
/// for `StudioRig`'s key/fill/ring directions), `blackbody_spectrum` itself,
/// `normalize`/`dot`/`cross`, and `powi_u(_, 28)`/`powi_u(_, 18)`/`powi_u(_, 6)` -- more
/// accumulated transcendental rounding than any other single Phase-1 function. See
/// [`super::CMF_ULP_BUDGET`]'s doc comment for the calibration philosophy.
///
/// # Measured amplification via `powi_u(_, 28)`
///
/// The FIRST measured run on this workspace's dev hardware found up to 1086 ULP, on
/// cases whose sampled direction sits very close to the key light's own axis (`key_dot`
/// near `1.0`). This is expected amplification, not a bug: `key_dot` itself is built
/// from `sin`/`cos`/`normalize`/`dot`, each contributing a handful of ULP of ordinary
/// driver-level rounding noise (the SAME 1-2 ULP floor `rng_check` measured per
/// operation); raising a value near `1.0` to the 28th power amplifies its RELATIVE
/// error by a factor of ~28 (`d(x^n)/x^n = n * dx/x`), which is exactly the multiplier
/// that turns a few-ULP `key_dot` disagreement into ~1000 ULP in the final radiance.
/// Set well above that measured figure with margin for a different driver.
pub const STUDIO_ENV_ULP_BUDGET: u32 = 8192;

/// Absolute-difference floor for `sample_studio_environment` comparisons.
///
/// See [`super::CMF_ABS_FLOOR`]'s doc comment for the rationale -- this covers the
/// ring-light spark threshold's `> 0.96` branch edge, where radiance can be arbitrarily
/// close to the ambient backdrop floor (`~0.005`-`0.03`) on one side of the threshold
/// and jump sharply on the other; a genuine algebra bug (a wrong lobe constant, a
/// dropped `spot_mult`/`exposure` factor, or a mis-set power exponent) moves radiance by
/// orders of magnitude more than this floor, as confirmed by this crate's
/// negative-control run.
pub const STUDIO_ENV_ABS_FLOOR: f32 = 1e-4;

/// Builds the studio env cases for the check.
#[must_use]
pub fn build_studio_env_cases() -> Vec<StudioEnvCase> {
    let mut cases = Vec::new();
    let directions = fibonacci_sphere(256);
    let poses = [(0.3f32, 0.6f32), (0.85, 0.95), (-0.5, 1.2)];
    let exposures = [0.5f32, 1.0, 2.0];
    let lambdas = [400.0f32, 500.0, 560.0, 650.0, 700.0];

    // The studio rig ignores the observer, so only the lit models get the second,
    // shadow-casting one (and the in-cone directions below).
    let lit_observer = Vec3::new(0.2, 0.9, -0.3).normalize();
    let no_observer = [Vec3::ZERO];
    let lit_observers = [Vec3::ZERO, lit_observer];

    // The UV lamps are CPU-only (`scene_routes_to_gpu`): the shader has no Gaussian lamp
    // spectrum, so there is nothing on the GPU to compare against.
    for preset in LightingPreset::ALL
        .into_iter()
        .filter(|preset| !preset.is_uv_lamp())
    {
        let params = preset.params();
        let use_d65 = f32::from(preset.uses_d65());
        let model = preset.model().gpu_id() as f32;
        let observers: &[Vec3] = if preset.model() == LightingModel::Studio {
            &no_observer
        } else {
            &lit_observers
        };
        for &observer in observers {
            for &(light_yaw, light_pitch) in &poses {
                for &exposure in &exposures {
                    for &dir in &directions {
                        for &lambda_nm in &lambdas {
                            cases.push(StudioEnvCase {
                                dir: dir.to_array(),
                                lambda_nm,
                                temp_k: params.temp_k,
                                spot_mult: params.spot_mult,
                                exposure,
                                light_yaw,
                                light_pitch,
                                model,
                                _pad1: 0.0,
                                use_d65,
                                observer: observer.to_array(),
                                _pad2: 0.0,
                            });
                        }
                    }
                }
            }
        }
        if preset.model() != LightingModel::Studio {
            push_head_shadow_cases(&mut cases, preset, lit_observer);
        }
    }

    // Adversarial: exactly on the key light's own axis (peak alignment, `key_dot ==
    // 1.0`), and exactly at the ring lights' `0.96` spark threshold on both sides.
    for &(light_yaw, light_pitch) in &poses {
        let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
        // `preset_temp(0)` is `LightingPreset::Daylight` (index 0 -- see
        // `LightingPreset::from_index`), so these cases need `use_d65 = 1.0` too.
        for dir in [rig.key_dir, rig.fill_dir].into_iter().chain(rig.ring_dirs) {
            cases.push(StudioEnvCase {
                dir: dir.to_array(),
                lambda_nm: 560.0,
                temp_k: preset_temp(0),
                spot_mult: 1.0,
                exposure: 1.0,
                light_yaw,
                light_pitch,
                model: 0.0,
                _pad1: 0.0,
                use_d65: 1.0,
                observer: [0.0; 3],
                _pad2: 0.0,
            });
        }
        // Just inside / just outside the ring spark threshold along the first ring dir.
        // `preset_temp(2)` is `LightingPreset::RingLights`, not Daylight, so
        // `use_d65 = 0.0` here.
        let ring0 = rig.ring_dirs[0];
        for &scale in &[0.999f32, 1.001] {
            let perturbed = (ring0 * scale + Vec3::new(1e-3, 0.0, 0.0)).normalize();
            cases.push(StudioEnvCase {
                dir: perturbed.to_array(),
                lambda_nm: 560.0,
                temp_k: preset_temp(2),
                spot_mult: LightingPreset::RingLights.params().spot_mult,
                exposure: 1.0,
                light_yaw,
                light_pitch,
                model: 0.0,
                _pad1: 0.0,
                use_d65: 0.0,
                observer: [0.0; 3],
                _pad2: 0.0,
            });
        }
    }

    cases
}

const fn preset_temp(index: i32) -> f32 {
    LightingPreset::from_index(index).params().temp_k
}

/// Adversarial head-shadow cases for one lit preset: exactly at the eye, and at 12 /
/// 16 / 20 degrees off it -- inside, across and outside the 14..18 degree fade.
fn push_head_shadow_cases(cases: &mut Vec<StudioEnvCase>, preset: LightingPreset, observer: Vec3) {
    let params = preset.params();
    let perp = observer.cross(Vec3::X).normalize();
    for degrees in [0.0f32, 12.0, 16.0, 20.0] {
        let (sin_a, cos_a) = degrees.to_radians().sin_cos();
        let dir = observer.mul_add(Vec3::splat(cos_a), perp * sin_a);
        cases.push(StudioEnvCase {
            dir: dir.to_array(),
            lambda_nm: 560.0,
            temp_k: params.temp_k,
            spot_mult: params.spot_mult,
            exposure: 1.0,
            light_yaw: 0.3,
            light_pitch: 0.6,
            model: preset.model().gpu_id() as f32,
            _pad1: 0.0,
            use_d65: f32::from(preset.uses_d65()),
            observer: observer.to_array(),
            _pad2: 0.0,
        });
    }
}

/// Runs the `sample_studio_environment` ULP-budget self-test against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse (see [`crate::renderer::gpu::layout_check::run`]'s doc
/// comment for the same rationale).
#[must_use]
pub fn run_studio_env(ctx: &crate::renderer::gpu::GpuContext) -> UlpCheckResult<StudioEnvCase> {
    let cases = build_studio_env_cases();
    let total = cases.len();

    let in_buf = compute::upload(
        &ctx.device,
        "studio_env cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "studio_env out",
        total,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "studio_env_main",
        SHADER_SRC,
        "studio_env_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "studio_env bind group",
        &pipeline,
        &[(4, &in_buf), (5, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total);

    let mut acc = UlpAccumulator::new(
        "sample_studio_environment",
        STUDIO_ENV_ULP_BUDGET,
        STUDIO_ENV_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = sample_studio_environment_observed(
            Vec3::from_array(case.dir),
            case.lambda_nm,
            preset_for_case(case),
            case.exposure,
            case.light_yaw,
            case.light_pitch,
            Vec3::from_array(case.observer),
        );
        acc.record(case, "radiance", cpu, gpu_out[idx]);
    }
    acc.finish()
}

fn preset_for_case(case: &StudioEnvCase) -> LightingPreset {
    let model_id = case.model as u32;
    match model_id {
        1 => LightingPreset::IsoHemisphere,
        2 => LightingPreset::LightTent,
        3 => LightingPreset::DaylightDome,
        _ => LightingPreset::from_index(preset_index_for(case.temp_k)),
    }
}

/// Recovers which built-in preset a case's `temp_k` came from, so `run_studio_env` can
/// call `sample_studio_environment` with the real `LightingPreset` enum rather than a
/// hand-reconstructed `(temp_k, spot_mult)` pair (`sample_studio_environment` takes the
/// enum directly, not the raw params).
fn preset_index_for(temp_k: f32) -> i32 {
    LightingPreset::ALL
        .iter()
        .position(|p| (p.params().temp_k - temp_k).abs() < 1e-6)
        .map_or(0, |i| i as i32)
}
