//! Wires every `AppModel` callback to the state: file actions to `crate::io`,
//! New Design to `EditorSession::from_template`, undo/redo to the session, and the
//! settings/camera/viewport callbacks to [`WebApp`](super::state::WebApp).
//!
//! Each handler mutates inside a short `borrow_mut()`, then pushes and persists.

use super::{
    Ctx,
    persist::schedule_save,
    push::{MessageKind, push_all, push_design, show_message},
    settings::material_options,
    solve::auto_solve,
    state::{DesignSource, DesignState},
};
use crate::{AppModel, AppWindow, editor::guide, io, render::viewport};
use indicatrix::optics::raytracer::LightingPreset;
use indicatrix_cut_core::FreshDesignSpec;
use indicatrix_editor::{
    EditorSession,
    guide::NEW_DESIGN_CREATED,
    loading::{parse_new_design_form, parse_preform_form},
    templates::template_cards,
};
use slint::{ComponentHandle, TimerMode};

/// The desktop New Design dialog's preform defaults (`ui/models/editor.slint`:
/// shape 1, half-width 1.50, L/W 1.00, depth 1.50) and its fixed cylinder side count.
const NEW_PREFORM: (i32, &str, &str, &str, usize) = (1, "1.50", "1.00", "1.50", 96);

/// The New Design spec for gallery card `template_index`: the dialog's default
/// preform, and either the dialog's default gear/symmetry/mirror (96, 8, on) for
/// "Empty" or the template's own (the desktop dialog locks them to it).
fn new_design_spec(template_index: i32) -> Result<FreshDesignSpec, String> {
    let (shape, half_width, lw, depth, sides) = NEW_PREFORM;
    let preform = parse_preform_form(shape, half_width, lw, depth, sides)?;
    let template = usize::try_from(template_index - 1)
        .ok()
        .and_then(|i| indicatrix_cut_core::templates::TEMPLATES.get(i));
    let (gear, symmetry, mirror) = template.map_or((96, 8, true), |t| {
        (t.gear_teeth, t.symmetry_order, t.mirror)
    });
    parse_new_design_form(gear, preform, &symmetry.to_string(), mirror, 0)
}

/// Gallery "Create": replaces the design with a new one from card `template_index`,
/// after the unsaved-changes dialog (`crate::editor::unsaved`) when there are edits to
/// lose.
fn create_new_design(ctx: &Ctx, template_index: i32) {
    let c = ctx.clone();
    crate::editor::unsaved::confirm_discard(ctx, move || install_new_design(&c, template_index));
}

/// [`create_new_design`]'s second half, once nothing unsaved is at stake.
fn install_new_design(ctx: &Ctx, template_index: i32) {
    let spec = match new_design_spec(template_index) {
        Ok(spec) => spec,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let name = template_cards()
        .into_iter()
        .nth(usize::try_from(template_index).unwrap_or(0))
        .map_or_else(|| "Empty".to_string(), |card| card.name);
    let session = EditorSession::from_template(spec, template_index);
    ctx.state.borrow_mut().replace_design(DesignState::new(
        session,
        DesignSource::Template(name.clone()),
    ));
    if let Some(ui) = ctx.ui.upgrade() {
        push_all(&ui, &ctx.state.borrow());
    }
    auto_solve(ctx);
    schedule_save(ctx);
    show_message(ctx, MessageKind::Success, &format!("New design: {name}."));
    // The guided walkthrough's first step waits for exactly this event.
    guide::notify(ctx, NEW_DESIGN_CREATED);
}

/// Undo (`redo == false`) or redo through the session.
fn undo_redo(ctx: &Ctx, redo: bool) {
    let outcome = {
        let mut app = ctx.state.borrow_mut();
        let Some(design) = app.design.as_mut() else {
            return;
        };
        if redo {
            design.session.redo()
        } else {
            design.session.undo()
        }
    };
    match outcome {
        Ok(Some(_change)) => {
            if let Some(ui) = ctx.ui.upgrade() {
                push_design(&ui, &ctx.state.borrow());
            }
            auto_solve(ctx);
            schedule_save(ctx);
            // Undo / redo can reach a step's goal, or leave it (the desktop's rule: goals
            // are judged from the design, whichever route led there).
            guide::check_progress(ctx);
        }
        Ok(None) => {}
        Err(e) => show_message(
            ctx,
            MessageKind::Error,
            &format!("Cannot {}: {e}", if redo { "redo" } else { "undo" }),
        ),
    }
}

/// The file actions: open, the saves, and the PNG export dialog.
fn wire_file_actions(model: &AppModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_open_file(move || io::picker::pick_and_open(&c));
    let c = ctx.clone();
    model.on_save_asc(move || io::save::save_asc(&c));
    let c = ctx.clone();
    model.on_save_native_pair(move || io::save::save_native_pair(&c));
    let c = ctx.clone();
    model.on_save_native_only(move || io::save::save_native_only(&c));
    let c = ctx.clone();
    model.on_download_cutting_sheet(move || io::save::download_cutting_sheet(&c));
    let c = ctx.clone();
    model.on_download_diagram(move || io::save::download_diagram(&c));
    let c = ctx.clone();
    model.on_export_png(move || crate::render::open_export_dialog(&c));
    let c = ctx.clone();
    model.on_new_design(move |index| create_new_design(&c, index));
    let c = ctx.clone();
    model.on_undo(move || undo_redo(&c, false));
    let c = ctx.clone();
    model.on_redo(move || undo_redo(&c, true));
    let c = ctx.clone();
    model.on_dismiss_message(move || {
        c.timers.toast.stop();
        if let Some(ui) = c.ui.upgrade() {
            ui.global::<AppModel>().set_toast_visible(false);
        }
    });
}

/// Material, lighting and view tab: stored by name/label in the settings, then
/// persisted. The renderer re-renders on these.
fn wire_settings(model: &AppModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_material_changed(move |index| {
        {
            let mut app = c.state.borrow_mut();
            let options = material_options(&app.custom_materials);
            if let Some(name) = usize::try_from(index).ok().and_then(|i| options.get(i)) {
                app.settings.material.clone_from(name);
            }
        }
        schedule_save(&c);
    });
    let c = ctx.clone();
    model.on_lighting_changed(move |index| {
        c.state.borrow_mut().settings.lighting =
            LightingPreset::from_index(index).label().to_string();
        schedule_save(&c);
    });
    let c = ctx.clone();
    model.on_view_tab_changed(move |tab| {
        c.state.borrow_mut().settings.view_tab = tab.clamp(0, 2);
        schedule_save(&c);
    });
}

/// Orbit and viewport size: kept in `WebApp::view` for the renderer; the
/// camera is also persisted with the settings. Resizes are debounced and clamped
/// (`crate::render::viewport`).
fn wire_viewport(model: &AppModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_camera_changed(move |yaw, pitch, distance| {
        {
            let mut app = c.state.borrow_mut();
            app.view.yaw = yaw;
            app.view.pitch = pitch;
            app.view.distance = distance;
            app.settings.camera_yaw = yaw;
            app.settings.camera_pitch = pitch;
            app.settings.camera_distance = distance;
        }
        schedule_save(&c);
    });
    let c = ctx.clone();
    model.on_render_size_changed(move |logical_width, logical_height| {
        let settle = c.clone();
        c.timers.resize.start(
            TimerMode::SingleShot,
            viewport::RESIZE_DEBOUNCE,
            move || {
                let Some(ui) = settle.ui.upgrade() else {
                    return;
                };
                let scale = ui.window().scale_factor();
                let (width, height) =
                    viewport::clamp_render_dims(logical_width, logical_height, scale);
                {
                    let mut app = settle.state.borrow_mut();
                    app.view.render_width = width;
                    app.view.render_height = height;
                }
                crate::render::request_sync(&settle);
            },
        );
    });
}

/// Registers every `AppModel` callback.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<AppModel>();
    wire_file_actions(&model, ctx);
    wire_settings(&model, ctx);
    wire_viewport(&model, ctx);
}
