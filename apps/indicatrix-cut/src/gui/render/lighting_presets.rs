//! The named-lighting/view-preset callbacks: create (lighting-only, the existing
//! button), create (full view, the new "Save as preset" viewport button), apply,
//! rename, delete, and toggle export-usable.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::{
        export_thread::filename_template::civil_from_unix_seconds,
        render_thread::{RenderContext, load_env_map},
    },
    gui::{env_map_status_text, refresh_lighting_preset_options, show_toast},
    settings::{
        LightingPreset as SavedLightingPreset, SettingsFile, SettingsPersister,
        model::{LightingValues, PITCH_RANGE_RAD, percent_from_surface_glare},
    },
};
use indicatrix::{optics::LightingPreset, renderer::env_map::EnvironmentMap};
use slint::{ComponentHandle, SharedString};
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

/// Wires up every named-lighting/view-preset callback: the existing lighting-only save,
/// the new full-view save, apply, rename, delete, and the export-usable toggle. A thin
/// orchestrator over six single-purpose helpers, each split out purely to keep every
/// individual function under clippy's function-length lint.
pub(in crate::gui) fn setup_lighting_preset_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
) {
    setup_suggest_lighting_name_callback(ui, render_ctx, settings_store);
    setup_save_lighting_preset_callback(ui, render_ctx, settings_store);
    setup_save_view_preset_callback(ui, render_ctx, settings_store);
    setup_apply_lighting_preset_callback(ui, render_ctx, settings_store, mesh_bounding_radius);
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
    surface_glare: f32,
    backdrop: crate::settings::model::Backdrop,
    head_shadow_deg: f32,
}

fn read_common_view_fields(
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) -> CommonViewFields {
    let (
        light_yaw_deg,
        light_pitch_deg,
        exposure,
        lighting_rig,
        camera_distance,
        surface_glare,
        backdrop,
        head_shadow_deg,
    ) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.light_yaw.to_degrees(),
            ctx.light_pitch.to_degrees(),
            ctx.exposure,
            ctx.lighting_preset.label().to_string(),
            ctx.distance,
            ctx.surface_glare,
            ctx.backdrop,
            ctx.head_shadow_deg,
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
        surface_glare,
        backdrop,
        head_shadow_deg,
    }
}

/// `base`, or `base (2)`, `base (3)`, ... when a saved lighting of that name exists already,
/// so a name offered for a new save never silently overwrites one.
fn unique_name(base: String, taken: &[String]) -> String {
    let is_taken = |name: &str| taken.iter().any(|existing| existing == name);
    if !is_taken(&base) {
        return base;
    }
    let mut n = 2_u32;
    loop {
        let candidate = format!("{base} ({n})");
        if !is_taken(&candidate) || n == u32::MAX {
            return candidate;
        }
        n += 1;
    }
}

/// Wires `ViewportModel.suggest_lighting_name`, the name the Lighting tab's save rows start
/// with: "<rig> - <date>" ([`SavedLightingPreset::default_save_name`]), so a new saved lighting
/// no longer collides with the name of a lighting rig, made unique among the saved lightings.
/// The date is today's, in UTC (the app has no time-zone database), as `YYYY-MM-DD`.
fn setup_suggest_lighting_name_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<ViewportModel>()
        .on_suggest_lighting_name(move || {
            let rig = RenderContext::lock(&render_ctx)
                .lighting_preset
                .label()
                .to_string();
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
            let (year, month, day, ..) = civil_from_unix_seconds(seconds);
            let date = format!("{year:04}-{month:02}-{day:02}");
            let taken: Vec<String> = settings_store
                .snapshot()
                .presets
                .iter()
                .map(|preset| preset.name.clone())
                .collect();
            unique_name(SavedLightingPreset::default_save_name(&rig, &date), &taken).into()
        });
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
                surface_glare: fields.surface_glare,
                backdrop: fields.backdrop,
                head_shadow_deg: fields.head_shadow_deg,
            };

            let mut result = Ok(());
            settings_store_save.update(|s| result = s.upsert_user_preset(preset.clone()));

            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_save.snapshot().presets);
                    show_toast(&ui, &format!("Saved lighting '{name}'"), "success");
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
/// that it is deliberately scoped to ONLY this button: the lighting-only save above keeps
/// writing `None`, so a user who liked the lighting-only behaviour keeps getting it from
/// the control they already know.
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
                surface_glare: fields.surface_glare,
                backdrop: fields.backdrop,
                head_shadow_deg: fields.head_shadow_deg,
            };

            let mut result = Ok(());
            settings_store_save.update(|s| result = s.upsert_user_preset(preset.clone()));

            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_save.snapshot().presets);
                    show_toast(
                        &ui,
                        &format!("Saved lighting '{name}' with the camera view"),
                        "success",
                    );
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// What applying a saved lighting does to the render context's HDR environment map.
///
/// A saved lighting is a complete look, so one WITHOUT a map clears any loaded map (the lighting
/// rig it names comes back), the same as choosing a rig; old presets without a map path meant
/// "no map" too. One WITH a map loads it. A map that could not be reloaded (`Err`: the file
/// moved or was deleted) leaves the current environment untouched.
fn apply_environment(
    ctx: &mut RenderContext,
    loaded: Option<&Result<Arc<EnvironmentMap>, String>>,
) {
    match loaded {
        Some(Ok(map)) => ctx.env_map = Some(Arc::clone(map)),
        None => ctx.env_map = None,
        Some(Err(_)) => {}
    }
}

/// Applies preset `idx` (looked up in the settings store's current preset list, the
/// same list the UI's `lighting_presets` model was built from): pushes its fields into
/// `RenderContext` -- setting `ctx.dirty = true` so the render restarts, exactly as
/// the existing light controls do -- mirrors them into the UI's own slider/dropdown
/// state, and persists them as the new "current" settings so the applied look survives
/// a restart even without the user touching a slider afterward.
///
/// The camera pose is restored only when the preset actually carries one
/// (`camera_yaw`/`camera_pitch` both `Some`); `None` leaves the current pose untouched. The HDR
/// environment map is part of the look: a preset without one clears any loaded map
/// ([`apply_environment`]). See `settings::model::LightingPreset`'s own doc comment.
fn setup_apply_lighting_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
) {
    let render_ctx_apply = render_ctx.clone();
    let settings_store_apply = settings_store.clone();
    let mesh_bounding_radius_apply = mesh_bounding_radius.clone();
    let ui_weak_apply = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_apply_lighting_preset(move |idx: i32| {
            let Some(preset) = usize::try_from(idx)
                .ok()
                .and_then(|i| settings_store_apply.snapshot().presets.get(i).cloned())
            else {
                return;
            };

            let lighting_preset =
                crate::gui::optics::offered_lighting::offered_from_label(&preset.lighting_rig);
            // The scene fields as the one shared struct, each limited the way the live
            // controls limit it (a hand-edited or old preset cannot feed the tracer nonsense).
            let values = preset.values().sanitised();

            // Loading an HDR file is I/O -- done here, BEFORE the lock below, matching
            // `gui::camera_lighting::setup_environment_map_callbacks`'s own "never assign
            // into `RenderContext::env_map` on `Err`" discipline: a preset pointing at a
            // file that has since moved or been deleted must leave whatever environment
            // (loaded map or studio rig) is currently active untouched, not clear it.
            let env_map_result = preset.env_map_path.as_deref().map(load_env_map);

            {
                let mut ctx = RenderContext::lock(&render_ctx_apply);
                apply_values_to_context(&mut ctx, &values, lighting_preset);
                // The same dynamic zoom clamp the live orbit camera uses, sized to the
                // CURRENT mesh's own bounding radius (the same live
                // `Arc<Mutex<f64>>` `gui::camera_lighting`'s own orbit/zoom handlers
                // read -- see `main_window::mesh_bounding_radius`'s doc comment)
                // rather than a fixed `[1.2, 8.0]` range or the
                // `DEFAULT_MESH_BOUNDING_RADIUS` placeholder, so a preset saved while
                // viewing a design larger/smaller than the default reapplies at a
                // distance that actually frames THIS design, not whatever design was
                // loaded when the preset was saved.
                let radius = *mesh_bounding_radius_apply
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (min_distance, max_distance) =
                    super::camera_lighting::orbit_distance_bounds(radius);
                if let Some(distance) = values.camera_distance {
                    ctx.distance = distance.clamp(min_distance, max_distance);
                }
                // Camera pose: only when the preset actually carries one -- see this
                // function's own doc comment. The same yaw/pitch clamps
                // `gui::camera_lighting::on_camera_orbit` applies to a live drag, so a
                // preset saved from an old build (before that clamp existed, however
                // unlikely) can't reapply an out-of-range pose.
                if let (Some(yaw), Some(pitch)) = (preset.camera_yaw, preset.camera_pitch) {
                    ctx.yaw = yaw;
                    ctx.pitch = pitch.clamp(-1.48, 1.48);
                }
                apply_environment(&mut ctx, env_map_result.as_ref());
                ctx.dirty = true;
            }

            settings_store_apply
                .update(|s| persist_preset_values(s, &values, &preset, env_map_result.as_ref()));
            // The settings now hold this preset's look, so it is the cutter's normal
            // lighting: a design's own saved lighting that was showing is let go (it
            // stays saved) instead of being restored over this choice later.
            super::design_lighting::normal_lighting_chosen();

            let Some(ui) = ui_weak_apply.upgrade() else {
                return;
            };
            push_lighting_values_to_controls(&ui, &values, lighting_preset);
            show_environment_result(&ui, &preset, env_map_result.as_ref());
            show_toast(&ui, &format!("Applied lighting '{}'", preset.name), "info");
        });
}

/// Puts the scene fields of `values` into the render context: the light, exposure, rig,
/// surface glare, backdrop and head shadow. The scene fields older presets lacked read their
/// defaults (serde), so applying any preset sets the whole look. The caller sets the camera, the
/// environment map and `dirty`.
pub(in crate::gui) const fn apply_values_to_context(
    ctx: &mut RenderContext,
    values: &LightingValues,
    rig: LightingPreset,
) {
    ctx.light_yaw = values.light_yaw_deg.to_radians();
    ctx.light_pitch = values
        .light_pitch_deg
        .to_radians()
        .clamp(PITCH_RANGE_RAD.0, PITCH_RANGE_RAD.1);
    ctx.exposure = values.exposure;
    ctx.lighting_preset = rig;
    ctx.surface_glare = values.surface_glare;
    ctx.backdrop = values.backdrop;
    ctx.head_shadow_deg = values.head_shadow_deg;
}

/// The controls half of showing `values`: the six lighting sliders / pills and the rig
/// drop-down. Setting a property from Rust does not fire the control's own "changed"
/// callback, so nothing is written to the settings file by this. Shared by applying a saved
/// lighting and by the Lighting tab's reset, so the two cannot drift apart.
pub(in crate::gui) fn push_lighting_values_to_controls(
    ui: &MainWindow,
    values: &LightingValues,
    rig: LightingPreset,
) {
    let settings = ui.global::<SettingsModel>();
    settings.set_light_yaw_deg(values.light_yaw_deg);
    settings.set_light_pitch_deg(values.light_pitch_deg);
    settings.set_exposure_val(values.exposure);
    settings.set_surface_glare_pct(percent_from_surface_glare(values.surface_glare));
    settings.set_backdrop_index(values.backdrop.index());
    settings.set_head_shadow_deg(values.head_shadow_deg);
    ui.global::<ViewportModel>()
        .set_selected_lighting_index(rig.index());
}

/// Writes an applied saved lighting into the settings file, so the applied look survives a
/// restart. The camera pose and distance only when the preset carries them; the environment
/// path follows `env_map_result` exactly as the render context does ([`apply_environment`]).
fn persist_preset_values(
    s: &mut SettingsFile,
    values: &LightingValues,
    preset: &SavedLightingPreset,
    env_map_result: Option<&Result<Arc<EnvironmentMap>, String>>,
) {
    s.settings.light_yaw_deg = values.light_yaw_deg;
    s.settings.light_pitch_deg = values.light_pitch_deg;
    s.settings.exposure = values.exposure;
    s.settings.lighting_rig.clone_from(&values.lighting_rig);
    if let Some(distance) = values.camera_distance {
        s.settings.camera_distance = distance;
    }
    s.settings.surface_glare = values.surface_glare;
    s.settings.backdrop = values.backdrop;
    s.settings.head_shadow_deg = values.head_shadow_deg;
    if let (Some(yaw), Some(pitch)) = (preset.camera_yaw, preset.camera_pitch) {
        s.settings.camera_yaw = yaw;
        s.settings.camera_pitch = pitch.clamp(-1.48, 1.48);
    }
    match (env_map_result, preset.env_map_path.as_ref()) {
        (Some(Ok(_)), Some(path)) => s.settings.env_map_path.clone_from(path),
        // A saved lighting without a map means "no map": the rig comes back.
        (None, _) => s.settings.env_map_path.clear(),
        // A map that could not be reloaded leaves the current environment alone.
        (Some(_), _) => {}
    }
}

/// Mirrors the environment-map outcome of applying `preset` into the HDR panel: the status text
/// and toggle (the same `env_map_status_text` helper `gui::camera_lighting::on_load_env_map`
/// uses, so a saved-lighting load reads exactly like a manual one), or an error toast when the
/// file could not be reloaded.
fn show_environment_result(
    ui: &MainWindow,
    preset: &SavedLightingPreset,
    env_map_result: Option<&Result<Arc<EnvironmentMap>, String>>,
) {
    let settings = ui.global::<SettingsModel>();
    match (env_map_result, preset.env_map_path.as_deref()) {
        (Some(Ok(map)), Some(path)) => {
            settings.set_env_map_status(env_map_status_text(map, path).into());
            settings.set_env_map_loaded(true);
        }
        (Some(Err(err)), _) => show_toast(
            ui,
            &format!("The lighting's HDR environment could not be reloaded: {err}"),
            "error",
        ),
        (None, _) => {
            // The preset carries no map, so a map loaded earlier is let go: a saved
            // lighting is a complete look, the same as choosing a rig.
            settings.set_env_map_status("".into());
            settings.set_env_map_loaded(false);
            // Nothing still on its way (a decode of a design's saved map) may land
            // over this choice.
            super::design_lighting::env_map_chosen();
        }
        // A loaded map always has the path that loaded it.
        (Some(Ok(_)), None) => {}
    }
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
                    show_toast(&ui, "Lighting renamed.", "success");
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
                    show_toast(&ui, &format!("Deleted lighting '{name}'"), "info");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// The Parameters panel's "usable for export" checkbox next to each preset row -- see
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

#[cfg(test)]
mod tests {
    use super::{apply_environment, unique_name};
    use crate::bridge::render_thread::RenderContext;
    use indicatrix::renderer::env_map::EnvironmentMap;
    use std::sync::Arc;

    fn loaded_map() -> Arc<EnvironmentMap> {
        Arc::new(EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]))
    }

    #[test]
    fn a_saved_lighting_without_a_map_clears_a_loaded_map() {
        let mut ctx = RenderContext {
            env_map: Some(loaded_map()),
            ..Default::default()
        };
        apply_environment(&mut ctx, None);
        assert!(ctx.env_map.is_none(), "the rig comes back");
    }

    #[test]
    fn a_saved_lighting_with_a_map_loads_it() {
        let mut ctx = RenderContext::default();
        let map = loaded_map();
        apply_environment(&mut ctx, Some(&Ok(Arc::clone(&map))));
        assert!(ctx.env_map.as_ref().is_some_and(|m| Arc::ptr_eq(m, &map)));
    }

    #[test]
    fn a_map_that_cannot_be_reloaded_leaves_the_environment_alone() {
        let current = loaded_map();
        let mut ctx = RenderContext {
            env_map: Some(Arc::clone(&current)),
            ..Default::default()
        };
        apply_environment(&mut ctx, Some(&Err("missing".to_string())));
        assert!(
            ctx.env_map
                .as_ref()
                .is_some_and(|m| Arc::ptr_eq(m, &current))
        );
        // And with no map loaded, nothing appears out of nowhere.
        let mut empty = RenderContext::default();
        apply_environment(&mut empty, Some(&Err("missing".to_string())));
        assert!(empty.env_map.is_none());
    }

    #[test]
    fn a_free_name_is_offered_unchanged() {
        assert_eq!(
            unique_name("Tent - 2026-10-12".into(), &[]),
            "Tent - 2026-10-12"
        );
    }

    #[test]
    fn a_taken_name_gets_the_next_free_number() {
        let taken = vec!["A".to_string(), "A (2)".to_string()];
        assert_eq!(unique_name("A".into(), &taken), "A (3)");
    }
}
