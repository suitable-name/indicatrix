//! Opt-in deterministic preview at the same pose and studio rig as catalogue previews.
use super::{
    Camera, DEFAULT_FOV_DEG, GemMaterial, GpuFacetPlane, PREVIEW_DISTANCE, PREVIEW_EXPOSURE,
    PREVIEW_HEAD_SHADOW_DEG, PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW, PREVIEW_LIGHTING_PRESET,
    PREVIEW_SURFACE_GLARE, PREVIEW_YAW, PreviewJob, PreviewView, ToolPrimitive, encode_png,
    preview_backdrop, tonemap_to_rgba,
};
use indicatrix::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{EnvironmentSource, build_plane_soa},
    },
    renderer::{
        engine::RayOptions,
        frame_scene::FrameScene,
        whitted::{WhittedDecline, WhittedPrep, whitted_pixels_interleaved_geom},
    },
};

/// Renders a comparison or thumbnail without denoising or modifying the spectral path.
///
/// # Errors
/// Returns a renderer refusal instead of substituting a different renderer.
#[expect(
    clippy::too_many_arguments,
    reason = "matches the existing pose-preview API"
)]
pub fn render_rgba_at_pose_ray(
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    frosted: &[u32],
    material: &GemMaterial,
    camera_pose: (f32, f32, f32),
    size: (u32, u32),
    ray: RayOptions,
    max_bounces: u32,
) -> Result<Vec<u8>, WhittedDecline> {
    let (width, height) = size;
    if width == 0 || height == 0 {
        return Ok(Vec::new());
    }
    let (yaw, pitch, distance) = camera_pose;
    let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG);
    let finishes = indicatrix::geometry::facet_finishes_with_frosted(planes, false, frosted);
    let scene = FrameScene {
        camera: &camera,
        width,
        height,
        planes,
        facet_finishes: &finishes,
        material,
        max_bounces,
        environment: EnvironmentSource::Studio {
            preset: PREVIEW_LIGHTING_PRESET,
            exposure: PREVIEW_EXPOSURE,
            light_yaw: PREVIEW_LIGHT_YAW,
            light_pitch: PREVIEW_LIGHT_PITCH,
            // The resolved tray; Ray-Traced reads it under its own rules until its wave-3
            // lane (B7) switches the whitted environment to the tray model.
            backdrop: preview_backdrop(),
            surface_glare: PREVIEW_SURFACE_GLARE,
            head_shadow_deg: PREVIEW_HEAD_SHADOW_DEG,
        },
    };
    let prep = WhittedPrep::new(&scene, Fluorescence::none(), ray)?;
    let soa = build_plane_soa(planes);
    let sums =
        whitted_pixels_interleaved_geom(&scene, tools, &soa, &prep, 0, 1, 0, u32::from(prep.taps));
    Ok(tonemap_to_rgba(&sums, 1.0 / f32::from(prep.taps)))
}

/// The catalogue's fixed front/top scene with the frozen deterministic engine.
pub(super) fn render_view_ray(job: &PreviewJob<'_>, view: PreviewView) -> Option<Vec<u8>> {
    match render_rgba_at_pose_ray(
        job.planes,
        job.tools,
        job.frosted,
        job.material,
        (PREVIEW_YAW, view.pitch(), PREVIEW_DISTANCE),
        (job.size, job.size),
        job.engine.ray,
        job.max_bounces,
    ) {
        Ok(rgba) => encode_png(job.size, job.size, &rgba),
        Err(reason) => {
            tracing::warn!("Ray-Traced thumbnail refused: {reason}");
            None
        }
    }
}
