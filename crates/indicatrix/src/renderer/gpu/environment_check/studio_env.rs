//! `sample_studio_environment` ULP-budget self-test: [`build_studio_env_cases`]'s dense
//! direction/pose sweep plus adversarial points, dispatched by [`run_studio_env`].

use glam::Vec3;

use crate::{
    optics::raytracer::{
        DEFAULT_HEAD_SHADOW_COSINES, DEFAULT_HEAD_SHADOW_DEG, LightingModel, LightingPreset,
        TentParams, head_shadow_cosines, sample_studio_environment_with_rig_shadow,
    },
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
    /// Head-shadow cone `[outer, inner]` cosines, evaluated on the CPU by
    /// [`head_shadow_cosines`] of `head_shadow_deg` -- the GPU twin reads only these two.
    head_shadow_outer_cos: f32,
    head_shadow_inner_cos: f32,
    /// The degrees the cosines came from; the shader never reads it, the CPU reference
    /// recomputes the pair from it.
    head_shadow_deg: f32,
    _pad3: f32,
    /// The light tent's per-preset knobs `[walls, cards, spark, ground]`
    /// ([`TentParams::to_array`]); read by the GPU twin for model 2 only, and decoded by
    /// [`preset_for_case`] to tell the tent variants apart.
    tent: [f32; 4],
    /// [`TentParams::flat`] (the fifth tent value); same role as `tent`.
    tent_flat: f32,
}

impl StudioEnvCase {
    /// This case with a head shadow `deg` degrees wide (`0.0`: off).
    fn with_head_shadow(mut self, deg: f32) -> Self {
        let [outer, inner] = head_shadow_cosines(deg);
        self.head_shadow_outer_cos = outer;
        self.head_shadow_inner_cos = inner;
        self.head_shadow_deg = deg;
        self
    }
}

const _: () = assert!(size_of::<StudioEnvCase>() == 100);

/// The light tent's own parameters, for every case that is not a tent variant.
const DEFAULT_TENT: [f32; 4] = TentParams::DEFAULT.to_array();

/// Default-cone fields shared by every case constructor below: the 16 degree literals.
const DEFAULT_SHADOW: (f32, f32, f32) = (
    DEFAULT_HEAD_SHADOW_COSINES[0],
    DEFAULT_HEAD_SHADOW_COSINES[1],
    DEFAULT_HEAD_SHADOW_DEG,
);

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

/// Presets the case generator does not cover yet. None: the direct-sun model (GPU id 5) has
/// its cases now (the dense sweep plus [`push_sun_cases`]'s disc-edge points), as do the
/// tent variants (the case row carries their tent parameters) and the contrast view.
const PRESETS_WITHOUT_CASES_YET: [LightingPreset; 0] = [];

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
        .filter(|preset| !preset.is_uv_lamp() && !PRESETS_WITHOUT_CASES_YET.contains(preset))
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
                                head_shadow_outer_cos: DEFAULT_SHADOW.0,
                                head_shadow_inner_cos: DEFAULT_SHADOW.1,
                                head_shadow_deg: DEFAULT_SHADOW.2,
                                _pad3: 0.0,
                                tent: params.tent.to_array(),
                                tent_flat: params.tent.flat,
                            });
                        }
                    }
                }
            }
        }
        if preset.model() != LightingModel::Studio {
            push_head_shadow_cases(&mut cases, preset, lit_observer);
        }
        if preset == LightingPreset::Aset {
            push_aset_cases(&mut cases);
        }
        if preset == LightingPreset::DaylightSun {
            push_sun_cases(&mut cases);
        }
    }

    push_adversarial_rig_cases(&mut cases, &poses);

    cases
}

/// Adversarial rig cases for each pose: exactly on the key light's own axis, and exactly at
/// the ring lights' spark threshold on both sides.
fn push_adversarial_rig_cases(cases: &mut Vec<StudioEnvCase>, poses: &[(f32, f32)]) {
    // Adversarial: exactly on the key light's own axis (peak alignment, `key_dot ==
    // 1.0`), and exactly at the ring lights' `0.96` spark threshold on both sides.
    for &(light_yaw, light_pitch) in poses {
        let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
        // Daylight preset params (index 0 is `LightTent` since the ALL reorder),
        // `LightTent`), so these cases need `use_d65 = 1.0` too.
        for dir in [rig.key_dir, rig.fill_dir].into_iter().chain(rig.ring_dirs) {
            cases.push(StudioEnvCase {
                dir: dir.to_array(),
                lambda_nm: 560.0,
                temp_k: LightingPreset::Daylight.params().temp_k,
                spot_mult: 1.0,
                exposure: 1.0,
                light_yaw,
                light_pitch,
                model: 0.0,
                _pad1: 0.0,
                use_d65: 1.0,
                observer: [0.0; 3],
                _pad2: 0.0,
                head_shadow_outer_cos: DEFAULT_SHADOW.0,
                head_shadow_inner_cos: DEFAULT_SHADOW.1,
                head_shadow_deg: DEFAULT_SHADOW.2,
                _pad3: 0.0,
                tent: DEFAULT_TENT,
                tent_flat: TentParams::DEFAULT.flat,
            });
        }
        // Just inside / just outside the ring spark threshold along the first ring dir.
        // `LightingPreset::RingLights` (not Daylight), so
        // `use_d65 = 0.0` here.
        let ring0 = rig.ring_dirs[0];
        for &scale in &[0.999f32, 1.001] {
            let perturbed = (ring0 * scale + Vec3::new(1e-3, 0.0, 0.0)).normalize();
            cases.push(StudioEnvCase {
                dir: perturbed.to_array(),
                lambda_nm: 560.0,
                temp_k: LightingPreset::RingLights.params().temp_k,
                spot_mult: LightingPreset::RingLights.params().spot_mult,
                exposure: 1.0,
                light_yaw,
                light_pitch,
                model: 0.0,
                _pad1: 0.0,
                use_d65: 0.0,
                observer: [0.0; 3],
                _pad2: 0.0,
                head_shadow_outer_cos: DEFAULT_SHADOW.0,
                head_shadow_inner_cos: DEFAULT_SHADOW.1,
                head_shadow_deg: DEFAULT_SHADOW.2,
                _pad3: 0.0,
                tent: DEFAULT_TENT,
                tent_flat: TentParams::DEFAULT.flat,
            });
        }
    }
}

/// Adversarial ASET cases for the wavelength dimension: at each zone band's centre and at
/// its half-power points (+-10 nm), plus one wavelength far outside every band, for
/// directions inside each elevation zone and on both sides of the 45 / 75 degree edges.
/// (The ASET model reads the wavelength itself, so the general `lambdas` grid, which misses
/// the band centres, would not pin the band shape.)
fn push_aset_cases(cases: &mut Vec<StudioEnvCase>) {
    let params = LightingPreset::Aset.params();
    let use_d65 = f32::from(LightingPreset::Aset.uses_d65());
    let model = LightingPreset::Aset.model().gpu_id() as f32;
    let elevations = [
        -10.0f32, 0.0, 10.0, 30.0, 43.0, 45.0, 47.0, 60.0, 73.0, 75.0, 77.0, 85.0, 90.0,
    ];
    let lambdas = [
        450.0f32, 460.0, 470.0, 530.0, 540.0, 550.0, 600.0, 610.0, 620.0, 700.0,
    ];
    for elevation in elevations {
        let (sin_e, cos_e) = elevation.to_radians().sin_cos();
        for lambda_nm in lambdas {
            cases.push(StudioEnvCase {
                dir: [cos_e, sin_e, 0.0],
                lambda_nm,
                temp_k: params.temp_k,
                spot_mult: params.spot_mult,
                exposure: 1.0,
                light_yaw: 0.3,
                light_pitch: 0.6,
                model,
                _pad1: 0.0,
                use_d65,
                observer: [0.0; 3],
                _pad2: 0.0,
                head_shadow_outer_cos: DEFAULT_SHADOW.0,
                head_shadow_inner_cos: DEFAULT_SHADOW.1,
                head_shadow_deg: DEFAULT_SHADOW.2,
                _pad3: 0.0,
                tent: DEFAULT_TENT,
                tent_flat: TentParams::DEFAULT.flat,
            });
        }
    }
}

/// Adversarial `DaylightSun` cases around the 0.27 degree disc: on the key axis, well inside
/// the disc (0.1 and 0.2 degrees off it), and well outside (0.35, 0.6 and 3 degrees; the
/// disc edge itself is avoided because the `dot >= cos` test may legitimately flip between
/// CPU and GPU rounding there), for the three poses and the unobserved and head-shadowed
/// observers (the sun must ignore the head shadow on both sides). Cloned from the last
/// dense-sweep case of the preset, which already carries every per-preset field, so only
/// the direction, pose and observer are overridden.
fn push_sun_cases(cases: &mut Vec<StudioEnvCase>) {
    let Some(template) = cases.last().copied() else {
        return;
    };
    let poses = [(0.3f32, 0.6f32), (0.85, 0.95), (-0.5, 1.2)];
    let observer_eye = Vec3::new(0.2, 0.9, -0.3).normalize();
    for (light_yaw, light_pitch) in poses {
        let rig = crate::optics::studio_rig::StudioRig::new(light_yaw, light_pitch);
        let perp = rig.key_dir.any_orthonormal_vector();
        for degrees in [0.0f32, 0.1, 0.2, 0.35, 0.6, 3.0] {
            let (sin_a, cos_a) = degrees.to_radians().sin_cos();
            let dir = (rig.key_dir * cos_a + perp * sin_a).normalize();
            for observer in [Vec3::ZERO, observer_eye, rig.key_dir] {
                for lambda_nm in [450.0f32, 560.0, 650.0] {
                    cases.push(StudioEnvCase {
                        dir: dir.to_array(),
                        lambda_nm,
                        exposure: 1.3,
                        light_yaw,
                        light_pitch,
                        observer: observer.to_array(),
                        ..template
                    });
                }
            }
        }
    }
}

/// Adversarial head-shadow cases for one lit preset: exactly at the eye, and at 12 /
/// 16 / 20 degrees off it -- inside, across and outside the 14..18 degree fade -- for the
/// default 16 degree shadow, plus the same angles under a shadow that is off (`0`) and
/// one 25 degrees wide (so the GPU twin is checked off-default too).
fn push_head_shadow_cases(cases: &mut Vec<StudioEnvCase>, preset: LightingPreset, observer: Vec3) {
    let params = preset.params();
    let perp = observer.cross(Vec3::X).normalize();
    for shadow_deg in [DEFAULT_HEAD_SHADOW_DEG, 0.0, 25.0] {
        for degrees in [0.0f32, 12.0, 16.0, 20.0, 24.0, 28.0] {
            let (sin_a, cos_a) = degrees.to_radians().sin_cos();
            let dir = observer.mul_add(Vec3::splat(cos_a), perp * sin_a);
            cases.push(
                StudioEnvCase {
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
                    head_shadow_outer_cos: DEFAULT_SHADOW.0,
                    head_shadow_inner_cos: DEFAULT_SHADOW.1,
                    head_shadow_deg: DEFAULT_SHADOW.2,
                    _pad3: 0.0,
                    tent: params.tent.to_array(),
                    tent_flat: params.tent.flat,
                }
                .with_head_shadow(shadow_deg),
            );
        }
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
        let rig = crate::optics::studio_rig::StudioRig::new(case.light_yaw, case.light_pitch);
        let cpu = sample_studio_environment_with_rig_shadow(
            Vec3::from_array(case.dir),
            case.lambda_nm,
            preset_for_case(case),
            case.exposure,
            &rig,
            Vec3::from_array(case.observer),
            head_shadow_cosines(case.head_shadow_deg),
        );
        acc.record(case, "radiance", cpu, gpu_out[idx]);
    }
    acc.finish()
}

fn preset_for_case(case: &StudioEnvCase) -> LightingPreset {
    let model_id = case.model as u32;
    match model_id {
        1 => LightingPreset::IsoHemisphere,
        2 => tent_preset_for(case),
        3 => LightingPreset::DaylightDome,
        4 => LightingPreset::Aset,
        5 => LightingPreset::DaylightSun,
        _ => studio_preset_for(case.temp_k, case.use_d65 != 0.0),
    }
}

/// Recovers which light-tent preset a model-2 case came from: the tent variants share the
/// model and differ in their `tent` parameters, `spot_mult` and SPD (`use_d65`), so all of
/// those are matched, bit for bit (the case was built from the preset's own values).
fn tent_preset_for(case: &StudioEnvCase) -> LightingPreset {
    let bits = |values: [f32; 4]| values.map(f32::to_bits);
    LightingPreset::ALL
        .into_iter()
        .find(|p| {
            let params = p.params();
            p.model() == LightingModel::LightTent
                && p.uses_d65() == (case.use_d65 != 0.0)
                && params.spot_mult.to_bits() == case.spot_mult.to_bits()
                && bits(params.tent.to_array()) == bits(case.tent)
                && params.tent.flat.to_bits() == case.tent_flat.to_bits()
        })
        .unwrap_or(LightingPreset::LightTent)
}

/// Recovers which Studio-model preset a case's `temp_k` / `use_d65` came from, so
/// `run_studio_env` can call `sample_studio_environment` with the real `LightingPreset`
/// enum rather than a hand-reconstructed `(temp_k, spot_mult)` pair. Only presets whose
/// model is `Studio` are candidates: `LightingPreset::ALL` lists the lit presets first and
/// `IsoHemisphere` / `DaylightDome` share Daylight's 6500 K, so a bare temperature match
/// would resolve a model-0 Daylight case to the head-shadowed hemisphere.
fn studio_preset_for(temp_k: f32, use_d65: bool) -> LightingPreset {
    LightingPreset::ALL
        .into_iter()
        .find(|p| {
            p.model() == LightingModel::Studio
                && p.uses_d65() == use_d65
                && (p.params().temp_k - temp_k).abs() < 1e-6
        })
        .unwrap_or(LightingPreset::Daylight)
}
