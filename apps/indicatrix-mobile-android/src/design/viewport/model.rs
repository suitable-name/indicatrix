//! State to Slint: writes everything `ViewportModel` shows.

use indicatrix_mobile_core::{
    app_state::AppState,
    layout::WindowClass,
    viewport::{
        HandleOverlay, ViewportState,
        frame::{WAITING_STATUS, max_edge_for},
        render_view,
    },
};
use slint::ComponentHandle;

use super::input;
use crate::{AppWindow, ViewportModel};

/// Tells the viewport the settings it reads (large handles, snapping, the raster cap:
/// 1280 px on a phone, 1600 px on a tablet, 960 px in battery saver).
/// Cheap and idempotent: the options live in a `Cell`.
pub(super) fn sync_options(state: &AppState) {
    let settings = &state.settings;
    let compact = state.window_class == WindowClass::Compact;
    state.viewport.set_options(
        settings.large_handles,
        compact,
        settings.snap_to_gear,
        max_edge_for(compact, settings.battery_saver),
    );
}

/// Pushes the viewport's picture rectangle, handles, Slice line, hint and cards.
pub(super) fn push(window: &AppWindow, state: &AppState) {
    sync_options(state);
    let viewport = &state.viewport;
    let selected = state.selected_tier;
    let model = window.global::<ViewportModel>();

    model.set_view_mode(viewport.mode.index());
    model.set_live_solid(render_view::live_solid(state));
    push_picture(&model, viewport);
    push_handles(&model, viewport, selected);
    push_tools(&model, viewport);

    // The Render mode has its own status line; the picture hints belong to the raster.
    let render = viewport.mode.is_render();
    let hint = if render {
        String::new()
    } else {
        viewport.hint_text(selected)
    };
    let hint = if hint.is_empty() && !render {
        status_line(state)
    } else {
        hint
    };
    model.set_hint_text(hint.into());

    match &viewport.info {
        Some(text) if !render => {
            model.set_info_text(text.as_str().into());
            model.set_info_visible(true);
        }
        _ => model.set_info_visible(false),
    }
    input::watch_slice(state);
}

/// The line that says why the picture is not the design's current state.
fn status_line(state: &AppState) -> String {
    let status = state.viewport.status.as_str();
    if status == WAITING_STATUS && state.solve.is_solving() {
        "Solving the design for the preview...".to_owned()
    } else {
        status.to_owned()
    }
}

fn push_picture(model: &ViewportModel<'_>, viewport: &ViewportState) {
    match viewport.image_rect() {
        Some((x, y, width, height)) => {
            model.set_image_x(x);
            model.set_image_y(y);
            model.set_image_width(width);
            model.set_image_height(height);
            model.set_has_frame(true);
        }
        None => model.set_has_frame(false),
    }
}

/// Length and angle in degrees of the segment from `from` to `to`.
fn stem(from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    (dx.hypot(dy), dy.atan2(dx).to_degrees())
}

fn push_handles(model: &ViewportModel<'_>, viewport: &ViewportState, selected: Option<usize>) {
    let HandleOverlay {
        visible,
        anchor,
        angle,
        depth,
        index,
        index_visible,
        active,
    } = viewport.handle_overlay(selected);
    model.set_handles_on(viewport.handles_on);
    model.set_handle_scale(viewport.options.get().handle_scale);
    model.set_handles_visible(visible);
    if !visible {
        model.set_active_handle(-1);
        return;
    }
    model.set_anchor_x(anchor.0);
    model.set_anchor_y(anchor.1);
    model.set_angle_x(angle.0);
    model.set_angle_y(angle.1);
    model.set_depth_x(depth.0);
    model.set_depth_y(depth.1);
    model.set_index_x(index.0);
    model.set_index_y(index.1);
    let (len, deg) = stem(anchor, angle);
    model.set_angle_len(len);
    model.set_angle_deg(deg);
    let (len, deg) = stem(anchor, depth);
    model.set_depth_len(len);
    model.set_depth_deg(deg);
    let (len, deg) = stem(anchor, index);
    model.set_index_len(len);
    model.set_index_deg(deg);
    model.set_index_visible(index_visible);
    model.set_active_handle(active);
}

fn push_tools(model: &ViewportModel<'_>, viewport: &ViewportState) {
    let slice = &viewport.slice;
    model.set_slice_mode(slice.mode);
    model.set_slice_symmetric(slice.symmetric);
    model.set_slice_session(slice.session.is_some());
    model.set_slice_keep_ok(
        slice
            .session
            .as_ref()
            .is_some_and(indicatrix_editor::manipulate::ProvisionalSlice::may_keep),
    );
    match slice.line {
        Some(line) => {
            let (len, deg) = stem(line.start, line.end);
            model.set_line_x(line.start.0);
            model.set_line_y(line.start.1);
            model.set_line_len(len);
            model.set_line_deg(deg);
            model.set_line_visible(true);
        }
        None => model.set_line_visible(false),
    }
}
