//! The `SolidModel` callbacks: camera (orbit, zoom, pose pills, reset), hover,
//! click and double click, keyboard selection, and the view options. Every one
//! changes state and asks for a coalesced [`super::refresh`]; none draws itself.

use super::{VIEWS, request_refresh, state::view_mode_for_tab};
use crate::{AppModel, AppWindow, SolidModel, app::Ctx};
use indicatrix_solid::preview::{
    CameraPose, camera,
    view::{contain_pixel, step_selection, toggle_enlarged_panel},
};
use slint::ComponentHandle;

/// Wires every `SolidModel` callback.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    wire_camera(ui, ctx);
    wire_pointer(ui, ctx);
    wire_selection(ui, ctx);
}

/// The shared orbit camera as the views read it (`WebApp::view`).
fn current_pose(ctx: &Ctx) -> CameraPose {
    let app = ctx.state.borrow();
    CameraPose {
        yaw: app.view.yaw,
        pitch: app.view.pitch,
        distance: app.view.distance,
    }
}

/// Moves the shared camera to `pose`: `WebApp::view`, the header's `AppModel`
/// camera, and `AppModel.camera-changed` (which persists it and tells the
/// renderer), then redraws.
fn set_pose(ctx: &Ctx, pose: CameraPose) {
    {
        let mut app = ctx.state.borrow_mut();
        app.view.yaw = pose.yaw;
        app.view.pitch = pose.pitch;
        app.view.distance = pose.distance;
    }
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<AppModel>();
        model.set_yaw(pose.yaw);
        model.set_pitch(pose.pitch);
        model.set_distance(pose.distance);
        model.invoke_camera_changed(pose.yaw, pose.pitch, pose.distance);
    }
    request_refresh(ctx);
}

/// The shown solid's bounding radius (zoom clamp and "Fit").
fn radius() -> f64 {
    VIEWS.with(|cell| cell.borrow().radius())
}

fn wire_camera(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<SolidModel>();
    let c = ctx.clone();
    model.on_orbit(move |dx, dy| set_pose(&c, camera::orbit_step(current_pose(&c), dx, dy)));
    let c = ctx.clone();
    model.on_zoom(move |delta| {
        let pose = current_pose(&c);
        let distance = camera::zoom_step(pose.distance, delta, radius());
        set_pose(&c, CameraPose { distance, ..pose });
    });
    let c = ctx.clone();
    model.on_set_view(move |kind| {
        set_pose(&c, camera::standard_view(kind, current_pose(&c), radius()));
    });
    let c = ctx.clone();
    model.on_reset_camera(move || set_pose(&c, camera::RESET_POSE));
    let c = ctx.clone();
    model.on_view_size_changed(move |width, height| {
        VIEWS.with(|cell| cell.borrow_mut().view_size = (width, height));
        request_refresh(&c);
    });
    let c = ctx.clone();
    model.on_options_changed(move || request_refresh(&c));
}

/// Which view the pointer is over: `Some(true)` Diagram, `Some(false)` Solid.
fn on_diagram(ui: &AppWindow) -> Option<bool> {
    view_mode_for_tab(ui.global::<AppModel>().get_view_tab()).map(|mode| mode == 3)
}

/// What is under the pointer at logical `(x, y)` in the current view: the facet
/// id, and (Diagram only) the index-wheel tooth and the panel.
struct Hit {
    facet: Option<u32>,
    tooth: Option<u32>,
    panel: Option<indicatrix_solid::diagram2d::PanelKind>,
}

/// Resolves `(x, y)` against the last frame's pick buffers through the view's
/// `image-fit: contain` mapping.
fn hit_at(diagram: bool, x: f32, y: f32) -> Hit {
    VIEWS.with(|cell| {
        let views = cell.borrow();
        let (view_w, view_h) = views.view_size;
        if diagram {
            let Some(frame) = &views.diagram else {
                return Hit {
                    facet: None,
                    tooth: None,
                    panel: None,
                };
            };
            let pixel = contain_pixel(x, y, view_w, view_h, frame.width, frame.height);
            Hit {
                facet: pixel.and_then(|(px, py)| frame.pick_at(px, py)),
                tooth: pixel.and_then(|(px, py)| frame.tooth_at(px, py)),
                panel: pixel.and_then(|(px, py)| frame.panel_at(px, py)),
            }
        } else {
            let facet = views.pick.as_ref().and_then(|pick| {
                contain_pixel(x, y, view_w, view_h, pick.width, pick.height)
                    .and_then(|(px, py)| pick.facet_at(px, py))
            });
            Hit {
                facet,
                tooth: None,
                panel: None,
            }
        }
    })
}

/// Sets the tooltip of the current view.
fn set_hover_text(ui: &AppWindow, diagram: bool, text: String) {
    let model = ui.global::<SolidModel>();
    if diagram {
        model.set_diagram_hover_text(text.into());
    } else {
        model.set_hover_text(text.into());
    }
}

/// Hover: highlight the facet under the pointer and show its text; off every
/// facet, the Diagram names a hovered wheel tooth ("Index 12"), otherwise the last
/// clicked facet's label stays readable (the desktop's rule).
fn hover(ctx: &Ctx, ui: &AppWindow, x: f32, y: f32) {
    let Some(diagram) = on_diagram(ui) else {
        return;
    };
    let hit = hit_at(diagram, x, y);
    let text = VIEWS.with(|cell| {
        let mut views = cell.borrow_mut();
        if views.overlay.hovered != hit.facet {
            views.overlay.hovered = hit.facet;
            views.overlay_dirty = true;
        }
        match (hit.facet, hit.tooth) {
            (Some(facet), _) => views
                .hover_text
                .get(facet as usize)
                .cloned()
                .unwrap_or_default(),
            (None, Some(tooth)) => format!("Index {tooth}"),
            (None, None) if diagram => views.diagram_selected_label.clone(),
            (None, None) => views.selected_label.clone(),
        }
    });
    set_hover_text(ui, diagram, text);
    request_refresh(ctx);
}

fn wire_pointer(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<SolidModel>();
    let c = ctx.clone();
    model.on_hover(move |x, y| {
        if let Some(ui) = c.ui.upgrade() {
            hover(&c, &ui, x, y);
        }
    });
    let c = ctx.clone();
    model.on_pointer_left(move || {
        let changed = VIEWS.with(|cell| {
            let mut views = cell.borrow_mut();
            let changed = views.overlay.hovered.take().is_some();
            views.overlay_dirty |= changed;
            changed
        });
        if changed {
            request_refresh(&c);
        }
    });
    let c = ctx.clone();
    model.on_click(move |x, y| {
        if let Some(ui) = c.ui.upgrade() {
            click(&c, &ui, x, y);
        }
    });
    let c = ctx.clone();
    model.on_double_click(move |x, y| {
        let Some(ui) = c.ui.upgrade() else {
            return;
        };
        if on_diagram(&ui) != Some(true) {
            return;
        }
        let model = ui.global::<SolidModel>();
        let next = toggle_enlarged_panel(model.get_enlarged_panel(), hit_at(true, x, y).panel);
        model.set_enlarged_panel(next);
        request_refresh(&c);
    });
}

/// A click: a facet selects it (highlight) and its tier (the tier list); in the
/// Diagram a wheel tooth is reported in `SolidModel.clicked-tooth`; a click on
/// nothing clears the selection (the desktop's rules for both views).
fn click(ctx: &Ctx, ui: &AppWindow, x: f32, y: f32) {
    let Some(diagram) = on_diagram(ui) else {
        return;
    };
    // While the Slice tool holds a provisional tier the frame's facet -> tier table may
    // name that tier (an index the design does not have), so a click selects nothing
    // until Keep or Discard.
    if VIEWS.with(|cell| cell.borrow().manip.has_provisional()) {
        return;
    }
    let hit = hit_at(diagram, x, y);
    let model = ui.global::<SolidModel>();
    if diagram && hit.facet.is_none() {
        if let Some(tooth) = hit.tooth {
            model.set_clicked_tooth(i32::try_from(tooth).unwrap_or(-1));
            return;
        }
        model.set_clicked_tooth(-1);
    }
    let (tier, label) = VIEWS.with(|cell| {
        let mut views = cell.borrow_mut();
        views.overlay.selected_facet = hit.facet;
        if diagram && hit.facet.is_none() {
            views.overlay.hovered = None;
        }
        if !diagram {
            // The facet the drag handles sit on (a miss forgets it).
            views.manip.remember_facet(hit.facet);
        }
        views.overlay_dirty = true;
        let tier = hit
            .facet
            .and_then(|facet| views.facet_tier.get(facet as usize).copied().flatten());
        let label = hit
            .facet
            .and_then(|facet| views.hover_text.get(facet as usize).cloned())
            .unwrap_or_default();
        if diagram {
            views.diagram_selected_label.clone_from(&label);
        } else {
            views.selected_label.clone_from(&label);
        }
        (tier, label)
    });
    set_hover_text(ui, diagram, label);
    match (hit.facet, tier) {
        // A facet with no tier (a preform plane) stays lit but selects nothing new.
        (Some(_), None) => request_refresh(ctx),
        (Some(_), Some(tier)) => super::select_tier(ctx, Some(tier)),
        (None, _) => super::select_tier(ctx, None),
    }
}

fn wire_selection(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<SolidModel>();
    let c = ctx.clone();
    model.on_step_selection(move |delta| {
        let next = {
            let app = c.state.borrow();
            let count = app
                .design
                .as_ref()
                .map_or(0, |d| d.session.design.tiers.len());
            step_selection(app.selected_tier, delta, count)
        };
        super::select_tier(&c, next);
    });
    let c = ctx.clone();
    model.on_clear_selection(move || super::select_tier(&c, None));
}
