//! The named-lighting/view-preset callbacks: create (lighting-only, the existing
//! button), create (full view, the new "Save as preset" viewport button), apply,
//! rename, delete, and toggle export-usable.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::{RenderContext, load_env_map},
    gui::{env_map_status_text, refresh_lighting_preset_options, show_toast},
    settings::{LightingPreset as SavedLightingPreset, SettingsPersister},
};
use indicatrix::optics::LightingPreset;
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

/// Wires up every named-lighting/view-preset callback: the existing lighting-only save,
/// the new full-view save, apply, rename, delete, and the export-usable toggle. A thin
/// orchestrator over six single-purpose helpers, each split out purely to keep every
/// individual function under clippy's function-length lint.
pub(in crate::gui) fn setup_lighting_preset_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    setup_save_lighting_preset_callback(ui, render_ctx, settings_store);
    setup_save_view_preset_callback(ui, render_ctx, settings_store);
    setup_apply_lighting_preset_callback(ui, render_ctx, settings_store);
    setup_rename_lighting_preset_callback(ui, settings_store);
    setup_delete_lighting_preset_callback(ui, settings_store);
    setup_toggle_preset_export_usable_callback(ui, settings_store);
}

/// Reads the fields every save path (lighting-only AND full-view) captures in common:
/// the lighting rig itself, plus the HDR environment map path currently loaded, if any
/// -- see `LightingPreset::env_map_path`'s own doc comment for why this is captured by
/// BOTH save buttons rather than being part of what distinguishes them (only camera
/// pose is). Returns the empty-string-means-none `settings_store` value already
/// translated to this type's `Option<String>` convention.
struct CommonViewFields {
    light_yaw_deg: f32,
    light_pitch_deg: f32,
    exposure: f32,
    lighting_rig: String,
    camera_distance: f32,
    env_map_path: Option<String>,
}

fn read_common_view_fields(
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) -> CommonViewFields {
    let (light_yaw_deg, light_pitch_deg, exposure, lighting_rig, camera_distance) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.light_yaw.to_degrees(),
            ctx.light_pitch.to_degrees(),
            ctx.exposure,
            ctx.lighting_preset.label().to_string(),
            ctx.distance,
        )
    };
    // The persisted settings string, not `RenderContext::env_map` itself: the render
    // context only ever holds the DECODED map (an `Arc<EnvironmentMap>`, with no path
    // recorded alongside it -- see that field's own doc comment), while the settings
    // store is exactly where `gui::camera_lighting::setup_environment_map_callbacks`
    // already keeps the path that decoded it in sync. Empty string (this crate's
    // existing "no map loaded" convention for `AppSettings::env_map_path`) becomes
    // `None` here, matching `LightingPreset::env_map_path`'s own `Option` convention.
    let env_map_path = {
        let path = settings_store.snapshot().settings.env_map_path;
        (!path.is_empty()).then_some(path)
    };
    CommonViewFields {
        light_yaw_deg,
        light_pitch_deg,
        exposure,
        lighting_rig,
        camera_distance,
        env_map_path,
    }
}

/// Captures the CURRENT live lighting-rig state -- light yaw/pitch, exposure, rig
/// selection, camera distance, and the loaded HDR map path if any -- and saves it as a
/// new preset, or overwrites an existing user preset of the same name. Deliberately
/// writes `camera_yaw`/`camera_pitch: None` -- see `settings::model::LightingPreset`'s
/// own doc comment for why that invariant survives, unchanged, for presets created via
/// THIS button specifically (the new "Save as preset" viewport button,
/// `setup_save_view_preset_callback` below, is the one that captures a camera pose).
fn setup_save_lighting_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_save = render_ctx.clone();
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_save_lighting_preset(move |name: SharedString| {
            let fields = read_common_view_fields(&render_ctx_save, &settings_store_save);
            let preset = SavedLightingPreset {
                name: name.to_string(),
                built_in: false,
                light_yaw_deg: fields.light_yaw_deg,
                light_pitch_deg: fields.light_pitch_deg,
                exposure: fields.exposure,
                lighting_rig: fields.lighting_rig,
                camera_distance: fields.camera_distance,
                camera_yaw: None,
                camera_pitch: None,
                env_map_path: fields.env_map_path,
                export_usable: false,
            };

            let mut result = Ok(());
            settings_store_save.update(|s| result = s.upsert_user_preset(preset.clone()));

            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_save.snapshot().presets);
                    show_toast(&ui, &format!("Saved lighting preset '{name}'"), "success");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// The new "Save as preset" viewport button: captures the FULL current view -- every
/// field [`setup_save_lighting_preset_callback`] above does, PLUS the camera's current
/// yaw/pitch as `Some(..)` -- so applying this preset later restores the exact shot,
/// not just the lighting mood. This is the user-requested reversal of this type's old
/// "camera is never part of a preset" invariant -- see
/// `settings::model::LightingPreset`'s own doc comment for the full reasoning, and note
/// that it is deliberately scoped to ONLY this new button: the pre-existing settings-
/// dialog save button above keeps writing `None`, so a user who liked the old
/// lighting-only behaviour keeps getting it from the control they already know.
fn setup_save_view_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_save = render_ctx.clone();
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_save_view_preset(move |name: SharedString| {
            let fields = read_common_view_fields(&render_ctx_save, &settings_store_save);
            let (camera_yaw, camera_pitch) = {
                let ctx = RenderContext::lock(&render_ctx_save);
                (ctx.yaw, ctx.pitch)
            };
            let preset = SavedLightingPreset {
                name: name.to_string(),
                built_in: false,
                light_yaw_deg: fields.light_yaw_deg,
                light_pitch_deg: fields.light_pitch_deg,
                exposure: fields.exposure,
                lighting_rig: fields.lighting_rig,
                camera_distance: fields.camera_distance,
                camera_yaw: Some(camera_yaw),
                camera_pitch: Some(camera_pitch),
                env_map_path: fields.env_map_path,
                export_usable: false,
            };

            let mut result = Ok(());
            settings_store_save.update(|s| result = s.upsert_user_preset(preset.clone()));

            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_save.snapshot().presets);
                    show_toast(&ui, &format!("Saved view preset '{name}'"), "success");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// Applies preset `idx` (looked up in the settings store's current preset list, the
/// same list the UI's `lighting_presets` model was built from): pushes its fields into
/// `RenderContext` -- setting `ctx.dirty = true` so the render restarts, exactly as
/// the existing light controls do -- mirrors them into the UI's own slider/dropdown
/// state, and persists them as the new "current" settings so the applied look survives
/// a restart even without the user touching a slider afterward.
///
/// Camera pose and the HDR environment map are restored only when the preset actually
/// carries them (`camera_yaw`/`camera_pitch`/`env_map_path` all `Some`) -- see
/// `settings::model::LightingPreset`'s own doc comment for exactly what `None` means
/// for each and why leaving the current value untouched, rather than resetting to some
/// default, is the correct behaviour for both.
fn setup_apply_lighting_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_apply = render_ctx.clone();
    let settings_store_apply = settings_store.clone();
    let ui_weak_apply = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_apply_lighting_preset(move |idx: i32| {
            let Some(preset) = usize::try_from(idx)
                .ok()
                .and_then(|i| settings_store_apply.snapshot().presets.get(i).cloned())
            else {
                return;
            };

            let lighting_preset = LightingPreset::from_label(&preset.lighting_rig);

            // Loading an HDR file is I/O -- done here, BEFORE the lock below, matching
            // `gui::camera_lighting::setup_environment_map_callbacks`'s own "never assign
            // into `RenderContext::env_map` on `Err`" discipline: a preset pointing at a
            // file that has since moved or been deleted must leave whatever environment
            // (loaded map or studio rig) is currently active untouched, not clear it.
            let env_map_result = preset.env_map_path.as_deref().map(load_env_map);

            {
                let mut ctx = RenderContext::lock(&render_ctx_apply);
                ctx.light_yaw = preset.light_yaw_deg.to_radians();
                ctx.light_pitch = preset.light_pitch_deg.to_radians().clamp(0.15, 1.55);
                ctx.exposure = preset.exposure.clamp(0.2, 5.0);
                ctx.lighting_preset = lighting_preset;
                // The same dynamic zoom clamp the live orbit camera uses, sized to the
                // current mesh's own bounding radius rather than a fixed `[1.2, 8.0]`
                // range, so a preset saved while viewing a design larger/smaller than
                // `DEFAULT_MESH_BOUNDING_RADIUS` reapplies at a distance that actually
                // frames it.
                let (min_distance, max_distance) = super::camera_lighting::orbit_distance_bounds(
                    crate::gui::solid_preview::preview_state::DEFAULT_MESH_BOUNDING_RADIUS,
                );
                ctx.distance = preset.camera_distance.clamp(min_distance, max_distance);
                // Camera pose: only when the preset actually carries one -- see this
                // function's own doc comment. The same yaw/pitch clamps
                // `gui::camera_lighting::on_camera_orbit` applies to a live drag, so a
                // preset saved from an old build (before that clamp existed, however
                // unlikely) can't reapply an out-of-range pose.
                if let (Some(yaw), Some(pitch)) = (preset.camera_yaw, preset.camera_pitch) {
                    ctx.yaw = yaw;
                    ctx.pitch = pitch.clamp(-1.48, 1.48);
                }
                if let Some(Ok(map)) = &env_map_result {
                    ctx.env_map = Some(map.clone());
                }
                ctx.dirty = true;
            }

            settings_store_apply.update(|s| {
                s.settings.light_yaw_deg = preset.light_yaw_deg;
                s.settings.light_pitch_deg = preset.light_pitch_deg;
                s.settings.exposure = preset.exposure;
                s.settings.lighting_rig.clone_from(&preset.lighting_rig);
                s.settings.camera_distance = preset.camera_distance;
                if let (Some(yaw), Some(pitch)) = (preset.camera_yaw, preset.camera_pitch) {
                    s.settings.camera_yaw = yaw;
                    s.settings.camera_pitch = pitch.clamp(-1.48, 1.48);
                }
                if let Some(Ok(_)) = &env_map_result {
                    s.settings.env_map_path.clone_from(
                        preset
                            .env_map_path
                            .as_ref()
                            .expect("env_map_result implies Some"),
                    );
                }
            });

            let Some(ui) = ui_weak_apply.upgrade() else {
                return;
            };
            ui.global::<SettingsModel>()
                .set_light_yaw_deg(preset.light_yaw_deg);
            ui.global::<SettingsModel>()
                .set_light_pitch_deg(preset.light_pitch_deg);
            ui.global::<SettingsModel>()
                .set_exposure_val(preset.exposure);
            ui.global::<ViewportModel>()
                .set_selected_lighting_index(lighting_preset.index());
            // Environment-map status text/toggle: kept in sync with the same
            // `env_map_status_text` helper `gui::camera_lighting::on_load_env_map` uses, so
            // the settings dialog's HDR panel reflects a preset-driven load exactly like a
            // manual one.
            match &env_map_result {
                Some(Ok(map)) => {
                    let path = preset
                        .env_map_path
                        .as_deref()
                        .expect("env_map_result implies Some(path)");
                    ui.global::<SettingsModel>()
                        .set_env_map_status(env_map_status_text(map, path).into());
                    ui.global::<SettingsModel>().set_env_map_loaded(true);
                }
                Some(Err(err)) => {
                    show_toast(
                        &ui,
                        &format!("Preset's HDR environment could not be reloaded: {err}"),
                        "error",
                    );
                }
                None => {}
            }
            show_toast(
                &ui,
                &format!("Applied lighting preset '{}'", preset.name),
                "info",
            );
        });
}

fn setup_rename_lighting_preset_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_rename = settings_store.clone();
    let ui_weak_rename = ui.as_weak();
    ui.global::<ViewportModel>().on_rename_lighting_preset(
        move |idx: i32, new_name: SharedString| {
            let Some(old_name) = usize::try_from(idx).ok().and_then(|i| {
                settings_store_rename
                    .snapshot()
                    .presets
                    .get(i)
                    .map(|p| p.name.clone())
            }) else {
                return;
            };

            let mut result = Ok(());
            settings_store_rename.update(|s| result = s.rename_preset(&old_name, &new_name));

            let Some(ui) = ui_weak_rename.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_rename.snapshot().presets);
                    show_toast(&ui, "Preset renamed.", "success");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        },
    );
}

fn setup_delete_lighting_preset_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_delete = settings_store.clone();
    let ui_weak_delete = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_delete_lighting_preset(move |idx: i32| {
            let Some(name) = usize::try_from(idx).ok().and_then(|i| {
                settings_store_delete
                    .snapshot()
                    .presets
                    .get(i)
                    .map(|p| p.name.clone())
            }) else {
                return;
            };

            let mut result = Ok(());
            settings_store_delete.update(|s| result = s.delete_preset(&name));

            let Some(ui) = ui_weak_delete.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_delete.snapshot().presets);
                    show_toast(&ui, &format!("Deleted preset '{name}'"), "info");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// The settings dialog's "usable for export" checkbox next to each preset row -- see
/// `settings::model::LightingPreset::export_usable`'s own doc comment for what this
/// controls, and `SettingsFile::set_preset_export_usable` for why this is deliberately
/// NOT refused for a built-in preset the way rename/delete are.
fn setup_toggle_preset_export_usable_callback(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store_toggle = settings_store.clone();
    let ui_weak_toggle = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_toggle_preset_export_usable(move |idx: i32, usable: bool| {
            let Some(name) = usize::try_from(idx).ok().and_then(|i| {
                settings_store_toggle
                    .snapshot()
                    .presets
                    .get(i)
                    .map(|p| p.name.clone())
            }) else {
                return;
            };

            let mut result = Ok(());
            settings_store_toggle.update(|s| result = s.set_preset_export_usable(&name, usable));

            let Some(ui) = ui_weak_toggle.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_toggle.snapshot().presets);
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}
