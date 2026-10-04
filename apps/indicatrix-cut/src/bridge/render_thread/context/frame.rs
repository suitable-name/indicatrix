//! Loading an HDR environment map, and snapshotting one frame's worth of
//! [`RenderContext`] fields out from under its lock.

use super::RenderContext;
use crate::{
    bridge::sample_cursor::LiveEpoch,
    settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale},
};
use glam::Vec3;
use indicatrix::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{fluorescence::Fluorescence, materials::GemMaterial, raytracer::LightingPreset},
    renderer::env_map::EnvironmentMap,
};
use std::sync::{Arc, Mutex};

/// Decodes a Radiance `.hdr` file at `path` into an [`EnvironmentMap`], wrapped in the
/// `Arc` `RenderContext::env_map` stores. `gui::mod`'s load callback shows an `Err` via
/// the toast mechanism and never assigns into `env_map`, so a bad file leaves the
/// previously active environment untouched.
///
/// Goes through `bridge::remote::hdr_asset::load_hdr_file`: the map is decoded with the
/// builder every render worker shares, and registered as a content-addressed asset so an
/// HDR-capable remote can be sent its exact bytes (protocol v14 HDR map transfer).
///
/// # Errors
///
/// Returns `Err` with a human-readable message if `path` cannot be read or does not
/// decode as a valid Radiance HDR image.
pub fn load_env_map(path: &str) -> Result<Arc<EnvironmentMap>, String> {
    crate::bridge::remote::hdr_asset::load_hdr_file(std::path::Path::new(path)).map(|(map, _)| map)
}

/// One frame's worth of inputs read out of `RenderContext` under its lock, copied out
/// so the mutex guard can be dropped immediately.
pub(in crate::bridge::render_thread) struct FrameInputs {
    pub(in crate::bridge::render_thread) width: u32,
    pub(in crate::bridge::render_thread) height: u32,
    pub(in crate::bridge::render_thread) yaw: f32,
    pub(in crate::bridge::render_thread) pitch: f32,
    pub(in crate::bridge::render_thread) distance: f32,
    pub(in crate::bridge::render_thread) light_yaw: f32,
    pub(in crate::bridge::render_thread) light_pitch: f32,
    pub(in crate::bridge::render_thread) material_name: String,
    pub(in crate::bridge::render_thread) material_override: Option<GemMaterial>,
    /// See [`RenderContext::material_unresolved`].
    pub(in crate::bridge::render_thread) material_unresolved: Option<String>,
    pub(in crate::bridge::render_thread) lighting_preset: LightingPreset,
    pub(in crate::bridge::render_thread) backdrop: crate::settings::model::Backdrop,
    pub(in crate::bridge::render_thread) surface_glare: f32,
    pub(in crate::bridge::render_thread) target_samples: u32,
    pub(in crate::bridge::render_thread) max_bounces: u32,
    pub(in crate::bridge::render_thread) exposure: f32,
    pub(in crate::bridge::render_thread) inclusion_sigma_s: f32,
    pub(in crate::bridge::render_thread) c_axis_override: Option<Vec3>,
    pub(in crate::bridge::render_thread) girdle_frosted: bool,
    pub(in crate::bridge::render_thread) edge_rounding_radius: f32,
    pub(in crate::bridge::render_thread) stone_width_mm: f32,
    pub(in crate::bridge::render_thread) physics_color: bool,
    pub(in crate::bridge::render_thread) active_planes: Arc<Vec<GpuFacetPlane>>,
    /// See [`RenderContext::active_tools`].
    pub(in crate::bridge::render_thread) active_tools: Arc<Vec<ToolPrimitive>>,
    pub(in crate::bridge::render_thread) custom_materials: Arc<Vec<GemMaterial>>,
    /// See [`RenderContext::active_fluorescence`]; `None` is no fluorescence.
    pub(in crate::bridge::render_thread) fluorescence: Option<Arc<Fluorescence>>,
    pub(in crate::bridge::render_thread) running: bool,
    pub(in crate::bridge::render_thread) dirty: bool,
    pub(in crate::bridge::render_thread) paused: bool,
    pub(in crate::bridge::render_thread) tab_visible: bool,
    pub(in crate::bridge::render_thread) denoise_enabled: bool,
    /// See [`RenderContext::redisplay_requested`].
    pub(in crate::bridge::render_thread) redisplay_requested: bool,
    pub(in crate::bridge::render_thread) remote_active: bool,
    /// See [`RenderContext::live_epoch`].
    pub(in crate::bridge::render_thread) live_epoch: Option<Arc<LiveEpoch>>,
    pub(in crate::bridge::render_thread) export_active: bool,
    pub(in crate::bridge::render_thread) live_compute_target: LiveComputeTarget,
    pub(in crate::bridge::render_thread) local_compute_target: LocalComputeTarget,
    pub(in crate::bridge::render_thread) local_preview_scale: LocalPreviewScale,
    pub(in crate::bridge::render_thread) camera_moving: bool,
    pub(in crate::bridge::render_thread) env_map: Option<Arc<EnvironmentMap>>,
    /// [`RenderContext::scene_generation`] of exactly the fields in this snapshot
    /// (read under the same lock) -- compared against the live epoch's own
    /// generation so a frame never claims from or merges with an epoch that was
    /// dispatched for a different scene.
    pub(in crate::bridge::render_thread) scene_generation: u64,
}

/// Snapshots every field the render loop needs for one frame out of `RenderContext`,
/// clearing `dirty` in the same locked section so a `dirty` set by a callback between
/// the read and the clear is never lost.
pub(in crate::bridge::render_thread) fn snapshot_frame_inputs(
    ctx: &Arc<Mutex<RenderContext>>,
) -> FrameInputs {
    // Recovers from a poisoned lock rather than panicking, matching every other
    // `RenderContext` lock in this crate. This runs once per frame on the render
    // thread, so panicking here would permanently kill rendering while the UI thread
    // (recovering the same way) keeps servicing the window -- indistinguishable from a
    // hang, with no console in a release build to show why. Every field is a plain
    // value written under this same lock, so the worst a poisoning writer leaves
    // behind is a stale-but-valid frame, overwritten next tick anyway.
    let mut ctx = RenderContext::lock(ctx);
    let dirty = ctx.dirty;
    ctx.dirty = false;
    // Consumed the same way as `dirty` -- read and cleared in this one locked
    // section, so a request set by a callback between the read and the clear is
    // never lost. See `RenderContext::redisplay_requested`'s own doc comment.
    let redisplay_requested = ctx.redisplay_requested;
    ctx.redisplay_requested = false;
    let scene_generation = ctx.scene_generation();
    FrameInputs {
        width: ctx.width,
        height: ctx.height,
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
        light_yaw: ctx.light_yaw,
        light_pitch: ctx.light_pitch,
        material_name: ctx.material_name.clone(),
        material_override: ctx.material_override.clone(),
        material_unresolved: ctx.material_unresolved.clone(),
        lighting_preset: ctx.lighting_preset,
        backdrop: ctx.backdrop,
        surface_glare: ctx.surface_glare,
        target_samples: ctx.target_samples,
        max_bounces: ctx.max_bounces,
        exposure: ctx.exposure,
        inclusion_sigma_s: ctx.inclusion_sigma_s,
        c_axis_override: ctx.c_axis_override,
        girdle_frosted: ctx.girdle_frosted,
        edge_rounding_radius: ctx.edge_rounding_radius,
        stone_width_mm: ctx.stone_width_mm,
        physics_color: ctx.physics_color(),
        // `Arc::clone`, not a deep copy -- see `RenderContext::active_planes`'s doc
        // comment.
        active_planes: Arc::clone(&ctx.active_planes),
        // `Arc::clone`, like the planes beside it.
        active_tools: Arc::clone(&ctx.active_tools),
        // `Arc::clone`, not a deep copy -- see `RenderContext::custom_materials`'s doc
        // comment.
        custom_materials: Arc::clone(&ctx.custom_materials),
        // `Arc::clone` of the active material's emitters, `None` for the common case.
        fluorescence: ctx.active_fluorescence(),
        running: ctx.running,
        dirty,
        paused: ctx.paused,
        tab_visible: ctx.tab_visible,
        denoise_enabled: ctx.denoise_enabled,
        redisplay_requested,
        remote_active: ctx.remote_active,
        // `Arc::clone`, not a deep copy -- see `live_epoch`'s doc comment.
        live_epoch: ctx.live_epoch.clone(),
        export_active: ctx.export_active(),
        // The EFFECTIVE target: a final-picture epoch behaves as `RemoteOnly`.
        live_compute_target: ctx.effective_live_target(),
        local_compute_target: ctx.local_compute_target,
        local_preview_scale: ctx.local_preview_scale,
        camera_moving: ctx.camera_moving,
        // `Arc::clone`, not a deep copy of the decoded panorama -- see `env_map`'s doc comment.
        env_map: ctx.env_map.clone(),
        scene_generation,
    }
}
