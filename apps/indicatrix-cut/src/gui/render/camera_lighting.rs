//! Camera orbit/zoom, light move/position/reset, and HDR environment-map load/clear
//! callback wiring.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

use crate::{
    MainWindow, SettingsModel, SolidPreviewModel, ViewportModel,
    bridge::render_thread::{RenderContext, load_env_map},
    gui::{
        env_map_status_text, show_toast,
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
    },
    settings::SettingsPersister,
};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

/// Re-issues `ctx`'s current `active_planes` at its (just-updated) camera pose to
/// `preview_state`, sized to the Solid viewport's own logical size -- see
/// `docs/plans/solid_preview_B2_WIRING.md` part (d): camera-follow only (same
/// planes, new pose), never a re-solve. A no-op while nothing has been solved
/// into the shared viewport yet (`active_planes` empty), matching
/// `gui::editor::view::refresh_viewport`'s own "no design, no planes" contract.
///
/// Picks which viewport's own view-mode to redraw at, and whether to bother at all:
/// - Live Render tab (`ui.get_render_view_tab() == 0`): always Solid (`view_mode` 0,
///   forced -- see [`resubmit_live_solid`]'s own doc comment for why this must never
///   leak the Edit tab's `SolidPreviewModel.view_mode`), and ONLY when that tab's own
///   `ViewportModel.live_view_mode` is actually `0` (Solid) -- a camera drag while the
///   Live tab is showing the path-traced image has no solid on screen to redraw, so
///   this is a no-op rather than wasted background work.
/// - Edit tab (`render_view_tab != 0`): the Edit tab's own `SolidPreviewModel.
///   view_mode`, unconditionally -- unchanged from before `live_view_mode` existed
///   (the Solid viewport's camera-follow always ran regardless of which of its four
///   modes was showing, so it was already up to date the moment the user switched to
///   it).
///
/// Also forwards `ctx.design_gear` (the currently loaded design's gear tooth
/// count/reference angle) so a Diagram-mode (`view_mode` 3) camera drag shows THIS
/// design's gear wheel -- via [`SolidPreviewState::request_redraw_with_gear`],
/// not the plain `request_redraw` -- rather than whatever gear info a previous
/// editor replan happened to leave in the worker's own `DiagramMemory`, which is
/// unrelated once the currently viewed design was loaded some other way (e.g. from
/// the library, never replanned through the editor). See
/// `bridge::render_thread::RenderContext::design_gear`'s doc comment for the full
/// list of writers that keep it in sync with `active_planes`.
pub(in crate::gui) fn resubmit_at_current_pose(
    ui: &MainWindow,
    ctx: &RenderContext,
    preview_state: &SolidPreviewState,
) {
    if ctx.active_planes.is_empty() {
        return;
    }
    let on_live_tab = ui.get_render_view_tab() == 0;
    if on_live_tab && ui.global::<ViewportModel>().get_live_view_mode() != 0 {
        return;
    }
    let view_mode = if on_live_tab {
        0
    } else {
        ui.global::<SolidPreviewModel>().get_view_mode() as u8
    };
    let planes = ctx
        .active_planes
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let size = (
        ui.global::<SolidPreviewModel>().get_viewport_width() as u32,
        ui.global::<SolidPreviewModel>().get_viewport_height() as u32,
    );
    preview_state.request_redraw_with_gear(
        planes,
        CameraPose {
            yaw: ctx.yaw,
            pitch: ctx.pitch,
            distance: ctx.distance,
        },
        size,
        view_mode,
        ctx.design_gear,
    );
}

/// The Live Render tab's own solid-raster redraw: identical to
/// [`resubmit_at_current_pose`]'s Live-tab branch, except it never checks
/// `ViewportModel.live_view_mode` first -- every call site here already knows the
/// solid image is (about to become) visible before calling this (a library
/// selection while already in Solid mode, or `live_view_mode` itself just flipping to
/// `0`), so re-checking would be redundant. ALWAYS forces `view_mode` `0` (Solid
/// raster): the Live Render tab shows only the flat-shaded raster, never the Edit
/// tab's own Path-traced/Both/Diagram choice (`SolidPreviewModel.view_mode`), which
/// must not leak across tabs -- two independent viewports sharing one worker/one
/// `SolidPreviewState`, each entitled to its own mode.
///
/// Called from: `gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`/
/// `gui::library::detail::apply_design_record_to_ui` (a library selection lands while
/// the Live tab is already in Solid mode -- the planes `apply_reconstructed_planes`
/// just wrote need an immediate redraw, not a wait for the next camera drag), and
/// `gui::mod`'s `on_live_view_mode_changed` handler (switching INTO Solid mode itself
/// must show something immediately, not wait for a drag).
pub(in crate::gui) fn resubmit_live_solid(
    ui: &MainWindow,
    ctx: &RenderContext,
    preview_state: &SolidPreviewState,
) {
    if ctx.active_planes.is_empty() {
        return;
    }
    let planes = ctx
        .active_planes
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let size = (
        ui.global::<SolidPreviewModel>().get_viewport_width() as u32,
        ui.global::<SolidPreviewModel>().get_viewport_height() as u32,
    );
    preview_state.request_redraw_with_gear(
        planes,
        CameraPose {
            yaw: ctx.yaw,
            pitch: ctx.pitch,
            distance: ctx.distance,
        },
        size,
        0,
        ctx.design_gear,
    );
}

/// Wires up camera orbit/zoom, light move/position, and reset-camera callbacks. Each
/// also feeds the debounced `settings_store` so camera pose and light position survive
/// a restart. Split out of `run_gui` purely to keep that function under
/// clippy's function-length lint.
///
/// `preview_state` is B2's solid-inspection preview controller -- dragging or
/// zooming the Live Render viewport also moves the Solid viewport's shared
/// camera (yaw/pitch/distance), so both `on_camera_orbit`/`on_camera_zoom` below
/// re-issue the last solved plane set at the new pose (see
/// `resubmit_at_current_pose`) while still holding `render_ctx`'s lock.
pub(in crate::gui) fn setup_camera_and_lighting_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx_orbit = render_ctx.clone();
    let settings_store_orbit = settings_store.clone();
    let preview_state_orbit = Arc::clone(preview_state);
    let ui_weak_orbit = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_camera_orbit(move |dx: f32, dy: f32| {
            let Some(ui) = ui_weak_orbit.upgrade() else {
                return;
            };
            let (yaw, pitch) = {
                let mut ctx = render_ctx_orbit.lock().unwrap();
                // Horizontal drag is INVERTED relative to vertical: dragging right turns
                // the stone's right side toward the viewer, as though the hand were on the
                // gem rather than on the camera. Vertical keeps its sign, which already
                // reads correctly -- the two axes genuinely want opposite conventions here,
                // so the asymmetry is deliberate, not a stray sign.
                ctx.yaw = dx.mul_add(-0.008, ctx.yaw);
                ctx.pitch = dy.mul_add(0.008, ctx.pitch).clamp(-1.48, 1.48);
                ctx.dirty = true;
                resubmit_at_current_pose(&ui, &ctx, &preview_state_orbit);
                (ctx.yaw, ctx.pitch)
            };
            settings_store_orbit.update(|s| {
                s.settings.camera_yaw = yaw;
                s.settings.camera_pitch = pitch;
            });
        });

    let render_ctx_zoom = render_ctx.clone();
    let settings_store_zoom = settings_store.clone();
    let preview_state_zoom = Arc::clone(preview_state);
    let ui_weak_zoom = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_camera_zoom(move |delta: f32| {
            let Some(ui) = ui_weak_zoom.upgrade() else {
                return;
            };
            let distance = {
                let mut ctx = render_ctx_zoom.lock().unwrap();
                ctx.distance = delta.mul_add(-0.002, ctx.distance).clamp(1.2, 8.0);
                ctx.dirty = true;
                resubmit_at_current_pose(&ui, &ctx, &preview_state_zoom);
                ctx.distance
            };
            settings_store_zoom.update(|s| s.settings.camera_distance = distance);
        });

    let render_ctx_light = render_ctx.clone();
    let settings_store_light = settings_store.clone();
    ui.global::<ViewportModel>()
        .on_light_move(move |dx: f32, dy: f32| {
            let (light_yaw, light_pitch) = {
                let mut ctx = render_ctx_light.lock().unwrap();
                ctx.light_yaw = dx.mul_add(0.01, ctx.light_yaw);
                ctx.light_pitch = dy.mul_add(0.01, ctx.light_pitch).clamp(0.15, 1.55);
                ctx.dirty = true;
                (ctx.light_yaw, ctx.light_pitch)
            };
            settings_store_light.update(|s| {
                s.settings.light_yaw_deg = light_yaw.to_degrees();
                s.settings.light_pitch_deg = light_pitch.to_degrees();
            });
        });

    let render_ctx_light_pos = render_ctx.clone();
    let settings_store_light_pos = settings_store.clone();
    ui.global::<ViewportModel>()
        .on_light_pos_changed(move |yaw_deg: f32, pitch_deg: f32| {
            {
                let mut ctx = render_ctx_light_pos.lock().unwrap();
                ctx.light_yaw = yaw_deg.to_radians();
                ctx.light_pitch = pitch_deg.to_radians().clamp(0.15, 1.55);
                ctx.dirty = true;
            }
            settings_store_light_pos.update(|s| {
                s.settings.light_yaw_deg = yaw_deg;
                s.settings.light_pitch_deg = pitch_deg;
            });
        });

    let render_ctx_reset = render_ctx.clone();
    let settings_store_reset = settings_store.clone();
    ui.global::<ViewportModel>().on_reset_camera(move || {
        {
            let mut ctx = render_ctx_reset.lock().unwrap();
            ctx.yaw = 0.60;
            ctx.pitch = 0.45;
            ctx.distance = 2.4;
            ctx.light_yaw = 0.85;
            ctx.light_pitch = 0.95;
            ctx.dirty = true;
        }
        settings_store_reset.update(|s| {
            s.settings.camera_yaw = 0.60;
            s.settings.camera_pitch = 0.45;
            s.settings.camera_distance = 2.4;
            s.settings.light_yaw_deg = 0.85_f32.to_degrees();
            s.settings.light_pitch_deg = 0.95_f32.to_degrees();
        });
    });
}

/// Wires up the HDR environment-map load/clear callbacks. Path is a plain typed text
/// field (`settings_dialog.slint`'s "Environment Map (HDR)" section) with no native
/// picker of its own -- unlike `gui::library::setup_import_callback`'s `.asc`
/// pickers, which use `rfd::FileDialog` (see that module's doc comment). Split out of
/// `run_gui` purely to keep that function under clippy's function-length lint.
pub(in crate::gui) fn setup_environment_map_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_load = render_ctx.clone();
    let settings_store_load = settings_store.clone();
    let ui_weak_load = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_load_env_map(move |raw_path: SharedString| {
            let Some(ui) = ui_weak_load.upgrade() else {
                return;
            };
            let path = raw_path.to_string();
            // `load_env_map` never panics -- a missing, unreadable, or malformed file
            // returns `Err` and `RenderContext::env_map`/the settings file are left exactly
            // as they were, per this task's own requirement (see that function's doc
            // comment).
            match load_env_map(&path) {
                Ok(map) => {
                    let status = env_map_status_text(&map, &path);
                    {
                        let mut ctx = render_ctx_load.lock().unwrap();
                        ctx.env_map = Some(map);
                        ctx.dirty = true;
                    }
                    settings_store_load.update(|s| s.settings.env_map_path.clone_from(&path));
                    ui.global::<SettingsModel>()
                        .set_env_map_status(status.clone().into());
                    ui.global::<SettingsModel>().set_env_map_loaded(true);
                    // Loading an environment map forces the switch to the (slower) CPU
                    // tracer -- see `indicatrix::renderer::gpu_backend`'s module doc comment --
                    // which must not leave the user wondering why rendering got slower with
                    // no visible cause.
                    show_toast(
                        &ui,
                        &format!("{status}. Rendering on CPU: the GPU backend has no HDR support."),
                        "info",
                    );
                }
                Err(err) => {
                    show_toast(
                        &ui,
                        &format!("Could not load HDR environment: {err}"),
                        "error",
                    );
                }
            }
        });

    let render_ctx_clear = render_ctx.clone();
    let settings_store_clear = settings_store.clone();
    let ui_weak_clear = ui.as_weak();
    ui.global::<SettingsModel>().on_clear_env_map(move || {
        let Some(ui) = ui_weak_clear.upgrade() else {
            return;
        };
        {
            let mut ctx = render_ctx_clear.lock().unwrap();
            ctx.env_map = None;
            ctx.dirty = true;
        }
        settings_store_clear.update(|s| s.settings.env_map_path.clear());
        ui.global::<SettingsModel>()
            .set_env_map_status(String::new().into());
        ui.global::<SettingsModel>().set_env_map_loaded(false);
        show_toast(
            &ui,
            "Cleared HDR environment; back to the studio rig.",
            "info",
        );
    });

    // Native file-open picker for `env_map_path` ("Browse..." in
    // `settings_dialog.slint`) -- fills the field, doesn't load anything itself;
    // `on_load_env_map` above still does that, unchanged, whichever way the path got
    // typed in. Filtered to exactly `.hdr`: the only format `EnvironmentMap::from_hdr_file`
    // (crates/indicatrix/src/renderer/env_map.rs) decodes, via `image::ImageFormat::Hdr`.
    // Returns the SAME text it was given when the user cancels, so the Slint-side
    // assignment (`root.env_map_path = root.pick_hdr_file(root.env_map_path)`) is a
    // no-op on cancel.
    ui.global::<SettingsModel>()
        .on_pick_hdr_file(|current: SharedString| {
            let mut dialog = rfd::FileDialog::new().add_filter("Radiance HDR", &["hdr"]);
            if let Some(dir) = crate::gui::starting_dir_from_picker_field(current.as_str()) {
                dialog = dialog.set_directory(dir);
            }
            // Blocking `rfd::FileDialog`, invoked directly on the Slint UI/event-loop
            // thread -- see `apps/indicatrix-cut/Cargo.toml`'s `rfd` dependency comment for
            // why that's the supported way to call it here.
            dialog
                .pick_file()
                .map_or(current, |path| path.display().to_string().into())
        });
}
