//! The header's material combo as the live render sees it: while the combo is linked to
//! the design it shows the design's traced material, and picking another one there
//! unlinks.

use crate::{AppModel, app::Ctx};
use indicatrix_web_core::settings::{material_options, render_material};
use slint::ComponentHandle;
use std::cell::RefCell;

thread_local! {
    /// `RenderSettings::material` at the last [`follow`]: a change means the user picked
    /// a material in the header.
    static LAST_SEEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// While linked, the header's material combo shows the design's traced material;
/// picking another one there unlinks (an explicit choice wins, and the render follows
/// the combo from then on).
pub(super) fn follow(ctx: &Ctx) {
    let (material, linked) = {
        let app = ctx.state.borrow();
        (app.settings.material.clone(), app.settings.link_material)
    };
    let previous = LAST_SEEN.with(|seen| seen.borrow_mut().replace(material.clone()));
    if linked && previous.is_some_and(|p| p != material) {
        ctx.state.borrow_mut().settings.link_material = false;
    }
}

/// While linked, shows the design's traced material in the header on every tab: the
/// Render tab's own sync only runs while that tab is showing, and the Solid view's tilt
/// dialog and metrics trace this material too.
pub(super) fn show_linked(ctx: &Ctx) {
    let name = {
        let app = ctx.state.borrow();
        let Some(design) = app.design.as_ref().filter(|_| app.settings.link_material) else {
            return;
        };
        match render_material(
            &app.settings,
            Some(&design.session.design),
            &app.custom_materials,
        ) {
            Ok(material) => material.name,
            Err(_) => return,
        }
    };
    show_in_header(ctx, &name);
}

/// Selects `name` in the header's material combo (without firing its callback).
fn show_in_header(ctx: &Ctx, name: &str) {
    let index = {
        let app = ctx.state.borrow();
        material_options(&app.custom_materials)
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
    };
    if let (Some(ui), Some(index)) = (ctx.ui.upgrade(), index) {
        ui.global::<AppModel>().set_material_index(index as i32);
    }
}
