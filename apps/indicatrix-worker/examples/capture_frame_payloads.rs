//! Captures real `FRAME` payloads to disk for the payload codec measurement
//! (`payload_codec_bench`).
//!
//! Renders one full-resolution delta per case with the production render core
//! ([`indicatrix_worker::render_core::trace_samples_with_gpu`], GPU when built with
//! `--features gpu` and an adapter is present, the CPU tracer otherwise) and writes the
//! exact bytes the emitter would put on the wire (`indicatrix_net::radiance::as_bytes`,
//! the same call `stream_emit::emitter::emit` makes). Nothing here touches the protocol.
//!
//! The delta is traced in sub-batches of at most [`SUBBATCH`] samples and summed with
//! `+=`, the way the emitter's pending delta folds the tracer's sub-batches.
//!
//! Cases: scenes studio (the GUI default `RingLights`) and light tent; materials
//! diamond and sapphire; 1080p and 4K; 8, 64 and 512 samples per delta. A case whose
//! projected trace time exceeds [`MAX_CAPTURE_SECS`] is skipped and reported. Files that
//! already exist at full size are kept, so an interrupted run resumes where it stopped.
//!
//! ```text
//! cargo run -p indicatrix-worker --features gpu --profile probe \
//!     --example capture_frame_payloads -- <output-dir>
//! ```
//!
//! Output files are named `<scene>-<material>-<w>x<h>-spp<n>.xyz` (raw little-endian
//! f32 XYZ sums, 12 bytes per pixel), which `payload_codec_bench` parses.

use std::{
    error::Error,
    path::{Path, PathBuf},
    time::Instant,
};

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{BACKDROP_GREY, LightingPreset},
    },
    renderer::gpu_backend::GpuBackend,
};
use indicatrix_net::{SceneState, radiance};
use indicatrix_worker::render_core::trace_samples_with_gpu;

/// Largest sub-batch traced in one call before folding into the delta.
const SUBBATCH: u32 = 64;

/// Default cap: a capture projected to take longer than this is skipped (a
/// ~2 min cap). An optional second argument overrides it, in seconds.
const MAX_CAPTURE_SECS: f64 = 120.0;

/// Per-delta sample counts, ascending so the projection has a measured rate to go on.
const SAMPLE_COUNTS: [u32; 3] = [8, 64, 512];

/// Frame sizes: 1080p and 4K UHD.
const SIZES: [(u32, u32); 2] = [(1920, 1080), (3840, 2160)];

/// Builds the scene the GUI would send for its default camera and light pose.
fn scene(preset: LightingPreset, material: &GemMaterial, width: u32, height: u32) -> SceneState {
    SceneState {
        width,
        height,
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 12,
        lighting_preset: preset,
        material: material.clone(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: BACKDROP_GREY,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: Default::default(),
    }
}

/// Traces one delta of `samples` samples starting at `first_sample`, folding
/// sub-batches of at most [`SUBBATCH`] into one summed buffer.
fn trace_delta(gpu: &GpuBackend, scene: &SceneState, first_sample: u32, samples: u32) -> Vec<Vec3> {
    let mut delta = vec![Vec3::ZERO; scene.width as usize * scene.height as usize];
    let mut done = 0;
    while done < samples {
        let n = SUBBATCH.min(samples - done);
        let sub = trace_samples_with_gpu(gpu, scene, first_sample + done, n, 0);
        for (acc, s) in delta.iter_mut().zip(&sub) {
            *acc += *s;
        }
        done += n;
    }
    delta
}

/// Renders and writes one case; returns the trace time in seconds.
fn capture_one(
    gpu: &GpuBackend,
    scene: &SceneState,
    samples: u32,
    path: &Path,
) -> Result<f64, Box<dyn Error>> {
    let start = Instant::now();
    // The second delta of a stream: [samples, 2 * samples).
    let delta = trace_delta(gpu, scene, samples, samples);
    let secs = start.elapsed().as_secs_f64();
    std::fs::write(path, radiance::as_bytes(&delta))?;
    Ok(secs)
}

/// Whether `path` already holds a full-size capture for `scene` (lets an interrupted
/// run resume without re-rendering what it already wrote).
fn is_complete(path: &Path, scene: &SceneState) -> bool {
    let expected =
        u64::from(scene.width) * u64::from(scene.height) * radiance::BYTES_PER_PIXEL as u64;
    std::fs::metadata(path).is_ok_and(|m| m.len() == expected)
}

/// Seconds per sample for `scene`, measured on a discarded 8-sample delta; used to
/// project a larger capture's time when no earlier case at this size was traced.
fn probe_rate(gpu: &GpuBackend, scene: &SceneState) -> f64 {
    let start = Instant::now();
    drop(trace_delta(gpu, scene, 0, SAMPLE_COUNTS[0]));
    start.elapsed().as_secs_f64() / f64::from(SAMPLE_COUNTS[0])
}

/// Entry point: `capture_frame_payloads <output-dir> [max-capture-secs]`.
fn main() -> Result<(), Box<dyn Error>> {
    let out_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: capture_frame_payloads <output-dir> [max-capture-secs]")?,
    );
    let max_secs = std::env::args()
        .nth(2)
        .map_or(Ok(MAX_CAPTURE_SECS), |arg| arg.parse::<f64>())?;
    std::fs::create_dir_all(&out_dir)?;

    let gpu = GpuBackend::acquire();
    println!(
        "adapter: {}",
        gpu.adapter_label()
            .unwrap_or_else(|| "none (CPU tracer)".to_string())
    );

    let scenes = [
        ("studio", LightingPreset::RingLights),
        ("tent", LightingPreset::LightTent),
    ];
    let materials = [
        ("diamond", GemMaterial::diamond()),
        ("sapphire", GemMaterial::sapphire()),
    ];

    // Warm-up: the first dispatch compiles/uploads; keep it out of the timings.
    let _ = trace_samples_with_gpu(&gpu, &scene(scenes[0].1, &materials[0].1, 64, 64), 0, 1, 0);

    for (scene_name, preset) in scenes {
        for (material_name, material) in &materials {
            for (width, height) in SIZES {
                let state = scene(preset, material, width, height);
                let mut secs_per_sample = 0.0_f64;
                for samples in SAMPLE_COUNTS {
                    let name =
                        format!("{scene_name}-{material_name}-{width}x{height}-spp{samples}.xyz");
                    let path = out_dir.join(&name);
                    if is_complete(&path, &state) {
                        println!("{name}: already captured, kept");
                        continue;
                    }
                    if secs_per_sample == 0.0 && samples > SAMPLE_COUNTS[0] {
                        secs_per_sample = probe_rate(&gpu, &state);
                    }
                    let projected = secs_per_sample * f64::from(samples);
                    if projected > max_secs {
                        println!("{name}: SKIPPED (projected {projected:.0} s)");
                        continue;
                    }
                    let secs = capture_one(&gpu, &state, samples, &path)?;
                    secs_per_sample = secs / f64::from(samples);
                    println!("{name}: {secs:.2} s");
                }
            }
        }
    }
    Ok(())
}
