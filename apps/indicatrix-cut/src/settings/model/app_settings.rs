//! [`AppSettings`]: the four originally in-memory-only settings, plus camera pose,
//! the selected material, and everything else migrated into persistent storage.

use super::{
    local_compute::LocalComputeTarget,
    remote_endpoint::{LegacyWorkerMigration, RemoteEndpoint, migrate_legacy_workers},
    surface_glare::{DEFAULT_SURFACE_GLARE, default_surface_glare, deserialize_surface_glare},
    worker::{LiveComputeTarget, LocalPreviewScale, WorkerSettings},
};
use crate::bridge::export_thread::DEFAULT_TEMPLATE as DEFAULT_EXPORT_FILENAME_TEMPLATE;
use indicatrix::optics::raytracer::DEFAULT_POSE;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// Defaults mirror `RenderContext::default()` and `settings_dialog.slint`'s property
// initializers, with ONE deliberate exception: `DEFAULT_LIGHTING_RIG` ("Light tent +
// black cards") does not match `RenderContext::default()`'s `lighting_preset`
// (`LightingPreset::RingLights`) -- the 2026-09-16 lighting rework changed the
// settings-dialog/startup default to the light tent without touching
// `RenderContext::default()` itself, which only ever seeds the render thread before
// `gui::startup_settings::apply_saved_settings` overwrites it with whatever
// `AppSettings` actually loads. Camera yaw/pitch are kept in RADIANS (matching
// `RenderContext`) while light yaw/pitch are kept in DEGREES (matching the
// settings-dialog sliders) -- each field stores whatever unit its main consumer
// already uses. See `gui::mod::apply_loaded_settings` for the one place that
// converts degrees -> radians.
/// `256` matches the old "High / Quality" preset's typical converged sample count --
/// see `RenderContext::default()`'s `target_samples`, which mirrors this.
pub const DEFAULT_TARGET_SAMPLES: u32 = 256;
/// Live render resolution -- matches `RenderContext::default()`'s `width`/`height`.
pub const DEFAULT_RENDER_WIDTH: u32 = 800;
/// Default render height for new settings.
pub const DEFAULT_RENDER_HEIGHT: u32 = 600;
/// Default max bounces for new settings: the raytracer's shared default.
pub use indicatrix::optics::raytracer::DEFAULT_MAX_BOUNCES;
/// Default exposure for new settings.
pub const DEFAULT_EXPOSURE: f32 = 1.0;
/// Default inclusion sigma s for new settings.
pub const DEFAULT_INCLUSION_SIGMA_S: f32 = 0.0;
/// Default c axis override enabled for new settings.
pub const DEFAULT_C_AXIS_OVERRIDE_ENABLED: bool = false;
/// Default c axis tilt deg for new settings.
pub const DEFAULT_C_AXIS_TILT_DEG: f32 = 0.0;
/// Default c axis azimuth deg for new settings.
pub const DEFAULT_C_AXIS_AZIMUTH_DEG: f32 = 0.0;
/// Default girdle frosted for new settings.
pub const DEFAULT_GIRDLE_FROSTED: bool = false;
/// Default edge rounding radius for new settings.
pub const DEFAULT_EDGE_ROUNDING_RADIUS: f32 = 0.0;
/// Default stone width mm for new settings.
pub const DEFAULT_STONE_WIDTH_MM: f32 = 0.0;
/// Default local preview scale for new settings.
pub const DEFAULT_LOCAL_PREVIEW_SCALE: LocalPreviewScale = LocalPreviewScale::Off;
/// `LiveComputeTarget::Both` is a no-op without a configured remote
/// (`orchestrator::poll_tick` only dispatches remote when `AppSettings::remote` is set),
/// so a fresh install behaves exactly as before -- `Both` only starts doing anything
/// once a remote is configured.
pub const DEFAULT_LIVE_COMPUTE_TARGET: LiveComputeTarget = LiveComputeTarget::Both;
/// `CpuGpu` matches `LocalComputeTarget::Default`, reproducing today's hybrid CPU+GPU
/// behaviour on a `gpu`-feature build (CPU-only otherwise, since `ViewportGpu` always
/// declines there regardless of this setting).
pub const DEFAULT_LOCAL_COMPUTE_TARGET: LocalComputeTarget = LocalComputeTarget::CpuGpu;
/// v16: on by default -- a configured remote's final-picture exports converge faster
/// with the idle local machine helping, and `worker::final_picture::contribution_allowed`'s
/// own guards (compute target, matching scene, HDR) already keep it from ever changing
/// what a picture looks like, only how fast it arrives.
pub const DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE: bool = true;
/// How many pictures a catalogue preview or tilt batch keeps in flight on the remote at
/// once, for new settings. `4` keeps a coordinator with a couple of joined workers busy
/// without queueing far more pictures than it can start.
pub const DEFAULT_REMOTE_BATCH_LANES: u32 = 4;
/// The fewest remote batch lanes: one picture in flight at a time.
pub const MIN_REMOTE_BATCH_LANES: u32 = 1;
/// The most remote batch lanes. A coordinator caps its concurrent connections (64 by
/// default), and every lane holds one for as long as its picture renders.
pub const MAX_REMOTE_BATCH_LANES: u32 = 32;

/// `lanes` limited to `1..=32` ([`MIN_REMOTE_BATCH_LANES`] to [`MAX_REMOTE_BATCH_LANES`])
/// -- the one rule a loaded, edited or hand-written lane count passes through.
#[must_use]
pub fn clamp_remote_batch_lanes(lanes: u32) -> u32 {
    lanes.clamp(MIN_REMOTE_BATCH_LANES, MAX_REMOTE_BATCH_LANES)
}
/// Default light yaw deg for new settings.
pub const DEFAULT_LIGHT_YAW_DEG: f32 = 48.0;
/// Default light pitch deg for new settings.
pub const DEFAULT_LIGHT_PITCH_DEG: f32 = 54.0;
/// The light tent: the lit model whose ambient sits at middle grey with black cards for
/// facet contrast, so an undialled-in render already reads like a photograph.
pub const DEFAULT_LIGHTING_RIG: &str = "Light tent + black cards";

/// What the camera sees behind the stone -- `EnvironmentSource::Studio::backdrop`. Only
/// the camera ray sees it; the stone's optics never do, so leakage stays dark.
///
/// The `level`/`index`/`from_index` derivation this carries is now pinned once in
/// `indicatrix::render_setup::Backdrop` (shared with the browser app) and mirrored
/// here variant-for-variant -- this type keeps its own definition, rather than a
/// re-export, purely so it can carry the `serde` derives its on-disk settings-file
/// representation needs: `indicatrix`'s own `serde` dependency is optional/feature-
/// gated (off by default, on only for `indicatrix-net`'s wire protocol), and this
/// crate doesn't enable it, so adding an unconditional `Serialize`/`Deserialize` to
/// the indicatrix side would grow ITS base dependency footprint for every consumer,
/// not just this one. Every method below just converts to the indicatrix mirror and
/// delegates, so the derivation logic itself lives in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Backdrop {
    /// The environment's own ground, as lit.
    AsLit,
    /// A neutral grey backdrop card (about sRGB 160), for like-for-like comparisons.
    #[default]
    Grey,
    /// A white light box.
    White,
}

impl Backdrop {
    /// Backdrop radiance handed to `EnvironmentSource::with_backdrop`.
    #[must_use]
    pub const fn level(self) -> f32 {
        self.to_indicatrix().level()
    }

    /// Index into the settings dialog's pill row (`SettingsModel.backdrop_index`).
    #[must_use]
    pub const fn index(self) -> i32 {
        self.to_indicatrix().index()
    }

    /// Inverse of [`Self::index`]; anything else is the default.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        Self::from_indicatrix(indicatrix::render_setup::Backdrop::from_index(index))
    }

    /// Converts to the pure `indicatrix::render_setup` mirror this type's own
    /// `level`/`index` delegate to.
    const fn to_indicatrix(self) -> indicatrix::render_setup::Backdrop {
        match self {
            Self::AsLit => indicatrix::render_setup::Backdrop::AsLit,
            Self::Grey => indicatrix::render_setup::Backdrop::Grey,
            Self::White => indicatrix::render_setup::Backdrop::White,
        }
    }

    /// Inverse of [`Self::to_indicatrix`].
    const fn from_indicatrix(backdrop: indicatrix::render_setup::Backdrop) -> Self {
        match backdrop {
            indicatrix::render_setup::Backdrop::AsLit => Self::AsLit,
            indicatrix::render_setup::Backdrop::Grey => Self::Grey,
            indicatrix::render_setup::Backdrop::White => Self::White,
        }
    }
}
/// Default camera yaw for new settings: the raytracer's shared default pose.
pub const DEFAULT_CAMERA_YAW: f32 = DEFAULT_POSE.yaw;
/// Default camera pitch for new settings: the raytracer's shared default pose.
pub const DEFAULT_CAMERA_PITCH: f32 = DEFAULT_POSE.pitch;
/// Default camera distance for new settings: the raytracer's shared default pose.
pub const DEFAULT_CAMERA_DISTANCE: f32 = DEFAULT_POSE.distance;
/// Default material for new settings.
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
/// The Live Render tab's own Solid/Path-traced view-mode selector -- see
/// `ViewportModel.live_view_mode`'s own doc comment (`ui/models/viewport.slint`). `1`
/// (Path-traced) by default so a settings file predating this control loads into the
/// same spectral-tracer view that already shipped as the Live Render tab's sole mode --
/// unlike [`DEFAULT_SOLID_VIEW_MODE`], whose `0` default matches the EDIT tab's
/// pre-existing sole mode instead.
pub const DEFAULT_LIVE_VIEW_MODE: u8 = 1;
/// The "Edit" sub-tab's auto-solve budget, in milliseconds -- see
/// `gui::editor::auto_solve::should_schedule_auto_solve`. After an edit, a design
/// whose last measured solve took less than this is re-solved automatically
/// (debounced); `300` was picked as comfortably above a small design's typical solve
/// cost while staying well under what would read as sluggish. `0` disables auto-solve
/// outright, reproducing this crate's pre-existing (Solve-button-only) behaviour.
pub const DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS: u32 = 300;
/// The Edit sub-tab's dock width, in logical pixels -- see `EditorModel.dock_width`'s
/// own doc comment (`ui/models/editor.slint`) and `gui::editor_layout` for the
/// drag-clamp/persistence wiring. `640.0` is the narrowest width at which the tier table shows every column, so an
/// existing settings file (predating the resizable viewport|dock split) restores
/// exactly the width it always rendered at.
pub const DEFAULT_EDITOR_DOCK_WIDTH: f32 = 640.0;
/// The Edit sub-tab's inspector height, in logical pixels -- see `EditorModel.
/// inspector_height`'s own doc comment. `340.0` is tall enough for the Tier tab to show
/// its title plus the Angle, Meets, Name and Indices rows and the pinned Save/Add row
/// without scrolling (the old fixed `260.0` left a ~160px fold that hid Meets/Name/
/// Indices). A settings file that already carries `editor_inspector_height` keeps its
/// own value; only a missing key takes this default.
pub const DEFAULT_EDITOR_INSPECTOR_HEIGHT: f32 = 340.0;

/// The four originally in-memory-only settings, plus camera pose and the selected
/// material -- everything migrated into persistent storage.
///
/// `#[serde(default)]` on the struct makes every field individually optional on
/// deserialization: a settings file that predates a field still loads successfully
/// with that field defaulted. Full-document parse failure is handled one level up, in
/// `store::load_with_outcome`.
///
/// Unknown keys are IGNORED (no `deny_unknown_fields`), which is how a removed setting
/// retires without a migration: a file still carrying the old `remote_render_samples`
/// key (removed when the live view's `target_samples` became the single global target
/// for the combined local + remote image) loads fine and simply drops it on the next
/// save.
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
    /// Render height in pixels.
    pub render_height: u32,
    /// Maximum number of ray bounces per path.
    pub max_bounces: u32,
    /// Exposure multiplier applied when tone-mapping.
    pub exposure: f32,
    /// Light yaw in degrees.
    pub light_yaw_deg: f32,
    /// Light pitch in degrees.
    pub light_pitch_deg: f32,
    /// The lighting "rig" selection (e.g. "Gem Studio Ring Lights") -- what
    /// `RenderContext::lighting_preset` calls a "lighting preset". Named
    /// `lighting_rig` to stay unambiguous next to `LightingPreset` (the saveable
    /// bundle, which itself has a `lighting_rig` field referencing this).
    pub lighting_rig: String,
    /// Camera yaw.
    pub camera_yaw: f32,
    /// Camera pitch.
    pub camera_pitch: f32,
    /// Camera distance from the stone.
    pub camera_distance: f32,
    /// Name of the selected material.
    pub selected_material: String,
    /// The one remote this viewer renders with and browses the library of (a
    /// coordinator, rendering itself when started with `--render`). `None` renders
    /// locally only. See [`RemoteEndpoint`].
    #[serde(default)]
    pub remote: Option<RemoteEndpoint>,
    /// The retired multi-worker list, read (never written) only so an old settings file
    /// migrates: [`AppSettings::migrate_legacy_remote_workers`] moves its first entry into
    /// [`Self::remote`] and logs the rest as dropped. Always empty after a load.
    #[serde(rename = "remote_workers", default, skip_serializing)]
    pub legacy_remote_workers: Vec<WorkerSettings>,
    /// Whether the À-Trous denoiser is applied to the merged accumulation, regardless
    /// of which backend produced it -- a single toggle covering the whole image, never
    /// per-source, since denoising is nonlinear and can only be applied once to the
    /// fully merged result.
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,
    /// What the camera sees behind the stone, in the live view and every export.
    #[serde(default)]
    pub backdrop: Backdrop,
    /// Scale of the white mirror image of the light on the table, `0.0..=1.0` (`1.0`,
    /// the default, is the unscaled render). Applies to the built-in lighting presets
    /// in the live view and every export, never to an HDR map. Limited when loaded
    /// (see [`super::clamp_surface_glare`]); a file without the key loads `1.0`.
    #[serde(
        default = "default_surface_glare",
        deserialize_with = "deserialize_surface_glare"
    )]
    pub surface_glare: f32,
    /// Inclusion/subsurface scattering amount: the Henyey-Greenstein `sigma_s`
    /// applied via `GemMaterial::with_scattering_amount` -- see `scattering_sigma_s`
    /// in `crates/indicatrix/src/optics/materials/mod.rs` for the `0.05`-`3.0` useful
    /// range. `0.0` (default) is off: the exact deterministic Beer-Lambert path.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation override -- off ("as cut") by default, leaving each
    /// material's own `GemMaterial::c_axis` untouched. When on, `c_axis_tilt_deg`/
    /// `c_axis_azimuth_deg` below replace it via `gui::optics::c_axis::angles_to_c_axis`.
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
    /// `bridge::render_thread::local_preview::effective_dimensions` for the mechanism.
    pub local_preview_scale: LocalPreviewScale,
    /// Live rendering's Local/Remote/Local+Remote choice -- see `LiveComputeTarget`.
    #[serde(default)]
    pub live_compute_target: LiveComputeTarget,
    /// Local live-rendering CPU/CPU+GPU/GPU choice -- see `LocalComputeTarget`.
    #[serde(default)]
    pub local_compute_target: LocalComputeTarget,
    /// v16: whether this machine's own idle CPU/GPU traces a share of a "final picture
    /// only" export/tilt-frame alongside the remote, uploading its sum for the
    /// coordinator to fold in before tone-mapping -- see
    /// [`DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE`] and
    /// `worker::final_picture::contribution_allowed` for when it actually applies.
    #[serde(default = "default_contribute_to_final_picture")]
    pub contribute_to_final_picture: bool,
    /// How many pictures a catalogue preview or tilt batch keeps in flight on the remote
    /// at once -- one remote dispatcher per lane, each rendering a whole picture. A
    /// coordinator with several workers wants more than one; see
    /// [`DEFAULT_REMOTE_BATCH_LANES`]. Limited to `1..=32` when loaded and when set (see
    /// [`clamp_remote_batch_lanes`]); a file without the key loads the default.
    #[serde(
        default = "default_remote_batch_lanes",
        deserialize_with = "deserialize_remote_batch_lanes"
    )]
    pub remote_batch_lanes: u32,
    /// The remembered answer to the question asked after an import about generating
    /// catalogue previews (`ask` shows the question every time).
    #[serde(default)]
    pub import_preview_choice: super::ImportPreviewChoice,
    /// How the viewer compresses the uploads it sends a coordinator (today the
    /// `CONTRIBUTION` of a final-picture export): `"auto"` (default) follows the measured
    /// link speed, `"raw"`, `"lz4"`, `"zstd"` or `"zstd:LEVEL"` pin one encoding. A
    /// missing or unreadable value loads as `"auto"`.
    #[serde(
        default,
        deserialize_with = "indicatrix_net::messages::adaptive::PayloadChoice::deserialize_lenient"
    )]
    pub payload_encoding: indicatrix_net::messages::adaptive::PayloadChoice,
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
    /// The folder the `.asc` file and folder pickers open in, updated to whatever
    /// the cutter last imported from. Empty means "never imported", which leaves
    /// the picker at the OS default; importing one file at a time out of a single
    /// book folder is the common case, so re-navigating there every time was pure
    /// friction. Not shared with `export_directory`: designs are typically read
    /// from somewhere quite different from where renders are written.
    #[serde(default)]
    pub last_import_directory: String,
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
    /// The Live Render tab's Solid/Path-traced view-mode selector -- see
    /// [`DEFAULT_LIVE_VIEW_MODE`].
    #[serde(default = "default_live_view_mode")]
    pub live_view_mode: u8,
    /// The "Edit" sub-tab's auto-solve budget (milliseconds) -- see
    /// [`DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS`].
    #[serde(default = "default_editor_auto_solve_budget_ms")]
    pub editor_auto_solve_budget_ms: u32,
    /// The Edit sub-tab's dock width -- see [`DEFAULT_EDITOR_DOCK_WIDTH`].
    #[serde(default = "default_editor_dock_width")]
    pub editor_dock_width: f32,
    /// The Edit sub-tab's inspector height -- see [`DEFAULT_EDITOR_INSPECTOR_HEIGHT`].
    #[serde(default = "default_editor_inspector_height")]
    pub editor_inspector_height: f32,
    /// Whether the Edit sub-tab's "DESIGN" section (`editor_design_settings.slint`)
    /// is collapsed.
    #[serde(default)]
    pub editor_settings_collapsed: bool,
    /// Whether the Edit sub-tab's "INSPECTOR" section (`editor_inspector.slint`) is
    /// collapsed.
    #[serde(default)]
    pub editor_inspector_collapsed: bool,
    /// Whether the Edit sub-tab's "GEAR REMAP" section (nested inside "DESIGN") is
    /// collapsed.
    #[serde(default)]
    pub editor_remap_collapsed: bool,
    /// Whether the user has ever changed the Edit sub-tab's layout (dock width,
    /// inspector height, or any section's collapsed state) -- see
    /// `gui::window_sizing`'s per-screen inspector-collapse default, which only
    /// applies while this is still `false`, and `gui::editor_layout::
    /// setup_editor_layout_callbacks`, the only place that ever sets it `true`.
    #[serde(default)]
    pub editor_layout_touched: bool,
    /// The Edit sub-tab's "Open Recent" list: up to
    /// [`MAX_RECENT_NATIVE_FILES`] design-file paths, most-recently-used first --
    /// `.indicatrix` design files, plus older `.indicatrix.toml`/`.gemcut.toml`
    /// sidecars that are still openable. Design-file paths only (never a bare
    /// `.asc`, and never a row in the read-only catalogue database, which this list
    /// deliberately has nothing to do with) -- see [`Self::record_recent_native_file`].
    #[serde(default)]
    pub recent_native_files: Vec<String>,
    /// Keys of the in-window `ConfirmActionDialog` prompts the cutter has ticked
    /// "Don't ask again" on (`ui/components/confirm_action_dialog.slint`'s
    /// `show_dont_ask`/`dont_ask`).
    /// A `BTreeSet` (not `HashMap`/`HashSet` -- house rule: no hash-map iteration
    /// in a decision path), keyed by a short, stable string each call site owns
    /// (e.g. `"write_confirm.not_closed_solid"`) -- see
    /// `gui::editor::native_io::confirm`'s `confirm_keys` module for the write-confirm
    /// keys and `gui::editor::state`'s anchor-explainer key. A key present here means
    /// that SPECIFIC prompt is silenced;
    /// nothing else is affected, so suppressing "not a closed solid" never
    /// silences any other prompt or vice versa.
    #[serde(default)]
    pub suppressed_confirmations: BTreeSet<String>,
}

/// [`AppSettings::recent_native_files`]'s cap: up to ten recent files.
pub const MAX_RECENT_NATIVE_FILES: usize = 10;

const fn default_preview_size() -> u32 {
    DEFAULT_PREVIEW_SIZE
}

const fn default_preview_spp() -> u32 {
    DEFAULT_PREVIEW_SPP
}

const fn default_denoise_enabled() -> bool {
    true
}

const fn default_contribute_to_final_picture() -> bool {
    DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE
}

const fn default_remote_batch_lanes() -> u32 {
    DEFAULT_REMOTE_BATCH_LANES
}

/// Reads [`AppSettings::remote_batch_lanes`] as a signed integer and limits it, so a
/// hand-edited value outside `1..=32` (a negative one included) loads as the nearest
/// valid count instead of failing the whole settings file.
fn deserialize_remote_batch_lanes<'de, D>(deserializer: D) -> Result<u32, D::Error>
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

const fn default_editor_auto_solve_budget_ms() -> u32 {
    DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS
}

const fn default_live_view_mode() -> u8 {
    DEFAULT_LIVE_VIEW_MODE
}

const fn default_editor_dock_width() -> f32 {
    DEFAULT_EDITOR_DOCK_WIDTH
}

const fn default_editor_inspector_height() -> f32 {
    DEFAULT_EDITOR_INSPECTOR_HEIGHT
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
            remote: None,
            legacy_remote_workers: Vec::new(),
            denoise_enabled: true,
            backdrop: Backdrop::default(),
            surface_glare: DEFAULT_SURFACE_GLARE,
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
            contribute_to_final_picture: DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE,
            remote_batch_lanes: DEFAULT_REMOTE_BATCH_LANES,
            import_preview_choice: super::ImportPreviewChoice::Ask,
            payload_encoding: indicatrix_net::messages::adaptive::PayloadChoice::Auto,
            env_map_path: String::new(),
            preview_size: DEFAULT_PREVIEW_SIZE,
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
            editor_remap_collapsed: false,
            editor_layout_touched: false,
            recent_native_files: Vec::new(),
            suppressed_confirmations: BTreeSet::new(),
        }
    }
}

impl AppSettings {
    /// The remote endpoint's connection settings, if one is configured -- what every
    /// render/library code path connects with.
    #[must_use]
    pub fn remote_worker(&self) -> Option<WorkerSettings> {
        self.remote.as_ref().map(|r| r.connection.clone())
    }

    /// Folds a legacy multi-worker `remote_workers` list into [`Self::remote`] -- see
    /// [`migrate_legacy_workers`]. Called by `settings::store::load_with_outcome` right
    /// after parsing; idempotent (a second call finds the legacy list empty).
    pub fn migrate_legacy_remote_workers(&mut self) -> Option<LegacyWorkerMigration> {
        migrate_legacy_workers(&mut self.legacy_remote_workers, &mut self.remote)
    }

    /// Sets [`Self::remote_batch_lanes`], limited to `1..=32` -- the only way the
    /// running app changes it.
    pub fn set_remote_batch_lanes(&mut self, lanes: u32) {
        self.remote_batch_lanes = clamp_remote_batch_lanes(lanes);
    }

    /// Records `path` as the most-recently-used native design file: moves it to the
    /// front if already present (never duplicated), inserts it at the front
    /// otherwise, and truncates back to [`MAX_RECENT_NATIVE_FILES`] entries. Called on
    /// every successful Save/Open -- see this field's own doc comment.
    ///
    /// Both file kinds are accepted. Recording a `.indicatrix` design file drops the
    /// older `.indicatrix.toml`/`.gemcut.toml` entry of the same folder and name, so
    /// the list prefers the new extension once a design has been saved in it.
    pub fn record_recent_native_file(&mut self, path: String) {
        self.recent_native_files.retain(|existing| {
            existing != &path && !super::recent_files::is_superseded_sidecar(existing, &path)
        });
        self.recent_native_files.insert(0, path);
        self.recent_native_files.truncate(MAX_RECENT_NATIVE_FILES);
    }

    /// Whether the confirm prompt named `key` has been silenced -- see
    /// [`Self::suppressed_confirmations`]'s own doc comment.
    #[must_use]
    pub fn is_confirm_suppressed(&self, key: &str) -> bool {
        self.suppressed_confirmations.contains(key)
    }

    /// Silences the confirm prompt named `key` -- idempotent, like every other
    /// `BTreeSet::insert`.
    pub fn suppress_confirm(&mut self, key: impl Into<String>) {
        self.suppressed_confirmations.insert(key.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_recent_native_file_inserts_at_the_front() {
        let mut settings = AppSettings::default();
        settings.record_recent_native_file("a.indicatrix.toml".to_string());
        settings.record_recent_native_file("b.indicatrix.toml".to_string());
        assert_eq!(
            settings.recent_native_files,
            vec![
                "b.indicatrix.toml".to_string(),
                "a.indicatrix.toml".to_string()
            ]
        );
    }

    #[test]
    fn record_recent_native_file_moves_an_existing_entry_to_the_front_without_duplicating() {
        let mut settings = AppSettings::default();
        settings.record_recent_native_file("a.indicatrix.toml".to_string());
        settings.record_recent_native_file("b.indicatrix.toml".to_string());
        settings.record_recent_native_file("a.indicatrix.toml".to_string());
        assert_eq!(
            settings.recent_native_files,
            vec![
                "a.indicatrix.toml".to_string(),
                "b.indicatrix.toml".to_string()
            ]
        );
    }

    #[test]
    fn recording_a_design_file_replaces_the_older_sidecar_entry_of_the_same_design() {
        let mut settings = AppSettings::default();
        settings.record_recent_native_file("d/round.indicatrix.toml".to_string());
        settings.record_recent_native_file("d/other.gemcut.toml".to_string());
        settings.record_recent_native_file("d/round.indicatrix".to_string());
        assert_eq!(
            settings.recent_native_files,
            vec![
                "d/round.indicatrix".to_string(),
                "d/other.gemcut.toml".to_string()
            ],
            "both kinds are accepted; the older entry of the saved design is gone"
        );
        // Opening an older sidecar again afterwards is still recorded.
        settings.record_recent_native_file("d/legacy.indicatrix.toml".to_string());
        assert_eq!(settings.recent_native_files[0], "d/legacy.indicatrix.toml");
        assert_eq!(settings.recent_native_files.len(), 3);
    }

    #[test]
    fn record_recent_native_file_caps_at_max_recent_native_files() {
        let mut settings = AppSettings::default();
        for i in 0..(MAX_RECENT_NATIVE_FILES + 3) {
            settings.record_recent_native_file(format!("design-{i}.indicatrix.toml"));
        }
        assert_eq!(settings.recent_native_files.len(), MAX_RECENT_NATIVE_FILES);
        // Most recent first: the last one recorded is still at the front, and the
        // oldest entries fell off the back rather than the front.
        assert_eq!(
            settings.recent_native_files[0],
            format!("design-{}.indicatrix.toml", MAX_RECENT_NATIVE_FILES + 2)
        );
    }

    // --- Suppressed confirmations: AppSettings::suppressed_confirmations ---

    #[test]
    fn a_fresh_settings_file_suppresses_nothing() {
        let settings = AppSettings::default();
        assert!(!settings.is_confirm_suppressed("write_confirm.not_closed_solid"));
    }

    #[test]
    fn suppress_confirm_is_reflected_immediately() {
        let mut settings = AppSettings::default();
        settings.suppress_confirm("write_confirm.not_closed_solid");
        assert!(settings.is_confirm_suppressed("write_confirm.not_closed_solid"));
    }

    #[test]
    fn suppressing_one_key_does_not_suppress_a_different_one() {
        let mut settings = AppSettings::default();
        settings.suppress_confirm("write_confirm.not_closed_solid");
        assert!(!settings.is_confirm_suppressed("write_confirm.overwrite_unrelated"));
    }

    #[test]
    fn suppress_confirm_is_idempotent() {
        let mut settings = AppSettings::default();
        settings.suppress_confirm("write_confirm.not_closed_solid");
        settings.suppress_confirm("write_confirm.not_closed_solid");
        assert_eq!(settings.suppressed_confirmations.len(), 1);
    }

    /// A round trip through TOML -- the actual persistence mechanism
    /// (`settings::store`) -- so a regression that drops `#[serde(default)]` or
    /// breaks `BTreeSet<String>` serialisation is caught here rather than only in
    /// a live app.
    #[test]
    fn suppressed_confirmations_round_trips_through_toml() {
        let mut settings = AppSettings::default();
        settings.suppress_confirm("write_confirm.not_closed_solid");
        settings.suppress_confirm("write_confirm.overwrite_unrelated");
        let toml_text = toml::to_string(&settings).expect("settings must serialize");
        let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
        assert_eq!(
            restored.suppressed_confirmations,
            settings.suppressed_confirmations
        );
    }

    /// A settings file saved BEFORE this field existed (no `suppressed_confirmations`
    /// key at all) must still load, with nothing suppressed -- the idempotent
    /// migration this field's own `#[serde(default)]` provides.
    #[test]
    fn a_settings_file_predating_this_field_loads_with_nothing_suppressed() {
        let settings = AppSettings::default();
        let mut toml_text = toml::to_string(&settings).expect("settings must serialize");
        // Strip the field this test is specifically about, simulating an
        // old-format file that never had it.
        let filtered: String = toml_text
            .lines()
            .filter(|line| !line.contains("suppressed_confirmations"))
            .collect::<Vec<_>>()
            .join("\n");
        toml_text = filtered;
        let restored: AppSettings = toml::from_str(&toml_text)
            .expect("a settings file missing this field must still parse");
        assert!(restored.suppressed_confirmations.is_empty());
    }
}
