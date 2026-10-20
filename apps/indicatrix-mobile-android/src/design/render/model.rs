//! State to Slint: the overlay's properties and the frame image on `RenderModel`.

use indicatrix_mobile_core::{
    app_state::AppState,
    render::{PauseReason, RenderFrame, RenderStatus},
    viewport::render_view::{progress_fraction, quality_index, status_line},
};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use crate::{AppWindow, RenderModel};

/// Pushes the status line, the progress, the chips' state and whether there is something
/// to render. Cheap: a handful of property writes (Slint skips unchanged values).
pub(super) fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<RenderModel>();
    let status = state.render.status();
    let generation = state.session.current_generation();
    let solved = state.solve.current_tiers(generation).is_some();

    model.set_status_text(status_line(state).into());
    model.set_backend(state.settings.render.backend.index());
    model.set_engine(i32::from(
        state.settings.render.engine_spec().engine.index(),
    ));
    model.set_backend_report(state.render.backend_report().into());
    if let Some(fraction) = progress_fraction(&status) {
        model.set_progress(if solved { fraction } else { 0.0 });
    }
    model.set_paused(status == RenderStatus::Paused(PauseReason::User));
    model.set_can_pause(solved && !matches!(status, RenderStatus::Idle | RenderStatus::Failed(_)));
    model.set_can_render(solved);
    model.set_quality(quality_index(state.settings.render.quality));
    model.set_denoise(state.settings.render.denoise);
    model.set_light_drag(state.viewport.light_drag.on);
}

/// Shows `frame` in the Render view. A frame whose pixel count does not match its size is
/// dropped (logged).
pub(super) fn show_frame(window: &AppWindow, frame: &RenderFrame) {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(frame.width, frame.height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() != frame.rgba.len() {
        tracing::warn!(
            width = frame.width,
            height = frame.height,
            len = frame.rgba.len(),
            "a render frame was dropped: its pixel count does not match its size"
        );
        return;
    }
    bytes.copy_from_slice(&frame.rgba);
    let model = window.global::<RenderModel>();
    model.set_frame(Image::from_rgba8(buffer));
    model.set_has_frame(true);
}
