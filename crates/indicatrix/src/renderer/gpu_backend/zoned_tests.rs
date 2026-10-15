//! GPU backend tests of zoned stones (`gpu` feature). Like the tests next door
//! they skip with a printed note when no adapter is present (`INDICATRIX_REQUIRE_GPU=1` turns
//! the skip into a failure).

use std::sync::atomic::AtomicBool;

use glam::{DVec3, Vec3};

use super::{
    GpuAccumulate, GpuBackend, GpuSceneRef,
    tests::{acquire_or_skip, bits, gpu_required},
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, build_plane_soa},
        zoning::{
            MAX_ZONES, Zone, ZoneAbsorption, ZoneFrame, ZoneKernelF32, ZoneShape, ZonedAbsorption,
        },
    },
    renderer::{
        buffers::GpuZoneTable,
        cpu_frame::trace_pixels_interleaved,
        frame_scene::FrameScene,
        gpu::{
            GpuContext,
            layout_check::{ZoneEchoSegment, run_zone_kernel_echo},
            zoned_cases::{ZONED_PATH_SCALE, ZonedImageCase, mirrored_bicolour_material},
        },
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
        if columns.contains(&(u32::try_from(i).expect("a test picture is small") % width)) {
            sum[0] += f64::from(p.x);
            sum[1] += f64::from(p.y);
            sum[2] += f64::from(p.z);
        }
    }
    sum
}

/// Relative per-channel difference of two total triples.
fn relative_differences(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|k| ((a[k] - b[k]) / b[k]).abs())
}

/// Asserts every channel is within `tolerance`; on failure the message reports ALL channels
/// (not only the first failing one), so a run shows whether Z moves together with X.
fn assert_channels_close(
    label: &str,
    (name_a, a): (&str, [f64; 3]),
    (name_b, b): (&str, [f64; 3]),
    tolerance: f64,
) {
    let rel = relative_differences(a, b);
    assert!(
        rel.iter().all(|r| *r <= tolerance),
        "{label}: {name_a} {a:?} vs {name_b} {b:?}; relative differences per channel \
         (X, Y, Z) {rel:.4?} (limit {tolerance})"
    );
}

fn assert_close(label: &str, gpu: [f64; 3], cpu: [f64; 3]) {
    assert_channels_close(
        label,
        ("GPU", gpu),
        ("CPU", cpu),
        ZONED_GPU_VS_CPU_MEAN_REL_TOL,
    );
}

/// How far the totals of a stone and of its mirror-written twin (same physical stone, base and
/// zone materials swapped) may differ on ONE device. Both renders use the same sample
/// sequence, so only float-order effects of the swapped zone indices remain.
const MIRRORED_SELF_CONSISTENCY_REL_TOL: f64 = 0.01;

/// Segments per fixture in the kernel twin test.
const KERNEL_SEGMENTS: usize = 256;

/// The documented tolerance of the zone-kernel twin, in units of `f32::EPSILON` times the
/// segment length. The WGSL kernel is a function-by-function port of `k32`; the one allowed
/// divergence is the shader compiler contracting `a * b + c` into an fma, worth an ulp or two
/// per operation.
const KERNEL_TWIN_ULPS: f32 = 4.0;

/// The same, for the prism fixtures, whose polygon edges call `sin`/`cos`: the GPU's
/// trigonometry is not the CPU's libm (WGSL only promises a loose absolute bound), so a
/// few-ulp difference in an edge direction reaches the cut points through a division by the
/// direction's projection. Relative to the segment length, about 170 ulp. This is an
/// estimate, not a measured budget, to be replaced by the printed worst case. The grazing-
/// segment arithmetic: an edge-normal error `e` (about 2^-21 for hardware `sin`) moves the cut
/// parameter by `e |q0| / |m_d|`, i.e. `e * 3.5 / (1.19e-7 * |m_d|)` ulp of the segment: 14 ulp
/// at `|m_d| = 1`, 140 at 0.1, 280 at 0.05. A legitimately grazing chord can therefore exceed
/// 170 on a given GPU; the test prints the worst ulp per fixture together with that segment's
/// `|m_d|` proxy so a twin bug (large ulp at large `|m_d|`) can be told from a grazing
/// segment (large ulp only at small `|m_d|`).
const KERNEL_TWIN_ULPS_TRIG: f32 = 170.0;

/// A deterministic xorshift stream, so the segment set is the same on every run.
fn next_unit(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    // The top 24 bits are exact in f32.
    (*state >> 8) as f32 / 16_777_216.0
}

fn kernel_segments(seed: u32) -> Vec<ZoneEchoSegment> {
    let mut state = seed | 1;
    let point = |state: &mut u32| -> [f32; 4] {
        [
            next_unit(state).mul_add(7.0, -3.5),
            next_unit(state).mul_add(7.0, -3.5),
            next_unit(state).mul_add(7.0, -3.5),
            0.0,
        ]
    };
    let mut out = vec![
        // Degenerate (zero length), and the three axis-parallel chords through the origin.
        ZoneEchoSegment {
            from: [0.3, 0.2, 0.1, 0.0],
            to: [0.3, 0.2, 0.1, 0.0],
        },
        ZoneEchoSegment {
            from: [-4.0, 0.0, 0.0, 0.0],
            to: [4.0, 0.0, 0.0, 0.0],
        },
        ZoneEchoSegment {
            from: [0.0, -4.0, 0.0, 0.0],
            to: [0.0, 4.0, 0.0, 0.0],
        },
        ZoneEchoSegment {
            from: [0.0, 0.0, -4.0, 0.0],
            to: [0.0, 0.0, 4.0, 0.0],
        },
    ];
    while out.len() < KERNEL_SEGMENTS {
        let from = point(&mut state);
        let to = point(&mut state);
        out.push(ZoneEchoSegment { from, to });
    }
    out
}

/// The GPU zone kernel (`02_zone_kernel.wgsl`) against `ZoneKernelF32::lengths` (`k32`),
/// segment by segment, for the six fixtures of `zoned_cases` (sharp and soft): the twin claim
/// as a measured fact rather than an image-total statistic.
#[test]
fn the_gpu_zone_kernel_matches_the_f32_kernel_per_segment() {
    let test = "the_gpu_zone_kernel_matches_the_f32_kernel_per_segment";
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                !gpu_required(),
                "{test}: INDICATRIX_REQUIRE_GPU=1 but no GPU adapter could be acquired ({error})"
            );
            println!("skipping {test}: no GPU adapter ({error})");
            return;
        }
    };
    for (index, case) in ZonedImageCase::ALL.into_iter().enumerate() {
        let zoning = case.zoning();
        let table = GpuZoneTable::encode(&zoning).expect("fixtures are GPU-renderable");
        let kernel = ZoneKernelF32::new(&zoning).expect("fixtures have no mesh shell");
        let segments = kernel_segments(
            0x9E37_79B9 ^ (u32::try_from(index).expect("few cases") + 1).wrapping_mul(0x85EB_CA6B),
        );
        let gpu = run_zone_kernel_echo(&ctx, &table, &segments);
        assert_eq!(gpu.len(), segments.len(), "{}", case.label());

        let ulps = if matches!(
            case,
            ZonedImageCase::Watermelon | ZonedImageCase::SoftWatermelon
        ) {
            KERNEL_TWIN_ULPS_TRIG
        } else {
            KERNEL_TWIN_ULPS
        };
        let mut worst = 0.0f32;
        // |m_d| proxy of the worst segment: the smallest absolute component of its unit
        // direction (the edge planes of the fixtures are not visible from here, so this is the
        // axis-grazing measure, not the exact edge-normal projection).
        let mut worst_md = f32::NAN;
        for (segment, got) in segments.iter().zip(&gpu) {
            let from = Vec3::new(segment.from[0], segment.from[1], segment.from[2]);
            let to = Vec3::new(segment.to[0], segment.to[1], segment.to[2]);
            let want = kernel.lengths(from, to);
            let len = (to - from).length();
            let unit = f32::EPSILON * len;
            let md = if len > 0.0 {
                let d = (to - from) / len;
                d.x.abs().min(d.y.abs()).min(d.z.abs())
            } else {
                f32::NAN
            };
            for z in 0..=MAX_ZONES {
                let diff = (got[z] - want[z]).abs();
                if unit > 0.0 && diff / unit > worst {
                    worst = diff / unit;
                    worst_md = md;
                }
                assert!(
                    diff <= ulps * unit,
                    "{}: zone {z} of {from:?} -> {to:?}: GPU {} vs k32 {} differ by {} ulp of \
                     the segment (limit {ulps}); segment |m_d| proxy {md:.4}: a twin bug shows \
                     large ulp at large |m_d|, a grazing segment only at small |m_d| (about \
                     14 ulp at 1, 280 at 0.05)",
                    case.label(),
                    got[z],
                    want[z],
                    diff / unit.max(f32::MIN_POSITIVE)
                );
            }
        }
        println!(
            "{}: {} segments, worst GPU-vs-k32 difference {worst:.2} ulp of the segment \
             (that segment's |m_d| proxy {worst_md:.4})",
            case.label(),
            segments.len()
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

/// The bicolour fixture and its mirror (base and zone materials swapped, half-space normal
/// flipped) are the same physical stone, so each device must render them identically. Run on
/// the GPU and on the CPU separately, this decides where a GPU-vs-CPU bicolour disagreement
/// lives: a pair that disagrees with itself on one device means that device mishandles a
/// plane-crossing segment; both pairs consistent but GPU != CPU means a shared convention.
#[test]
fn a_bicolour_renders_the_same_whichever_half_is_the_base() {
    let Some(backend) = acquire_or_skip("a_bicolour_renders_the_same_whichever_half_is_the_base")
    else {
        return;
    };
    let size = (48u32, 48u32);
    let width = size.0;
    let spp = 24;
    let all = 0..width;
    let plain = ZonedImageCase::Bicolour.material();
    let mirrored = mirrored_bicolour_material();
    assert!(plain.gpu_supported() && mirrored.gpu_supported());

    let (outcome_a, gpu_plain) = render_gpu(&backend, &plain, size, spp);
    let (outcome_b, gpu_mirrored) = render_gpu(&backend, &mirrored, size, spp);
    assert_eq!(outcome_a, GpuAccumulate::Done);
    assert_eq!(outcome_b, GpuAccumulate::Done);
    let cpu_plain = render_cpu(&plain, size, spp);
    let cpu_mirrored = render_cpu(&mirrored, size, spp);

    let gpu_pair = (
        totals(&gpu_plain, width, &all),
        totals(&gpu_mirrored, width, &all),
    );
    let cpu_pair = (
        totals(&cpu_plain, width, &all),
        totals(&cpu_mirrored, width, &all),
    );
    println!(
        "bicolour totals (X, Y, Z): GPU {:?} / mirrored {:?}; CPU {:?} / mirrored {:?}",
        gpu_pair.0, gpu_pair.1, cpu_pair.0, cpu_pair.1
    );
    // Both devices are checked before either verdict, so one run reports both pairs.
    let gpu_rel = relative_differences(gpu_pair.1, gpu_pair.0);
    let cpu_rel = relative_differences(cpu_pair.1, cpu_pair.0);
    let tol = MIRRORED_SELF_CONSISTENCY_REL_TOL;
    let gpu_ok = gpu_rel.iter().all(|r| *r <= tol);
    let cpu_ok = cpu_rel.iter().all(|r| *r <= tol);
    assert!(
        gpu_ok && cpu_ok,
        "mirrored bicolour self-consistency (limit {tol}): GPU pair differs per channel by \
         {gpu_rel:.4?} ({}), CPU pair by {cpu_rel:.4?} ({})",
        if gpu_ok { "consistent" } else { "INCONSISTENT" },
        if cpu_ok { "consistent" } else { "INCONSISTENT" },
    );
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

/// How far the totals of a stone whose half-space zone carries the base's own bands may differ
/// from the unzoned stone, relative, on one device. The zone lengths partition the segment and
/// both alpha paths agree, so only float-order noise remains: measured 1e-9 on the GPU and on
/// the CPU (2026-10-12); the limit leaves three orders of magnitude of room.
const EQUAL_BANDS_REL_TOL: f64 = 1.0e-6;

/// A half-space whose zone carries the SAME bands as the base renders like the unzoned stone,
/// on the GPU and on the CPU each. It brackets the partition and the alpha paths: the zone
/// lengths must sum to the whole segment and the zoned absorption must agree with the
/// homogeneous one, while any error in WHERE the plane cuts cancels (both sides absorb alike).
/// Together with `a_single_half_space_zone_renders_on_the_gpu_like_on_the_cpu_on_either_side`,
/// which proves the split itself, and the per-segment kernel echo, it is what found the
/// uninitialised-array defect of 2026-10-12 (see `zone_fractions_sharp` in `02_zone_kernel.wgsl`).
#[test]
fn a_zone_carrying_the_base_bands_renders_like_the_unzoned_stone() {
    let Some(backend) =
        acquire_or_skip("a_zone_carrying_the_base_bands_renders_like_the_unzoned_stone")
    else {
        return;
    };
    let tensor = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(560.0, 70.0, 0.15)]);
    let plain = GemMaterial::diamond()
        .with_chromophore_absorption(tensor.clone())
        .with_absorption_path_scale(ZONED_PATH_SCALE);
    let split = GemMaterial::diamond()
        .with_zoning(ZonedAbsorption {
            frame: ZoneFrame::IDENTITY,
            base: ZoneAbsorption::per_mm(tensor.clone()),
            zones: vec![Zone {
                shape: ZoneShape::HalfSpace {
                    normal: DVec3::X,
                    offset: 0.0,
                },
                absorption: ZoneAbsorption::per_mm(tensor),
            }],
            boundary_softness_mm: 0.0,
        })
        .with_absorption_path_scale(ZONED_PATH_SCALE);
    let size = (48u32, 48u32);
    let width = size.0;
    let spp = 24;
    let all = 0..width;
    let (plain_outcome, gpu_plain) = render_gpu(&backend, &plain, size, spp);
    let (split_outcome, gpu_split) = render_gpu(&backend, &split, size, spp);
    assert_eq!(plain_outcome, GpuAccumulate::Done);
    assert_eq!(split_outcome, GpuAccumulate::Done);
    let cpu_plain = render_cpu(&plain, size, spp);
    let cpu_split = render_cpu(&split, size, spp);

    let gpu_pair = (
        totals(&gpu_plain, width, &all),
        totals(&gpu_split, width, &all),
    );
    let cpu_pair = (
        totals(&cpu_plain, width, &all),
        totals(&cpu_split, width, &all),
    );
    println!(
        "equal bands totals (X, Y, Z): GPU plain {:?} / split {:?}; CPU plain {:?} / split {:?}",
        gpu_pair.0, gpu_pair.1, cpu_pair.0, cpu_pair.1
    );
    assert_channels_close(
        "equal bands on the GPU",
        ("split", gpu_pair.1),
        ("plain", gpu_pair.0),
        EQUAL_BANDS_REL_TOL,
    );
    assert_channels_close(
        "equal bands on the CPU",
        ("split", cpu_pair.1),
        ("plain", cpu_pair.0),
        EQUAL_BANDS_REL_TOL,
    );
}

/// A clear base with one absorbing half-space zone renders on the GPU like on the CPU, on the
/// +X side and on the -X side of the plane separately: whole-image and per-half channel
/// totals agree within [`ZONED_GPU_VS_CPU_MEAN_REL_TOL`]. It proves the split itself on each
/// side (a wrong side or a wrong cut moves the absorbed light between the halves), which the
/// equal-bands test above cancels by construction. With that test and the per-segment kernel
/// echo it is what found the uninitialised-array defect of 2026-10-12.
#[test]
fn a_single_half_space_zone_renders_on_the_gpu_like_on_the_cpu_on_either_side() {
    let test = "a_single_half_space_zone_renders_on_the_gpu_like_on_the_cpu_on_either_side";
    let Some(backend) = acquire_or_skip(test) else {
        return;
    };
    let size = (48u32, 48u32);
    let width = size.0;
    let spp = 24;
    let all = 0..width;
    let left = 0..width / 2;
    let right = width / 2..width;
    for (label, normal) in [("+X", DVec3::X), ("-X", -DVec3::X)] {
        let material = GemMaterial::diamond()
            .with_zoning(ZonedAbsorption {
                frame: ZoneFrame::IDENTITY,
                base: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new())),
                zones: vec![Zone {
                    shape: ZoneShape::HalfSpace {
                        normal,
                        offset: 0.0,
                    },
                    absorption: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![
                        AbsorptionBand::new(560.0, 70.0, 0.3),
                    ])),
                }],
                boundary_softness_mm: 0.0,
            })
            .with_absorption_path_scale(ZONED_PATH_SCALE);
        let (outcome, gpu) = render_gpu(&backend, &material, size, spp);
        assert_eq!(outcome, GpuAccumulate::Done, "zone on {label}");
        let cpu = render_cpu(&material, size, spp);

        let gpu_all = totals(&gpu, width, &all);
        let cpu_all = totals(&cpu, width, &all);
        let gpu_left = totals(&gpu, width, &left);
        let cpu_left = totals(&cpu, width, &left);
        let gpu_right = totals(&gpu, width, &right);
        let cpu_right = totals(&cpu, width, &right);
        println!(
            "single half-space zone on {label} (X, Y, Z): GPU {gpu_all:?} vs CPU {cpu_all:?} \
             (left half GPU {gpu_left:?} CPU {cpu_left:?}; right half GPU {gpu_right:?} \
             CPU {cpu_right:?})"
        );
        assert_close(&format!("zone on {label}"), gpu_all, cpu_all);
        assert_close(&format!("zone on {label} (left half)"), gpu_left, cpu_left);
        assert_close(
            &format!("zone on {label} (right half)"),
            gpu_right,
            cpu_right,
        );
    }
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
