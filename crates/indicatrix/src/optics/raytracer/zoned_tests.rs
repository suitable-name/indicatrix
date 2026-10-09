//! Tests of the zoned absorption in the CPU tracer (`zoning` feature): the segment optical depth
//! per zone, the bit-exact reduction to the unzoned path, the bicolour render, the zoned
//! scattering estimator and the zone swatches.

use super::{
    NUM_CHANNELS,
    absorption::{apply_absorption, apply_segment_absorption, channel_absorption_alphas_assigned},
    camera::{Camera, Ray},
    environment::LightingPreset,
    refraction::{RayMaterialContext, build_ray_wavelength_cache},
    scattering::{
        hg::{ZonedScatterParams, maybe_scatter_or_extinguish_zoned},
        maybe_scatter_or_extinguish,
    },
    transport::trace_spectral_ray,
    zoned::{ZoneAlphas, segment_optical_depths},
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{AbsorptionUnit, GemMaterial, zone_swatches},
        polarization::StokesVector,
        zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
    },
    render_setup::{absorption_path_scale_for, measure_model_width},
};
use glam::{DVec3, Vec3};

const LAMBDAS: [f32; NUM_CHANNELS] = [420.0, 460.0, 500.0, 540.0, 580.0, 620.0, 660.0, 700.0];

fn clear() -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()))
}

fn band_tensor(center_nm: f32, width_nm: f32, peak_per_mm: f32) -> AbsorptionTensor {
    AbsorptionTensor::isotropic(vec![AbsorptionBand::new(center_nm, width_nm, peak_per_mm)])
}

fn band_zone(center_nm: f32, width_nm: f32, peak_per_mm: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(band_tensor(center_nm, width_nm, peak_per_mm))
}

/// A cubic, non-dispersive probe stone; the caller installs the absorption.
fn probe(name: &str) -> GemMaterial {
    GemMaterial::new_custom(name, 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
}

const fn half_space(normal: DVec3, absorption: ZoneAbsorption) -> Zone {
    Zone {
        shape: ZoneShape::HalfSpace {
            normal,
            offset: 0.0,
        },
        absorption,
    }
}

const fn zoned(base: ZoneAbsorption, zones: Vec<Zone>) -> ZonedAbsorption {
    ZonedAbsorption {
        frame: ZoneFrame::IDENTITY,
        base,
        zones,
        boundary_softness_mm: 0.0,
    }
}

/// A plain material with `tensor` and the same absorption in a single zone whose plane is pushed
/// so far away that the zone covers the whole stone; `scattering` is `(sigma_s, g)` for both.
fn covering_pair(
    tensor: &AbsorptionTensor,
    scale: f32,
    scattering: Option<(f32, f32)>,
) -> (GemMaterial, GemMaterial) {
    let mut plain = probe("plain")
        .with_chromophore_absorption(tensor.clone())
        .with_absorption_path_scale(scale);
    let mut covering = probe("zoned")
        .with_zoning(zoned(
            clear(),
            vec![half_space(DVec3::X, ZoneAbsorption::per_mm(tensor.clone()))],
        ))
        .with_absorption_path_scale(scale);
    if let Some((sigma_s, g)) = scattering {
        plain = plain.with_scattering(sigma_s, g);
        covering = covering.with_scattering(sigma_s, g);
    }
    // Everything of the stone is on the zone's side: push the plane far away.
    if let Some(z) = covering.zoning.as_mut() {
        z.zones[0].shape = ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: -1.0e6,
        };
    }
    (plain, covering)
}

const fn context(material: &GemMaterial) -> RayMaterialContext<'_> {
    RayMaterialContext {
        material,
        lambdas: LAMBDAS,
        hero_idx: 0,
        c_axis: Vec3::Y,
        is_anisotropic: false,
        enable_internal_mode_coupling: true,
    }
}

const fn unit_stokes() -> [StokesVector; NUM_CHANNELS] {
    [StokesVector::unpolarized(1.0); NUM_CHANNELS]
}

/// A single zone that covers the whole segment must attenuate bit for bit like the unzoned
/// material with the same absorption (the kernel's lengths are used as fractions, so the
/// covered segment keeps exactly `path_len * scale`).
#[test]
fn a_zone_covering_the_whole_segment_absorbs_bit_identically_to_no_zones() {
    let tensor = band_tensor(560.0, 70.0, 0.15);
    let scale = 3.5;
    let (plain, covering) = covering_pair(&tensor, scale, None);
    assert_eq!(covering.absorption_unit, AbsorptionUnit::PerMm);

    let ctx_plain = context(&plain);
    let ctx_zoned = context(&covering);
    let cache_plain = build_ray_wavelength_cache(&ctx_plain);
    let cache_zoned = build_ray_wavelength_cache(&ctx_zoned);
    assert!(cache_zoned.zoned.is_some() && cache_plain.zoned.is_none());

    let rays = [
        (Vec3::new(0.1, -0.2, 0.3), Vec3::new(1.0, -0.5, 0.2), 1.234),
        (Vec3::new(-0.6, 0.4, -0.1), Vec3::new(0.2, 0.9, -0.3), 0.77),
        (Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 2.0),
    ];
    for (origin, dir, len) in rays {
        let dir = dir.normalize();
        let mut a = unit_stokes();
        let mut b = unit_stokes();
        apply_absorption(&ctx_plain, &cache_plain, dir, false, len, &mut a);
        apply_segment_absorption(
            &ctx_zoned,
            &cache_zoned,
            dir,
            false,
            (Ray { origin, dir }, len),
            &mut b,
        );
        assert!(a[3].intensity() < 0.99, "the probe must actually absorb");
        for k in 0..NUM_CHANNELS {
            assert_eq!(
                a[k].intensity().to_bits(),
                b[k].intensity().to_bits(),
                "channel {k}, ray {origin:?} {dir:?}"
            );
        }
    }
}

/// The per-channel optical depth of a ray inside one zone, and of a ray crossing a boundary, is
/// `sum_z alpha_z(lambda_k) * len_z` with the lengths in millimetres.
#[test]
fn segment_optical_depth_follows_the_zone_of_each_part_of_the_segment() {
    let scale = 3.0;
    let base = band_zone(450.0, 40.0, 0.20);
    let right = band_zone(610.0, 40.0, 0.12);
    let material = probe("bicolour")
        .with_zoning(zoned(
            base.clone(),
            vec![half_space(DVec3::X, right.clone())],
        ))
        .with_absorption_path_scale(scale);
    let ctx = context(&material);
    let cache = build_ray_wavelength_cache(&ctx);
    let alpha_at = |zone: &ZoneAbsorption, k: usize| zone.alpha(f64::from(LAMBDAS[k]), None);

    // (origin x, length in model units): fully right, fully left, and a crossing with 60 percent
    // of the 1.5 mm in the base zone (x from -0.9 mm to +0.6 mm).
    let cases = [
        (0.2_f32, 0.5_f32, 0.0_f64),
        (-0.7, 0.5, 1.0),
        (-0.3, 0.5, 0.6),
    ];
    for (x0, len, base_fraction) in cases {
        let ray = Ray {
            origin: Vec3::new(x0, 0.0, 0.0),
            dir: Vec3::X,
        };
        let depths = segment_optical_depths(&ctx, &cache, Vec3::X, false, ray, len)
            .expect("a zoned material gives zoned depths");
        let mm = f64::from(len) * f64::from(scale);
        for (k, depth) in depths.iter().enumerate() {
            let expected = (alpha_at(&base, k) * mm).mul_add(
                base_fraction,
                alpha_at(&right, k) * mm * (1.0 - base_fraction),
            );
            let err = (f64::from(*depth) - expected).abs();
            assert!(
                err <= 2e-4_f64.mul_add(expected.max(1e-2), 1e-6),
                "x0 {x0}, channel {k}: depth {depth} vs expected {expected}"
            );
        }
    }

    // The unzoned material has no zoned depths: the caller keeps the homogeneous path.
    let plain = probe("plain");
    let ctx_plain = context(&plain);
    let cache_plain = build_ray_wavelength_cache(&ctx_plain);
    let ray = Ray {
        origin: Vec3::ZERO,
        dir: Vec3::X,
    };
    assert!(segment_optical_depths(&ctx_plain, &cache_plain, Vec3::X, false, ray, 1.0).is_none());
}

fn x_chromaticity(sum: Vec3) -> f32 {
    sum.x / (sum.x + sum.y + sum.z).max(1e-9)
}

/// X chromaticity of the left and the right half of a small studio render of `material`.
fn half_chromaticities(material: &GemMaterial) -> (f32, f32) {
    const GRID: usize = 16;
    const SAMPLES: u32 = 32;
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let mut left = Vec3::ZERO;
    let mut right = Vec3::ZERO;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            let mut pixel = Vec3::ZERO;
            for s in 0..SAMPLES {
                let pixel_id = (iy * GRID + ix) as u32;
                let seed =
                    super::sampling::hash_u32(pixel_id ^ super::sampling::hash_u32(s ^ 0x20_4E));
                pixel += trace_spectral_ray(
                    ray,
                    &planes,
                    material,
                    16,
                    LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
                    seed,
                    (super::sampling::hash_u32(seed) as f32) / 4_294_967_295.0,
                    None,
                );
            }
            if ix < GRID / 2 {
                left += pixel;
            } else {
                right += pixel;
            }
        }
    }
    (x_chromaticity(left), x_chromaticity(right))
}

/// A bicolour stone (one half absorbs red and looks blue-green, the other absorbs blue and
/// looks yellow): the two image halves differ in hue, and mirroring the zones mirrors the
/// difference. The test does not assume which image side the stone's +X lands on.
#[test]
fn a_bicolour_stone_renders_its_two_halves_in_different_hues() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let scale = absorption_path_scale_for(AbsorptionUnit::PerMm, 7.0, measure_model_width(&planes))
        .expect("the round brilliant has a measurable width");
    let build = |normal: DVec3| {
        probe("bicolour probe")
            .with_zoning(zoned(
                band_zone(450.0, 40.0, 0.20),
                vec![half_space(normal, band_zone(610.0, 40.0, 0.12))],
            ))
            .with_absorption_path_scale(scale)
    };
    let (l_pos, r_pos) = half_chromaticities(&build(DVec3::X));
    let (l_neg, r_neg) = half_chromaticities(&build(-DVec3::X));
    let diff_pos = l_pos - r_pos;
    let diff_neg = l_neg - r_neg;
    assert!(
        diff_pos * diff_neg < 0.0,
        "mirroring the zones must mirror the left-right hue difference ({diff_pos} vs {diff_neg})"
    );
    assert!(
        (diff_pos - diff_neg).abs() > 0.02,
        "the halves must differ visibly in x chromaticity ({diff_pos} vs {diff_neg})"
    );
}

/// The zoned scattering estimator equals the homogeneous one when one zone covers the whole
/// segment: same scatter-or-survive decision, same free-flight distance and weights.
#[test]
fn zoned_scattering_matches_the_homogeneous_estimator_for_a_single_zone() {
    let tensor = band_tensor(540.0, 60.0, 0.10);
    let scale = 3.0;
    let sigma_s = 0.4;
    let g = 0.3;
    let (plain, covering) = covering_pair(&tensor, scale, Some((sigma_s, g)));
    let ctx_plain = context(&plain);
    let ctx_zoned = context(&covering);
    let cache_plain = build_ray_wavelength_cache(&ctx_plain);
    let cache_zoned = build_ray_wavelength_cache(&ctx_zoned);
    let zoned_cache = cache_zoned.zoned.as_ref().expect("zoned cache");
    let k_hat = Vec3::new(0.0, 0.0, 1.0);
    let zone_alphas: ZoneAlphas = zoned_cache.zone_alphas(&ctx_zoned, &cache_zoned, k_hat, false);
    let alphas = channel_absorption_alphas_assigned(&ctx_plain, &cache_plain, k_hat, false);
    let ray = Ray {
        origin: Vec3::new(-0.2, 0.1, -0.9),
        dir: k_hat,
    };
    let hit_t = 1.8;

    let mut scattered = 0;
    let mut survived = 0;
    for rng_seed in 0..96_u32 {
        let mut stokes_h = unit_stokes();
        let mut stokes_z = unit_stokes();
        let mut pdf_h = [1.0_f32; NUM_CHANNELS];
        let mut pdf_z = [1.0_f32; NUM_CHANNELS];
        let homogeneous = maybe_scatter_or_extinguish(
            &alphas,
            sigma_s,
            g,
            0,
            ray.dir,
            hit_t,
            scale,
            rng_seed,
            0,
            &mut stokes_h,
            &mut pdf_h,
        );
        let zoned_outcome = maybe_scatter_or_extinguish_zoned(
            zoned_cache,
            &zone_alphas,
            &ZonedScatterParams {
                sigma_s_model: sigma_s,
                g,
                hero_idx: 0,
                ray,
                hit_t,
                path_scale: scale,
                rng_seed,
                bounce: 0,
            },
            &mut stokes_z,
            &mut pdf_z,
        );
        assert_eq!(
            homogeneous.is_some(),
            zoned_outcome.is_some(),
            "seed {rng_seed}: scatter-or-survive must agree"
        );
        if let (Some((t_h, d_h)), Some((t_z, d_z))) = (homogeneous, zoned_outcome) {
            scattered += 1;
            assert!(
                (t_h - t_z).abs() <= 1e-4 * t_h.max(1e-3),
                "seed {rng_seed}: {t_h} vs {t_z}"
            );
            assert!((d_h - d_z).length() <= 1e-5, "seed {rng_seed}");
        } else {
            survived += 1;
        }
        for k in 0..NUM_CHANNELS {
            let (a, b) = (stokes_h[k].intensity(), stokes_z[k].intensity());
            assert!(
                (a - b).abs() <= 2e-3 * a.abs().max(1e-3),
                "seed {rng_seed}, channel {k}: weight {a} vs {b}"
            );
            let (a, b) = (pdf_h[k], pdf_z[k]);
            assert!(
                (a - b).abs() <= 2e-3 * a.abs().max(1e-3),
                "seed {rng_seed}, channel {k}: path pdf {a} vs {b}"
            );
        }
    }
    assert!(
        scattered > 0 && survived > 0,
        "the seeds must exercise both branches ({scattered} scattered, {survived} survived)"
    );
}

/// Through the whole tracer: a scattering stone whose zones are all identical renders like the
/// unzoned stone (the studio rig has no NEE, so only the free-flight inversion differs).
#[test]
fn identical_zones_with_scattering_render_like_the_unzoned_stone() {
    const GRID: usize = 10;
    const SAMPLES: u32 = 24;
    let planes = StandardGemCuts::standard_round_brilliant();
    let scale = absorption_path_scale_for(AbsorptionUnit::PerMm, 7.0, measure_model_width(&planes))
        .expect("measurable width");
    let tensor = band_tensor(560.0, 60.0, 0.08);
    let plain = probe("plain")
        .with_chromophore_absorption(tensor.clone())
        .with_absorption_path_scale(scale)
        .with_scattering(0.5, 0.3);
    let identical = probe("identical zones")
        .with_zoning(zoned(
            ZoneAbsorption::per_mm(tensor.clone()),
            vec![half_space(DVec3::X, ZoneAbsorption::per_mm(tensor))],
        ))
        .with_absorption_path_scale(scale)
        .with_scattering(0.5, 0.3);

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let mean = |material: &GemMaterial| {
        let mut sum = Vec3::ZERO;
        for iy in 0..GRID {
            for ix in 0..GRID {
                let ray =
                    camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
                for s in 0..SAMPLES {
                    let pixel_id = (iy * GRID + ix) as u32;
                    let seed =
                        super::sampling::hash_u32(pixel_id ^ super::sampling::hash_u32(s ^ 0x5CA7));
                    sum += trace_spectral_ray(
                        ray,
                        &planes,
                        material,
                        16,
                        LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
                        seed,
                        (super::sampling::hash_u32(seed) as f32) / 4_294_967_295.0,
                        None,
                    );
                }
            }
        }
        sum / (GRID * GRID) as f32 / SAMPLES as f32
    };
    let a = mean(&plain);
    let b = mean(&identical);
    let rel = ((a - b).abs() / a.abs().max(Vec3::splat(1e-6))).max_element();
    assert!(
        rel < 0.03,
        "identical zones must render like no zones: {a:?} vs {b:?} (relative {rel})"
    );
}

/// The swatches list the base zone first, then the zones in order, each with its own colour.
#[test]
fn zone_swatches_list_the_base_zone_first() {
    let material = probe("swatches")
        .with_zoning(zoned(
            clear(),
            vec![
                half_space(DVec3::X, band_zone(610.0, 40.0, 0.25)),
                half_space(-DVec3::X, band_zone(450.0, 40.0, 0.25)),
            ],
        ))
        .with_absorption_path_scale(3.5);
    // `with_zoning` makes the plain absorption the base zone's, and the material physical.
    assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(material.absorption.o_ray, [] as [AbsorptionBand; 0]);

    let swatches = zone_swatches(&material, 7.0);
    assert_eq!(swatches.len(), 3);
    let [base, red_absorbed, blue_absorbed] = [swatches[0], swatches[1], swatches[2]];
    assert!(
        base.iter().all(|c| *c > 0.9),
        "the clear base zone is near white: {base:?}"
    );
    assert!(
        red_absorbed[0] < red_absorbed[2],
        "a zone absorbing red looks bluish: {red_absorbed:?}"
    );
    assert!(
        blue_absorbed[2] < blue_absorbed[0],
        "a zone absorbing blue looks yellowish: {blue_absorbed:?}"
    );

    assert_eq!(zone_swatches(&probe("plain"), 7.0), [] as [[f32; 3]; 0]);
}
