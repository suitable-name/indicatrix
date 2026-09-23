//! Applying settings loaded from disk into the render context and the UI's own
//! mirrored properties at startup ([`apply_loaded_settings`]), and the small
//! settings-value <-> UI-pill-index conversions that go with it (`bounces_index`,
//! `resolution_index`, `local_preview_scale_index`/`from_index`,
//! `local_compute_target_index`/`from_index`, `color_space_from_index`,
//! `find_option_index`), plus the two option-list refreshers
//! ([`refresh_lighting_preset_options`], [`refresh_material_options`]) and
//! [`env_map_status_text`]/[`is_c_axis_override_available`], both shared with a
//! later, user-initiated callback. Moved out of `gui::mod` purely to keep that file
//! from growing further -- same reasoning as `gui`'s other submodules.

use crate::{
    EditorModel, LibraryModel, LightingPresetItem, MainWindow, RemoteWorkerModel, SettingsModel,
    SolidPreviewModel, ViewportModel,
    bridge::render_thread::{RenderContext, load_env_map, resolve_material},
    gui::{
        optics::c_axis::angles_to_c_axis,
        remote::{live_compute_target_index, remote_samples_count_to_exponent},
        render::sample_scale::count_to_exponent,
        show_toast,
    },
    settings::{
        LightingPreset as SavedLightingPreset, LocalComputeTarget, LocalPreviewScale, SettingsFile,
    },
};
use indicatrix::{
    color::ColorSpace,
    optics::{
        LightingPreset,
        materials::{GemMaterial, OpticalCharacter},
    },
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};

/// The material every session falls back to when a persisted `selected_material`
/// is not found. Matches `RenderContext::default()`'s starting material.
const DEFAULT_MATERIAL_NAME: &str = "Diamond";

/// Applies settings loaded from disk into the render context and into the UI's own
/// mirrored properties (`target_samples_exponent`, `resolution_index`, `bounce_index`,
/// `exposure_val`, `light_yaw_deg`, `light_pitch_deg`, `inclusion_sigma_s`,
/// `c_axis_override_enabled`/`c_axis_tilt_deg`/`c_axis_azimuth_deg`, `girdle_frosted`,
/// `edge_rounding_radius`, `stone_width_mm` -- hoisted onto `MainWindow` for exactly this reason, see the
/// comment beside them in `app.slint`). Called once at startup, before the render
/// thread or any callback is wired up, so there is no risk of a callback firing
/// mid-application and racing this.
pub(super) fn apply_loaded_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    loaded: &SettingsFile,
) {
    let s = &loaded.settings;
    // Parse the persisted lighting-rig label back into its enum at this one
    // boundary -- gracefully migrating any legacy or unrecognized label (including the
    // old, mislabelled `"D65 Daylight (5500K)"` string) via `from_label`'s own
    // fallback, rather than resetting the user's choice. See
    // `indicatrix::optics::LightingPreset::from_label`.
    let lighting_preset = LightingPreset::from_label(&s.lighting_rig);

    // Resolve the persisted material once upfront so the render context, dropdown,
    // and UI defaults all agree on the effective name. A deleted custom material
    // must not leave mismatched state between context and UI.
    let material_options = ui.global::<ViewportModel>().get_material_options();
    let material_found = find_option_index(&material_options, &s.selected_material).is_some();
    let effective_material_name: &str = if material_found {
        &s.selected_material
    } else {
        DEFAULT_MATERIAL_NAME
    };

    apply_loaded_render_context(render_ctx, s, lighting_preset, effective_material_name);
    apply_loaded_ui_mirrors(ui, s, lighting_preset);

    if let Some(idx) = find_option_index(&material_options, effective_material_name) {
        ui.global::<ViewportModel>()
            .set_selected_material_index(idx);
    }
    if !material_found {
        show_toast(
            ui,
            &format!(
                "Material '{}' no longer exists; using {DEFAULT_MATERIAL_NAME}.",
                s.selected_material
            ),
            "info",
        );
    }
    // Whether the crystal-axis control is interactive at all depends on the
    // STARTING material -- must be set here too, not just from `on_material_changed`,
    // or a session restored on an isotropic material (e.g. the "Diamond" default) would
    // show the slider as enabled until the user touched the material dropdown once.
    let starting_material = resolve_material(
        &GemMaterial::all_materials(),
        &RenderContext::lock(render_ctx).custom_materials,
        effective_material_name,
    );
    ui.global::<SettingsModel>()
        .set_c_axis_override_available(is_c_axis_override_available(&starting_material));
    // The RI-tolerance filter default -- same "must be set here too" reasoning as
    // `c_axis_override_available` immediately above: `on_material_changed` (wired later,
    // in `material_quality::setup_material_changed_callback`) only fires on a SUBSEQUENT
    // change, so the STARTING material's RI must be pushed once here or the filter
    // panel would show a stale `2.417` (this property's `.slint` placeholder default)
    // for a session restored on any other material. `ri_tolerance_center` is seeded to
    // the same value -- it is `in-out` (the user can drag it away afterward), but a
    // freshly opened session should start with the tolerance band centred on whatever
    // is actually loaded, not on the placeholder.
    let ri_default = crate::bridge::preview_render::material_ri_at_sodium_d(&starting_material);
    // `ri_default` is an f64; Slint's `float` property type is f32 -- a lossy but
    // harmless cast for a display/filter-default value in the 1.0-3.0 RI range,
    // `cast_possible_truncation` is workspace-`allow`ed (`Cargo.toml`).
    ui.global::<LibraryModel>()
        .set_ri_material_default(ri_default as f32);
    ui.global::<LibraryModel>()
        .set_ri_tolerance_center(ri_default as f32);

    // Reload the last-loaded HDR environment map, if any. Mirrors
    // `on_load_env_map` (in `setup_environment_map_callbacks`) but runs before any
    // callback is wired up, so a load failure here can only toast, not race a
    // simultaneous user-initiated load. `s.env_map_path` is left untouched either way
    // -- a transient failure (file on a currently-unmounted drive, say) shouldn't
    // silently forget the path the next successful launch could still use.
    ui.global::<SettingsModel>()
        .set_env_map_path(s.env_map_path.clone().into());
    if s.env_map_path.is_empty() {
        ui.global::<SettingsModel>().set_env_map_loaded(false);
        ui.global::<SettingsModel>()
            .set_env_map_status(String::new().into());
    } else {
        match load_env_map(&s.env_map_path) {
            Ok(map) => {
                ui.global::<SettingsModel>()
                    .set_env_map_status(env_map_status_text(&map, &s.env_map_path).into());
                ui.global::<SettingsModel>().set_env_map_loaded(true);
                RenderContext::lock(render_ctx).env_map = Some(map);
            }
            Err(err) => {
                ui.global::<SettingsModel>().set_env_map_loaded(false);
                ui.global::<SettingsModel>()
                    .set_env_map_status(String::new().into());
                show_toast(
                    ui,
                    &format!("Could not reload saved HDR environment: {err}"),
                    "error",
                );
            }
        }
    }
}

/// The `RenderContext` half of [`apply_loaded_settings`]: writes every
/// render-context field the settings load restores. Split out to keep
/// [`apply_loaded_settings`] under clippy's length lint.
///
/// `material_name` is the caller's already-resolved effective name, never the
/// raw persisted value, so this cannot reintroduce stale-name mismatches.
fn apply_loaded_render_context(
    render_ctx: &Arc<Mutex<RenderContext>>,
    s: &crate::settings::model::AppSettings,
    lighting_preset: LightingPreset,
    material_name: &str,
) {
    let mut ctx = RenderContext::lock(render_ctx);
    ctx.target_samples = s.target_samples;
    ctx.width = s.render_width;
    ctx.height = s.render_height;
    ctx.max_bounces = s.max_bounces;
    ctx.exposure = s.exposure;
    ctx.inclusion_sigma_s = s.inclusion_sigma_s;
    // The settings dialog drags degrees, `RenderContext` stores the already-
    // resolved `Vec3` -- see `RenderContext::c_axis_override`'s own doc comment for
    // why this crossing happens here rather than downstream in `bridge`.
    ctx.c_axis_override = s
        .c_axis_override_enabled
        .then(|| angles_to_c_axis(s.c_axis_tilt_deg, s.c_axis_azimuth_deg));
    ctx.girdle_frosted = s.girdle_frosted;
    ctx.edge_rounding_radius = s.edge_rounding_radius;
    ctx.stone_width_mm = s.stone_width_mm;
    ctx.light_yaw = s.light_yaw_deg.to_radians();
    ctx.light_pitch = s.light_pitch_deg.to_radians().clamp(0.15, 1.55);
    ctx.lighting_preset = lighting_preset;
    ctx.yaw = s.camera_yaw;
    ctx.pitch = crate::gui::render::camera_lighting::wrap_pitch(s.camera_pitch);
    // Same dynamic zoom clamp as the live orbit camera, not a fixed range.
    // A saved distance for a larger/smaller design must not get pulled back.
    let (min_distance, max_distance) = crate::gui::render::camera_lighting::orbit_distance_bounds(
        crate::gui::solid_preview::preview_state::DEFAULT_MESH_BOUNDING_RADIUS,
    );
    ctx.distance = s.camera_distance.clamp(min_distance, max_distance);
    ctx.material_name = material_name.to_string();
    ctx.denoise_enabled = s.denoise_enabled;
    ctx.backdrop = s.backdrop;
    // Local preview-then-settle rendering / remote render sample
    // budget -- both live-update `RenderContext` at startup exactly like every
    // other setting in this block, `camera_moving` deliberately left at its
    // `Default` (`false`): it's re-derived from live camera-pose polling by
    // `gui::remote::poll_tick` within the first tick after the window opens, never
    // something a settings FILE has an opinion on.
    ctx.local_preview_scale = s.local_preview_scale;
    ctx.remote_render_samples = s.remote_render_samples;
    ctx.live_compute_target = s.live_compute_target;
    ctx.local_compute_target = s.local_compute_target;
    ctx.dirty = true;
}

/// The UI-mirrored-properties half of [`apply_loaded_settings`]: pushes every
/// `SettingsModel`/`RemoteWorkerModel`/`LibraryModel`/`SolidPreviewModel`/
/// `EditorModel`/`ViewportModel` property this settings load restores. Split out
/// purely to keep that function under clippy's function-length lint.
fn apply_loaded_ui_mirrors(
    ui: &MainWindow,
    s: &crate::settings::model::AppSettings,
    lighting_preset: LightingPreset,
) {
    // The slider stores an EXPONENT, the settings file stores a COUNT -- see
    // `gui::sample_scale`'s module doc comment for why, and for this conversion's
    // inverse (`exponent_to_count`, used when the slider itself changes).
    ui.global::<SettingsModel>()
        .set_target_samples_exponent(count_to_exponent(s.target_samples) as f32);
    ui.global::<SettingsModel>()
        .set_resolution_index(resolution_index(s.render_width, s.render_height));
    ui.global::<SettingsModel>()
        .set_preview_size_val(s.preview_size as f32);
    ui.global::<SettingsModel>()
        .set_preview_spp_val(s.preview_spp as f32);
    ui.global::<SettingsModel>()
        .set_bounce_index(bounces_index(s.max_bounces));
    ui.global::<SettingsModel>().set_exposure_val(s.exposure);
    ui.global::<SettingsModel>()
        .set_backdrop_index(s.backdrop.index());
    ui.global::<SettingsModel>()
        .set_inclusion_sigma_s(s.inclusion_sigma_s);
    ui.global::<SettingsModel>()
        .set_c_axis_override_enabled(s.c_axis_override_enabled);
    ui.global::<SettingsModel>()
        .set_c_axis_tilt_deg(s.c_axis_tilt_deg);
    ui.global::<SettingsModel>()
        .set_c_axis_azimuth_deg(s.c_axis_azimuth_deg);
    ui.global::<SettingsModel>()
        .set_girdle_frosted(s.girdle_frosted);
    ui.global::<SettingsModel>()
        .set_edge_rounding_radius(s.edge_rounding_radius);
    ui.global::<SettingsModel>()
        .set_stone_width_mm(s.stone_width_mm);
    ui.global::<SettingsModel>()
        .set_local_preview_scale_index(local_preview_scale_index(s.local_preview_scale));
    ui.global::<SettingsModel>()
        .set_live_compute_target_index(live_compute_target_index(s.live_compute_target));
    ui.global::<SettingsModel>()
        .set_local_compute_target_index(local_compute_target_index(s.local_compute_target));
    ui.global::<RemoteWorkerModel>()
        .set_render_samples_exponent(
            remote_samples_count_to_exponent(s.remote_render_samples) as f32
        );
    ui.global::<SettingsModel>()
        .set_light_yaw_deg(s.light_yaw_deg);
    ui.global::<SettingsModel>()
        .set_light_pitch_deg(s.light_pitch_deg);
    // Restore the library panel's collapsed/expanded state.
    ui.global::<LibraryModel>()
        .set_panel_collapsed(s.library_panel_collapsed);
    // Restore the Solid viewport's remembered view mode (0 Solid / 1
    // Path-traced / 2 Both) -- see `AppSettings::solid_view_mode`'s own doc
    // comment. This may fire `app.slint`'s `changed editor_solid_view_mode`
    // handler (hence `editor_solid_view_mode_changed`) before
    // `setup_editor_solid_view_mode_changed_callback` below ever connects a
    // handler to it -- harmless: a Slint callback with nothing connected yet is
    // simply a no-op, not a panic, and the debounced writer would only be asked
    // to persist the exact value it just loaded anyway.
    ui.global::<SolidPreviewModel>()
        .set_view_mode(i32::from(s.solid_view_mode));
    // Restore the Live Render tab's own remembered view mode (0 Solid / 1
    // Path-traced) -- see `AppSettings::live_view_mode`'s own doc comment. Same
    // harmless-early-`changed`-fire caveat as `solid_view_mode` above: nothing is
    // connected to `live_view_mode_changed` yet at this point in startup.
    ui.global::<ViewportModel>()
        .set_live_view_mode(i32::from(s.live_view_mode));
    // The "Edit" sub-tab's auto-solve budget (`gui::editor::auto_solve`) -- same
    // harmless-early-`changed`-fire caveat as `editor_solid_view_mode` above applies
    // here too.
    ui.global::<EditorModel>()
        .set_auto_solve_budget_ms(i32::try_from(s.editor_auto_solve_budget_ms).unwrap_or(i32::MAX));
    ui.global::<ViewportModel>()
        .set_selected_lighting_index(lighting_preset.index());
    // File > Open Recent (`MainWindow.recent_native_files`, `ui/app.slint`) -- a
    // root-component property, not an `EditorModel` one. `gui::editor::native_io`'s
    // `record_recent_native_file` keeps this same property live afterward on every
    // Save/Open Native (it reads/writes the settings file directly rather than
    // through this call's own already-loaded `s`, so the two never race each
    // other); this seeds it once at startup, before that module's own callbacks are
    // even wired up.
    ui.set_recent_native_files(ModelRc::new(VecModel::from(
        s.recent_native_files
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}

/// The settings dialog's "Loaded: <file> (`WxH`)" status line for a decoded
/// [`indicatrix::renderer::env_map::EnvironmentMap`] -- shared by the startup
/// reload in `apply_loaded_settings` and the user-initiated load in
/// `setup_environment_map_callbacks` so both report the map identically. Shows the
/// file name alone (not the full path, which can be long and is already visible in the
/// path field above it).
pub(in crate::gui) fn env_map_status_text(
    map: &indicatrix::renderer::env_map::EnvironmentMap,
    path: &str,
) -> String {
    let name = std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
    format!("Loaded: {name} ({}\u{d7}{})", map.width(), map.height())
}

/// Whether the crystal-axis orientation override has any effect on
/// `material` -- `false` for an isotropic material (Diamond, Spinel, Cubic Zirconia,
/// and any custom material with `birefringence_delta == 0.0`), whose optic axis is
/// physically meaningless: there is no birefringence to orient. Drives
/// `MainWindow.c_axis_override_available`, which `settings_dialog.slint` uses to grey
/// out the control and explain why, rather than silently letting the user drag a
/// slider that does nothing.
pub(in crate::gui) fn is_c_axis_override_available(material: &GemMaterial) -> bool {
    material.optical_character != OpticalCharacter::Isotropic
}

/// Reverse of the mapping baked into `settings_dialog.slint`'s bounce-count pill
/// selector (4/8/12/24/64/128 at indices 0-5, raised from the old 4/8/12/16/24 ladder
/// per the `bounce_cost.rs` benchmark -- see that pill block's own doc comment for the
/// measurements). Exact matches map directly; anything else -- a settings file written
/// by an older build (e.g. the retired 16-bounce rung) or a hand-edited value -- picks
/// the *nearest* rung by absolute distance rather than a single hardcoded fallback like
/// `resolution_index`/`local_preview_scale_index` below use. Those get away with
/// "assume the default option" because their lists are short and evenly spaced; this
/// ladder now spans 4..128 unevenly (24 -> 64 is a 40-bounce gap), so a blanket
/// fallback would misplace a value like 40 or 96 by a wide margin. Never used to
/// reject or clamp the actual stored/rendered `max_bounces` -- see `on_bounces_changed`
/// in `material_quality.rs`, which honours the raw persisted value untouched regardless
/// of which pill this highlights.
const fn bounces_index(bounces: u32) -> i32 {
    const RUNGS: [u32; 6] = [4, 8, 12, 24, 64, 128];
    let mut best_idx = 0usize;
    let mut best_dist = u32::MAX;
    let mut i = 0usize;
    while i < RUNGS.len() {
        let dist = RUNGS[i].abs_diff(bounces);
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
        i += 1;
    }
    best_idx as i32
}

/// Reverse of the mapping baked into `settings_dialog.slint`'s "Render Resolution"
/// pill selector (640x480/800x600/1280x720/1920x1080 at indices 0-3) -- a blanket
/// fixed-list-with-fallback treatment, used only to highlight the closest pill at
/// startup. Falls back to the 800x600 index (1) for any pair that isn't one of the
/// four presets (a hand-edited settings file, or one saved before this control existed
/// -- see `AppSettings::render_width`'s doc comment), matching
/// `RenderContext::default()`/`DEFAULT_RENDER_WIDTH`/`DEFAULT_RENDER_HEIGHT`. Never
/// used to reject or clamp the actual stored/rendered value -- `render_width`/
/// `render_height` themselves pass through untouched regardless of what this returns.
const fn resolution_index(width: u32, height: u32) -> i32 {
    match (width, height) {
        (640, 480) => 0,
        (1280, 720) => 2,
        (1920, 1080) => 3,
        _ => 1, // 800x600 and anything unrecognized
    }
}

/// Reverse of the mapping baked into `settings_dialog.slint`'s "Motion Preview
/// Resolution" pill selector (Off/Half/Quarter at indices 0-2) -- same fixed-list
/// treatment as `resolution_index` above, used to seed the pill at startup from a
/// persisted `LocalPreviewScale`.
const fn local_preview_scale_index(scale: LocalPreviewScale) -> i32 {
    match scale {
        LocalPreviewScale::Off => 0,
        LocalPreviewScale::Half => 1,
        LocalPreviewScale::Quarter => 2,
    }
}

/// Inverse of [`local_preview_scale_index`]: what `on_local_preview_scale_changed`
/// (in `setup_material_and_quality_callbacks`) converts the pill's clicked index back
/// into. Falls back to `Off` for anything outside `0..=2` (a value the fixed pill
/// selector itself can never actually send), matching `resolution_index`'s own
/// unrecognized-value fallback convention.
pub(in crate::gui) const fn local_preview_scale_from_index(index: i32) -> LocalPreviewScale {
    match index {
        1 => LocalPreviewScale::Half,
        2 => LocalPreviewScale::Quarter,
        _ => LocalPreviewScale::Off,
    }
}

/// Reverse of the mapping baked into `settings_dialog.slint`'s "Local Compute" pill
/// selector (CPU/CPU+GPU/GPU only at indices 0-2) -- same fixed-list treatment as
/// `local_preview_scale_index` above, used to seed the pill at startup from a
/// persisted `LocalComputeTarget`.
const fn local_compute_target_index(target: LocalComputeTarget) -> i32 {
    match target {
        LocalComputeTarget::Cpu => 0,
        LocalComputeTarget::CpuGpu => 1,
        LocalComputeTarget::Gpu => 2,
    }
}

/// Inverse of [`local_compute_target_index`]: what `on_local_compute_target_changed`
/// (in `material_quality::setup_material_and_quality_callbacks`) converts the pill's
/// clicked index back into. Falls back to `CpuGpu` for anything outside `0..=2` (a
/// value the fixed pill selector itself can never actually send), matching
/// `LocalComputeTarget::default()` -- the same "unset behaves like the default, not
/// like the first variant" convention `gui::remote::live_compute_target_from_index`
/// already uses for its own three-way pill.
pub(in crate::gui) const fn local_compute_target_from_index(index: i32) -> LocalComputeTarget {
    match index {
        0 => LocalComputeTarget::Cpu,
        2 => LocalComputeTarget::Gpu,
        _ => LocalComputeTarget::CpuGpu,
    }
}

/// Finds `needle`'s index in a Slint `[string]` model for restoring `ComboBox`
/// selections (material, lighting rig) from persisted names. Returns `None` if the
/// name isn't present (e.g., deleted custom material or legacy option).
///
/// Public to `gui` so other modules can reuse this logic instead of duplicating it.
#[must_use]
pub(in crate::gui) fn find_option_index(
    options: &ModelRc<SharedString>,
    needle: &str,
) -> Option<i32> {
    (0..options.row_count()).find_map(|i| {
        let matches = options.row_data(i).is_some_and(|s| s.as_str() == needle);
        matches.then_some(i as i32)
    })
}

/// Rebuilds the `MainWindow.lighting_presets` model from the settings store's current
/// preset list. Called after startup load and after every create/rename/delete so the
/// settings dialog's preset rows stay in sync with what's actually persisted.
pub(in crate::gui) fn refresh_lighting_preset_options(
    ui: &MainWindow,
    presets: &[SavedLightingPreset],
) {
    let items: Vec<LightingPresetItem> = presets
        .iter()
        .map(|p| LightingPresetItem {
            name: p.name.clone().into(),
            built_in: p.built_in,
            export_usable: p.export_usable,
            has_env_map: p.env_map_path.is_some(),
            // Not export-dialog state -- see `LightingPresetItem.selected`'s own doc
            // comment; this list is the settings dialog's, which never reads it.
            selected: false,
        })
        .collect();
    ui.global::<ViewportModel>()
        .set_lighting_presets(ModelRc::new(VecModel::from(items)));
}

/// Wires up the high-resolution export flow: validates the request, captures
/// a `SceneSnapshot` independent of the live viewport's `RenderContext.width`/`height`
/// and accumulation buffer (see `export_thread`'s module doc comment for why that
/// separation matters), and spawns it on its own worker thread via
/// `export_thread::spawn_export`. The returned `ExportHandle` is kept in a
/// `Rc<RefCell<Option<_>>>` -- plain UI-thread-only state, not `Arc<Mutex<_>>>`, since
/// both callbacks here only ever run on the Slint event loop -- so `cancel_export` can
/// reach the in-flight export. Split out of `run_gui` purely to keep that function
/// under clippy's function-length lint.
/// Inverse of `export_dialog.slint`'s "Colour Space" pill selector
/// (sRGB/Display P3/Rec.2020 at indices 0-2) -- same fixed-list-with-fallback treatment
/// as `local_preview_scale_from_index` above. Falls back to `ColorSpace::Srgb` (index
/// 0, the required default -- see `bridge::export_thread`'s module doc comment on why
/// that space's output must stay byte-identical to before this control existed) for
/// any value the fixed pill selector itself can never actually send.
///
/// `ColorSpace::AcesCg` has no index here at all -- it is not offered by the picker,
/// see `export_dialog.slint`'s own doc comment for why a scene-linear space doesn't
/// belong in an 8-bit PNG export.
pub(in crate::gui) const fn color_space_from_index(index: i32) -> ColorSpace {
    match index {
        1 => ColorSpace::DisplayP3,
        2 => ColorSpace::Rec2020,
        _ => ColorSpace::Srgb,
    }
}

/// Every built-in material the Render Material `ComboBox` should offer, sorted
/// alphabetically (case-insensitive).
///
/// `GemMaterial::all_materials()` -- not a second, hand-maintained name list -- is the
/// single source of truth for "every built-in species exists"; this function only
/// decides the display order. Alphabetical is safe for persisted settings because
/// `AppSettings::selected_material` stores the NAME, not an index (custom materials
/// are appended after this list by `refresh_material_options`).
fn built_in_material_option_names() -> Vec<String> {
    let mut names: Vec<String> = GemMaterial::all_materials()
        .into_iter()
        .map(|m| m.name)
        .collect();
    names.sort_by_key(|a| a.to_ascii_lowercase());
    names
}

pub(in crate::gui) fn refresh_material_options(ui: &MainWindow, custom_mats: &[GemMaterial]) {
    let mut names = built_in_material_option_names();
    for m in custom_mats {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&m.name)) {
            names.push(m.name.clone());
        }
    }
    let model: Vec<SharedString> = names.into_iter().map(std::convert::Into::into).collect();
    ui.global::<ViewportModel>()
        .set_material_options(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounces_index_round_trips_all_six_pill_values() {
        assert_eq!(bounces_index(4), 0);
        assert_eq!(bounces_index(8), 1);
        assert_eq!(bounces_index(12), 2);
        assert_eq!(bounces_index(24), 3);
        assert_eq!(bounces_index(64), 4);
        assert_eq!(bounces_index(128), 5);
    }

    #[test]
    fn bounces_index_picks_the_nearest_rung_for_unknown_values() {
        // The retired 16-bounce rung from the old ladder: 4 away from 12, 8 away
        // from 24, so it lands on the same index (2) the old blanket fallback used
        // to give it -- but via nearest-distance, not a hardcoded default.
        assert_eq!(bounces_index(16), 2);
        // Below the lowest rung and above the highest rung both clamp to the nearest
        // end of the ladder rather than falling back to the default (12).
        assert_eq!(bounces_index(1), 0);
        assert_eq!(bounces_index(999), 5);
        // Roughly equidistant between 24 and 64 (20 either way) -- ties resolve to
        // whichever rung is checked first in `RUNGS`, i.e. the lower one.
        assert_eq!(bounces_index(44), 3);
    }

    #[test]
    fn resolution_index_round_trips_all_four_pill_values() {
        assert_eq!(resolution_index(640, 480), 0);
        assert_eq!(resolution_index(800, 600), 1);
        assert_eq!(resolution_index(1280, 720), 2);
        assert_eq!(resolution_index(1920, 1080), 3);
    }

    #[test]
    fn resolution_index_falls_back_to_800x600_for_unknown_values() {
        assert_eq!(resolution_index(1, 1), 1);
        assert_eq!(resolution_index(3840, 2160), 1);
        // A mismatched pair (e.g. a hand-edited file with one dimension changed but not
        // the other) must not accidentally match a pill via one coordinate alone.
        assert_eq!(resolution_index(640, 600), 1);
    }

    #[test]
    fn local_preview_scale_index_round_trips_all_three_pill_values() {
        for scale in [
            LocalPreviewScale::Off,
            LocalPreviewScale::Half,
            LocalPreviewScale::Quarter,
        ] {
            let idx = local_preview_scale_index(scale);
            assert_eq!(
                local_preview_scale_from_index(idx),
                scale,
                "scale={scale:?}"
            );
        }
        assert_eq!(local_preview_scale_index(LocalPreviewScale::Off), 0);
        assert_eq!(local_preview_scale_index(LocalPreviewScale::Half), 1);
        assert_eq!(local_preview_scale_index(LocalPreviewScale::Quarter), 2);
    }

    #[test]
    fn local_preview_scale_from_index_falls_back_to_off_for_unknown_values() {
        assert_eq!(local_preview_scale_from_index(-1), LocalPreviewScale::Off);
        assert_eq!(local_preview_scale_from_index(99), LocalPreviewScale::Off);
    }

    #[test]
    fn local_compute_target_index_round_trips_all_three_pill_values() {
        for target in [
            LocalComputeTarget::Cpu,
            LocalComputeTarget::CpuGpu,
            LocalComputeTarget::Gpu,
        ] {
            let idx = local_compute_target_index(target);
            assert_eq!(
                local_compute_target_from_index(idx),
                target,
                "target={target:?}"
            );
        }
        assert_eq!(local_compute_target_index(LocalComputeTarget::Cpu), 0);
        assert_eq!(local_compute_target_index(LocalComputeTarget::CpuGpu), 1);
        assert_eq!(local_compute_target_index(LocalComputeTarget::Gpu), 2);
    }

    #[test]
    fn local_compute_target_from_index_falls_back_to_cpu_gpu_for_unknown_values() {
        assert_eq!(
            local_compute_target_from_index(-1),
            LocalComputeTarget::CpuGpu
        );
        assert_eq!(
            local_compute_target_from_index(99),
            LocalComputeTarget::CpuGpu
        );
    }

    #[test]
    fn color_space_from_index_maps_all_three_pill_values() {
        assert_eq!(color_space_from_index(0), ColorSpace::Srgb);
        assert_eq!(color_space_from_index(1), ColorSpace::DisplayP3);
        assert_eq!(color_space_from_index(2), ColorSpace::Rec2020);
    }

    #[test]
    fn color_space_from_index_falls_back_to_srgb_for_unknown_values() {
        assert_eq!(color_space_from_index(-1), ColorSpace::Srgb);
        assert_eq!(color_space_from_index(99), ColorSpace::Srgb);
    }

    #[test]
    fn find_option_index_locates_an_exact_match() {
        let options: ModelRc<SharedString> = ModelRc::new(VecModel::from(vec![
            SharedString::from("Diamond"),
            SharedString::from("Sapphire"),
            SharedString::from("Ruby"),
        ]));
        assert_eq!(find_option_index(&options, "Sapphire"), Some(1));
        assert_eq!(find_option_index(&options, "Ruby"), Some(2));
    }

    #[test]
    fn find_option_index_returns_none_when_absent_rather_than_guessing() {
        let options: ModelRc<SharedString> =
            ModelRc::new(VecModel::from(vec![SharedString::from("Diamond")]));
        assert_eq!(find_option_index(&options, "Moissanite"), None);
    }

    #[test]
    fn find_option_index_on_an_empty_model_returns_none() {
        let options: ModelRc<SharedString> =
            ModelRc::new(VecModel::from(Vec::<SharedString>::new()));
        assert_eq!(find_option_index(&options, "anything"), None);
    }
}
