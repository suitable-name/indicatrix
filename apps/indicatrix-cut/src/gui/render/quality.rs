//! Render-quality (lighting preset, target samples, resolution, inclusion scattering,
//! pause, bounce count, exposure, local render path, surface glare, head shadow,
//! backdrop) callback wiring.
//!
//! Split mechanically out of the former `material_quality.rs` (the material half lives in
//! [`super::material`]) so that the stone/material lane and the render-quality lane do not
//! collide on one file. No behaviour change.

use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        local_compute_target_from_index, local_preview_scale_from_index,
        render::sample_scale::exponent_to_count, show_toast, tutorial_events::raise,
    },
    settings::{
        SettingsPersister,
        model::{head_shadow_deg_from_slider, surface_glare_from_percent},
    },
};
use indicatrix_editor::guide::viewing_events as events;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

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
            let preset = crate::gui::optics::offered_lighting::offered_from_index(idx);
            let mut ctx = RenderContext::lock(&render_ctx_lit);
            ctx.lighting_preset = preset;
            ctx.dirty = true;
            drop(ctx);
            settings_store_lit.update(|s| s.settings.lighting_rig = preset.label().to_string());
            if let Some(ui) = ui_weak_lit.upgrade() {
                show_toast(&ui, &format!("Lighting rig: {}", preset.label()), "info");
                // A tutorial step may wait for a lighting preset to be chosen.
                raise(&ui, events::LIGHTING_PRESET_CHOSEN);
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
            let mut ctx = RenderContext::lock(&render_ctx_samples);
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
            let (width, height) = (width.cast_unsigned(), height.cast_unsigned());
            let mut ctx = RenderContext::lock(&render_ctx_res);
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

    // (Inclusion haze is a per-design stone effect now: `render::material`.)

    // Render Pause / Resume -- explicit user intent.
    // Independent of the tab-visibility auto-suspend: this is the one the button reflects.
    let render_ctx_pause = render_ctx.clone();
    ui.global::<ViewportModel>()
        .on_pause_toggled(move |paused: bool| {
            let mut ctx = RenderContext::lock(&render_ctx_pause);
            ctx.paused = paused;
        });

    // Automatic render suspend when the rendered image isn't visible anywhere is driven
    // by `RenderContext::tab_visible`, recomputed by `gui::render::render_visibility::
    // recompute_tab_visible` from all the signals `render_is_visible` combines (outer
    // tab, inner sub-tab, each sub-tab's own Solid/Path-traced view-mode toggle, and
    // the Retarget dialog) -- see that function's own doc comment for the full list of
    // call sites. That path must never touch `ctx.paused`: a manual pause must survive
    // switching tabs (or flipping a view mode) away and back.

    let render_ctx_bnc = render_ctx.clone();
    let settings_store_bnc = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_bounces_changed(move |bounces: i32| {
            let clamped = bounces.cast_unsigned().max(1);
            let mut ctx = RenderContext::lock(&render_ctx_bnc);
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
            let mut ctx = RenderContext::lock(&render_ctx_exp);
            ctx.exposure = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_exp.update(|s| s.settings.exposure = clamped);
        });

    // The Parameters panel's "Some advanced settings are in use" note (Simple mode).
    super::advanced_in_use::setup_advanced_in_use_callback(ui);
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
            RenderContext::lock(&render_ctx_preview).local_preview_scale = scale;
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
            RenderContext::lock(&render_ctx_local_compute).local_compute_target = target;
            settings_store_local_compute.update(|s| s.settings.local_compute_target = target);
        });
}

/// Wires the surface-glare slider: the scale of the white mirror image of the light in
/// the live view and every export (`RenderContext::surface_glare`, persisted as
/// `AppSettings::surface_glare`). The slider reports a percent; a drag that stays on the
/// same 5 % step changes nothing and so never restarts accumulation.
pub(in crate::gui) fn setup_surface_glare_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_surface_glare_changed(move |percent: f32| {
            let glare = surface_glare_from_percent(percent);
            let mut ctx = RenderContext::lock(&render_ctx);
            if ctx.surface_glare.to_bits() == glare.to_bits() {
                return;
            }
            ctx.surface_glare = glare;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.surface_glare = glare);
        });
}

/// Wires the head-shadow slider: the angular radius of the viewer's head shadow on the
/// lit lighting presets, in the live view and every export
/// (`RenderContext::head_shadow_deg`, persisted as `AppSettings::head_shadow_deg`). The
/// slider reports whole degrees; a drag that stays on the same degree changes nothing and
/// so never restarts accumulation.
pub(in crate::gui) fn setup_head_shadow_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_head_shadow_changed(move |degrees: f32| {
            let deg = head_shadow_deg_from_slider(degrees);
            let mut ctx = RenderContext::lock(&render_ctx);
            if ctx.head_shadow_deg.to_bits() == deg.to_bits() {
                return;
            }
            ctx.head_shadow_deg = deg;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.head_shadow_deg = deg);
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
            let mut ctx = RenderContext::lock(&render_ctx);
            ctx.backdrop = backdrop;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.backdrop = backdrop);
        });
}
