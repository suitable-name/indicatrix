use glam::{Mat3, Vec3};

use crate::{
    geometry::StandardGemCuts,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, build_plane_soa},
    },
    renderer::{cpu_frame::scatter_interleaved, env_map::EnvironmentMap, frame_scene::FrameScene},
};

use super::{
    WhittedPrep, aa::OFFSETS, aa_offset, chain::trace_chain, whitted_pixels_interleaved_geom,
    whitted_tap_xyz,
};

/// The radical inverse in f64, so the f32 table is compared with the mathematical value rather
/// than with a second f32 evaluation that rounds in a different order.
fn inverse(mut index: u32, base: u32) -> f64 {
    let mut value = 0.0;
    let mut divisor = f64::from(base);
    while index != 0 {
        value += f64::from(index % base) / divisor;
        index /= base;
        divisor *= f64::from(base);
    }
    value
}

#[test]
fn taps_are_centre_first_and_inside_the_pixel() {
    assert_eq!(aa_offset(16, 0), (0.0, 0.0));
    for (index, &(x, y)) in OFFSETS.iter().enumerate() {
        let index = u32::try_from(index).expect("sixteen offsets fit u32");
        assert!((-0.5..0.5).contains(&x) && (-0.5..0.5).contains(&y));
        for (actual, base) in [(x, 2), (y, 3)] {
            let expected = (inverse(index, base) + 0.5).fract() - 0.5;
            // The f32 table sums at most three terms of `digit * place`, where `place` is the
            // repeated product of the rounded reciprocal (relative error (k + 1) * 2^-24 at
            // place k). The terms contribute at most 2.4 * 2^-24, the three sum roundings at most
            // 1.5 * 2^-24 and the `+ 0.5` shift at most 2^-24: 5 * 2^-24 bounds the error.
            assert!((f64::from(actual) - expected).abs() <= 5.0 * 2.0_f64.powi(-24));
        }
        for taps in [1u8, 4, 8, 16] {
            assert_eq!(
                aa_offset(taps, index + u32::from(taps)),
                aa_offset(taps, index)
            );
        }
    }
}

#[test]
fn partitions_recombine_bit_exactly() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let env = EnvironmentMap::uniform(4, 2, [0.7, 0.5, 0.3]);
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
    let soa = build_plane_soa(&planes);
    let prep = WhittedPrep::for_tests(
        1.5,
        Vec3::new(0.2, 0.1, 0.3),
        Vec3::new(0.7, 0.5, 0.3),
        64,
        8,
    );
    let full = whitted_pixels_interleaved_geom(&scene, &[], &soa, &prep, 0, 1, 0, 8);
    for stride in [1, 3, 7] {
        let mut reconstructed = vec![Vec3::ZERO; full.len()];
        for first in 0..stride {
            let owned =
                whitted_pixels_interleaved_geom(&scene, &[], &soa, &prep, first, stride, 0, 8);
            scatter_interleaved(&mut reconstructed, first, stride, &owned);
        }
        assert_eq!(full, reconstructed);
    }
    // Splitting into individual absolute taps retains the same summation order.
    let mut per_tap = vec![Vec3::ZERO; full.len()];
    for tap in 0..8 {
        let owned = whitted_pixels_interleaved_geom(&scene, &[], &soa, &prep, 0, 1, tap, 1);
        scatter_interleaved(&mut per_tap, 0, 1, &owned);
    }
    assert_eq!(full, per_tap);
}

#[test]
fn a_uniform_environment_gives_a_uniform_picture() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::new_custom("clear", 1.5, 0.0, 0.0, [0.0; 3]);
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let env = EnvironmentMap::uniform(4, 2, [0.7, 0.5, 0.3]);
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
    let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::new(0.7, 0.5, 0.3), 64, 4);
    let expected = prep.rgb_to_xyz * Vec3::new(0.7, 0.5, 0.3);
    let frame =
        whitted_pixels_interleaved_geom(&scene, &[], &build_plane_soa(&planes), &prep, 0, 1, 0, 4);
    for xyz in frame {
        assert!(
            ((xyz / 4.0 - expected) / expected).abs().max_element() < 0.002,
            "{xyz:?} vs {expected:?}"
        );
    }
}

#[test]
fn the_primary_hit_is_reported_once_and_a_miss_clears_it() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let env = EnvironmentMap::uniform(4, 2, [1.0; 3]);
    let mut scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment: EnvironmentSource::HdrMap(&env),
    };
    let soa = build_plane_soa(&planes);
    let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::ONE, 64, 1);
    let mut hit = None;
    let _ = whitted_tap_xyz(&scene, &[], &soa, &prep, 8 * 24 + 12, 0, Some(&mut hit));
    assert!(hit.is_some());
    scene.planes = &[];
    let empty = build_plane_soa(&[]);
    let _ = whitted_tap_xyz(&scene, &[], &empty, &prep, 0, 0, Some(&mut hit));
    assert!(hit.is_none());
    assert_eq!(
        whitted_pixels_interleaved_geom(&scene, &[], &empty, &prep, 0, 0, 0, 1),
        Vec::<Vec3>::new()
    );
}

#[test]
fn fire_splits_the_channels_by_snell() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::new_custom("dispersive clear", 1.7, 0.2, 0.0, [0.0; 3]);
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let pixels = (0..32)
        .flat_map(|y| {
            (0..64).map(move |x| {
                let value = (y as f32 / 32.0).mul_add(0.2, (x as f32 / 64.0).mul_add(0.7, 0.1));
                [value; 3]
            })
        })
        .collect();
    let env = EnvironmentMap::from_rgb(64, 32, pixels).expect("directional map");
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
    let mut prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        crate::renderer::engine::RayOptions {
            aa_taps: 1,
            fire: true,
        },
    )
    .expect("supported scene");
    assert_eq!(prep.chain_count, 3);
    assert!(prep.chains[0].n < prep.chains[2].n);
    // Inspect linear RGB directly: each channel must follow its own scalar
    // Snell path, rather than tinting one shared, green-index path afterward.
    prep.rgb_to_xyz = Mat3::IDENTITY;
    let soa = build_plane_soa(&planes);
    let mut separation = 0.0f32;
    for pixel in 0..24 * 16 {
        let ray = camera.generate_ray(
            (pixel % 24) as f32,
            (pixel / 24) as f32,
            24.0,
            16.0,
            0.0,
            0.0,
        );
        let expected: [f32; 3] = std::array::from_fn(|channel| {
            trace_chain(
                &scene,
                &[],
                &soa,
                &prep,
                ray,
                prep.chains[channel].n,
                -ray.dir,
                None,
            )[channel]
        });
        let actual = whitted_tap_xyz(&scene, &[], &soa, &prep, pixel, 0, None);
        assert!((actual - Vec3::from_array(expected)).abs().max_element() < 1e-6);
        separation = separation.max((actual.x - actual.z).abs());
    }
    assert!(
        separation > 1e-4,
        "dispersion must move the paths in directional light"
    );
}

#[test]
#[ignore = "pinned at Gate A"]
fn pinned_tiny_frame() {
    if !cfg!(target_os = "windows") {
        return;
    }
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let env = EnvironmentMap::uniform(4, 2, [0.7, 0.5, 0.3]);
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
    let prep = WhittedPrep::for_tests(
        1.5,
        Vec3::new(0.2, 0.1, 0.3),
        Vec3::new(0.7, 0.5, 0.3),
        64,
        8,
    );
    let frame =
        whitted_pixels_interleaved_geom(&scene, &[], &build_plane_soa(&planes), &prep, 0, 1, 0, 8);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for pixel in frame {
        for channel in pixel.to_array() {
            for byte in channel.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    println!("whitted_tiny_frame={hash}");
}
