//! CPU frame tracer throughput (BLD-2; findings PERF-5/PERF-6/PERF-9 in
//! `review_perf_2026-10-12_fable.md`, section 5, table row 4).
//!
//! Each iteration traces 1 000 samples through `trace_pixels_interleaved` on a 400x300
//! frame: 1 000 pixels (every 120th) at 1 sample each, the same per-sample path the cut
//! viewport, the worker and the browser Web Workers run (stone bounces, `StudioRig` setup
//! per sample, `extraordinary_index_at` for the uniaxial stone). Single thread, so the
//! number is samples per core. Expected before any fix: tens to a few hundred
//! microseconds per sample; after PERF-5 (per-sample `StudioRig`) +5-10 % and after PERF-6
//! +2-5 % in samples/s. Isotropic is Diamond, uniaxial is Ruby; Studio is the ring-light
//! rig, HDR is a procedural 64x32 sky map with one bright sun texel.
//!
//! Run: `cargo bench -p indicatrix --bench tracer_cpu`.

use divan::black_box;
use indicatrix::{
    geometry::StandardGemCuts,
    optics::{
        GemMaterial,
        raytracer::{
            Camera, DEFAULT_MAX_BOUNCES, EnvironmentSource, LightingPreset, build_plane_soa,
        },
    },
    renderer::{
        cpu_frame::trace_pixels_interleaved, env_map::EnvironmentMap, frame_scene::FrameScene,
    },
};

fn main() {
    divan::main();
}

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
/// Every 120th pixel of the 120 000: 1 000 pixels, one sample each.
const STRIDE: u32 = 120;

/// A procedural equirectangular sky: a vertical gradient plus one bright sun texel.
fn procedural_hdr() -> EnvironmentMap {
    let (w, h) = (64usize, 32usize);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        for x in 0..w {
            let sun = if x == 40 && y == 8 { 400.0 } else { 0.0 };
            pixels.push([
                0.6f32.mul_add(1.0 - t, 0.2) + sun,
                0.5f32.mul_add(1.0 - t, 0.25) + sun,
                0.7f32.mul_add(1.0 - t, 0.3) + sun,
            ]);
        }
    }
    EnvironmentMap::from_rgb(w, h, pixels).expect("pixel count matches the dimensions")
}

/// Traces the 1 000-sample batch against `environment` for `material`.
fn run(bencher: divan::Bencher, material: &GemMaterial, hdr: bool) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let soa = build_plane_soa(&planes);
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let map = procedural_hdr();
    let environment = if hdr {
        EnvironmentSource::HdrMap(&map)
    } else {
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
    };
    let scene = FrameScene {
        camera: &camera,
        width: WIDTH,
        height: HEIGHT,
        planes: &planes,
        facet_finishes: &[],
        material,
        max_bounces: DEFAULT_MAX_BOUNCES,
        environment,
    };
    bencher.bench_local(|| {
        black_box(trace_pixels_interleaved(
            black_box(&scene),
            black_box(&soa),
            0,
            STRIDE,
            0,
            1,
        ))
    });
}

/// Studio rig, isotropic stone.
#[divan::bench(sample_count = 30)]
fn studio_isotropic(bencher: divan::Bencher) {
    run(bencher, &GemMaterial::diamond(), false);
}

/// Studio rig, uniaxial stone.
#[divan::bench(sample_count = 30)]
fn studio_uniaxial(bencher: divan::Bencher) {
    run(bencher, &GemMaterial::ruby(), false);
}

/// HDR map, isotropic stone.
#[divan::bench(sample_count = 30)]
fn hdr_isotropic(bencher: divan::Bencher) {
    run(bencher, &GemMaterial::diamond(), true);
}

/// HDR map, uniaxial stone.
#[divan::bench(sample_count = 30)]
fn hdr_uniaxial(bencher: divan::Bencher) {
    run(bencher, &GemMaterial::ruby(), true);
}
