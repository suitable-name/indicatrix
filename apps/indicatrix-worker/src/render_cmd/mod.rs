//! The `render` subcommand: trace a scene straight to a PNG, no networking.
//!
//! Deliberately built and tested BEFORE `serve` -- it exercises the whole
//! trace-and-encode path (scene loading, validation, parallel tracing, tone-mapping,
//! PNG output) with nothing networked to debug at the same time, and it has standalone
//! value of its own: batch 4K stills without running the interactive viewer at all.

use crate::{
    cli::{ComputeMode, RenderArgs},
    png_out, render_core, validate,
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::SceneState;

/// Hard cap on `render`'s `--samples`.
///
/// Unlike `serve`'s much smaller [`validate::MAX_SAMPLES_PER_REQUEST`] (one batch of a
/// larger accumulation), this is the total spp for one whole image traced locally by a
/// trusted invocation -- a fat-finger guard, not a `DoS` boundary.
pub const MAX_CLI_SAMPLES: u32 = 1_000_000;

/// Validates `render`'s own `--width`/`--height`/`--samples` flags.
///
/// Independent of `--scene`; checked before the scene file is even read, so a bad flag
/// fails fast rather than after a possibly-slow file load and JSON parse.
///
/// # Errors
///
/// Returns a human-readable message describing the first thing wrong with the values.
pub fn validate_render_params(width: u32, height: u32, samples: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("--width and --height must both be positive".to_string());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > u64::from(validate::MAX_PIXELS) {
        return Err(format!(
            "--width x --height ({width}x{height} = {pixels} px) exceeds the maximum of {} px",
            validate::MAX_PIXELS
        ));
    }
    if samples == 0 {
        return Err("--samples must be positive".to_string());
    }
    if samples > MAX_CLI_SAMPLES {
        return Err(format!(
            "--samples must be <= {MAX_CLI_SAMPLES} (got {samples})"
        ));
    }
    Ok(())
}

/// Runs the `render` subcommand end to end.
///
/// Validates flags, loads and validates the scene, traces `args.samples` total samples
/// across the whole frame, tone-maps, and writes a PNG to `args.out`.
///
/// Acquires a [`GpuBackend`] once for this invocation (`OnlyCpu` forces
/// [`GpuBackend::disabled`]) and traces through [`render_core::trace_samples_with_gpu`],
/// which falls back to the CPU tracer whenever the GPU declines.
///
/// `render` traces its whole sample budget as one call with no adaptive sub-batching --
/// there's no concurrent hybrid CPU+GPU split to calibrate here (that exists for
/// `serve`'s long-running streamed jobs via `render_core::hybrid`), so
/// [`ComputeMode::Hybrid`] and [`ComputeMode::OnlyGpu`] resolve identically here.
///
/// # Errors
///
/// Returns a human-readable message for any failure: bad flags, an unreadable or
/// unparseable `--scene` file, a scene that fails [`validate::validate_scene`], or a
/// PNG-encoding failure.
pub fn run(args: &RenderArgs) -> Result<(), String> {
    validate_render_params(args.width, args.height, args.samples)?;

    let scene_text = std::fs::read_to_string(&args.scene)
        .map_err(|e| format!("could not read scene file {}: {e}", args.scene.display()))?;
    let mut scene: SceneState = serde_json::from_str(&scene_text).map_err(|e| {
        format!(
            "could not parse scene file {} as JSON: {e}",
            args.scene.display()
        )
    })?;

    // --width/--height are authoritative for the output image, overwriting whatever
    // scene.json serialized, so the same file can be re-rendered at other resolutions.
    scene.width = args.width;
    scene.height = args.height;

    validate::validate_scene(&scene)?;

    let gpu = match args.compute_mode {
        ComputeMode::OnlyCpu => GpuBackend::disabled(),
        ComputeMode::Hybrid | ComputeMode::OnlyGpu => GpuBackend::acquire(),
    };
    let threads = render_core::effective_thread_count(args.threads);
    let backend_desc = gpu.adapter_label().map_or_else(
        || format!("CPU across {threads} threads"),
        |label| format!("GPU: {label} (CPU fallback across {threads} threads if needed)"),
    );
    tracing::info!(
        "indicatrix-worker render: {}x{} @ {} spp -> {} ({backend_desc})",
        args.width,
        args.height,
        args.samples,
        args.out.display()
    );

    let sums = render_core::trace_samples_with_gpu(&gpu, &scene, 0, args.samples, args.threads);
    let rgba = png_out::tonemap_to_rgba(args.width, args.height, args.samples, &sums);
    png_out::write_png(args.width, args.height, rgba, &args.out)?;

    tracing::info!("wrote {}", args.out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use std::path::PathBuf;

    #[test]
    fn validate_render_params_rejects_zero_and_negative() {
        // "negative" is rejected one layer up, at CLI parsing (u32 can't hold one).
        assert!(validate_render_params(0, 1080, 64).is_err());
        assert!(validate_render_params(1920, 0, 64).is_err());
        assert!(validate_render_params(1920, 1080, 0).is_err());
    }

    #[test]
    fn validate_render_params_rejects_absurd_dimensions() {
        let err = validate_render_params(100_000, 100_000, 64).unwrap_err();
        assert!(err.contains("exceeds the maximum"), "{err}");
    }

    #[test]
    fn validate_render_params_rejects_absurd_sample_counts() {
        let err = validate_render_params(1920, 1080, 50_000_000).unwrap_err();
        assert!(err.contains("--samples"), "{err}");
    }

    #[test]
    fn validate_render_params_accepts_sensible_values() {
        assert!(validate_render_params(3840, 2160, 4096).is_ok());
        assert!(validate_render_params(8, 8, 4).is_ok());
    }

    fn write_tiny_scene_json(dir: &std::path::Path) -> PathBuf {
        let scene = SceneState {
            width: 999, // deliberately wrong -- must be overwritten by --width/--height
            height: 999,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
        };
        let path = dir.join("scene.json");
        std::fs::write(&path, serde_json::to_string(&scene).unwrap()).unwrap();
        path
    }

    /// End-to-end smoke test: a real tiny scene file through the real `run` path (JSON
    /// loading, validation, tracing, tone-mapping, PNG encoding), nothing mocked.
    #[test]
    fn run_produces_a_png_of_the_requested_dimensions() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-worker-render-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let scene_path = write_tiny_scene_json(&dir);
        let out_path = dir.join("render.png");

        let args = RenderArgs {
            scene: scene_path,
            out: out_path.clone(),
            width: 8,
            height: 8,
            samples: 4,
            threads: 2,
            // Forcing CPU keeps this deterministic regardless of the `gpu` feature; see
            // the `_via_gpu` counterpart below.
            compute_mode: ComputeMode::OnlyCpu,
        };
        run(&args).unwrap();

        let img = image::open(&out_path).expect("render must produce a valid, readable PNG");
        assert_eq!(img.width(), 8);
        assert_eq!(img.height(), 8);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_rejects_a_missing_scene_file() {
        let args = RenderArgs {
            scene: PathBuf::from("this/path/does/not/exist.json"),
            out: std::env::temp_dir().join("never.png"),
            width: 8,
            height: 8,
            samples: 4,
            threads: 1,
            compute_mode: ComputeMode::OnlyCpu,
        };
        assert!(run(&args).is_err());
    }

    #[test]
    fn run_rejects_a_scene_with_a_degenerate_normal() {
        // A zero-length normal is finite and round-trips through JSON fine (unlike
        // NaN/Infinity, which serde_json can't represent), so it exercises
        // `validate::validate_scene`'s degenerate-normal check specifically.
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-worker-render-badscene-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut scene = SceneState {
            width: 8,
            height: 8,
            yaw: 0.0,
            pitch: 0.0,
            distance: 3.0,
            light_yaw: 0.0,
            light_pitch: 0.0,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
        };
        scene.planes[0].normal = [0.0, 0.0, 0.0];
        let scene_path = dir.join("scene.json");
        std::fs::write(&scene_path, serde_json::to_string(&scene).unwrap()).unwrap();

        let args = RenderArgs {
            scene: scene_path,
            out: dir.join("never.png"),
            width: 8,
            height: 8,
            samples: 4,
            threads: 1,
            compute_mode: ComputeMode::OnlyCpu,
        };
        let err = run(&args).unwrap_err();
        assert!(err.contains("degenerate"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GPU counterpart to `run_produces_a_png_of_the_requested_dimensions`: with
    /// `OnlyGpu`, a usable adapter actually dispatches `GpuFrameRenderer` end to end
    /// (diamond is isotropic, so `GemMaterial::gpu_supported()` is true). Falls back to
    /// CPU without failing the test on a machine with no usable adapter.
    #[cfg(feature = "gpu")]
    #[test]
    fn run_produces_a_png_of_the_requested_dimensions_via_gpu() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-worker-render-gpu-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let scene_path = write_tiny_scene_json(&dir);
        let out_path = dir.join("render.png");

        let args = RenderArgs {
            scene: scene_path,
            out: out_path.clone(),
            width: 8,
            height: 8,
            samples: 4,
            threads: 2,
            compute_mode: ComputeMode::OnlyGpu,
        };
        run(&args).unwrap();

        let img = image::open(&out_path).expect("render must produce a valid, readable PNG");
        assert_eq!(img.width(), 8);
        assert_eq!(img.height(), 8);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
