//! [`AppSettings`]: the four originally in-memory-only settings, plus camera pose,
//! the selected material, and everything else migrated into persistent storage.

use super::{
    local_compute::LocalComputeTarget,
    worker::{LiveComputeTarget, LocalPreviewScale, WorkerSettings},
};
use crate::bridge::export_thread::DEFAULT_TEMPLATE as DEFAULT_EXPORT_FILENAME_TEMPLATE;
use serde::{Deserialize, Serialize};

// Defaults mirror `RenderContext::default()` and `settings_dialog.slint`'s property
// initializers. Camera yaw/pitch are kept in RADIANS (matching `RenderContext`) while
// light yaw/pitch are kept in DEGREES (matching the settings-dialog sliders) -- each
// field stores whatever unit its main consumer already uses. See
// `gui::mod::apply_loaded_settings` for the one place that converts degrees -> radians.
/// `256` matches the old "High / Quality" preset's typical converged sample count --
/// see `RenderContext::default()`'s `target_samples`, which mirrors this.
pub const DEFAULT_TARGET_SAMPLES: u32 = 256;
/// Live render resolution -- matches `RenderContext::default()`'s `width`/`height`.
pub const DEFAULT_RENDER_WIDTH: u32 = 800;
pub const DEFAULT_RENDER_HEIGHT: u32 = 600;
pub const DEFAULT_MAX_BOUNCES: u32 = 12;
pub const DEFAULT_EXPOSURE: f32 = 1.0;
pub const DEFAULT_INCLUSION_SIGMA_S: f32 = 0.0;
pub const DEFAULT_C_AXIS_OVERRIDE_ENABLED: bool = false;
pub const DEFAULT_C_AXIS_TILT_DEG: f32 = 0.0;
pub const DEFAULT_C_AXIS_AZIMUTH_DEG: f32 = 0.0;
pub const DEFAULT_GIRDLE_FROSTED: bool = false;
pub const DEFAULT_EDGE_ROUNDING_RADIUS: f32 = 0.0;
pub const DEFAULT_STONE_WIDTH_MM: f32 = 0.0;
pub const DEFAULT_LOCAL_PREVIEW_SCALE: LocalPreviewScale = LocalPreviewScale::Off;
/// Unchanged from the value this used to be hardcoded to
/// (`gui::remote::REMOTE_RENDER_SAMPLES`).
pub const DEFAULT_REMOTE_RENDER_SAMPLES: u32 = 512;
/// `LiveComputeTarget::Both` is a no-op without a configured worker
/// (`orchestrator::poll_tick` only dispatches remote when a worker is both selected
/// and present in `remote_workers`), so a fresh install behaves exactly as before --
/// `Both` only starts doing anything once a worker is added.
pub const DEFAULT_LIVE_COMPUTE_TARGET: LiveComputeTarget = LiveComputeTarget::Both;
/// `CpuGpu` matches `LocalComputeTarget::Default`, reproducing today's hybrid CPU+GPU
/// behaviour on a `gpu`-feature build (CPU-only otherwise, since `ViewportGpu` always
/// declines there regardless of this setting).
pub const DEFAULT_LOCAL_COMPUTE_TARGET: LocalComputeTarget = LocalComputeTarget::CpuGpu;
pub const DEFAULT_LIGHT_YAW_DEG: f32 = 48.0;
pub const DEFAULT_LIGHT_PITCH_DEG: f32 = 54.0;
pub const DEFAULT_LIGHTING_RIG: &str = "Gem Studio Ring Lights";
pub const DEFAULT_CAMERA_YAW: f32 = 0.60;
pub const DEFAULT_CAMERA_PITCH: f32 = 0.45;
pub const DEFAULT_CAMERA_DISTANCE: f32 = 2.4;
pub const DEFAULT_MATERIAL: &str = "Diamond";
/// Cached-preview (front/top thumbnail) square render size, in pixels -- see
/// `bridge::preview_render`'s doc comment for the measured cost table this was picked
/// from. `160` at [`DEFAULT_PREVIEW_SPP`] costs ~1.1s/image on 16 cores, small enough
/// to sit comfortably in a diagram-list card with room for two side by side. Exposed
/// as a setting rather than a hardcoded constant since the speed/crispness trade-off
/// is a matter of taste.
pub const DEFAULT_PREVIEW_SIZE: u32 = 160;
/// See [`DEFAULT_PREVIEW_SIZE`] for the measured cost this pairs with.
pub const DEFAULT_PREVIEW_SPP: u32 = 256;
/// The solid preview's Solid/Path-traced/Both view-mode selector -- `0` is Solid (the
/// software-rasterized inspection view), `1` Path-traced (today's GPU/CPU render),
/// `2` Both (edges from the solid rasterizer composited over the path-traced image --
/// see `solid_preview::edges_layer`). A plain `u8` since that's what a Slint `int`
/// property round-trips most directly. `0` (Solid) by default so a settings file
/// predating this control loads into the same view that already shipped as the sole
/// mode.
pub const DEFAULT_SOLID_VIEW_MODE: u8 = 0;
/// The "Edit" sub-tab's auto-solve budget, in milliseconds -- see
/// `gui::editor::auto_solve::should_schedule_auto_solve`. After an edit, a design
/// whose last measured solve took less than this is re-solved automatically
/// (debounced); `300` was picked as comfortably above a small design's typical solve
/// cost while staying well under what would read as sluggish. `0` disables auto-solve
/// outright, reproducing this crate's pre-existing (Solve-button-only) behaviour.
pub const DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS: u32 = 300;

/// The four originally in-memory-only settings, plus camera pose and the selected
/// material -- everything migrated into persistent storage.
///
/// `#[serde(default)]` on the struct makes every field individually optional on
/// deserialization: a settings file that predates a field still loads successfully
/// with that field defaulted. Full-document parse failure is handled one level up, in
/// `store::load_or_default`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// The user's progressive-accumulation target sample count, replacing the old
    /// four-tier `quality_preset` string (which TOML silently drops on load).
    pub target_samples: u32,
    /// Live render resolution, driving `RenderContext.width`/`.height` directly.
    /// Restricted in the UI to a fixed pill list -- 640x480, 800x600 (default),
    /// 1280x720, 1920x1080 -- rather than tracking the viewport `Image`'s actual size,
    /// which Slint has no mechanism to report back to Rust. A hand-edited value
    /// outside the four pills still loads and renders fine.
    pub render_width: u32,
    pub render_height: u32,
    pub max_bounces: u32,
    pub exposure: f32,
    pub light_yaw_deg: f32,
    pub light_pitch_deg: f32,
    /// The lighting "rig" selection (e.g. "Gem Studio Ring Lights") -- what
    /// `RenderContext::lighting_preset` calls a "lighting preset". Named
    /// `lighting_rig` to stay unambiguous next to `LightingPreset` (the saveable
    /// bundle, which itself has a `lighting_rig` field referencing this).
    pub lighting_rig: String,
    pub camera_yaw: f32,
    pub camera_pitch: f32,
    pub camera_distance: f32,
    pub selected_material: String,
    /// Configured remote render workers, global (not per-session) -- see
    /// `add_worker`/`update_worker`/`remove_worker`.
    #[serde(default)]
    pub remote_workers: Vec<WorkerSettings>,
    /// Whether the À-Trous denoiser is applied to the merged accumulation, regardless
    /// of which backend produced it -- a single toggle covering the whole image, never
    /// per-source, since denoising is nonlinear and can only be applied once to the
    /// fully merged result.
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,
    /// Inclusion/subsurface scattering amount: the Henyey-Greenstein `sigma_s`
    /// applied via `GemMaterial::with_scattering_amount` -- see `scattering_sigma_s`
    /// in `crates/indicatrix/src/optics/materials.rs` for the `0.05`-`3.0` useful
    /// range. `0.0` (default) is off: the exact deterministic Beer-Lambert path.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation override -- off ("as cut") by default, leaving each
    /// material's own `GemMaterial::c_axis` untouched. When on, `c_axis_tilt_deg`/
    /// `c_axis_azimuth_deg` below replace it via `gui::c_axis::angles_to_c_axis`.
    /// Disabled in the UI for an isotropic material, whose optic axis is physically
    /// meaningless.
    pub c_axis_override_enabled: bool,
    /// Tilt from the table normal (`+Y`), 0-90 degrees. `0.0` reproduces every
    /// material's default `c_axis` (`Vec3::Y`); `90.0` (azimuth `0.0`) reproduces
    /// Tourmaline's cut-orientation override (`Vec3::X`) to within `f32` precision.
    pub c_axis_tilt_deg: f32,
    /// Azimuth around `+Y`, 0-360 degrees.
    pub c_axis_azimuth_deg: f32,
    /// Bruted (frosted) girdle finish toggle -- off by default (every facet
    /// `Polished`). On, the girdle band from `girdle::girdle_facet_finishes` renders
    /// `Frosted` instead -- a measured +17.3% face-up brightness change on the
    /// standard round brilliant.
    pub girdle_frosted: bool,
    /// Facet (meet-point) edge rounding radius, in world units (girdle radius of order
    /// 1 model unit) -- see `GemMaterial::edge_rounding_radius`, which calls `0.01`
    /// comfortably inside the soft-glint-not-a-bevel range. `0.0` (default) is off. The
    /// slider's `0.03` ceiling is the largest radius the energy-conservation furnace
    /// test verifies end to end.
    pub edge_rounding_radius: f32,
    /// Physical stone size: girdle width in millimetres to treat the active design as
    /// measuring -- see `RenderContext::stone_width_mm` for the full mechanism.
    /// `0.0` (default) is off: unscaled, this crate's pre-existing behaviour.
    pub stone_width_mm: f32,
    /// Local preview-then-settle rendering -- while the camera moves, trace at a
    /// fraction of `render_width`/`render_height`, then snap to full resolution once
    /// settled. `Off` (default) reproduces pre-existing behaviour -- see
    /// `bridge::local_preview::effective_dimensions` for the mechanism.
    pub local_preview_scale: LocalPreviewScale,
    /// The one-shot full-quality remote render's total sample count -- global (like
    /// `denoise_enabled`), not per-worker, since every worker renders the identical
    /// request. `512` by default, matching the old hardcoded
    /// `gui::remote::REMOTE_RENDER_SAMPLES`.
    pub remote_render_samples: u32,
    /// Live rendering's Local/Remote/Local+Remote choice -- see `LiveComputeTarget`.
    #[serde(default)]
    pub live_compute_target: LiveComputeTarget,
    /// Local live-rendering CPU/CPU+GPU/GPU choice -- see `LocalComputeTarget`.
    #[serde(default)]
    pub local_compute_target: LocalComputeTarget,
    /// HDR environment maps: path to the last-loaded Radiance `.hdr` file, or empty
    /// for "no map loaded, use the studio rig" (same empty-string-means-off
    /// convention as `selected_material`/`lighting_rig`). `gui::mod::apply_loaded_settings`
    /// reloads it at startup via `load_env_map`; a failure surfaces as a toast and
    /// leaves the render on the studio rig without clearing this field, so fixing the
    /// file and relaunching picks it back up.
    #[serde(default)]
    pub env_map_path: String,
    /// Square render size (pixels) for cached front/top design-catalogue preview
    /// thumbnails -- see `bridge::preview_render` and [`DEFAULT_PREVIEW_SIZE`].
    #[serde(default = "default_preview_size")]
    pub preview_size: u32,
    /// Samples per pixel for the cached preview thumbnails -- see [`DEFAULT_PREVIEW_SPP`].
    #[serde(default = "default_preview_spp")]
    pub preview_spp: u32,
    /// The directory every high-resolution export writes into, chosen once via a
    /// native folder picker and remembered from then on. Empty means "not chosen
    /// yet" (same convention as `env_map_path`), which correctly triggers the
    /// one-time picker prompt on the next export.
    #[serde(default)]
    pub export_directory: String,
    /// The export filename template -- see `export_thread::filename_template` for
    /// the variable list and sanitising/collision behaviour. Defaults to the same
    /// template a fresh install gets (not an empty string, which would sanitise
    /// every file down to the "export" fallback name).
    #[serde(default = "default_export_filename_template")]
    pub export_filename_template: String,
    /// Whether the left-hand diagram-library panel is collapsed to a thin rail, to
    /// give the viewport more room.
    #[serde(default)]
    pub library_panel_collapsed: bool,
    /// The solid preview's Solid/Path-traced/Both view-mode selector -- see
    /// [`DEFAULT_SOLID_VIEW_MODE`].
    #[serde(default)]
    pub solid_view_mode: u8,
    /// The "Edit" sub-tab's auto-solve budget (milliseconds) -- see
    /// [`DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS`].
    #[serde(default = "default_editor_auto_solve_budget_ms")]
    pub editor_auto_solve_budget_ms: u32,
}

const fn default_preview_size() -> u32 {
    DEFAULT_PREVIEW_SIZE
}

const fn default_preview_spp() -> u32 {
    DEFAULT_PREVIEW_SPP
}

const fn default_denoise_enabled() -> bool {
    true
}

const fn default_editor_auto_solve_budget_ms() -> u32 {
    DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS
}

fn default_export_filename_template() -> String {
    DEFAULT_EXPORT_FILENAME_TEMPLATE.to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
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
            remote_workers: Vec::new(),
            denoise_enabled: true,
            inclusion_sigma_s: DEFAULT_INCLUSION_SIGMA_S,
            c_axis_override_enabled: DEFAULT_C_AXIS_OVERRIDE_ENABLED,
            c_axis_tilt_deg: DEFAULT_C_AXIS_TILT_DEG,
            c_axis_azimuth_deg: DEFAULT_C_AXIS_AZIMUTH_DEG,
            girdle_frosted: DEFAULT_GIRDLE_FROSTED,
            edge_rounding_radius: DEFAULT_EDGE_ROUNDING_RADIUS,
            stone_width_mm: DEFAULT_STONE_WIDTH_MM,
            local_preview_scale: DEFAULT_LOCAL_PREVIEW_SCALE,
            remote_render_samples: DEFAULT_REMOTE_RENDER_SAMPLES,
            live_compute_target: DEFAULT_LIVE_COMPUTE_TARGET,
            local_compute_target: DEFAULT_LOCAL_COMPUTE_TARGET,
            env_map_path: String::new(),
            preview_size: DEFAULT_PREVIEW_SIZE,
            preview_spp: DEFAULT_PREVIEW_SPP,
            export_directory: String::new(),
            export_filename_template: DEFAULT_EXPORT_FILENAME_TEMPLATE.to_string(),
            library_panel_collapsed: false,
            solid_view_mode: DEFAULT_SOLID_VIEW_MODE,
            editor_auto_solve_budget_ms: DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS,
        }
    }
}

impl AppSettings {
    /// Adds a new remote worker to the end of the list.
    pub fn add_worker(&mut self, worker: WorkerSettings) {
        self.remote_workers.push(worker);
    }

    /// Overwrites the worker at `index`.
    ///
    /// # Errors
    ///
    /// Returns an error message if `index` is out of range.
    pub fn update_worker(&mut self, index: usize, worker: WorkerSettings) -> Result<(), String> {
        let slot = self
            .remote_workers
            .get_mut(index)
            .ok_or_else(|| format!("No worker at index {index}."))?;
        *slot = worker;
        Ok(())
    }

    /// Removes the worker at `index`.
    ///
    /// # Errors
    ///
    /// Returns an error message if `index` is out of range.
    pub fn remove_worker(&mut self, index: usize) -> Result<(), String> {
        if index >= self.remote_workers.len() {
            return Err(format!("No worker at index {index}."));
        }
        self.remote_workers.remove(index);
        Ok(())
    }
}
