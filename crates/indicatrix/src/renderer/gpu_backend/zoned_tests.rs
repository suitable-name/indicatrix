//! GPU backend tests of zoned stones (`zoning` + `gpu` features). Like the tests next door
//! they skip with a printed note when no adapter is present (`INDICATRIX_REQUIRE_GPU=1` turns
//! the skip into a failure).

use std::sync::atomic::AtomicBool;

use glam::{DVec3, Vec3};

use super::{
    GpuAccumulate, GpuBackend, GpuSceneRef,
    tests::{acquire_or_skip, bits},
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, build_plane_soa},
        zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
    },
    renderer::{
        cpu_frame::trace_pixels_interleaved,
        frame_scene::FrameScene,
        gpu::zoned_cases::{ZONED_PATH_SCALE, ZonedImageCase},
    },
};

/// How far the GPU and the CPU tracer whole-image channel totals (and the totals of each
/// image half) may differ, relative. `gpu_backend` has no GPU-vs-CPU tolerance of its own
/// (its other tests compare the GPU with itself); this is a deliberately loose bound for two
/// unbiased estimators at 48 x 48 x 24 samples, where the sampling noise of a total is well
/// below 1 percent. The statistically exact comparison is the Tier 3 z-score check
/// (`estimator_check::run_image_comparison_zoned`, reported by the GPU harness).
const ZONED_GPU_VS_CPU_MEAN_REL_TOL: f64 = 0.03;

const MAX_BOUNCES: u32 = 6;

fn camera() -> Camera {
    Camera::new(0.35, 0.28, 5.0, 18.0)
}

fn render_gpu(
    backend: &GpuBackend,
    material: &GemMaterial,
    (width, height): (u32, u32),
    spp: u32,
) -> (GpuAccumulate, Vec<Vec3>) {
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let scene = GpuSceneRef {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material,
        max_bounces: MAX_BOUNCES,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut accum = vec![Vec3::ZERO; (width * height) as usize];
    let never_cancel = AtomicBool::new(false);
    let outcome = backend.try_accumulate_cancellable(&scene, 0, spp, &mut accum, &never_cancel);
    (outcome, accum)
}

fn render_cpu(material: &GemMaterial, (width, height): (u32, u32), spp: u32) -> Vec<Vec3> {
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let scene = FrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material,
        max_bounces: MAX_BOUNCES,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let soa = build_plane_soa(&planes);
    trace_pixels_interleaved(&scene, &soa, 0, 1, 0, spp)
}

fn totals(pixels: &[Vec3], width: u32, columns: &std::ops::Range<u32>) -> [f64; 3] {
    let mut sum = [0.0f64; 3];
    for (i, p) in pixels.iter().enumerate() {
        if columns.contains(&(i as u32 % width)) {
            sum[0] += f64::from(p.x);
            sum[1] += f64::from(p.y);
            sum[2] += f64::from(p.z);
        }
    }
    sum
}

fn assert_close(label: &str, gpu: [f64; 3], cpu: [f64; 3]) {
    for k in 0..3 {
        let rel = ((gpu[k] - cpu[k]) / cpu[k]).abs();
        assert!(
            rel <= ZONED_GPU_VS_CPU_MEAN_REL_TOL,
            "{label}: channel {k} GPU {} vs CPU {} differ by {rel:.4} (limit {})",
            gpu[k],
            cpu[k],
            ZONED_GPU_VS_CPU_MEAN_REL_TOL
        );
    }
}

/// A zoned stone renders on the GPU like it does on the CPU: whole-image and per-half
/// channel totals agree within [`ZONED_GPU_VS_CPU_MEAN_REL_TOL`], for sharp and soft zones of
/// every shape family.
#[test]
fn zoned_stones_render_on_the_gpu_like_on_the_cpu() {
    let Some(backend) = acquire_or_skip("zoned_stones_render_on_the_gpu_like_on_the_cpu") else {
        return;
    };
    let size = (48u32, 48u32);
    let width = size.0;
    let spp = 24;
    for case in ZonedImageCase::ALL {
        let material = case.material();
        assert!(material.gpu_supported(), "{}", case.label());
        let (outcome, gpu) = render_gpu(&backend, &material, size, spp);
        assert_eq!(outcome, GpuAccumulate::Done, "{}", case.label());
        let cpu = render_cpu(&material, size, spp);
        for (suffix, columns) in [
            ("", 0..width),
            (" (left half)", 0..width / 2),
            (" (right half)", width / 2..width),
        ] {
            assert_close(
                &format!("{}{suffix}", case.label()),
                totals(&gpu, width, &columns),
                totals(&cpu, width, &columns),
            );
        }
    }
}

/// One zone that covers the whole stone, with the absorption of a plain stone, renders bit
/// for bit like that plain stone on the GPU (the zone lengths are used as fractions of the
/// segment, so a segment inside one zone keeps exactly `path * scale`).
#[test]
fn a_zone_covering_the_whole_stone_renders_bitwise_like_the_unzoned_stone() {
    let Some(backend) =
        acquire_or_skip("a_zone_covering_the_whole_stone_renders_bitwise_like_the_unzoned_stone")
    else {
        return;
    };
    let tensor = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(560.0, 70.0, 0.15)]);
    let plain = GemMaterial::diamond()
        .with_chromophore_absorption(tensor.clone())
        .with_absorption_path_scale(ZONED_PATH_SCALE);
    let covering = GemMaterial::diamond()
        .with_zoning(ZonedAbsorption {
            frame: ZoneFrame::IDENTITY,
            base: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new())),
            zones: vec![Zone {
                // Everything of the stone is on the inner side of this plane.
                shape: ZoneShape::HalfSpace {
                    normal: DVec3::X,
                    offset: -1.0e6,
                },
                absorption: ZoneAbsorption::per_mm(tensor),
            }],
            boundary_softness_mm: 0.0,
        })
        .with_absorption_path_scale(ZONED_PATH_SCALE);
    let clear = GemMaterial::diamond().with_absorption_path_scale(ZONED_PATH_SCALE);

    let size = (40u32, 40u32);
    let (plain_outcome, plain_image) = render_gpu(&backend, &plain, size, 4);
    let (zoned_outcome, zoned_image) = render_gpu(&backend, &covering, size, 4);
    let (clear_outcome, clear_image) = render_gpu(&backend, &clear, size, 4);
    assert_eq!(plain_outcome, GpuAccumulate::Done);
    assert_eq!(zoned_outcome, GpuAccumulate::Done);
    assert_eq!(clear_outcome, GpuAccumulate::Done);
    assert_ne!(
        bits(&plain_image),
        bits(&clear_image),
        "the probe absorption must be visible, or the comparison proves nothing"
    );
    assert_eq!(bits(&zoned_image), bits(&plain_image));
}

/// A mesh-shell zone declines the GPU (`gpu_supported` is false), leaving the caller buffer
/// untouched and the backend healthy, so the CPU tracer renders it.
#[test]
fn a_mesh_shell_zone_declines_the_gpu() {
    let Some(backend) = acquire_or_skip("a_mesh_shell_zone_declines_the_gpu") else {
        return;
    };
    let mesh = Zone {
        shape: ZoneShape::MeshShell {
            vertices: vec![
                [-1.0, -1.0, -1.0],
                [1.0, -1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [-1.0, -1.0, 1.0],
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
        },
        absorption: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            560.0, 70.0, 0.15,
        )])),
    };
    let material = GemMaterial::diamond()
        .with_zoning(ZonedAbsorption {
            frame: ZoneFrame::IDENTITY,
            base: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new())),
            zones: vec![mesh],
            boundary_softness_mm: 0.0,
        })
        .with_absorption_path_scale(ZONED_PATH_SCALE);
    assert!(!material.gpu_supported());
    let (outcome, accum) = render_gpu(&backend, &material, (16, 16), 2);
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(accum.iter().all(|p| *p == Vec3::ZERO));
    assert!(!backend.is_lost(), "a decline is not a device loss");
}
