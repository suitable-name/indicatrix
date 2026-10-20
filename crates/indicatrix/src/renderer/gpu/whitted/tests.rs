use super::{SHADER_SOURCE, WhittedGpuRenderer};
use crate::{
    geometry::{StandardGemCuts, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, LightingModel, LightingPreset, build_plane_soa,
            environment::{
                head_shadow_cosines, sample_studio_environment_with_rig_spec_power_shadow_tray,
            },
        },
        studio_rig::StudioRig,
    },
    renderer::{
        buffers::whitted::GpuWhittedParams,
        env_map::EnvironmentMap,
        frame_scene::FrameScene,
        gpu::{
            compute,
            environment_check::{STUDIO_ENV_ABS_FLOOR, STUDIO_ENV_ULP_BUDGET},
            event_wavefront::compare::{FrameDiff, compare_frames},
            test_support::require_gpu,
            ulp::within_tolerance,
        },
        whitted::{
            WhittedPrep,
            fresnel::{fresnel_unpolarised_f32, refract},
            whitted_pixels_interleaved_geom,
        },
    },
};
use glam::Vec3;

#[test]
fn whitted_cpu_gpu_frames_are_close_and_two_dispatches_are_identical() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let env = EnvironmentMap::uniform(8, 4, [0.7, 0.5, 0.3]);
    for material in [
        GemMaterial::diamond(),
        GemMaterial::new_custom("clear", 1.5, 0.0, 0.0, [0.0; 3]),
    ] {
        for tools in [
            Vec::new(),
            vec![ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.2)],
        ] {
            let scene = FrameScene {
                camera: &camera,
                width: 24,
                height: 16,
                planes: &planes,
                facet_finishes: &[],
                material: &material,
                max_bounces: 64,
                environment: EnvironmentSource::HdrMap(&env),
            };
            let prep = WhittedPrep::new(
                &scene,
                Fluorescence::none(),
                crate::renderer::engine::RayOptions {
                    aa_taps: 4,
                    fire: true,
                },
            )
            .expect("prep");
            let uploaded = renderer.prepare(&scene, &tools, &prep).expect("upload");
            let gpu = renderer
                .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
                .expect("GPU frame");
            let again = renderer
                .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
                .expect("repeat frame");
            assert_eq!(gpu, again);
            let cpu = whitted_pixels_interleaved_geom(
                &scene,
                &tools,
                &build_plane_soa(&planes),
                &prep,
                0,
                1,
                0,
                4,
            );
            let difference = compare_frames(&cpu, &gpu, 4);
            assert!(difference.tol_passed(cpu.len()), "{difference:?}");
        }
    }
}

/// Pixels of the 384-pixel lit-tray frame allowed beyond the tolerance (facet-edge ties).
const LIT_TRAY_OUTLIERS: usize = 6;

/// ULP distance the GPU `exp` may have from `simd::exp_lane` (both results are positive, so
/// the bit patterns order like the values).
const EXP_ULP_BUDGET: u32 = 2;

/// Under a lit backdrop tray the GPU frame (primary misses included, which now take the same
/// lookup as every window) matches the CPU frame for a lit model, a Studio rig and ASET.
#[test]
fn whitted_cpu_gpu_frames_agree_over_a_lit_tray() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let plane_soa = build_plane_soa(&planes);
    let options = crate::renderer::engine::RayOptions {
        aa_taps: 4,
        fire: true,
    };
    // CPU twin frame for a given camera and tray albedo (the GPU frame is only ever taken
    // at `camera` / 0.81).
    let cpu_frame = |preset: LightingPreset, camera: &Camera, tray: f32| {
        let scene = FrameScene {
            camera,
            width: 24,
            height: 16,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 64,
            environment: preset.studio(1.0, 0.85, 0.95).with_backdrop(tray),
        };
        let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
        whitted_pixels_interleaved_geom(&scene, &[], &plane_soa, &prep, 0, 1, 0, 4)
    };
    for preset in [
        LightingPreset::IsoHemisphere,
        LightingPreset::LightTent,
        LightingPreset::Daylight,
        LightingPreset::Aset,
    ] {
        let scene = FrameScene {
            camera: &camera,
            width: 24,
            height: 16,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 64,
            environment: preset.studio(1.0, 0.85, 0.95).with_backdrop(0.81),
        };
        let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
        let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
        let gpu = renderer
            .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
            .expect("GPU frame");
        let cpu = cpu_frame(preset, &camera, 0.81);

        // Purpose of the test: the tray must actually reach pixels in this scene.
        let untrayed = cpu_frame(preset, &camera, 0.0);
        assert!(
            cpu.iter().zip(&untrayed).any(|(a, b)| a != b),
            "{preset:?}: the 0.81 tray changes no CPU pixel relative to tray 0"
        );

        // The G-D10 comparator (`compare.rs`, plan section 11.5) with its standard outlier
        // allowance, max(2, 0.1 %) of pixels, which is what absorbs facet-edge ties.
        //
        // ASET joins the Studio branch although it is not a Studio-model rig: the GPU looks up
        // the CPU-built 1024-row ASET elevation table with its own f32 index arithmetic, and at
        // ASET's steep smoothstep zone edges a few ulp of d.y shift the interpolated value by up
        // to about 1e-3 relative. Measured on the real GPU under the strict rule: tray 0, fire
        // on: 8 pixels over 1e-5 relative and 1 over 1e-3 (pixel 182, a TIR-or-exit tie of the
        // red-index chain, about 5 %); tray 0, fire off: 4 and 0; tray 0.81, fire on: 6 and 1
        // (pixel 182 again); tray 0.81, fire off: 3 and 0. The mismatch exists at tray 0, so it
        // is not the tray. IsoHemisphere and LightTent keep the per-pixel rule. The outlier
        // allowance of this 384-pixel frame is a fixed `LIT_TRAY_OUTLIERS` instead of the
        // standard max(2, 0.1 %), which is 2 here: edge ties scale with the stone's edges, not
        // with the frame's area. Measured on the owner's Linux server GPU (2026-10-20):
        // LightTent at tray 0.81 with fire on had 4 pixels over the tolerance (largest
        // absolute difference 0.27), the TIR-or-exit ties documented above; the owner's
        // Windows GPU had none. A tray that failed to reach the GPU frame would move far more.
        if preset.model() != LightingModel::Studio && preset != LightingPreset::Aset {
            let difference = compare_frames(&cpu, &gpu, 4);
            assert!(
                difference.non_finite_pixels == 0
                    && difference.tol_failing_pixels <= LIT_TRAY_OUTLIERS,
                "{preset:?}: {difference:?}"
            );
            continue;
        }

        // Studio-model rigs: the CPU and GPU light functions agree pointwise within
        // `STUDIO_ENV_ULP_BUDGET` (8192 ULP, `environment_check/studio_env.rs`), i.e.
        // 8192 * 2^-23 ~ 9.8e-4 relative (see `whitted_physics_and_every_studio_rig_match_the_
        // cpu_twins`). The per-pixel frame tolerance consistent with that budget is, per
        // channel, `STUDIO_ENV_ULP_BUDGET * f32::EPSILON * max(|cpu|, |gpu|) + 4e-6`, with the
        // G-D10 outlier allowance max(2, 0.1 %) of pixels (`FrameDiff::allowed_tol_failing`,
        // `compare.rs`). Measured on the real GPU: Daylight, Incandescent and RingLights at
        // tray 0 and Daylight at tray 0.81 with fire off agree on EVERY pixel within this
        // tolerance; with fire on exactly one pixel (Daylight, tray 0.81, index 182) differs,
        // red 3.53 vs 3.30 (about 6 %): a TIR-or-exit tie of the red-index chain at the
        // critical angle, made visible by the bright tray. That facet-edge tie is what the
        // outlier allowance absorbs.
        let budget = STUDIO_ENV_ULP_BUDGET as f32 * f32::EPSILON;
        let failing = cpu
            .iter()
            .zip(&gpu)
            .filter(|(c, g)| {
                !c.is_finite()
                    || !g.is_finite()
                    || (0..3).any(|k| {
                        let scaled = budget * c[k].abs().max(g[k].abs());
                        (c[k] - g[k]).abs() > scaled + 4e-6
                    })
            })
            .count();
        assert!(
            failing <= FrameDiff::allowed_tol_failing(cpu.len()),
            "{preset:?}: {failing} of {} pixels beyond the {STUDIO_ENV_ULP_BUDGET}-ULP              per-channel tolerance (allowed {})",
            cpu.len(),
            FrameDiff::allowed_tol_failing(cpu.len())
        );
        // Edge ties move energy between neighbouring pixels but not out of the frame.
        let mean = |frame: &[Vec3]| {
            frame
                .iter()
                .map(|p| f64::from(p.element_sum()))
                .sum::<f64>()
                / frame.len() as f64
        };
        let (cpu_mean, gpu_mean) = (mean(&cpu), mean(&gpu));
        assert!(
            (cpu_mean - gpu_mean).abs() <= 1e-3 * cpu_mean.abs().max(gpu_mean.abs()),
            "{preset:?}: frame means differ, CPU {cpu_mean} vs GPU {gpu_mean}"
        );
    }
}

#[test]
fn whitted_white_furnace_matches_the_analytic_background() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer =
        pollster::block_on(WhittedGpuRenderer::from_context_async(context)).expect("pipeline");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::new_custom("clear", 1.5, 0.0, 0.0, [0.0; 3]);
    let env = EnvironmentMap::uniform(8, 4, [1.0; 3]);
    let scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment: EnvironmentSource::HdrMap(&env),
    };
    let prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        crate::renderer::engine::RayOptions {
            aa_taps: 4,
            fire: false,
        },
    )
    .expect("prep");
    let expected = prep.rgb_to_xyz * Vec3::ONE;
    let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
    for value in renderer
        .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
        .expect("frame")
    {
        assert!(
            ((value / 4.0 - expected) / expected).abs().max_element() < 0.002,
            "{value:?}"
        );
    }
}

/// 4096 sphere directions with an exp input, each followed by a refraction case.
fn probe_cases() -> Vec<[f32; 4]> {
    let mut cases = Vec::with_capacity(8192);
    for index in 0..4096 {
        let y = 1.0 - 2.0 * (index as f32 + 0.5) / 4096.0;
        let phi = index as f32 * 2.399_963_1;
        let radius = y.mul_add(-y, 1.0).sqrt();
        let dir = Vec3::new(radius * phi.cos(), y, radius * phi.sin());
        let exp_input = (index as f32).mul_add(97.0 / 4095.0, -87.0);
        cases.push([dir.x, dir.y, dir.z, exp_input]);
        cases.push([0.0, 0.0, 1.0, ((index % 22) as f32).mul_add(0.1, 0.4)]);
    }
    cases
}

/// Compare the GPU probe output for one preset/shadow/tray against the CPU twins.
fn check_probe_results(
    cases: &[[f32; 4]],
    results: &[f32],
    rig: &StudioRig,
    preset: LightingPreset,
    head_shadow_deg: f32,
    tray: f32,
) {
    let shadow = head_shadow_cosines(head_shadow_deg);
    for index in 0..4096 {
        let dir = Vec3::new(
            cases[2 * index][0],
            cases[2 * index][1],
            cases[2 * index][2],
        );
        let x = cases[2 * index][3];
        let eta = cases[2 * index + 1][3];
        let result = &results[index * 8..index * 8 + 8];
        assert!((result[0] - fresnel_unpolarised_f32(x, eta)).abs() <= 1e-6);
        // Within 2 ULP of the CPU twin, not bitwise: a driver may contract the polynomial's
        // multiply-adds (WGSL cannot forbid it). The owner's Linux server GPU (2026-10-20)
        // was 1 ULP off at x = -86.97631, the bottom of the normal f32 range.
        let (gpu_bits, cpu_bits) = (result[1].to_bits(), crate::simd::exp_lane(x).to_bits());
        assert!(
            gpu_bits.abs_diff(cpu_bits) <= EXP_ULP_BUDGET,
            "exp at {x}: GPU {gpu_bits} vs CPU {cpu_bits}"
        );
        let refracted = refract(dir, Vec3::Z, eta);
        assert_eq!(result[5] != 0.0, refracted.is_some());
        if let Some(expected) = refracted {
            assert!(
                (Vec3::new(result[2], result[3], result[4]) - expected)
                    .abs()
                    .max_element()
                    < 1e-6
            );
        }
        let g_at = |direction: Vec3| {
            sample_studio_environment_with_rig_spec_power_shadow_tray(
                direction,
                preset,
                0.9,
                rig,
                Vec3::Z,
                1.0,
                shadow,
                tray,
            )
        };
        let g = g_at(dir);
        // The studio function chains `pow` lobes (`powi` on the CPU, exp2/log2 on the GPU), which
        // amplify a few ulp of `dot(d, light)` by the exponent. Same function, same budget as the
        // megakernel harness's `sample_studio_environment` check (8192 ULP, 1e-4 absolute floor;
        // see `STUDIO_ENV_ULP_BUDGET` for why). That budget supersedes the former ad-hoc
        // rounding allowance, which is removed.
        assert!(
            within_tolerance(g, result[6], STUDIO_ENV_ULP_BUDGET, STUDIO_ENV_ABS_FLOOR),
            "{preset:?}, shadow {head_shadow_deg}, tray {tray}, direction {dir:?}: {g} vs {}",
            result[6]
        );
    }
}

#[test]
fn whitted_physics_and_every_studio_rig_match_the_cpu_twins() {
    let Some(context) = require_gpu() else {
        return;
    };
    let source = format!(
        "{SHADER_SOURCE}\n{}",
        include_str!("../../shaders/whitted/probe.wgsl")
    );
    let pipeline = compute::create_compute_pipeline(
        &context.device,
        "Whitted physics probes",
        &source,
        "whitted_probe",
    );
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let cases = probe_cases();
    let input = compute::upload(
        &context.device,
        "Whitted probe inputs",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let rig = StudioRig::new(0.7, 0.8);
    // 16 degrees is the default shadow; 0 (off) and 30 exercise the `wt.shadow` uniform.
    // `tray` is the backdrop tray radiance (`values.z`): 0 = As Lit, then a dim, a white-tray and
    // a Studio-rig-bright tray, so every arm's `tray > 0.0` branch is probed below, in and above
    // the horizon band (the directions cover the whole sphere).
    for (preset, head_shadow_deg, tray) in LightingPreset::ALL
        .into_iter()
        .filter(|preset| !preset.is_uv_lamp() && *preset != LightingPreset::Aset)
        .flat_map(|preset| {
            [0.0_f32, 16.0, 30.0].into_iter().flat_map(move |deg| {
                [0.0_f32, 0.05, 0.81, 3.2]
                    .into_iter()
                    .map(move |tray| (preset, deg, tray))
            })
        })
    {
        let scene = FrameScene {
            camera: &camera,
            width: 1,
            height: 1,
            planes: &[],
            facet_finishes: &[],
            material: &material,
            max_bounces: 12,
            environment: EnvironmentSource::Studio {
                preset,
                exposure: 0.9,
                light_yaw: 0.7,
                light_pitch: 0.8,
                backdrop: tray,
                surface_glare: 1.0,
                head_shadow_deg,
            },
        };
        let prep = WhittedPrep::new(
            &scene,
            Fluorescence::none(),
            crate::renderer::engine::RayOptions::default(),
        )
        .expect("prep");
        let mut params = GpuWhittedParams::new(&scene, 0, &prep);
        params.range[1] = 4096;
        let uniform = compute::upload(
            &context.device,
            "Whitted probe params",
            &[params],
            wgpu::BufferUsages::UNIFORM,
        );
        let output = compute::zeroed_buffer::<f32>(
            &context.device,
            "Whitted probe output",
            4096 * 8,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let bindings = compute::bind_buffers(
            &context.device,
            "Whitted probe bindings",
            &pipeline,
            &[(1, &uniform), (2, &input), (6, &output)],
        );
        compute::dispatch_and_wait(
            &context.device,
            &context.queue,
            &pipeline,
            &bindings,
            (64, 1, 1),
        );
        let results = compute::readback::<f32>(&context.device, &context.queue, &output, 4096 * 8);
        check_probe_results(&cases, &results, &rig, preset, head_shadow_deg, tray);
    }
}

#[test]
fn large_raster_coordinates_and_public_prep_counts_match_cpu() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer =
        pollster::block_on(WhittedGpuRenderer::from_context_async(context)).expect("pipeline");
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let env = EnvironmentMap::from_rgb(
        4,
        1,
        vec![
            [0.1, 0.2, 0.3],
            [0.8, 0.2, 0.1],
            [0.3, 0.7, 0.4],
            [0.2, 0.4, 0.9],
        ],
    )
    .expect("directional map");
    let scene = FrameScene {
        camera: &camera,
        width: (1 << 24) + 1,
        height: 1,
        planes: &[],
        facet_finishes: &[],
        material: &material,
        max_bounces: 1,
        environment: EnvironmentSource::HdrMap(&env),
    };
    let soa = build_plane_soa(&[]);
    let mut prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        crate::renderer::engine::RayOptions::default(),
    )
    .expect("prep");
    // Prep is public. GPU upload must follow the CPU's defensive count handling.
    prep.chain_count = u8::MAX;
    for (taps, tap) in [(0, 0), (u8::MAX, 15)] {
        prep.taps = taps;
        let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
        let actual = renderer
            .trace_chunk(&uploaded, scene.width - 1, 1, tap, 1)
            .expect("pixel")[0];
        let expected = crate::renderer::whitted::whitted_tap_xyz(
            &scene,
            &[],
            &soa,
            &prep,
            scene.width - 1,
            tap,
            None,
        );
        let difference = compare_frames(&[expected], &[actual], 1);
        assert_eq!(difference.non_finite_pixels, 0, "{difference:?}");
        assert_eq!(difference.tol_failing_pixels, 0, "{difference:?}");
    }
}
