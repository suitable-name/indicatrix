//! The serde default and clamp hooks of the remote-batch and editor-layout fields, and
//! `Default for AppSettings`, split out of `app_settings.rs` to keep it under 800 lines.

use super::{
    AppSettings, Backdrop, DEFAULT_C_AXIS_AZIMUTH_DEG, DEFAULT_C_AXIS_OVERRIDE_ENABLED,
    DEFAULT_C_AXIS_TILT_DEG, DEFAULT_CAMERA_DISTANCE, DEFAULT_CAMERA_PITCH, DEFAULT_CAMERA_YAW,
    DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE, DEFAULT_EDGE_ROUNDING_RADIUS,
    DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS, DEFAULT_EDITOR_DOCK_WIDTH,
    DEFAULT_EDITOR_INSPECTOR_HEIGHT, DEFAULT_EXPORT_FILENAME_TEMPLATE, DEFAULT_EXPOSURE,
    DEFAULT_GIRDLE_FROSTED, DEFAULT_HEAD_SHADOW_DEG, DEFAULT_INCLUSION_SIGMA_S,
    DEFAULT_LIGHT_PITCH_DEG, DEFAULT_LIGHT_YAW_DEG, DEFAULT_LIGHTING_RIG,
    DEFAULT_LIVE_COMPUTE_TARGET, DEFAULT_LIVE_VIEW_MODE, DEFAULT_LOCAL_COMPUTE_TARGET,
    DEFAULT_LOCAL_PREVIEW_SCALE, DEFAULT_MATERIAL, DEFAULT_MAX_BOUNCES, DEFAULT_PREVIEW_SIZE,
    DEFAULT_PREVIEW_SPP, DEFAULT_REMOTE_BATCH_LANES, DEFAULT_REMOTE_PREVIEW_BATCH_SIZE,
    DEFAULT_RENDER_HEIGHT, DEFAULT_RENDER_WIDTH, DEFAULT_SOLID_VIEW_MODE, DEFAULT_STONE_WIDTH_MM,
    DEFAULT_SURFACE_GLARE, DEFAULT_TARGET_SAMPLES, MAX_REMOTE_BATCH_LANES,
    MAX_REMOTE_PREVIEW_BATCH_SIZE, MIN_REMOTE_BATCH_LANES, MIN_REMOTE_PREVIEW_BATCH_SIZE,
    TiltVideoCompute, UiMode,
};
use serde::Deserialize;
use std::collections::BTreeSet;

pub(super) const fn default_remote_batch_lanes() -> u32 {
    DEFAULT_REMOTE_BATCH_LANES
}

pub(super) const fn default_remote_preview_batch_size() -> u32 {
    DEFAULT_REMOTE_PREVIEW_BATCH_SIZE
}

/// Reads [`AppSettings::remote_preview_batch_size`] as a signed integer and limits it, like
/// [`deserialize_remote_batch_lanes`].
pub(super) fn deserialize_remote_preview_batch_size<'de, D>(
    deserializer: D,
) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = i64::deserialize(deserializer)?;
    let limited = raw.clamp(
        i64::from(MIN_REMOTE_PREVIEW_BATCH_SIZE),
        i64::from(MAX_REMOTE_PREVIEW_BATCH_SIZE),
    );
    Ok(u32::try_from(limited).unwrap_or(DEFAULT_REMOTE_PREVIEW_BATCH_SIZE))
}

/// Reads [`AppSettings::remote_batch_lanes`] as a signed integer and limits it, so a
/// hand-edited value outside `1..=32` (a negative one included) loads as the nearest
/// valid count instead of failing the whole settings file.
pub(super) fn deserialize_remote_batch_lanes<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = i64::deserialize(deserializer)?;
    let limited = raw.clamp(
        i64::from(MIN_REMOTE_BATCH_LANES),
        i64::from(MAX_REMOTE_BATCH_LANES),
    );
    Ok(u32::try_from(limited).unwrap_or(DEFAULT_REMOTE_BATCH_LANES))
}

pub(super) const fn default_editor_auto_solve_budget_ms() -> u32 {
    DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS
}

pub(super) const fn default_editor_dock_width() -> f32 {
    DEFAULT_EDITOR_DOCK_WIDTH
}

pub(super) const fn default_editor_inspector_height() -> f32 {
    DEFAULT_EDITOR_INSPECTOR_HEIGHT
}

pub(super) fn default_export_filename_template() -> String {
    DEFAULT_EXPORT_FILENAME_TEMPLATE.to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ray_trace: super::super::RaySettings::default(),
            target_samples: DEFAULT_TARGET_SAMPLES,
            render_width: DEFAULT_RENDER_WIDTH,
            render_height: DEFAULT_RENDER_HEIGHT,
            max_bounces: DEFAULT_MAX_BOUNCES,
            exposure: DEFAULT_EXPOSURE,
            light_yaw_deg: DEFAULT_LIGHT_YAW_DEG,
            light_pitch_deg: DEFAULT_LIGHT_PITCH_DEG,
            lighting_rig: DEFAULT_LIGHTING_RIG.to_string(),
            camera_yaw: DEFAULT_CAMERA_YAW,
            camera_pitch: DEFAULT_CAMERA_PITCH,
            camera_distance: DEFAULT_CAMERA_DISTANCE,
            selected_material: DEFAULT_MATERIAL.to_string(),
            remote: None,
            legacy_remote_workers: Vec::new(),
            denoise_enabled: true,
            backdrop: Backdrop::default(),
            surface_glare: DEFAULT_SURFACE_GLARE,
            head_shadow_deg: DEFAULT_HEAD_SHADOW_DEG,
            inclusion_sigma_s: DEFAULT_INCLUSION_SIGMA_S,
            c_axis_override_enabled: DEFAULT_C_AXIS_OVERRIDE_ENABLED,
            c_axis_tilt_deg: DEFAULT_C_AXIS_TILT_DEG,
            c_axis_azimuth_deg: DEFAULT_C_AXIS_AZIMUTH_DEG,
            girdle_frosted: DEFAULT_GIRDLE_FROSTED,
            edge_rounding_radius: DEFAULT_EDGE_ROUNDING_RADIUS,
            stone_width_mm: DEFAULT_STONE_WIDTH_MM,
            local_preview_scale: DEFAULT_LOCAL_PREVIEW_SCALE,
            live_compute_target: DEFAULT_LIVE_COMPUTE_TARGET,
            local_compute_target: DEFAULT_LOCAL_COMPUTE_TARGET,
            tilt_video_compute_target: TiltVideoCompute::Both,
            contribute_to_final_picture: DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE,
            remote_batch_lanes: DEFAULT_REMOTE_BATCH_LANES,
            remote_batch_lanes_user_set: false,
            remote_preview_batch_size: DEFAULT_REMOTE_PREVIEW_BATCH_SIZE,
            plan_time_limit_secs: crate::plan_limit::DEFAULT_LIMIT_SECS,
            import_preview_choice: super::super::ImportPreviewChoice::Ask,
            payload_encoding: indicatrix_net::messages::adaptive::PayloadChoice::Auto,
            env_map_path: String::new(),
            preview_size: DEFAULT_PREVIEW_SIZE,
            thumbnail_renderer: indicatrix::renderer::engine::RenderEngine::default(),
            thumbnail_backdrop: Backdrop::Grey,
            preview_spp: DEFAULT_PREVIEW_SPP,
            export_directory: String::new(),
            last_import_directory: String::new(),
            export_filename_template: DEFAULT_EXPORT_FILENAME_TEMPLATE.to_string(),
            library_panel_collapsed: false,
            solid_view_mode: DEFAULT_SOLID_VIEW_MODE,
            live_view_mode: DEFAULT_LIVE_VIEW_MODE,
            editor_auto_solve_budget_ms: DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS,
            editor_dock_width: DEFAULT_EDITOR_DOCK_WIDTH,
            editor_inspector_height: DEFAULT_EDITOR_INSPECTOR_HEIGHT,
            editor_settings_collapsed: false,
            editor_inspector_collapsed: false,
            editor_tier_table_collapsed: false,
            editor_remap_collapsed: false,
            editor_layout_touched: false,
            recent_native_files: Vec::new(),
            suppressed_confirmations: BTreeSet::new(),
            // A brand-new install: the simple interface, with the welcome tour still to
            // come. A settings file that merely lacks these keys is an EXISTING install
            // and loads the field defaults above instead (Advanced, tour done).
            ui_mode: UiMode::Simple,
            first_run_tour_done: false,
            tutorials_completed: BTreeSet::new(),
            ui_scale_percent: 0,
            high_contrast: false,
            large_handles: false,
            manipulate_snap_off: false,
            slice_symmetric: true,
            render_body_color_override: None,
            output: super::super::output_settings::OutputSettings::default(),
            // A brand-new install has no old per-app stone effects to carry over.
            migrated_stone_effects: true,
            parameters_dock_open: false,
            parameters_dock_tab: 0,
            update_check: indicatrix_update::UpdatePreferences::default(),
        }
    }
}
