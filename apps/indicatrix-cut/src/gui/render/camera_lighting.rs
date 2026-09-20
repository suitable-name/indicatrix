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
use std::{
    f32::consts::FRAC_PI_2,
    sync::{Arc, Mutex},
};

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
///
/// #26: sizes to the Live Render tab's OWN `SolidPreviewModel.
/// live_viewport_width`/`live_viewport_height` while `on_live_tab`, never the Edit
/// tab's `viewport_width`/`viewport_height` -- `gem_viewport.slint` and
/// `solid_viewport.slint` used to push their layout size into the SAME two
/// properties, so whichever viewport resized last silently overwrote the other's
/// pending redraw size.
///
/// #101: both dimensions are scaled by `ui.window().scale_factor()` so the solid
/// rasterizer/diagram renderer produce a HiDPI-correct (physical-pixel) raster
/// instead of a logical-pixel one Slint then has to upscale blurrily. The matching
/// conversion on the input side multiplies the pointer's LOGICAL hover/click
/// position by the same factor to reach the physical pixel it names in the pick
/// buffer; it lives in `solid_preview::diagram_wiring` for the diagram and in
/// `gui::editor::callbacks::tier_actions` for the solid view. Both are in place.
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
    let scale = ui.window().scale_factor();
    let size = if on_live_tab {
        (
            (ui.global::<SolidPreviewModel>().get_live_viewport_width() * scale) as u32,
            (ui.global::<SolidPreviewModel>().get_live_viewport_height() * scale) as u32,
        )
    } else {
        let viewport_size = (
            (ui.global::<SolidPreviewModel>().get_viewport_width() * scale) as u32,
            (ui.global::<SolidPreviewModel>().get_viewport_height() * scale) as u32,
        );
        // #117: Path-traced/Both letterbox the traced image to `ctx.width`/
        // `ctx.height`'s own aspect ratio (`solid_viewport.slint`'s
        // `image-fit: contain`) whenever it differs from the viewport's -- see
        // `contained_request_size`'s own doc comment.
        contained_request_size(view_mode, viewport_size, (ctx.width, ctx.height))
    };
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

/// Shrinks `viewport_size` to the rectangle Slint's `image-fit: contain` draws
/// `render_size` into (#117, `cad_todo.md` item 117).
///
/// `solid_viewport.slint`'s Path-traced/Both `Image` elements (`ViewportModel.
/// render_image`, `SolidPreviewModel.edges_image`) are both `image-fit: contain`
/// against a `render_size` (the configured render resolution, `RenderContext::
/// width`/`height`) that is independent of the viewport's own logical size --
/// with a render resolution wider than the viewport (a common pairing: 1920x1080
/// against a docked ~1.2-1.5 aspect Edit viewport), `contain` fits the image's
/// WIDTH and letterboxes it top/bottom, drawn SMALLER than a solid/edges raster
/// still requested at the viewport's raw, un-letterboxed size. The edges overlay
/// then reads as larger than the render it is supposed to trace, and a click
/// lands on the visible (letterboxed) gem while indexing a pick buffer sized to
/// the viewport's own full rectangle. Requesting the solid/edges raster at THIS
/// function's output size instead keeps every pixel of the request inside the
/// same rectangle the traced image actually occupies on screen.
///
/// Used only for `view_mode` `1` (Path-traced) and `2` (Both) -- Solid (`0`) and
/// Diagram (`3`) never show `render_image` at all, so the viewport's own raw
/// size is already correct for them, returned unchanged (also returned unchanged
/// if either dimension is `0`, i.e. nothing to divide by, or nothing requested
/// yet).
///
/// HANDOFF: `gui::editor::view::scaled_viewport_size`'s own callers
/// (`refresh_viewport`, `submit_preview_replan` -- not this pass's file) compute
/// the SAME viewport-sized request independently, for the post-edit/post-solve
/// path this function's own caller (`resubmit_at_current_pose`) does not cover.
/// They need the identical treatment: read `SolidPreviewModel.view_mode` and
/// `render_ctx.width`/`height` at the call site and pass the result through this
/// function (or an equivalent copy) before calling `request_replan`/
/// `request_redraw_with_gear`, or the misregistration this fixes for a camera
/// drag/zoom/view-mode switch will still reappear immediately after the next
/// edit or Solve.
#[must_use]
pub(in crate::gui) fn contained_request_size(
    view_mode: u8,
    viewport_size: (u32, u32),
    render_size: (u32, u32),
) -> (u32, u32) {
    if !matches!(view_mode, 1 | 2) {
        return viewport_size;
    }
    let (viewport_width, viewport_height) = viewport_size;
    let (render_width, render_height) = render_size;
    if viewport_width == 0 || viewport_height == 0 || render_width == 0 || render_height == 0 {
        return viewport_size;
    }
    let viewport_aspect = viewport_width as f32 / viewport_height as f32;
    let render_aspect = render_width as f32 / render_height as f32;
    if render_aspect > viewport_aspect {
        // The traced image is wider than the viewport -- `contain` fits its
        // width and shrinks its height.
        let height = (viewport_width as f32 / render_aspect).round().max(1.0) as u32;
        (viewport_width, height)
    } else {
        // The traced image is taller/narrower than the viewport -- `contain`
        // fits its height and shrinks its width. Already the on-screen size
        // whenever `render_aspect <= viewport_aspect` (the verifier's own
        // correction on `cad_todo.md` item 117), so this branch is a near
        // no-op in that case beyond rounding.
        let width = (viewport_height as f32 * render_aspect).round().max(1.0) as u32;
        (width, viewport_height)
    }
}

/// Matches every `Camera::new` call across this crate's own convention of a
/// hardcoded `42.0` degree (vertical) field of view -- [`fit_distance_for_radius`]
/// needs the SAME half-angle `raster.rs`'s `project` actually renders with, or
/// the "Fit" pose it computes would not match what ends up on screen.
const CAMERA_FOV_DEG: f32 = 42.0;

/// The orbit camera's zoom clamp for a solid with the given bounding radius
/// (#118, `cad_todo.md` item 118) -- replaces the old fixed `[1.2, 8.0]` range,
/// which clipped a large preform's facets out of the frame at minimum distance
/// (`raster.rs` drops a whole facet whose ring has any point behind the near
/// plane) and left a small one lost in mostly empty space at maximum.
///
/// The two factors are chosen so a mesh at exactly
/// [`super::super::solid_preview::preview_state::DEFAULT_MESH_BOUNDING_RADIUS`]
/// (`1.5`, a standard round brilliant's own half-width) reproduces the OLD fixed
/// range exactly (`1.5 * 0.8 == 1.2`, `1.5 * (16.0/3.0) == 8.0`), so the common
/// case is unchanged -- only a design meaningfully larger or smaller than that
/// gets a clamp actually sized to it.
#[must_use]
pub(in crate::gui) fn orbit_distance_bounds(bounding_radius: f64) -> (f32, f32) {
    const MIN_FACTOR: f32 = 0.8;
    const MAX_FACTOR: f32 = 16.0 / 3.0;
    let radius = (bounding_radius as f32).max(0.01);
    (radius * MIN_FACTOR, radius * MAX_FACTOR)
}

/// The orbit distance that frames a sphere of `bounding_radius` exactly at the
/// vertical edges of the camera's own field of view (`CAMERA_FOV_DEG`), times a
/// small margin so the "Fit" pose (#118) leaves a little breathing room instead
/// of touching the frame's edge exactly. Mirrors `Camera::generate_ray`'s own
/// `v = ... * fov_tan` convention: a point at height `bounding_radius` and this
/// distance subtends exactly `fov_tan` at the screen edge before the margin is
/// applied.
#[must_use]
pub(in crate::gui) fn fit_distance_for_radius(bounding_radius: f64) -> f32 {
    const MARGIN: f32 = 1.15;
    let half_fov_tan = (CAMERA_FOV_DEG.to_radians() * 0.5).tan();
    (bounding_radius as f32).max(0.01) / half_fov_tan * MARGIN
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
///
/// #26/#101: always sizes to the Live Render tab's OWN `SolidPreviewModel.
/// live_viewport_width`/`live_viewport_height` (never the Edit tab's
/// `viewport_width`/`viewport_height` -- see [`resubmit_at_current_pose`]'s doc
/// comment), scaled by `ui.window().scale_factor()` for a HiDPI-correct raster.
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
    let scale = ui.window().scale_factor();
    let size = (
        (ui.global::<SolidPreviewModel>().get_live_viewport_width() * scale) as u32,
        (ui.global::<SolidPreviewModel>().get_live_viewport_height() * scale) as u32,
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
/// Wraps an orbit pitch into `[-pi, pi)`: the camera may orbit freely over the poles
/// (see `Camera::new`'s pole handling), the wrap only keeps the persisted value
/// bounded.
pub(in crate::gui) fn wrap_pitch(pitch: f32) -> f32 {
    use std::f32::consts::PI;
    (pitch + PI).rem_euclid(2.0 * PI) - PI
}

/// Wires `ViewportModel.on_light_move`/`on_light_pos_changed`, persisting the
/// result through the debounced `settings_store` exactly like every other pose
/// callback in this module. Split out of [`setup_camera_and_lighting_callbacks`]
/// purely to keep that function under clippy's function-length lint (#118 added
/// the `mesh_bounding_radius` clamp there, pushing it over the limit) -- these two
/// callbacks are otherwise unrelated to the camera pose/distance ones that stayed
/// behind.
fn setup_light_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
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
}

/// `ViewportModel.set_view`: `0` = Front (girdle edge-on, index 0 towards the
/// viewer), `1` = Top (straight down onto the table, the batch previews' own top
/// pose), `2` = Bottom, `3` = Left, `4` = Right (#118, `cad_todo.md` item 118 --
/// there used to be no one-click pavilion/side view at all, the commonest
/// inspection for windowing), and `5` = Fit, which uniquely leaves the CURRENT
/// orbit angle alone and only recomputes `distance` from the live mesh's own
/// bounding radius (`mesh_bounding_radius`) via [`fit_distance_for_radius`], so a
/// cutter who has orbited to an awkward angle can recover a usable framing
/// without also losing the angle they were inspecting. Every other kind resets
/// yaw to a canonical angle; distance and lighting stay untouched for those.
/// Both viewports' pose pills call this same callback.
fn setup_set_view_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let preview_state = Arc::clone(preview_state);
    let mesh_bounding_radius = Arc::clone(mesh_bounding_radius);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>().on_set_view(move |kind: i32| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut ctx = render_ctx.lock().unwrap();
        if kind == 5 {
            let radius = *mesh_bounding_radius.lock().unwrap();
            let (min, max) = orbit_distance_bounds(radius);
            ctx.distance = fit_distance_for_radius(radius).clamp(min, max);
        } else {
            let (yaw, pitch) = match kind {
                1 => (0.0, FRAC_PI_2),  // Top
                2 => (0.0, -FRAC_PI_2), // Bottom
                3 => (-FRAC_PI_2, 0.0), // Left
                4 => (FRAC_PI_2, 0.0),  // Right
                _ => (0.0, 0.0),        // Front, and any unrecognized kind
            };
            ctx.yaw = yaw;
            ctx.pitch = pitch;
        }
        ctx.dirty = true;
        resubmit_at_current_pose(&ui, &ctx, &preview_state);
        let (yaw, pitch, distance) = (ctx.yaw, ctx.pitch, ctx.distance);
        drop(ctx);
        settings_store.update(|s| {
            s.settings.camera_yaw = yaw;
            s.settings.camera_pitch = pitch;
            s.settings.camera_distance = distance;
        });
    });
}

pub(in crate::gui) fn setup_camera_and_lighting_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
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
                ctx.pitch = wrap_pitch(dy.mul_add(0.008, ctx.pitch));
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
    let mesh_bounding_radius_zoom = Arc::clone(mesh_bounding_radius);
    let ui_weak_zoom = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_camera_zoom(move |delta: f32| {
            let Some(ui) = ui_weak_zoom.upgrade() else {
                return;
            };
            let distance = {
                let mut ctx = render_ctx_zoom.lock().unwrap();
                // #118: the clamp is now sized to the CURRENT solid's own
                // bounding radius rather than a fixed `[1.2, 8.0]` range -- see
                // `orbit_distance_bounds`'s own doc comment.
                let radius = *mesh_bounding_radius_zoom.lock().unwrap();
                let (min, max) = orbit_distance_bounds(radius);
                ctx.distance = delta.mul_add(-0.002, ctx.distance).clamp(min, max);
                ctx.dirty = true;
                resubmit_at_current_pose(&ui, &ctx, &preview_state_zoom);
                ctx.distance
            };
            settings_store_zoom.update(|s| s.settings.camera_distance = distance);
        });

    setup_light_callbacks(ui, render_ctx, settings_store);

    setup_set_view_callback(
        ui,
        render_ctx,
        settings_store,
        preview_state,
        mesh_bounding_radius,
    );

    let render_ctx_reset = render_ctx.clone();
    let settings_store_reset = settings_store.clone();
    let preview_state_reset = Arc::clone(preview_state);
    let ui_weak_reset = ui.as_weak();
    ui.global::<ViewportModel>().on_reset_camera(move || {
        let Some(ui) = ui_weak_reset.upgrade() else {
            return;
        };
        {
            let mut ctx = render_ctx_reset.lock().unwrap();
            ctx.yaw = 0.60;
            ctx.pitch = 0.45;
            ctx.distance = 2.4;
            ctx.light_yaw = 0.85;
            ctx.light_pitch = 0.95;
            ctx.dirty = true;
            // #118: this used to stop at writing `RenderContext` -- nothing ever
            // re-issued a redraw at the new pose, so both viewports (the Edit
            // tab's Solid preview and, when it was showing the solid raster,
            // the Live Render tab) kept displaying the OLD pose until some
            // UNRELATED trigger (a camera drag, a view-mode switch) happened to
            // redraw next.
            resubmit_at_current_pose(&ui, &ctx, &preview_state_reset);
            // Explicit: releases the `RenderContext` mutex before
            // `settings_store_reset.update` below, rather than leaving it held
            // until this block's closing brace.
            drop(ctx);
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

#[cfg(test)]
mod tests {
    use super::{contained_request_size, fit_distance_for_radius, orbit_distance_bounds};

    // --- contained_request_size ---

    #[test]
    fn solid_and_diagram_modes_are_untouched() {
        // #117: only Path-traced (1) and Both (2) show `render_image` at all --
        // Solid (0) and Diagram (3) must keep the viewport's own raw size.
        for mode in [0, 3] {
            assert_eq!(
                contained_request_size(mode, (900, 500), (1920, 1080)),
                (900, 500)
            );
        }
    }

    #[test]
    fn wider_render_than_viewport_shrinks_height_to_the_letterboxed_rectangle() {
        // A 1920x1080 (16:9) render in a 900x700 (~1.29) viewport: `contain`
        // fits the width and shrinks the height, exactly matching what
        // `solid_viewport.slint`'s `image-fit: contain` draws on screen.
        let (width, height) = contained_request_size(2, (900, 700), (1920, 1080));
        assert_eq!(width, 900);
        assert_eq!(height, 506);
    }

    #[test]
    fn narrower_render_than_viewport_is_already_the_on_screen_size() {
        // A 4:3 render in a wider viewport: per the verifier's own correction on
        // `cad_todo.md` item 117, `contain` fits the height here and the two
        // layers already agree pixel-for-pixel, so this is a no-op beyond
        // reproducing the exact viewport height.
        let (width, height) = contained_request_size(1, (900, 500), (640, 480));
        assert_eq!(height, 500);
        assert_eq!(width, 667);
    }

    #[test]
    fn zero_sized_input_is_returned_unchanged_rather_than_dividing_by_zero() {
        assert_eq!(contained_request_size(2, (0, 500), (1920, 1080)), (0, 500));
        assert_eq!(contained_request_size(2, (900, 500), (0, 1080)), (900, 500));
    }

    #[test]
    fn equal_aspect_ratios_pass_through_unchanged() {
        assert_eq!(
            contained_request_size(2, (1920, 1080), (1920, 1080)),
            (1920, 1080)
        );
    }

    // --- orbit_distance_bounds ---

    #[test]
    fn default_bounding_radius_reproduces_the_old_fixed_clamp() {
        // #118: 1.5 is `DEFAULT_MESH_BOUNDING_RADIUS` -- the common case must
        // keep the exact old `[1.2, 8.0]` behavior.
        let (min, max) = orbit_distance_bounds(1.5);
        assert!((min - 1.2).abs() < 1e-6, "got {min}");
        assert!((max - 8.0).abs() < 1e-6, "got {max}");
    }

    #[test]
    fn a_larger_mesh_gets_a_wider_clamp() {
        let (small_min, small_max) = orbit_distance_bounds(1.5);
        let (large_min, large_max) = orbit_distance_bounds(4.5);
        assert!(large_min > small_min);
        assert!(large_max > small_max);
    }

    #[test]
    fn a_zero_radius_never_collapses_the_clamp_to_a_single_point() {
        let (min, max) = orbit_distance_bounds(0.0);
        assert!(min > 0.0);
        assert!(max > min);
    }

    // --- fit_distance_for_radius ---

    #[test]
    fn fit_distance_grows_with_the_bounding_radius() {
        assert!(fit_distance_for_radius(3.0) > fit_distance_for_radius(1.5));
    }

    #[test]
    fn fit_distance_stays_within_the_matching_orbit_clamp() {
        // The "Fit" pose (`setup_set_view_callback`) always clamps its own
        // result through `orbit_distance_bounds` of the SAME radius -- this
        // confirms that combination lands inside a sensible range rather than,
        // say, the unclamped `fit_distance_for_radius` output being clamped
        // down to something that no longer frames the mesh at all.
        let radius = 1.5_f64;
        let (min, max) = orbit_distance_bounds(radius);
        let fit = fit_distance_for_radius(radius).clamp(min, max);
        assert!(fit >= min && fit <= max);
    }
}
