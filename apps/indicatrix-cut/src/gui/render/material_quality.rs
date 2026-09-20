//! Material-selection, quality (lighting preset/target samples/resolution/inclusion
//! scattering/pause/bounce count/exposure), and material-effect-override (crystal-axis
//! orientation, frosted girdle, edge rounding, physical stone size) callback wiring.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

use crate::{
    LibraryModel, MainWindow, SettingsModel, ViewportModel,
    bridge::{
        preview_render::material_ri_at_sodium_d,
        render_thread::{RenderContext, resolve_material},
    },
    gui::{
        is_c_axis_override_available, local_compute_target_from_index,
        local_preview_scale_from_index,
        optics::c_axis::{angles_to_c_axis, c_axis_to_angles},
        render::sample_scale::exponent_to_count,
        show_toast,
    },
    settings::SettingsPersister,
};
use indicatrix::optics::{LightingPreset, materials::GemMaterial};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

/// Wires up the material-selection callback. Split out of
/// `setup_material_and_quality_callbacks` purely to keep that function under clippy's
/// function-length lint.
pub(in crate::gui) fn setup_material_changed_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_mat = render_ctx.clone();
    let settings_store_mat = settings_store.clone();
    let ui_weak_mat = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_material_changed(move |material: SharedString| {
            let mut ctx = render_ctx_mat.lock().unwrap();
            ctx.material_name = material.to_string();
            // Cleared, not left alone: `resolve_material_with_override` PREFERS
            // `material_override` over the name just written, and
            // `editor::view::refresh_design_settings` sets one on every refresh while
            // "Linked to design" is on. Without this, picking a material here moved the
            // label, the toast and the persisted setting, and left the trace untouched
            // -- the dropdown looked like it worked and did nothing at all.
            ctx.material_override = None;
            // Same reasoning one step further: a design that names no material and
            // matches no built-in within tolerance suspends tracing outright
            // (`SuspensionFlags`, via `editor::view::traced_material_for`'s refusal).
            // Naming one by hand is exactly the way out of that, and the refusal cannot
            // be re-imposed now that the link below is off.
            ctx.material_unresolved = None;
            ctx.dirty = true;
            // Resolved once and reused for both derived UI values below -- the
            // crystal-axis-availability check and the RI-tolerance filter's "defaults to
            // the currently loaded material" default (`MainWindow::ri_material_default`).
            // Deliberately folded into this same handler rather than a second
            // `on_material_changed` registration: Slint only keeps the last handler
            // registered for a given callback, so a second one would silently replace it.
            let resolved = resolve_material(
                &GemMaterial::all_materials(),
                &ctx.custom_materials,
                &material,
            );
            let available = is_c_axis_override_available(&resolved);
            let ri_default = material_ri_at_sodium_d(&resolved);
            drop(ctx);
            settings_store_mat.update(|s| s.settings.selected_material = material.to_string());
            if let Some(ui) = ui_weak_mat.upgrade() {
                // Picking a material by hand IS the independent choice "Linked to
                // design" exists to suppress (see `ViewportModel::
                // viewport_material_linked`'s own doc comment). Left on, the next
                // editor refresh would put the design's own material straight back --
                // and write a fresh `material_override` with it, making this dropdown
                // inert again. Turning it off is also what makes the pick durable:
                // `refresh_design_settings` touches none of the three fields above
                // while unlinked. The link's own handler already re-syncs on the way
                // back ON, so nothing is lost by switching it off here.
                let was_linked = ui.global::<ViewportModel>().get_viewport_material_linked();
                if was_linked {
                    ui.global::<ViewportModel>()
                        .set_viewport_material_linked(false);
                }
                ui.global::<SettingsModel>()
                    .set_c_axis_override_available(available);
                // `ri_default` is f64; Slint's `float` property type is f32 -- a lossy
                // but harmless cast for a display/filter-default value in the 1.0-3.0 RI
                // range.
                ui.global::<LibraryModel>()
                    .set_ri_material_default(ri_default as f32);
                let message = if was_linked {
                    format!("Material switched to {material} \u{2014} unlinked from the design.")
                } else {
                    format!("Material switched to {material}")
                };
                show_toast(&ui, &message, "info");
            }
        });
}

/// Wires up lighting-preset/target-samples/render-resolution/inclusion-scattering
/// changes, pause/tab-visibility, bounce count, and exposure callbacks. Each of the
/// persisted settings also feeds the debounced `settings_store`. Split out of
/// `run_gui` purely to keep that function under clippy's function-length lint.
pub(in crate::gui) fn setup_material_and_quality_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // The UI carries the lighting preset's enum discriminant as a plain `int`;
    // `LightingPreset::from_index` maps it back to the enum here, the one boundary
    // crossing from "UI index" to "physics enum", and `.label()` is what's actually
    // persisted/toasted as text.
    let render_ctx_lit = render_ctx.clone();
    let settings_store_lit = settings_store.clone();
    let ui_weak_lit = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_lighting_changed(move |idx: i32| {
            let preset = LightingPreset::from_index(idx);
            let mut ctx = render_ctx_lit.lock().unwrap();
            ctx.lighting_preset = preset;
            ctx.dirty = true;
            drop(ctx);
            settings_store_lit.update(|s| s.settings.lighting_rig = preset.label().to_string());
            if let Some(ui) = ui_weak_lit.upgrade() {
                show_toast(&ui, &format!("Lighting preset: {}", preset.label()), "info");
            }
        });

    // Target Samples slider. Carries the slider's exponent (3..=10);
    // `exponent_to_count` is the one boundary crossing from "slider position" to "the
    // actual sample count", which is what's stored/persisted/rendered.
    let render_ctx_samples = render_ctx.clone();
    let settings_store_samples = settings_store.clone();
    let ui_weak_samples = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_target_samples_changed(move |exponent: i32| {
            let target_samples = exponent_to_count(u32::try_from(exponent).unwrap_or(0));
            let mut ctx = render_ctx_samples.lock().unwrap();
            ctx.target_samples = target_samples;
            ctx.dirty = true;
            drop(ctx);
            settings_store_samples.update(|s| s.settings.target_samples = target_samples);
            if let Some(ui) = ui_weak_samples.upgrade() {
                show_toast(&ui, &format!("Target samples: {target_samples}"), "info");
            }
        });

    // Render Resolution pill selector: carries the resolved (width, height) pair
    // directly, unlike the samples slider above. Setting `ctx.width`/`.height` is all
    // that's needed to reset progressive accumulation cleanly: the render loop's
    // `update_accumulation_state` already reallocates the accumulation buffer, the
    // denoiser guide buffers, and `FramebufferTransfer` whenever it sees `width`/
    // `height` differ on the next frame -- `ctx.dirty = true` here is the same
    // belt-and-suspenders every other setting in this function sets, not load-bearing.
    let render_ctx_res = render_ctx.clone();
    let settings_store_res = settings_store.clone();
    let ui_weak_res = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_resolution_changed(move |width: i32, height: i32| {
            let (width, height) = (width as u32, height as u32);
            let mut ctx = render_ctx_res.lock().unwrap();
            ctx.width = width;
            ctx.height = height;
            ctx.dirty = true;
            drop(ctx);
            settings_store_res.update(|s| {
                s.settings.render_width = width;
                s.settings.render_height = height;
            });
            if let Some(ui) = ui_weak_res.upgrade() {
                show_toast(&ui, &format!("Render resolution: {width}x{height}"), "info");
            }
        });

    setup_local_render_path_callbacks(ui, render_ctx, settings_store);

    // Inclusion/subsurface scattering amount. Linear, unlike the samples slider
    // above -- `scattering_sigma_s`'s doc comment gives the 0.0-3.0 useful range
    // directly, so there's no perceptual remapping to invert here.
    let render_ctx_inc = render_ctx.clone();
    let settings_store_inc = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_inclusion_changed(move |sigma_s: f32| {
            let clamped = sigma_s.clamp(0.0, 3.0);
            let mut ctx = render_ctx_inc.lock().unwrap();
            ctx.inclusion_sigma_s = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_inc.update(|s| s.settings.inclusion_sigma_s = clamped);
        });

    // Render Pause / Resume -- explicit user intent.
    // Independent of the tab-visibility auto-suspend: this is the one the button reflects.
    let render_ctx_pause = render_ctx.clone();
    ui.global::<ViewportModel>()
        .on_pause_toggled(move |paused: bool| {
            let mut ctx = render_ctx_pause.lock().unwrap();
            ctx.paused = paused;
        });

    // Automatic render suspend when the rendered image isn't visible anywhere is driven
    // by `RenderContext::tab_visible`, recomputed by `gui::detached_render::
    // recompute_tab_visible` from all five signals `render_is_visible` combines (outer
    // tab, inner sub-tab, detached window, and each sub-tab's own Solid/Path-traced
    // view-mode toggle) -- see that function's own doc comment for the full list of
    // call sites. That path must never touch `ctx.paused`: a manual pause must survive
    // switching tabs (or docking/undocking, or flipping a view mode) away and back.

    let render_ctx_bnc = render_ctx.clone();
    let settings_store_bnc = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_bounces_changed(move |bounces: i32| {
            let clamped = (bounces as u32).max(1);
            let mut ctx = render_ctx_bnc.lock().unwrap();
            ctx.max_bounces = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_bnc.update(|s| s.settings.max_bounces = clamped);
        });

    let render_ctx_exp = render_ctx.clone();
    let settings_store_exp = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_exposure_changed(move |exposure: f32| {
            let clamped = exposure.clamp(0.2, 5.0);
            let mut ctx = render_ctx_exp.lock().unwrap();
            ctx.exposure = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_exp.update(|s| s.settings.exposure = clamped);
        });
}

/// Wires up the two local render-path controls: the preview-then-settle resolution
/// reduction and the CPU/GPU compute target. Split out of
/// `setup_material_and_quality_callbacks` purely to keep that function under clippy's
/// function-length lint.
///
/// What these two share, and what makes them the natural seam to cut on: neither sets
/// `ctx.dirty` and neither raises a toast, because neither changes what is already on
/// screen -- they only change how the next frame (or the next drag) is traced.
fn setup_local_render_path_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // Local preview-then-settle rendering: optional resolution reduction while the
    // camera is moving -- see `RenderContext::local_preview_scale`'s doc comment for
    // the mechanism. `Off` (index 0, the default) reproduces this crate's
    // pre-existing behaviour exactly. No `ctx.dirty`/toast: this alone never changes
    // what's on screen right now, only whether the next drag renders reduced.
    let render_ctx_preview = render_ctx.clone();
    let settings_store_preview = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_local_preview_scale_changed(move |index: i32| {
            let scale = local_preview_scale_from_index(index);
            render_ctx_preview.lock().unwrap().local_preview_scale = scale;
            settings_store_preview.update(|s| s.settings.local_preview_scale = scale);
        });

    // Local Compute: which engine(s) the local (non-remote) render loop uses -- see
    // `RenderContext::local_compute_target`'s doc comment. Live-updates `RenderContext`
    // (read fresh by the render loop every frame) in addition to persisting the
    // choice. No `ctx.dirty`/toast: this only changes how the next frame is traced,
    // never what's already accumulated -- switching engines mid-render continues the
    // same running average rather than restarting it.
    let render_ctx_local_compute = render_ctx.clone();
    let settings_store_local_compute = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_local_compute_target_changed(move |index: i32| {
            let target = local_compute_target_from_index(index);
            render_ctx_local_compute
                .lock()
                .unwrap()
                .local_compute_target = target;
            settings_store_local_compute.update(|s| s.settings.local_compute_target = target);
        });
}

/// Wires up the crystal-axis orientation override, the frosted-girdle toggle, the
/// edge-rounding slider, and the physical stone-size control. Split out of
/// `setup_material_and_quality_callbacks` purely to keep that function under clippy's
/// function-length lint.
pub(in crate::gui) fn setup_material_effect_override_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // Crystal-axis orientation override. One callback carries both angles -- the
    // moved slider's new value plus the other slider's current value, read directly
    // off `root.*` in `settings_dialog.slint` -- so a single-axis drag never has to
    // guess where the other axis currently sits. `angles_to_c_axis` is the one
    // boundary crossing from these degree sliders to the physical `Vec3`
    // `RenderContext::c_axis_override` stores.
    let render_ctx_axis_angles = render_ctx.clone();
    let settings_store_axis_angles = settings_store.clone();
    ui.global::<SettingsModel>().on_c_axis_angles_changed(
        move |tilt_deg: f32, azimuth_deg: f32| {
            let tilt_deg = tilt_deg.clamp(0.0, 90.0);
            let azimuth_deg = azimuth_deg.clamp(0.0, 360.0);
            let mut ctx = render_ctx_axis_angles.lock().unwrap();
            ctx.c_axis_override = Some(angles_to_c_axis(tilt_deg, azimuth_deg));
            ctx.dirty = true;
            drop(ctx);
            settings_store_axis_angles.update(|s| {
                s.settings.c_axis_tilt_deg = tilt_deg;
                s.settings.c_axis_azimuth_deg = azimuth_deg;
            });
        },
    );

    // Crystal-axis override on/off switch. Off ("as cut", the default) leaves the
    // resolved material's own `c_axis` untouched. Turning it on seeds the two angle
    // sliders from the currently selected material's own `c_axis` via
    // `c_axis_to_angles` (the inverse of `angles_to_c_axis` above), so enabling the
    // override never makes the stone visibly jump.
    let render_ctx_axis_toggle = render_ctx.clone();
    let settings_store_axis_toggle = settings_store.clone();
    let ui_weak_axis_toggle = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_c_axis_override_changed(move |enabled: bool| {
            let mut ctx = render_ctx_axis_toggle.lock().unwrap();
            if enabled {
                let base = resolve_material(
                    &GemMaterial::all_materials(),
                    &ctx.custom_materials,
                    &ctx.material_name,
                );
                let (tilt_deg, azimuth_deg) = c_axis_to_angles(base.c_axis);
                ctx.c_axis_override = Some(angles_to_c_axis(tilt_deg, azimuth_deg));
                ctx.dirty = true;
                drop(ctx);
                settings_store_axis_toggle.update(|s| {
                    s.settings.c_axis_override_enabled = true;
                    s.settings.c_axis_tilt_deg = tilt_deg;
                    s.settings.c_axis_azimuth_deg = azimuth_deg;
                });
                if let Some(ui) = ui_weak_axis_toggle.upgrade() {
                    ui.global::<SettingsModel>().set_c_axis_tilt_deg(tilt_deg);
                    ui.global::<SettingsModel>()
                        .set_c_axis_azimuth_deg(azimuth_deg);
                }
            } else {
                ctx.c_axis_override = None;
                ctx.dirty = true;
                drop(ctx);
                settings_store_axis_toggle.update(|s| s.settings.c_axis_override_enabled = false);
            }
        });

    // Bruted (frosted) girdle finish toggle -- a plain on/off switch, not a slider.
    // See `RenderContext::girdle_frosted`'s doc comment.
    let render_ctx_girdle = render_ctx.clone();
    let settings_store_girdle = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_girdle_frosted_changed(move |frosted: bool| {
            let mut ctx = render_ctx_girdle.lock().unwrap();
            ctx.girdle_frosted = frosted;
            ctx.dirty = true;
            drop(ctx);
            settings_store_girdle.update(|s| s.settings.girdle_frosted = frosted);
        });

    // Facet edge rounding radius, same opt-in-linear treatment as the inclusion
    // slider -- see `RenderContext::edge_rounding_radius`'s doc comment for the
    // `0.0`-`0.03` range's sourcing.
    let render_ctx_edge = render_ctx.clone();
    let settings_store_edge = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_edge_rounding_changed(move |radius: f32| {
            let clamped = radius.clamp(0.0, 0.03);
            let mut ctx = render_ctx_edge.lock().unwrap();
            ctx.edge_rounding_radius = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_edge.update(|s| s.settings.edge_rounding_radius = clamped);
        });

    // Physical stone size: girdle width in millimetres, off ("today's look",
    // unscaled) at 0.0. No upper clamp beyond staying non-negative -- this is a
    // free-typed measurement (a settings-dialog spin box) rather than a bounded
    // slider; `apply_material_overrides` guards the resulting scale against
    // non-finite/non-positive results regardless.
    let render_ctx_stone = render_ctx.clone();
    let settings_store_stone = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_stone_width_changed(move |width_mm: f32| {
            let clamped = width_mm.max(0.0);
            let mut ctx = render_ctx_stone.lock().unwrap();
            ctx.stone_width_mm = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_stone.update(|s| s.settings.stone_width_mm = clamped);
        });
}

/// Wires the backdrop pill row: what the camera sees behind the stone, in the live
/// view and every export (`RenderContext::backdrop`, persisted as
/// `AppSettings::backdrop`).
pub(in crate::gui) fn setup_backdrop_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_backdrop_changed(move |index: i32| {
            let backdrop = crate::settings::model::Backdrop::from_index(index);
            let mut ctx = render_ctx.lock().unwrap();
            ctx.backdrop = backdrop;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.backdrop = backdrop);
        });
}
