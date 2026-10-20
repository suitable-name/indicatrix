//! Camera handle registration and controls for runtime camera reframe and UI harness.

use crate::bridge::render_thread::RenderContext;
use crate::gui::render::camera_lighting::{
    orbit_distance_bounds, resubmit_at_current_pose, wrap_pitch,
};
use crate::gui::solid_preview::preview_state::SolidPreviewState;
use crate::{MainWindow, settings::SettingsPersister};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError, Weak},
};

pub struct CameraHandles {
    pub(crate) ui: slint::Weak<MainWindow>,
    pub(crate) render_ctx: Weak<Mutex<RenderContext>>,
    pub(crate) settings_store: Weak<SettingsPersister>,
    pub(crate) preview_state: Weak<SolidPreviewState>,
    pub(crate) mesh_bounding_radius: Weak<Mutex<f64>>,
}

thread_local! {
    /// The camera handles for the main window built last (UI thread only).
    static CAMERA: RefCell<Option<CameraHandles>> = const { RefCell::new(None) };
}

/// Remembers weak handles to the camera context and wiring components for camera reframing and harness accessors.
pub fn register_camera(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
) {
    CAMERA.with(|cell| {
        *cell.borrow_mut() = Some(CameraHandles {
            ui: ui.as_weak(),
            render_ctx: Arc::downgrade(render_ctx),
            settings_store: Arc::downgrade(settings_store),
            preview_state: Arc::downgrade(preview_state),
            mesh_bounding_radius: Arc::downgrade(mesh_bounding_radius),
        });
    });
}

/// Clears camera handles when a window is released.
pub fn release_camera() {
    CAMERA.with(|cell| cell.borrow_mut().take());
}

/// The current camera pose (yaw, pitch in radians, distance) of the active viewport, if registered.
#[cfg(feature = "ui-harness")]
#[must_use]
pub fn camera_pose() -> Option<(f32, f32, f32)> {
    CAMERA.with(|cell| {
        let guard = cell.borrow();
        let handles = guard.as_ref()?;
        let render_ctx = handles.render_ctx.upgrade()?;
        let ctx = RenderContext::lock(&render_ctx);
        Some((ctx.yaw, ctx.pitch, ctx.distance))
    })
}

/// Sets the camera pose (yaw, pitch in radians, optional distance). If `distance` is `None`,
/// the current distance is preserved (clamped to mesh bounding bounds).
pub fn set_camera_pose(yaw: f32, pitch: f32, distance: Option<f32>) -> bool {
    CAMERA.with(|cell| {
        let guard = cell.borrow();
        let Some(handles) = guard.as_ref() else {
            return false;
        };
        let Some(ui) = handles.ui.upgrade() else {
            return false;
        };
        let Some(render_ctx) = handles.render_ctx.upgrade() else {
            return false;
        };
        let Some(settings_store) = handles.settings_store.upgrade() else {
            return false;
        };
        let Some(preview_state) = handles.preview_state.upgrade() else {
            return false;
        };
        let Some(mesh_bounding_radius) = handles.mesh_bounding_radius.upgrade() else {
            return false;
        };

        let radius = *mesh_bounding_radius
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (min, max) = orbit_distance_bounds(radius);
        let pitch = wrap_pitch(pitch);

        let mut ctx = RenderContext::lock(&render_ctx);
        let target_distance = distance.unwrap_or(ctx.distance).clamp(min, max);
        ctx.yaw = yaw;
        ctx.pitch = pitch;
        ctx.distance = target_distance;
        ctx.dirty = true;
        resubmit_at_current_pose(&ui, &ctx, &preview_state);
        drop(ctx);

        settings_store.update(|s| {
            s.settings.camera_yaw = yaw;
            s.settings.camera_pitch = pitch;
            s.settings.camera_distance = target_distance;
        });
        true
    })
}

/// Returns what `set_view(5)` (Fit) would set for the current view and stone size.
#[cfg(feature = "ui-harness")]
#[must_use]
pub fn fit_distance_now() -> Option<f32> {
    CAMERA.with(|cell| {
        let guard = cell.borrow();
        let handles = guard.as_ref()?;
        let ui = handles.ui.upgrade()?;
        let render_ctx = handles.render_ctx.upgrade()?;
        let mesh_bounding_radius = handles.mesh_bounding_radius.upgrade()?;
        Some(crate::gui::render::fit_view::fit_distance(
            &ui,
            &render_ctx,
            &mesh_bounding_radius,
        ))
    })
}
