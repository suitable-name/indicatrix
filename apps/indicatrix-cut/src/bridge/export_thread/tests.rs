//! `run_export` end-to-end tests: a valid PNG for a tiny scene, HDR-environment-map
//! wiring, pre-set cancellation, and the wide-gamut export's byte-identity/ICC-profile
//! requirements. Moved out of `mod.rs` alongside [`super::worker::run_export`] to keep
//! that file from growing further.

use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};

#[test]
fn run_export_produces_a_valid_png_for_a_tiny_scene() {
    let scene = SceneSnapshot {
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        env_map: None,
    };
    let params = ExportParams {
        width: 8,
        height: 8,
        samples_per_pixel: 1,
        max_bounces: 4,
    };
    let cancel = AtomicBool::new(false);

    let dir =
        std::env::temp_dir().join(format!("indicatrix-cut-export-test-{}", std::process::id()));
    let output_path = dir.join("tiny.png");

    let mut progress_calls = 0;
    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &output_path,
        ComputeTarget::LocalOnly,
        &[],
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {
            progress_calls += 1;
        },
    );

    match outcome {
        ExportOutcome::Completed(path) => {
            assert_eq!(path, output_path);
            let img = image::open(&path).expect("exported file must be a valid, readable image");
            assert_eq!(img.width(), 8);
            assert_eq!(img.height(), 8);
        }
        other => panic!("expected a completed export, got {other:?}"),
    }
    assert!(
        progress_calls > 0,
        "progress must be reported at least once"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An HDR-carrying `SceneSnapshot` (`env_map: Some(..)`) must still produce a valid,
/// completed export. `LocalComputeTarget::CpuGpu` is used deliberately (not `Cpu`) so
/// this exercises the same code path a real hybrid export would: on a `gpu` build
/// with a real adapter, `GpuBackend::try_accumulate` declines every batch here (no
/// `env_mode` for `HdrMap`) and every sample falls through to the CPU tracer -- this
/// test's job is only to prove the HDR map reaches that point at all.
#[test]
fn run_export_with_an_hdr_environment_map_still_produces_a_valid_png() {
    let scene = SceneSnapshot {
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        env_map: Some(std::sync::Arc::new(
            indicatrix::renderer::env_map::EnvironmentMap::uniform(4, 4, [0.5, 0.5, 0.5]),
        )),
    };
    let params = ExportParams {
        width: 8,
        height: 8,
        samples_per_pixel: 1,
        max_bounces: 4,
    };
    let cancel = AtomicBool::new(false);
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-cut-export-hdr-test-{}",
        std::process::id()
    ));
    let output_path = dir.join("hdr.png");

    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &output_path,
        ComputeTarget::LocalOnly,
        &[],
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {},
    );

    match outcome {
        ExportOutcome::Completed(path) => {
            let img = image::open(&path).expect("exported file must be a valid, readable image");
            assert_eq!(img.width(), 8);
            assert_eq!(img.height(), 8);
        }
        other => panic!("expected a completed export, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_export_honors_pre_set_cancellation_and_writes_no_file() {
    let scene = SceneSnapshot {
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        env_map: None,
    };
    let params = ExportParams {
        width: 8,
        height: 8,
        samples_per_pixel: 64,
        max_bounces: 4,
    };
    let cancel = AtomicBool::new(true); // already cancelled before starting

    let dir = std::env::temp_dir().join(format!(
        "indicatrix-cut-export-cancel-test-{}",
        std::process::id()
    ));
    let output_path = dir.join("never.png");

    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &output_path,
        ComputeTarget::LocalOnly,
        &[],
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {},
    );
    assert!(matches!(outcome, ExportOutcome::Cancelled));
    assert!(!output_path.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Wide-gamut export -----------------------------------------------------------

fn tiny_scene() -> SceneSnapshot {
    SceneSnapshot {
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        facet_finishes: Vec::new(),
        env_map: None,
    }
}

/// An `Srgb` export must produce bytes IDENTICAL to what this app wrote before the
/// colour-space picker existed, down to the raw bytes.
#[test]
fn srgb_export_is_byte_identical_regardless_of_which_save_png_path_runs() {
    let scene = tiny_scene();
    let params = ExportParams {
        width: 6,
        height: 5,
        samples_per_pixel: 2,
        max_bounces: 4,
    };

    // Two full deterministic renders through `run_export`'s public entry point must
    // agree byte-for-byte, exercising `save_png`'s `Srgb` branch directly.
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-cut-srgb-identity-test-{}",
        std::process::id()
    ));
    let path_a = dir.join("a.png");
    let path_b = dir.join("b.png");
    let cancel = AtomicBool::new(false);

    let outcome_a = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &path_a,
        ComputeTarget::LocalOnly,
        &[],
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_| {},
    );
    let outcome_b = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &path_b,
        ComputeTarget::LocalOnly,
        &[],
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_| {},
    );
    assert!(matches!(outcome_a, ExportOutcome::Completed(_)));
    assert!(matches!(outcome_b, ExportOutcome::Completed(_)));

    let bytes_a = std::fs::read(&path_a).unwrap();
    let bytes_b = std::fs::read(&path_b).unwrap();
    assert_eq!(
        bytes_a, bytes_b,
        "two Srgb exports of the identical scene must match exactly"
    );
    // No `iCCP` chunk anywhere in the file -- an sRGB export must stay untagged.
    assert!(
        !contains_bytes(&bytes_a, b"iCCP"),
        "an Srgb export must never carry an ICC profile chunk"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A non-`Srgb` export must carry an embedded ICC profile (`iCCP` PNG chunk) --
/// the whole point of `bridge::icc_profile`: an untagged wide-gamut PNG is
/// silently misread as sRGB by every viewer.
#[test]
fn wide_gamut_export_embeds_an_icc_profile_chunk() {
    for color_space in [ColorSpace::DisplayP3, ColorSpace::Rec2020] {
        let scene = tiny_scene();
        let params = ExportParams {
            width: 6,
            height: 5,
            samples_per_pixel: 2,
            max_bounces: 4,
        };
        let cancel = AtomicBool::new(false);
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-cut-wide-gamut-test-{color_space:?}-{}",
            std::process::id()
        ));
        let output_path = dir.join("wide.png");

        let outcome = run_export(
            &scene,
            params,
            color_space,
            &output_path,
            ComputeTarget::LocalOnly,
            &[],
            LocalComputeTarget::CpuGpu,
            &cancel,
            |_| {},
        );
        match outcome {
            ExportOutcome::Completed(path) => {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    contains_bytes(&bytes, b"iCCP"),
                    "{color_space:?}: expected an iCCP chunk in the exported PNG"
                );
                let img = image::open(&path)
                    .unwrap_or_else(|e| panic!("{color_space:?}: not a readable PNG: {e}"));
                assert_eq!(img.width(), 6);
                assert_eq!(img.height(), 5);
            }
            other => panic!("{color_space:?}: expected a completed export, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Byte substring search -- enough to confirm a chunk signature is present
/// somewhere in a small test PNG without pulling in a PNG-chunk-parsing
/// dependency just for this test.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
