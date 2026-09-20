//! Input validation shared by both subcommands.
//!
//! `serve` accepts caller-supplied geometry and `render` a caller-supplied scene file;
//! neither should hand attacker- or fat-finger-controlled numbers straight to
//! `indicatrix` unchecked. [`validate_scene`] catches the geometry/material half
//! (non-finite or degenerate plane normals, implausible refractive indices, runaway
//! bounce counts); [`validate_request`] catches the size/count half.

use glam::Vec3;
use indicatrix_net::SceneState;

/// Hard cap on `width * height` for a single scene, shared by `render`'s CLI dimensions
/// and `serve`'s per-request `SceneState`.
///
/// 7680x4320 (8K UHD) keeps a traced radiance buffer
/// (`width * height * 12` bytes, see `indicatrix_net::radiance::BYTES_PER_PIXEL`) at
/// ~379 MiB, comfortably under [`indicatrix_net::framing::MAX_FRAME_LEN`] (512 MiB) so a
/// `serve` reply for a maximum-sized scene always fits in one `FRAME` message.
pub const MAX_PIXELS: u32 = 7680 * 4320;

/// Hard cap on a single `RenderRequest.samples` value in `serve`.
///
/// A `serve` request's `samples` is meant to be one batch out of a much larger total
/// (unlike `render`'s `--samples`, the total spp for a whole trusted local invocation),
/// so this is far smaller than
/// [`render_cmd::MAX_CLI_SAMPLES`](crate::render_cmd::MAX_CLI_SAMPLES). Without this
/// cap, a malicious or buggy `RenderRequest` could make a worker spend unbounded CPU
/// time before ever replying -- a denial-of-service vector, not a memory one.
pub const MAX_SAMPLES_PER_REQUEST: u32 = 65_536;

/// Hard cap on `SceneState::max_bounces`.
///
/// 128 is the top rung of the GUI's own bounce ladder (4/8/12/24/64/128; see
/// `apps/indicatrix-cut/src/bridge/export_thread/params.rs::MAX_EXPORT_BOUNCES`). This
/// must never sit below that ceiling: it used to be 64, which made a "Local + Remote"
/// export at 128 bounces reply `StreamEvent::Error` and hang the connection at 0% GPU
/// with no error surfaced to the user.
///
/// Cost is measured, not assumed: `crates/indicatrix/examples/bounce_cost.rs` found only
/// a 1.27-1.38x wall-time increase across an 85x cap sweep -- the median path terminates
/// at 4 bounces, p95 at 29, and only 0.03% of paths ever reach bounce 128. Raising the
/// cap from 64 to 128 is not a meaningful `DoS` amplification lever.
pub const MAX_BOUNCES: u32 = 128;

/// Plausible refractive-index bounds, checked at the sodium D line and at both ends of
/// the visible spectrum (see [`validate_scene`]).
///
/// No real gem material in this workspace exceeds ~2.9 (moissanite, rutile); these
/// bounds are looser to avoid rejecting a legitimate exotic material, while still
/// catching a Sellmeier/Cauchy fit gone to NaN, negative, or many orders of magnitude
/// off, before it reaches `trace_spectral_ray`.
pub const MIN_PLAUSIBLE_RI: f32 = 1.0;
pub const MAX_PLAUSIBLE_RI: f32 = 6.0;

/// Validates a fully-resolved [`SceneState`].
///
/// Checks finite, non-degenerate facet-plane normals; a plausible refractive index
/// across the visible spectrum; finite camera/light/exposure/material scalars; and a
/// bounded `max_bounces`.
///
/// Does not check `width`/`height` against [`MAX_PIXELS`] itself -- callers
/// (`render_cmd`, `serve`) check that against their own caller-supplied dimensions,
/// since `render`'s `--width`/`--height` flags are authoritative over a `scene.json`
/// file's own ignored `width`/`height` fields.
///
/// # Errors
///
/// Returns a human-readable message describing the first thing wrong with `scene`.
pub fn validate_scene(scene: &SceneState) -> Result<(), String> {
    for (name, v) in [
        ("yaw", scene.yaw),
        ("pitch", scene.pitch),
        ("light_yaw", scene.light_yaw),
        ("light_pitch", scene.light_pitch),
        ("exposure", scene.exposure),
        ("backdrop", scene.backdrop),
    ] {
        if !v.is_finite() {
            return Err(format!("scene.{name} must be finite (got {v})"));
        }
    }
    if !scene.distance.is_finite() || scene.distance <= 0.0 {
        return Err(format!(
            "scene.distance must be finite and positive (got {})",
            scene.distance
        ));
    }
    if scene.max_bounces == 0 || scene.max_bounces > MAX_BOUNCES {
        return Err(format!(
            "scene.max_bounces must be between 1 and {MAX_BOUNCES} (got {})",
            scene.max_bounces
        ));
    }

    if scene.planes.is_empty() {
        return Err("scene.planes must not be empty".to_string());
    }
    for (i, plane) in scene.planes.iter().enumerate() {
        let normal = Vec3::from_array(plane.normal);
        if !normal.is_finite() {
            return Err(format!(
                "scene.planes[{i}].normal is non-finite: {:?}",
                plane.normal
            ));
        }
        if normal.length() <= 1e-6 {
            return Err(format!(
                "scene.planes[{i}].normal is degenerate (near-zero length): {:?}",
                plane.normal
            ));
        }
        if !plane.d.is_finite() {
            return Err(format!("scene.planes[{i}].d is non-finite: {}", plane.d));
        }
    }

    if !scene.material.c_axis.is_finite() {
        return Err(format!(
            "scene.material.c_axis is non-finite: {}",
            scene.material.c_axis
        ));
    }
    if !scene.material.birefringence_delta.is_finite() {
        return Err(format!(
            "scene.material.birefringence_delta is non-finite: {}",
            scene.material.birefringence_delta
        ));
    }
    if let Some(delta) = scene.material.biaxial_delta_beta_alpha
        && !delta.is_finite()
    {
        return Err(format!(
            "scene.material.biaxial_delta_beta_alpha is non-finite: {delta}"
        ));
    }
    // This scale (see `GemMaterial::absorption_path_scale`) multiplies every interior
    // path length before Beer-Lambert absorption; zero/negative collapses or inverts
    // path lengths, NaN poisons every absorbed sample.
    if !scene.material.absorption_path_scale.is_finite()
        || scene.material.absorption_path_scale <= 0.0
    {
        return Err(format!(
            "scene.material.absorption_path_scale must be finite and positive (got {})",
            scene.material.absorption_path_scale
        ));
    }

    // `renderer::buffers::GpuGemMaterial` flattens each eigenmode's `Vec<AbsorptionBand>`
    // into a fixed-capacity `[GpuAbsorptionBand; MAX_ABSORPTION_BANDS]` array for GPU
    // encoding. Reject an oversized `Vec` here rather than silently truncating it the
    // first time the GPU path encodes it.
    for (mode_name, bands) in [
        ("o_ray", &scene.material.absorption.o_ray),
        ("e_ray", &scene.material.absorption.e_ray),
    ] {
        if bands.len() > indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS {
            return Err(format!(
                "scene.material.absorption.{mode_name} has {} band(s), exceeding the GPU \
                 encoding's cap of {} (see renderer::buffers::MAX_ABSORPTION_BANDS)",
                bands.len(),
                indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS
            ));
        }
    }

    // Sodium D line (589.3nm, the conventional n_d reference) plus both ends of the
    // visible spectrum -- a fit that looks fine at n_d can still blow up (or be masked
    // by `DispersionModel::evaluate`'s `n2.max(1.0)` clamp) at the edge of its range.
    for lambda_nm in [380.0f32, 589.3, 780.0] {
        let n = scene.material.dispersion.evaluate(lambda_nm);
        if !n.is_finite() || !(MIN_PLAUSIBLE_RI..=MAX_PLAUSIBLE_RI).contains(&n) {
            return Err(format!(
                "scene.material's refractive index at {lambda_nm}nm is implausible: {n} (expected {MIN_PLAUSIBLE_RI}..={MAX_PLAUSIBLE_RI})"
            ));
        }
    }

    Ok(())
}

/// Validates one `serve` [`indicatrix_net::messages::RenderRequest`].
///
/// Checks the embedded scene (via [`validate_scene`]), the scene's own dimensions
/// against [`MAX_PIXELS`], the requested sample count against
/// [`MAX_SAMPLES_PER_REQUEST`], and that `first_sample + samples` doesn't overflow
/// `u32` -- an overflow would wrap the absolute sample numbering the seed formula
/// depends on, which must never repeat within one accumulation.
///
/// # Errors
///
/// Returns a human-readable message describing the first thing wrong with the request.
pub fn validate_request(scene: &SceneState, first_sample: u32, samples: u32) -> Result<(), String> {
    if scene.width == 0 || scene.height == 0 {
        return Err(format!(
            "scene dimensions must be positive (got {}x{})",
            scene.width, scene.height
        ));
    }
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    if pixels > u64::from(MAX_PIXELS) {
        return Err(format!(
            "scene dimensions {}x{} ({pixels} px) exceed the maximum of {MAX_PIXELS} px",
            scene.width, scene.height
        ));
    }
    validate_scene(scene)?;

    if samples == 0 {
        return Err("samples must be positive".to_string());
    }
    if samples > MAX_SAMPLES_PER_REQUEST {
        return Err(format!(
            "samples per request must be <= {MAX_SAMPLES_PER_REQUEST} (got {samples})"
        ));
    }
    if first_sample.checked_add(samples).is_none() {
        return Err(format!(
            "first_sample + samples overflows u32 (first_sample={first_sample}, samples={samples})"
        ));
    }
    Ok(())
}

/// Validates the [`indicatrix_net::messages::StreamConfig`] half of a `serve`
/// [`indicatrix_net::messages::RenderRequest`] ([`validate_request`] covers the rest).
///
/// Only [`StreamConfig::preview`] needs checking: `cadence_ms` has no invalid value
/// (`0` legitimately means "as fast as possible"), and `transfer_mode` is an enum with
/// no invalid variant.
///
/// # Errors
///
/// Returns a human-readable message if a configured preview has a zero `width`/`height`,
/// or more pixels than the scene it's a reduced-resolution preview of.
pub fn validate_stream_config(
    stream: &indicatrix_net::messages::StreamConfig,
    scene: &SceneState,
) -> Result<(), String> {
    let Some(preview) = stream.preview else {
        return Ok(());
    };
    if preview.width == 0 || preview.height == 0 {
        return Err(format!(
            "stream.preview dimensions must be positive (got {}x{})",
            preview.width, preview.height
        ));
    }
    let preview_pixels = u64::from(preview.width) * u64::from(preview.height);
    let scene_pixels = u64::from(scene.width) * u64::from(scene.height);
    if preview_pixels > scene_pixels {
        return Err(format!(
            "stream.preview {}x{} ({preview_pixels} px) must not exceed the scene's own \
             {}x{} ({scene_pixels} px) -- a preview is a REDUCED-resolution snapshot",
            preview.width, preview.height, scene.width, scene.height
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::{GpuFacetPlane, cuts::StandardGemCuts},
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    fn valid_scene() -> SceneState {
        SceneState {
            width: 64,
            height: 64,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 6,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
        }
    }

    #[test]
    fn accepts_a_well_formed_scene() {
        assert!(validate_scene(&valid_scene()).is_ok());
    }

    #[test]
    fn rejects_empty_planes() {
        let mut scene = valid_scene();
        scene.planes.clear();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn rejects_a_nan_normal() {
        let mut scene = valid_scene();
        scene.planes[0].normal = [f32::NAN, 0.0, 0.0];
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("non-finite"), "{err}");
    }

    #[test]
    fn rejects_an_infinite_normal() {
        let mut scene = valid_scene();
        scene.planes[0].normal = [f32::INFINITY, 0.0, 0.0];
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn rejects_a_zero_length_normal() {
        let mut scene = valid_scene();
        scene.planes[0] = GpuFacetPlane {
            normal: [0.0, 0.0, 0.0],
            d: 1.0,
        };
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("degenerate"), "{err}");
    }

    #[test]
    fn rejects_non_finite_plane_offset() {
        let mut scene = valid_scene();
        scene.planes[0].d = f32::NAN;
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn rejects_zero_distance() {
        let mut scene = valid_scene();
        scene.distance = 0.0;
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn rejects_non_finite_exposure() {
        let mut scene = valid_scene();
        scene.exposure = f32::INFINITY;
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn rejects_zero_and_excessive_max_bounces() {
        let mut scene = valid_scene();
        scene.max_bounces = 0;
        assert!(validate_scene(&scene).is_err());

        scene.max_bounces = MAX_BOUNCES + 1;
        assert!(validate_scene(&scene).is_err());

        scene.max_bounces = MAX_BOUNCES;
        assert!(validate_scene(&scene).is_ok());
    }

    /// Pins the cross-crate contract with the GUI's bounce ladder
    /// (`apps/indicatrix-cut/src/bridge/export_thread/params.rs::MAX_EXPORT_BOUNCES`) as a
    /// literal so drift on either side trips this test instead of silently reintroducing
    /// the "remote export goes silent" bug.
    #[test]
    fn max_bounces_matches_the_guis_own_ladder_ceiling() {
        assert_eq!(MAX_BOUNCES, 128);
    }

    #[test]
    fn rejects_too_many_absorption_bands() {
        use indicatrix::optics::absorption::AbsorptionBand;

        let mut scene = valid_scene();
        let too_many: Vec<AbsorptionBand> = (0
            ..=indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS)
            .map(|i| AbsorptionBand::new(400.0 + i as f32, 10.0, 1.0))
            .collect();
        scene.material.absorption.o_ray = too_many.clone();
        scene.material.absorption.e_ray = too_many;
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("exceeding the GPU encoding's cap"), "{err}");
    }

    #[test]
    fn accepts_absorption_bands_up_to_the_cap() {
        use indicatrix::optics::absorption::AbsorptionBand;

        let mut scene = valid_scene();
        let at_cap: Vec<AbsorptionBand> = (0..indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS)
            .map(|i| AbsorptionBand::new(400.0 + i as f32, 10.0, 1.0))
            .collect();
        scene.material.absorption.o_ray = at_cap.clone();
        scene.material.absorption.e_ray = at_cap;
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn rejects_non_finite_absorption_path_scale() {
        let mut scene = valid_scene();
        scene.material.absorption_path_scale = f32::NAN;
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("absorption_path_scale"), "{err}");

        scene.material.absorption_path_scale = f32::INFINITY;
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn rejects_zero_and_negative_absorption_path_scale() {
        let mut scene = valid_scene();
        scene.material.absorption_path_scale = 0.0;
        assert!(validate_scene(&scene).is_err());

        scene.material.absorption_path_scale = -1.0;
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn accepts_a_scaled_absorption_path() {
        let mut scene = valid_scene();
        scene.material = scene.material.with_absorption_path_scale(2.5);
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn rejects_implausible_refractive_index() {
        let mut scene = valid_scene();
        // A custom material whose Cauchy fit is nowhere near a real gemstone's.
        scene.material = GemMaterial::new_custom("Absurd", 50.0, 0.0, 0.0, [0.0, 0.0, 0.0]);
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("refractive index"), "{err}");
    }

    #[test]
    fn validate_request_rejects_a_zero_sample_count() {
        let scene = valid_scene();
        assert!(validate_request(&scene, 0, 0).is_err());
    }

    #[test]
    fn validate_request_rejects_an_excessive_sample_count() {
        let scene = valid_scene();
        assert!(validate_request(&scene, 0, MAX_SAMPLES_PER_REQUEST + 1).is_err());
        assert!(validate_request(&scene, 0, MAX_SAMPLES_PER_REQUEST).is_ok());
    }

    #[test]
    fn validate_request_rejects_first_sample_plus_samples_overflow() {
        let scene = valid_scene();
        assert!(validate_request(&scene, u32::MAX - 10, 100).is_err());
    }

    #[test]
    fn validate_request_rejects_oversized_scene_dimensions() {
        let mut scene = valid_scene();
        scene.width = 100_000;
        scene.height = 100_000;
        assert!(validate_request(&scene, 0, 16).is_err());
    }

    #[test]
    fn validate_request_accepts_a_well_formed_request() {
        let scene = valid_scene();
        assert!(validate_request(&scene, 128, 64).is_ok());
    }

    use indicatrix_net::messages::{PreviewConfig, StreamConfig, TransferMode};

    fn stream_config(preview: Option<PreviewConfig>) -> StreamConfig {
        StreamConfig {
            transfer_mode: TransferMode::LiveProgressive,
            cadence_ms: 250,
            preview,
        }
    }

    #[test]
    fn validate_stream_config_accepts_no_preview() {
        let scene = valid_scene();
        assert!(validate_stream_config(&stream_config(None), &scene).is_ok());
    }

    #[test]
    fn validate_stream_config_accepts_a_smaller_preview() {
        let scene = valid_scene();
        let stream = stream_config(Some(PreviewConfig {
            width: 16,
            height: 16,
        }));
        assert!(validate_stream_config(&stream, &scene).is_ok());
    }

    #[test]
    fn validate_stream_config_rejects_a_zero_dimension_preview() {
        let scene = valid_scene();
        let stream = stream_config(Some(PreviewConfig {
            width: 0,
            height: 16,
        }));
        let err = validate_stream_config(&stream, &scene).unwrap_err();
        assert!(err.contains("must be positive"), "{err}");
    }

    #[test]
    fn validate_stream_config_rejects_a_preview_larger_than_the_scene() {
        let scene = valid_scene(); // 64x64
        let stream = stream_config(Some(PreviewConfig {
            width: 128,
            height: 128,
        }));
        let err = validate_stream_config(&stream, &scene).unwrap_err();
        assert!(err.contains("must not exceed"), "{err}");
    }

    #[test]
    fn validate_stream_config_accepts_a_preview_equal_to_the_scene() {
        let scene = valid_scene(); // 64x64
        let stream = stream_config(Some(PreviewConfig {
            width: 64,
            height: 64,
        }));
        assert!(validate_stream_config(&stream, &scene).is_ok());
    }
}
