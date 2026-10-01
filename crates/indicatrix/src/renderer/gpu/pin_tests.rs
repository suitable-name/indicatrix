//! Identity pin for [`hybrid::cpu_accumulate`], recorded BEFORE the refactor
//! that moved its scene type ([`GpuFrameScene`]) and its per-pixel tracer
//! (`cpu_sample_xyz`/`cpu_trace_range`) out to backend-independent homes
//! (`renderer::frame_scene`, `renderer::cpu_frame`). This is a pure-refactor
//! project: the estimator's output must not change by one bit, and this hash is the
//! external check that it hasn't -- see `meet-solver-external-verification`-style
//! reasoning applied to a render estimator instead of a solver.
//!
//! Two small, fixed scenes (24x16 px, 3 spp, at two different `sample_offset`s so the
//! Cranley-Patterson rotation and hero-wavelength draw both exercise more than sample
//! index 0): a Diamond round brilliant under the Light tent preset (all-`Polished`
//! facets), and a birefringent Zircon round brilliant with a frosted girdle under the
//! Daylight preset. Each pins one FNV-1a hash of every output pixel's XYZ, taken over
//! `f32::to_bits` so the hash is exact bit-pattern equality, not a float comparison.

use glam::Vec3;

use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{BACKDROP_GREY, Camera, FacetFinish, LightingPreset, build_plane_soa},
    },
    renderer::cpu_frame::{scatter_interleaved, trace_pixels_interleaved},
};

use super::{
    estimator_check::bruted_girdle_finishes, frame::GpuFrameScene, hybrid::cpu_accumulate,
};

const WIDTH: u32 = 24;
const HEIGHT: u32 = 16;

/// FNV-1a (64-bit) over the little-endian `f32::to_bits` bytes of every accumulated
/// pixel's X, Y, Z in order -- an exact bit-pattern digest, not a float comparison.
fn fnv1a_hash_xyz(buf: &[Vec3]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for pixel in buf {
        for component in [pixel.x, pixel.y, pixel.z] {
            for byte in component.to_bits().to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
    }
    hash
}

/// Renders `scene` with [`cpu_accumulate`] into a fresh zeroed buffer and hashes it.
fn hash_cpu_accumulate(scene: &GpuFrameScene<'_>, sample_offset: u32, spp: u32) -> u64 {
    let num_pixels = scene.width as usize * scene.height as usize;
    let mut accum = vec![Vec3::ZERO; num_pixels];
    cpu_accumulate(scene, sample_offset, spp, &mut accum);
    fnv1a_hash_xyz(&accum)
}

/// Diamond round brilliant, all-`Polished` facets, Light tent preset -- the everyday
/// case: an isotropic material, no frosted-bounce branch.
const fn diamond_scene<'a>(
    camera: &'a Camera,
    planes: &'a [crate::geometry::GpuFacetPlane],
    material: &'a GemMaterial,
) -> GpuFrameScene<'a> {
    GpuFrameScene {
        camera,
        width: WIDTH,
        height: HEIGHT,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: 8,
        environment: LightingPreset::LightTent
            .studio(1.0, 0.4, 0.35)
            .with_backdrop(BACKDROP_GREY),
    }
}

/// Zircon (uniaxial birefringent) round brilliant with a frosted girdle band, Daylight
/// preset -- exercises the ordinary/extraordinary eigenmode split and the
/// `apply_frosted_bounce` branch together.
const fn zircon_frosted_scene<'a>(
    camera: &'a Camera,
    planes: &'a [crate::geometry::GpuFacetPlane],
    finishes: &'a [FacetFinish],
    material: &'a GemMaterial,
) -> GpuFrameScene<'a> {
    GpuFrameScene {
        camera,
        width: WIDTH,
        height: HEIGHT,
        planes,
        facet_finishes: finishes,
        material,
        max_bounces: 8,
        environment: LightingPreset::Daylight
            .studio(1.0, 0.5, 0.6)
            .with_backdrop(BACKDROP_GREY),
    }
}

/// `cpu_accumulate`'s output for the diamond fixture must not change by one bit.
///
/// Hash recorded before the refactor (`GpuFrameScene` moved to
/// `renderer::frame_scene::FrameScene`, the per-pixel tracer moved to
/// `renderer::cpu_frame`); this test was run once after the refactor to confirm the
/// hash is unchanged, per that lane's brief.
///
/// Re-pinned on 2026-10-01 when the test profile started optimising this crate
/// (`[profile.test.package.indicatrix]` in the workspace manifest): the studio sampler
/// rounds a few ULP differently between the unoptimised and the optimised build, and the
/// optimised test profile and a release build agree on these hashes.
///
/// The hashes are Windows bits. The studio sampler's `exp` runs through the platform
/// math library once per sample, and glibc rounds a few arguments differently from the
/// Windows runtime, so some samples, and with them a hash, differ on Linux (measured
/// 2026-10-01: the hash at sample offset 0 agrees, the one at offset 5 does not). The
/// test is therefore ignored off Windows; running it there with `--ignored` prints that
/// platform's hashes in the failure message, which is all a per-platform table needs.
#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding; run with --ignored to read \
              this platform's hashes"
)]
fn cpu_accumulate_diamond_light_tent_pin() {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let scene = diamond_scene(&camera, &planes, &material);
    assert_pinned(
        "diamond, light tent",
        &scene,
        [0xc464_d298_6d52_47d5, 0x9ebb_4ae1_6688_88ea],
    );
}

/// `cpu_accumulate`'s output for the frosted, birefringent Zircon fixture must not
/// change by one bit. See [`cpu_accumulate_diamond_light_tent_pin`] for the pin's
/// purpose, for the 2026-10-01 re-pin to the optimised test profile's bits, and for why
/// it is ignored off Windows; all of it applies here too.
#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding; run with --ignored to read \
              this platform's hashes"
)]
fn cpu_accumulate_zircon_frosted_daylight_pin() {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let finishes = bruted_girdle_finishes(planes.len());
    let material = GemMaterial::by_name("Zircon").expect("Zircon is a built-in material");
    let scene = zircon_frosted_scene(&camera, &planes, &finishes, &material);
    assert_pinned(
        "zircon, frosted girdle, daylight",
        &scene,
        [0x4271_13db_ce1c_850b, 0x7206_6f28_bfed_cef1],
    );
}

/// Hashes `scene` at sample offsets 0 and 5 (3 spp each) against `expected` and reports
/// every mismatch in one message, with the actual hashes in the table's own form, so a
/// single run is enough to re-pin both.
fn assert_pinned(label: &str, scene: &GpuFrameScene<'_>, expected: [u64; 2]) {
    let mut changed = Vec::new();
    for (offset, want) in [0u32, 5].into_iter().zip(expected) {
        let got = hash_cpu_accumulate(scene, offset, 3);
        if got != want {
            changed.push(format!("sample_offset={offset}: {got:#018x}"));
        }
    }
    assert!(
        changed.is_empty(),
        "cpu_accumulate({label}, spp=3) changed bit-for-bit; actual hashes: {}",
        changed.join(", ")
    );
}

/// Bit-pattern digest of every pixel's X/Y/Z, for exact (not IEEE-`==`) equality
/// between two independently computed buffers.
fn xyz_bits(buf: &[Vec3]) -> Vec<(u32, u32, u32)> {
    buf.iter()
        .map(|v| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits()))
        .collect()
}

/// Traces the whole frame by partitioning its pixels into `stride` interleaved sets
/// (one [`trace_pixels_interleaved`] call per `first_pixel` in `0..stride`) and
/// scattering each partition's sums back into one buffer with [`scatter_interleaved`]
/// -- the same shape of work `hybrid::cpu_trace_range`'s thread pool (and a browser's
/// Worker pool) both do, just single-threaded here since only the RESULT is under
/// test.
fn trace_whole_frame_via_stride(
    scene: &GpuFrameScene<'_>,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    let num_pixels = scene.width as usize * scene.height as usize;
    let mut out = vec![Vec3::ZERO; num_pixels];
    let plane_soa = build_plane_soa(scene.planes);
    for first_pixel in 0..stride {
        let sums =
            trace_pixels_interleaved(scene, &plane_soa, first_pixel, stride, sample_offset, spp);
        scatter_interleaved(&mut out, first_pixel, stride, &sums);
    }
    out
}

/// [`trace_pixels_interleaved`]/[`scatter_interleaved`] must reproduce
/// [`cpu_accumulate`] bit-for-bit regardless of how the frame is partitioned -- see
/// `cpu_frame`'s own "Determinism and partitioning" doc. Stride 1 is the trivial case
/// (one partition, every pixel); strides 3 and 7 split the 24x16 frame (384 pixels)
/// into several INTERLEAVED partitions, and 7 does not divide 384 evenly, matching how
/// a desktop thread pool or a browser's Worker pool would actually split a frame.
#[test]
fn trace_pixels_interleaved_recombines_to_cpu_accumulate_for_several_strides() {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let scene = diamond_scene(&camera, &planes, &material);

    let num_pixels = (WIDTH * HEIGHT) as usize;
    let mut reference = vec![Vec3::ZERO; num_pixels];
    cpu_accumulate(&scene, 0, 3, &mut reference);
    let reference_bits = xyz_bits(&reference);

    for stride in [1, 3, 7] {
        let recombined = trace_whole_frame_via_stride(&scene, stride, 0, 3);
        assert_eq!(
            xyz_bits(&recombined),
            reference_bits,
            "stride {stride} did not recombine to cpu_accumulate's output bit-for-bit"
        );
    }
}
