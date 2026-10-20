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
///
/// The one definition lives in `indicatrix_dispatch`, where the coordinator sizes its
/// chunks against it.
pub use indicatrix_dispatch::DEFAULT_MAX_CHUNK_SAMPLES as MAX_SAMPLES_PER_REQUEST;

/// Hard cap on `SceneState::max_bounces`.
///
/// 128 is the top rung of the GUI's own bounce ladder (4/8/12/24/64/128; see
/// `apps/indicatrix-cut/src/bridge/export_thread/params.rs::MAX_EXPORT_BOUNCES`). This
/// must never sit below that ceiling: a lower cap here would make a "Local + Remote"
/// export at 128 bounces reply `StreamEvent::Error` and hang the connection at 0% GPU
/// with no error surfaced to the user.
///
/// Cost is measured, not assumed: `crates/indicatrix/examples/bounce_cost.rs` found only
/// a 1.27-1.38x wall-time increase across an 85x cap sweep -- the median path terminates
/// at 4 bounces, p95 at 29, and only 0.03% of paths ever reach bounce 128. Raising the
/// cap from 64 to 128 is not a meaningful `DoS` amplification lever.
pub const MAX_BOUNCES: u32 = 128;

/// Hard cap on `SceneState::backdrop`, the tray radiance at exposure 1 (protocol v29).
///
/// The brightest plausible tray, a white tray (albedo 0.9) under the brightest Studio rig,
/// is about 8; 16 leaves a factor of two and still refuses a corrupt or hostile 1e30 emitter.
/// `0.0` (As Lit) is the floor.
pub const MAX_BACKDROP: f32 = 16.0;

/// Plausible refractive-index bounds, checked at the sodium D line and at both ends of
/// the visible spectrum (see [`validate_scene`]).
///
/// No real gem material in this workspace exceeds ~2.9 (moissanite, rutile); these
/// bounds are looser to avoid rejecting a legitimate exotic material, while still
/// catching a Sellmeier/Cauchy fit gone to NaN, negative, or many orders of magnitude
/// off, before it reaches `trace_spectral_ray`.
pub const MIN_PLAUSIBLE_RI: f32 = 1.0;
/// Largest refractive index accepted as plausible.
pub const MAX_PLAUSIBLE_RI: f32 = 6.0;

/// Hard cap on `scene.planes.len()`.
///
/// A plausible cut has well under a thousand facets; without this cap, an unsolved
/// design's raw preform planes, or an attacker- or fat-finger-supplied scene, could
/// send enough planes to make `build_plane_soa` and the GPU encoding path spend
/// unbounded time and memory before ever tracing a sample.
pub const MAX_PLANES: usize = 4096;

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
    if scene.engine.is_ray() {
        if !indicatrix::renderer::engine::AA_TAP_CHOICES.contains(&scene.engine.ray.aa_taps) {
            return Err("Ray-Traced AA taps must be 1, 4, 8 or 16".to_owned());
        }
        if scene.hdr().is_none() && scene.lighting_preset.is_uv_lamp() {
            return Err(indicatrix::renderer::whitted::WhittedDecline::UvLamp
                .message()
                .to_owned());
        }
        if !scene.fluorescence.is_empty() {
            return Err(indicatrix::renderer::whitted::WhittedDecline::Fluorescence
                .message()
                .to_owned());
        }
    }
    for (name, v) in [
        ("yaw", scene.yaw),
        ("pitch", scene.pitch),
        ("light_yaw", scene.light_yaw),
        ("light_pitch", scene.light_pitch),
        ("exposure", scene.exposure),
        ("backdrop", scene.backdrop),
        ("surface_glare", scene.surface_glare),
        ("head_shadow_deg", scene.head_shadow_deg),
    ] {
        if !v.is_finite() {
            return Err(format!("scene.{name} must be finite (got {v})"));
        }
    }
    // The tray radiance at exposure 1 (protocol v29): the brightest plausible tray, a white
    // tray under the brightest rig, is about 8; a hostile or corrupt scene must not set a
    // 1e30 emitter.
    if !(0.0..=MAX_BACKDROP).contains(&scene.backdrop) {
        return Err(format!(
            "scene.backdrop must be between 0 and {MAX_BACKDROP} (got {})",
            scene.backdrop
        ));
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

    check_planes_and_tools(scene)?;

    // Fluorescent emitters (v20): at most `MAX_EMITTERS` of at most `MAX_BANDS` excitation
    // and emission bands each, every value finite, quantum yields in [0, 1] -- the
    // tracer builds per-emitter sampling tables from them.
    scene
        .fluorescence
        .validate()
        .map_err(|e| format!("scene.fluorescence is invalid: {e}"))?;

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
    check_absorption_band_caps(scene)?;

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

    validate_environment(scene)
}

/// Checks the facet planes and the concave tools of `scene`.
///
/// # Errors
///
/// Returns a message naming the first plane or tool that is empty, oversized or malformed.
fn check_planes_and_tools(scene: &SceneState) -> Result<(), String> {
    if scene.planes.is_empty() {
        return Err("scene.planes must not be empty".to_string());
    }
    if scene.planes.len() > MAX_PLANES {
        return Err(format!(
            "scene.planes has {} facet(s), exceeding the maximum of {MAX_PLANES}",
            scene.planes.len()
        ));
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

    // Concave tools (v19). The count bound is the shared cap the kernel's tool scan is
    // sized for; each primitive is checked by the same validator the desktop applies, so
    // a worker never feeds the tracer a tool with a non-finite, degenerate or
    // out-of-range field.
    if scene.tools.len() > indicatrix::geometry::MAX_TOOL_PRIMITIVES {
        return Err(format!(
            "scene.tools has {} primitive(s), exceeding the maximum of {}",
            scene.tools.len(),
            indicatrix::geometry::MAX_TOOL_PRIMITIVES
        ));
    }
    for (i, tool) in scene.tools.iter().enumerate() {
        tool.validate()
            .map_err(|e| format!("scene.tools[{i}] is invalid: {e}"))?;
    }
    // Frosted planes (v26): at most one entry per plane, so a scene cannot carry an unbounded
    // list. An index past the planes is harmless (it names nothing) and is not an error.
    if scene.frosted_planes.len() > scene.planes.len() {
        return Err(format!(
            "scene.frosted_planes has {} entries for {} facet(s)",
            scene.frosted_planes.len(),
            scene.planes.len()
        ));
    }
    Ok(())
}

/// Rejects an absorption band list longer than the GPU encoding's fixed-size array.
///
/// # Errors
///
/// Returns a message naming the oversized ray when a list exceeds the cap.
fn check_absorption_band_caps(scene: &SceneState) -> Result<(), String> {
    for (mode_name, bands) in [
        ("o_ray", &scene.material.absorption.o_ray),
        ("e_ray", &scene.material.absorption.e_ray),
    ]
    .into_iter()
    .chain(
        scene
            .material
            .absorption
            .beta_ray
            .as_ref()
            .filter(|_| scene.engine.is_ray())
            .map(|bands| ("beta_ray", bands)),
    ) {
        if bands.len() > indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS {
            return Err(format!(
                "scene.material.absorption.{mode_name} has {} band(s), exceeding the GPU \
                 encoding's cap of {} (see renderer::buffers::MAX_ABSORPTION_BANDS)",
                bands.len(),
                indicatrix::renderer::buffers::MAX_ABSORPTION_BANDS
            ));
        }
    }
    Ok(())
}

/// Validates the scene's HDR environment, if any (v14).
///
/// Its declared size must be within `indicatrix::renderer::env_map::HdrLimits::DEFAULT`
/// -- checked before the server ever asks for the map's bytes, so an over-limit map is
/// refused without a transfer.
///
/// # Errors
///
/// A human-readable message when the declared size is zero or over the limits.
pub fn validate_environment(scene: &SceneState) -> Result<(), String> {
    let Some(hdr) = scene.hdr() else {
        return Ok(());
    };
    let limits = indicatrix::renderer::env_map::HdrLimits::DEFAULT;
    if hdr.width == 0 || hdr.height == 0 || !limits.admits(hdr.width, hdr.height) {
        return Err(format!(
            "scene.environment declares a {}x{} HDR map; it must be non-empty and within {}x{} \
             texels ({} MiB decoded)",
            hdr.width,
            hdr.height,
            limits.max_width,
            limits.max_height,
            limits.max_decoded_bytes / (1024 * 1024)
        ));
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
    if scene.engine.is_ray()
        && first_sample
            .checked_add(samples)
            .is_none_or(|end| end > u32::from(scene.engine.ray.aa_taps))
    {
        return Err("Ray-Traced request exceeds its fixed AA tap range".to_owned());
    }

    if samples == 0 {
        return Err("samples must be positive".to_string());
    }
    if samples > MAX_SAMPLES_PER_REQUEST {
        return Err(format!(
            "samples per request must be <= {MAX_SAMPLES_PER_REQUEST} (got {samples})"
        ));
    }
    // Beyond plain overflow: `first_sample + samples` must also leave headroom below
    // `u32::MAX` for at least one more `MAX_SAMPLES_PER_REQUEST`-sized chunk -- other
    // absolute-sample-index arithmetic downstream (chunk end bounds, a coordinator's
    // own accounting) adds a further chunk's worth without re-checking this request's
    // own bound, and must never wrap doing so.
    if first_sample
        .checked_add(samples)
        .is_none_or(|end| end > u32::MAX - MAX_SAMPLES_PER_REQUEST)
    {
        return Err(format!(
            "first_sample + samples must not overflow u32 and must leave headroom below \
             u32::MAX for chunking (first_sample={first_sample}, samples={samples})"
        ));
    }
    Ok(())
}

/// Validates the [`indicatrix_net::messages::StreamConfig`] half of a `serve`
/// [`indicatrix_net::messages::RenderRequest`].
///
/// [`validate_request`] covers the rest of the request; this also CLAMPS
/// [`indicatrix_net::messages::StreamConfig::cadence_ms`] up to this worker's
/// advertised floor.
///
/// # `cadence_ms` clamping
///
/// `stream.cadence_ms` below [`crate::stream_emit::MIN_CADENCE_FLOOR_MS`] (the same
/// floor `WELCOME::min_cadence_ms` already advertises) is silently raised to it, IN
/// PLACE -- `stream` is taken `&mut` for exactly this. **`0` does not mean "as fast
/// as possible, unbounded"**: combined with a full-scale
/// [`indicatrix_net::messages::StreamConfig::preview`] under
/// [`indicatrix_net::messages::TransferMode::FinalOnly`], an unclamped `cadence_ms = 0`
/// would make the emitter write the ENTIRE frame's preview on every ~20ms `EMITTER_POLL`
/// tick for the whole request, not just at cadence boundaries -- exactly the bandwidth a
/// cadence exists to pace. See the check below for why a full-scale preview under
/// `FinalOnly` is rejected outright rather than left to this clamp alone.
///
/// # Full-scale preview under `FinalOnly`
///
/// Also rejected outright: under `FinalOnly`, `FRAME` is sent exactly once at the end,
/// so the emitter's "skip a redundant full-scale `PREVIEW`" logic (which only fires
/// when a `FRAME` delta went out on the SAME tick) never applies there -- a full-scale
/// `PREVIEW` would duplicate the whole frame's data on every cadence tick for the
/// entire request instead of exactly once. `LiveProgressive` is unaffected: it sends
/// `FRAME` deltas throughout, so the emitter's existing redundancy skip already covers
/// a full-scale preview there.
///
/// # Errors
///
/// Returns a human-readable message if a configured preview has a zero `width`/`height`,
/// more pixels than the scene it's a reduced-resolution preview of, or is exactly the
/// scene's own resolution under `TransferMode::FinalOnly`.
pub fn validate_stream_config(
    stream: &mut indicatrix_net::messages::StreamConfig,
    scene: &SceneState,
) -> Result<(), String> {
    if stream.cadence_ms < crate::stream_emit::MIN_CADENCE_FLOOR_MS {
        stream.cadence_ms = crate::stream_emit::MIN_CADENCE_FLOOR_MS;
    }

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
    if stream.transfer_mode == indicatrix_net::messages::TransferMode::FinalOnly
        && preview.width == scene.width
        && preview.height == scene.height
    {
        return Err(format!(
            "stream.preview {}x{} must not equal the scene's own resolution under FinalOnly \
             transfer mode -- a full-scale PREVIEW would duplicate the single FRAME reply on \
             every cadence tick; request a smaller preview, drop it, or use LiveProgressive \
             instead",
            preview.width, preview.height
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
