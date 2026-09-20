//! End-to-end smoke test of the hybrid CPU+GPU export path, kept separate from
//! [`super::tests`] and moved out of `mod.rs` alongside [`super::worker::run_export`]
//! to keep that file from growing further.

use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};

/// End-to-end smoke test of the hybrid export path: a small frame rendered
/// through the real `run_export` (calibration, batching, merge, PNG write).
/// Without a GPU adapter or the `gpu` feature this exercises the CPU-only
/// path; with both, it exercises calibration plus concurrent hybrid
/// batches. Asserts completion and a non-empty file, not pixel values (the
/// hybrid sample-to-engine assignment is calibration-dependent by design).
#[test]
fn export_completes_via_hybrid_or_cpu_path() {
    let scene = SceneSnapshot {
        yaw: 0.6,
        pitch: -0.4,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::by_name("Zircon").expect("built-in material"),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 12,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        env_map: None,
    };
    let params = ExportParams {
        width: 48,
        height: 48,
        samples_per_pixel: 12,
        max_bounces: 12,
    };
    let out = std::env::temp_dir().join("indicatrix_hybrid_export_smoke.png");
    let cancel = AtomicBool::new(false);
    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &out,
        ComputeTarget::LocalOnly,
        &[],
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {},
    );
    match outcome {
        ExportOutcome::Completed(path) => {
            let len = std::fs::metadata(&path).map_or(0, |m| m.len());
            assert!(len > 0, "export wrote an empty file");
            let _ = std::fs::remove_file(path);
        }
        ExportOutcome::Cancelled => panic!("export reported cancelled without a cancel"),
        ExportOutcome::Failed(e) => panic!("export failed: {e}"),
    }
}
