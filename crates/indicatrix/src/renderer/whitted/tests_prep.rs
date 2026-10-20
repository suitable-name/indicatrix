use glam::Vec3;

use super::{AlphaModel, WhittedDecline, WhittedEnv, WhittedPrep, frost, tint};
use crate::{
    color::{
        ColorSpace,
        body_color::{Illuminant, body_colors},
        cie1931::cie_1931_cmf,
    },
    geometry::StandardGemCuts,
    optics::{
        absorption::AbsorptionBand,
        fluorescence::{EmissionBand, Fluorescence, FluorescentEmitter},
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, LightingPreset, apply_von_kries_white_balance,
            environment::{
                sample_studio_environment_with_rig_shadow,
                sample_studio_environment_with_rig_shadow_tray,
            },
        },
        studio_rig::StudioRig,
    },
    render_setup::MODEL_UNIT_FACE_UP_PATH,
    renderer::{engine::RayOptions, env_map::EnvironmentMap, frame_scene::FrameScene},
};

fn close(actual: Vec3, expected: Vec3, tolerance: f32) {
    let error = (actual - expected).abs();
    assert!(
        error.max_element() <= tolerance,
        "{actual:?} != {expected:?}: {error:?}"
    );
}

fn integrated_rgb(spectrum: impl Fn(f32) -> f32) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for wavelength in (380_u16..=780).step_by(5) {
        let wavelength = f32::from(wavelength);
        sum += Vec3::from_array(cie_1931_cmf(wavelength)) * (spectrum(wavelength) * 5.0);
    }
    ColorSpace::Srgb.xyz_to_linear(sum / 106.856)
}

#[test]
fn the_tint_reproduces_the_body_color_at_the_face_up_path() {
    for name in ["Ruby", "Sapphire", "Emerald", "Tanzanite", "Peridot"] {
        let material = GemMaterial::by_name(name).expect("built-in material");
        for preset in [LightingPreset::IsoHemisphere, LightingPreset::Incandescent] {
            let environment = preset.studio(1.0, 0.85, 0.95);
            let white = integrated_rgb(|w| preset.spectral_power(w));
            let check = |bands: &[AbsorptionBand]| {
                let alpha = tint::alpha(bands, material.absorption_path_scale, environment);
                let transmitted = integrated_rgb(|w| {
                    let coefficient: f32 = bands.iter().map(|band| band.evaluate(w)).sum();
                    preset.spectral_power(w)
                        * (-coefficient.max(0.0)
                            * material.absorption_path_scale
                            * MODEL_UNIT_FACE_UP_PATH)
                            .exp()
                });
                let expected = (transmitted / white).clamp(Vec3::splat(1.0e-4), Vec3::ONE);
                let actual = (-alpha * MODEL_UNIT_FACE_UP_PATH).exp();
                close(actual, expected, 1.0e-4);
                alpha
            };
            let o = check(&material.absorption.o_ray);
            let e = check(&material.absorption.e_ray);
            if let Some(beta) = &material.absorption.beta_ray {
                let b = check(beta);
                let mean = (o + e + b) / 3.0;
                close(
                    (-mean * MODEL_UNIT_FACE_UP_PATH).exp(),
                    ((-o * MODEL_UNIT_FACE_UP_PATH).exp()
                        * (-e * MODEL_UNIT_FACE_UP_PATH).exp()
                        * (-b * MODEL_UNIT_FACE_UP_PATH).exp())
                    .powf(1.0 / 3.0),
                    1.0e-4,
                );
            }
        }
    }
}

/// Independent reference: the physical body color of each eigenmode from
/// `body_colors` (1 nm grid, f64, its own illuminant and path handling) in linear
/// sRGB under D65, which `IsoHemisphere` uses as its illuminant.
#[test]
fn the_tint_matches_the_independent_body_color_reference() {
    let preset = LightingPreset::IsoHemisphere;
    let environment = preset.studio(1.0, 0.85, 0.95);
    for name in ["Ruby", "Sapphire", "Emerald", "Tanzanite", "Peridot"] {
        let material = GemMaterial::by_name(name).expect("built-in material");
        let path_mm = f64::from(material.absorption_path_scale * MODEL_UNIT_FACE_UP_PATH);
        let colors = body_colors(&material.absorption, path_mm, Illuminant::D65);
        let reference = |xyz: [f64; 3]| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "f64 reference to f32 render"
            )]
            let xyz = Vec3::new(xyz[0] as f32, xyz[1] as f32, xyz[2] as f32);
            ColorSpace::Srgb
                .xyz_to_linear(xyz)
                .clamp(Vec3::splat(1.0e-4), Vec3::ONE)
        };
        let tinted = |bands: &[AbsorptionBand]| {
            let alpha = tint::alpha(bands, material.absorption_path_scale, environment);
            (-alpha * MODEL_UNIT_FACE_UP_PATH).exp()
        };
        close(
            tinted(&material.absorption.o_ray),
            reference(colors.o_ray.xyz),
            2.0e-3,
        );
        close(
            tinted(&material.absorption.e_ray),
            reference(colors.e_ray.xyz),
            2.0e-3,
        );
        if let (Some(beta), Some(color)) = (&material.absorption.beta_ray, colors.beta_ray) {
            close(tinted(beta), reference(color.xyz), 2.0e-3);
        }
    }
}

#[test]
fn the_uniaxial_tint_follows_the_axis() {
    let o = Vec3::new(0.1, 0.3, 0.8);
    let e = Vec3::new(0.6, 0.4, 0.2);
    let alpha = AlphaModel::Uniaxial {
        o,
        e,
        axis: Vec3::Y,
    };
    close(alpha.at(Vec3::Y), o, 1.0e-7);
    close(alpha.at(Vec3::NEG_Y), o, 1.0e-7);
    close(alpha.at(Vec3::X), (o + e) * 0.5, 1.0e-7);
}

#[test]
fn a_colorless_material_has_no_tint() {
    let material = GemMaterial::diamond();
    let alpha = tint::alpha(
        &material.absorption.o_ray,
        1.0,
        LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95),
    );
    close(alpha, Vec3::ZERO, 1.0e-6);
}

#[test]
fn the_studio_leaf_is_g_times_the_illuminant() {
    let rig = StudioRig::new(0.85, 0.95);
    let observer = Vec3::new(0.6, 0.4, 1.0).normalize();
    for preset in LightingPreset::ALL {
        if preset == LightingPreset::Aset || preset.is_uv_lamp() {
            continue;
        }
        for shadow_degrees in [0.0, 18.0, 45.0] {
            let source = preset
                .studio(1.3, 0.85, 0.95)
                .with_head_shadow(shadow_degrees);
            let env = WhittedEnv::new(source);
            for k in 0_u16..37 {
                let y = f32::from(k).mul_add(2.0 / 36.0, -1.0);
                let phi = f32::from(k) * 2.399_963_1;
                let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
                let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());
                let reference = integrated_rgb(|w| {
                    sample_studio_environment_with_rig_shadow(
                        dir,
                        w,
                        preset,
                        1.3,
                        &rig,
                        observer,
                        crate::optics::raytracer::head_shadow_cosines(shadow_degrees),
                    )
                });
                close(
                    env.leaf(dir, observer),
                    reference,
                    reference.abs().max_element().max(1.0e-5) * 1.0e-3,
                );
            }
        }
    }
}

/// The reference for the 1024-row ASET table: the spectral integral at the two table rows that
/// bracket `y`, interpolated linearly exactly as `WhittedEnv::leaf` does. The table samples `y`
/// every 2 / 1023, and the ASET zone edges are smoothsteps only 0.04 wide in `y` (second
/// derivative up to 6 / 0.04^2), so a straight comparison with the integral at an arbitrary `y`
/// inside an edge differs by up to (2 / 1023)^2 / 8 * 3750 * 5 = 9e-3 on the blue channel of
/// 5.0 from interpolation alone. At a row itself the reference is the integral at that `y`.
fn aset_table_reference(y: f32, spectrum: impl Fn(Vec3, f32) -> f32) -> Vec3 {
    let at_row = |row: f32| {
        let row_y = (row / 1023.0).mul_add(2.0, -1.0);
        let dir = Vec3::new(row_y.mul_add(-row_y, 1.0).max(0.0).sqrt(), row_y, 0.0);
        integrated_rgb(|w| spectrum(dir, w))
    };
    let position = (y.clamp(-1.0, 1.0) + 1.0) * 511.5;
    let lower = position.floor();
    at_row(lower).lerp(at_row((lower + 1.0).min(1023.0)), position - lower)
}

#[test]
fn the_aset_table_matches_the_spectral_integral() {
    let preset = LightingPreset::Aset;
    let env = WhittedEnv::new(preset.studio(1.0, 0.85, 0.95));
    let rig = StudioRig::new(0.85, 0.95);
    let rows = [1000_u16, 1020].map(|row| (f32::from(row) / 1023.0).mul_add(2.0, -1.0));
    for y in [-0.9_f32, -0.2, 0.0, 0.2, 0.4, 0.6, 0.8, 0.98]
        .into_iter()
        .chain(rows)
    {
        let dir = Vec3::new(y.mul_add(-y, 1.0).sqrt(), y, 0.0);
        let reference = aset_table_reference(y, |dir, w| {
            crate::optics::raytracer::sample_studio_environment_with_rig(
                dir,
                w,
                preset,
                1.0,
                &rig,
                Vec3::ZERO,
            )
        });
        close(env.leaf(dir, Vec3::Z), reference, 2.0e-3);
    }
}

#[test]
fn the_hdr_leaf_is_the_map_radiance_and_can_outlive_the_source() {
    let expected = Vec3::new(0.3, 0.7, 1.2);
    let owned = {
        let map = EnvironmentMap::uniform(4, 2, expected.to_array());
        let env = WhittedEnv::new(EnvironmentSource::HdrMap(&map));
        close(env.leaf(Vec3::Z, Vec3::Y), expected, 1.0e-6);
        env.into_owned()
    };
    close(owned.leaf(Vec3::NEG_X, Vec3::Y), expected, 1.0e-6);
}

/// There is no separate primary-miss lookup any more: the camera ray that misses the stone and
/// a ray that leaves it both call `leaf`, which sees the lit tray below the horizon.
#[test]
fn the_primary_miss_equals_the_leaf() {
    let preset = LightingPreset::IsoHemisphere;
    let tray = 0.81;
    let source = preset.studio(1.3, 0.85, 0.95).with_backdrop(tray);
    let env = WhittedEnv::new(source);
    // Straight down is far below the horizon band and outside the head-shadow cone.
    close(
        env.leaf(Vec3::NEG_Y, Vec3::Y),
        tint::illuminant_rgb(source) * (tray * 1.3),
        1.0e-5,
    );
    // The whole chain: a 1x1 frame with no stone looks along the camera's forward axis.
    let camera = Camera::new(0.6, std::f32::consts::FRAC_PI_2 - 0.01, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let scene = FrameScene {
        camera: &camera,
        width: 1,
        height: 1,
        planes: &[],
        facet_finishes: &[],
        material: &material,
        max_bounces: 12,
        environment: source,
    };
    let prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        RayOptions {
            aa_taps: 1,
            fire: false,
        },
    )
    .expect("prep");
    let soa = crate::optics::raytracer::build_plane_soa(&[]);
    let pixel = super::whitted_tap_xyz(&scene, &[], &soa, &prep, 0, 0, None);
    let expected = prep.rgb_to_xyz * WhittedEnv::new(source).leaf(camera.forward, -camera.forward);
    close(pixel, expected, expected.abs().max_element() * 1.0e-3);
}

/// Windows and the surround are one lookup: under the grading dome the pixels the stone covers
/// gain light when the lit tray replaces the nothing-below-the-girdle ground, and no pixel loses
/// any; the surround is the tray itself.
#[test]
fn a_window_shows_the_backdrop() {
    let planes = StandardGemCuts::emerald_cut();
    let soa = crate::optics::raytracer::build_plane_soa(&planes);
    let material = GemMaterial::new_custom("clear", 1.57, 0.0, 0.0, [0.0; 3]);
    let camera = Camera::new(0.0, std::f32::consts::FRAC_PI_2 - 0.01, 3.2, 42.0);
    let tray = 0.9;
    let lit = |backdrop: f32| {
        let environment = LightingPreset::IsoHemisphere
            .studio(1.0, 0.85, 0.95)
            .with_backdrop(backdrop);
        let scene = FrameScene {
            camera: &camera,
            width: 32,
            height: 32,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 64,
            environment,
        };
        let prep = WhittedPrep::new(
            &scene,
            Fluorescence::none(),
            RayOptions {
                aa_taps: 1,
                fire: false,
            },
        )
        .expect("prep");
        (0..32_u32 * 32)
            .map(|pixel| {
                let mut hit = None;
                let xyz =
                    super::whitted_tap_xyz(&scene, &[], &soa, &prep, pixel, 0, Some(&mut hit));
                (xyz.y, hit.is_some())
            })
            .collect::<Vec<_>>()
    };
    let as_lit = lit(0.0);
    let tray_lit = lit(tray);
    let mut stone = 0_usize;
    let (mut as_lit_sum, mut tray_sum) = (0.0_f32, 0.0_f32);
    let (mut surround_sum, mut as_lit_surround) = (0.0_f32, 0.0_f32);
    let mut surround = 0_usize;
    for (dark, bright) in as_lit.iter().zip(&tray_lit) {
        assert_eq!(dark.1, bright.1, "the tray never moves the silhouette");
        assert!(
            bright.0 >= dark.0.abs().mul_add(-1.0e-5, dark.0),
            "a pixel lost light: {} -> {}",
            dark.0,
            bright.0
        );
        if bright.1 {
            stone += 1;
            as_lit_sum += dark.0;
            tray_sum += bright.0;
        } else {
            surround += 1;
            surround_sum += bright.0;
            as_lit_surround += dark.0;
        }
    }
    assert!(
        stone > 100 && surround > 20,
        "{stone} stone, {surround} surround"
    );
    #[expect(clippy::cast_precision_loss, reason = "pixel counts are tiny")]
    let (stone_f, surround_f) = (stone as f32, surround as f32);
    assert!(
        tray_sum / stone_f > 1.005 * (as_lit_sum / stone_f),
        "windows must show the tray: {} vs {}",
        tray_sum / stone_f,
        as_lit_sum / stone_f
    );
    // Looking straight down, the surround is the tray (the sky is above the horizon), and the
    // windows read at the level of the surround scaled by the glass's transmission: same order.
    assert!(
        as_lit_surround / surround_f < 0.01 * (surround_sum / surround_f),
        "As Lit has nothing below the girdle: {as_lit_surround} vs {surround_sum}"
    );
    assert!(
        tray_sum / stone_f > 0.2 * (surround_sum / surround_f),
        "windows read as light, not as holes: {} vs {}",
        tray_sum / stone_f,
        surround_sum / surround_f
    );
}

/// Under a tray the Ray-Traced leaf equals the CPU rig's `_tray` lookup for every lit model and
/// Studio rig, in the head-shadow settings the scene can carry, below, inside and above the
/// horizon band.
#[test]
fn the_studio_leaf_with_a_tray_is_g_times_the_illuminant() {
    let rig = StudioRig::new(0.85, 0.95);
    let observer = Vec3::new(0.6, 0.4, 1.0).normalize();
    for preset in LightingPreset::ALL {
        if preset == LightingPreset::Aset || preset.is_uv_lamp() {
            continue;
        }
        for shadow_degrees in [0.0, 18.0, 45.0] {
            for tray in [0.05, 0.81, 3.2] {
                let source = preset
                    .studio(1.3, 0.85, 0.95)
                    .with_head_shadow(shadow_degrees)
                    .with_backdrop(tray);
                let env = WhittedEnv::new(source);
                for k in 0_u16..37 {
                    let y = f32::from(k).mul_add(2.0 / 36.0, -1.0);
                    let phi = f32::from(k) * 2.399_963_1;
                    let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
                    let dir = Vec3::new(r * phi.cos(), y, r * phi.sin());
                    let reference = integrated_rgb(|w| {
                        sample_studio_environment_with_rig_shadow_tray(
                            dir,
                            w,
                            preset,
                            1.3,
                            &rig,
                            observer,
                            crate::optics::raytracer::head_shadow_cosines(shadow_degrees),
                            tray,
                        )
                    });
                    close(
                        env.leaf(dir, observer),
                        reference,
                        reference.abs().max_element().max(1.0e-5) * 1.0e-3,
                    );
                }
            }
        }
    }
}

/// The ASET table is built from the tray-aware lookup, so its below-horizon rows read the tray.
#[test]
fn the_aset_table_shows_the_tray_below_the_horizon() {
    let preset = LightingPreset::Aset;
    let tray = 0.81;
    let env = WhittedEnv::new(preset.studio(1.0, 0.85, 0.95).with_backdrop(tray));
    let as_lit = WhittedEnv::new(preset.studio(1.0, 0.85, 0.95));
    let rig = StudioRig::new(0.85, 0.95);
    for y in [-0.9_f32, -0.5, -0.2, 0.0, 0.2, 0.6, 0.98] {
        let dir = Vec3::new(y.mul_add(-y, 1.0).sqrt(), y, 0.0);
        let reference = aset_table_reference(y, |dir, w| {
            sample_studio_environment_with_rig_shadow_tray(
                dir,
                w,
                preset,
                1.0,
                &rig,
                Vec3::ZERO,
                crate::optics::raytracer::DEFAULT_HEAD_SHADOW_COSINES,
                tray,
            )
        });
        close(env.leaf(dir, Vec3::Z), reference, 2.0e-3);
    }
    assert!(env.leaf(Vec3::NEG_Y, Vec3::Z).max_element() > 0.1);
    close(as_lit.leaf(Vec3::NEG_Y, Vec3::Z), Vec3::ZERO, 1.0e-6);
}

/// A frosted facet pointing down averages the environment over its cosine lobe, which now
/// includes the tray.
#[test]
fn frost_of_a_downward_facet_sees_the_tray() {
    let tray = 0.81;
    let source = LightingPreset::IsoHemisphere
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(tray);
    let env = WhittedEnv::new(source);
    let mean = frost::mean(&env, Vec3::NEG_Y, Vec3::Y);
    close(
        mean,
        tint::illuminant_rgb(source) * tray,
        tint::illuminant_rgb(source).max_element() * tray * 0.03,
    );
    let as_lit = WhittedEnv::new(LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95));
    assert!(frost::mean(&as_lit, Vec3::NEG_Y, Vec3::Y).max_element() < 0.05);
}

#[test]
fn white_balance_matches_the_tracer() {
    let camera = Camera::new(0.6, 0.45, 3.5, 42.0);
    let material = GemMaterial::diamond();
    let planes = StandardGemCuts::standard_round_brilliant();
    let environment = LightingPreset::Incandescent.studio(1.0, 0.85, 0.95);
    let scene = FrameScene {
        camera: &camera,
        width: 1,
        height: 1,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment,
    };
    let prep = WhittedPrep::new(&scene, Fluorescence::none(), RayOptions::default()).unwrap();
    let rgb = tint::illuminant_rgb(environment);
    let xyz = apply_von_kries_white_balance(
        prep.rgb_to_xyz * rgb,
        prep.white_balance.expect("Planckian white balance"),
    );
    let displayed = ColorSpace::Srgb.xyz_to_linear(xyz);
    assert!((displayed.max_element() - displayed.min_element()) / displayed.max_element() < 0.01);
}

#[test]
fn uv_lamps_and_fluorescence_decline() {
    let camera = Camera::new(0.6, 0.45, 3.5, 42.0);
    let material = GemMaterial::diamond();
    let scene = |environment| FrameScene {
        camera: &camera,
        width: 1,
        height: 1,
        planes: &[],
        facet_finishes: &[],
        material: &material,
        max_bounces: 12,
        environment,
    };
    for preset in [LightingPreset::UvLamp365, LightingPreset::UvLamp395] {
        assert!(matches!(
            WhittedPrep::new(
                &scene(preset.studio(1.0, 0.0, 0.0)),
                Fluorescence::none(),
                RayOptions::default()
            ),
            Err(WhittedDecline::UvLamp)
        ));
    }
    let fluorescence = Fluorescence::new(vec![FluorescentEmitter {
        excitation: vec![AbsorptionBand::new(400.0, 20.0, 1.0)],
        emission: vec![EmissionBand::new(550.0, 20.0, 1.0)],
        quantum_yield: 0.5,
    }]);
    assert!(matches!(
        WhittedPrep::new(
            &scene(LightingPreset::LightTent.studio(1.0, 0.0, 0.0)),
            &fluorescence,
            RayOptions::default()
        ),
        Err(WhittedDecline::Fluorescence)
    ));
}

#[test]
fn frost_of_a_uniform_environment_is_the_environment() {
    let rgb = Vec3::new(0.2, 0.6, 1.7);
    let env = WhittedEnv::for_tests(rgb);
    for normal in [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(0.4, -0.7, 0.2).normalize(),
    ] {
        close(frost::mean(&env, normal, Vec3::Z), rgb, 2.0e-6);
    }
}

#[test]
fn one_chain_when_fire_is_off_or_dispersion_is_negligible() {
    let camera = Camera::new(0.6, 0.45, 3.5, 42.0);
    let dispersive = GemMaterial::diamond();
    let constant = GemMaterial::new_custom("Constant", 1.5, 0.0, 0.0, [0.0; 3]);
    for (material, fire, expected) in [
        (&dispersive, false, 1),
        (&constant, true, 1),
        (&dispersive, true, 3),
    ] {
        let scene = FrameScene {
            camera: &camera,
            width: 1,
            height: 1,
            planes: &[],
            facet_finishes: &[],
            material,
            max_bounces: 12,
            environment: LightingPreset::LightTent.studio(1.0, 0.0, 0.0),
        };
        let prep = WhittedPrep::new(
            &scene,
            Fluorescence::none(),
            RayOptions { aa_taps: 8, fire },
        )
        .unwrap();
        assert_eq!(prep.chain_count, expected);
        if expected == 1 {
            assert_eq!(
                prep.chains[0].n.to_bits(),
                material.dispersion.n_d().to_bits()
            );
            assert_eq!(prep.chains[0].mask, Vec3::ONE);
        }
    }
}
