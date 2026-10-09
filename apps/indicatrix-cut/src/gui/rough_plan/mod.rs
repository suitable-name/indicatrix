//! The Rough Planner window (Library menu, "Plan Rough..."): model a rough, plan the
//! ten best ways to cut library designs from it.
//!
//! This module wires the main window's menu entry to the window in [`host`]. The
//! window's callbacks live with what they drive: model editing in [`editing`], the plan
//! run in [`run`], the live model figures in [`shape_worker`], the 3D view in [`view`],
//! library links in [`library_link`], saved plans in [`saved`], the designs excluded
//! from planning in [`exclusions`], the inclusions of a mesh rough in [`inclusions`] and
//! locating an inclusion from photos on a camera rig (the locate window and the rig window)
//! in [`locate`].

mod base;
mod carat;
#[cfg(feature = "zoning")]
mod colour_link;
mod counts;
mod cut_faces;
mod cut_rows;
mod editing;
mod exclusions;
mod format;
mod host;
mod inclusions;
mod inputs;
mod library_link;
mod locate;
mod mesh_task;
mod metrics;
mod obj_import;
mod run;
mod saved;
mod session;
mod shape_worker;
mod view;
#[cfg(feature = "zoning")]
mod zoning_hooks;

use self::host::open_planner;
pub(in crate::gui) use self::host::{
    close_planner_window, planner_work_at_risk, refresh_links_enabled,
};
use crate::{MainWindow, RoughPlanModel, bridge::library::source::LibrarySource};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Redraws the rough view (`zoning` builds): the Rough colour wizard changed the zones whose
/// handles the view draws.
#[cfg(feature = "zoning")]
pub(in crate::gui) fn redraw_rough_view() {
    host::on_host(view::redraw);
}

/// The Rough colour wizard stored a colour for a saved plan (`zoning` builds): the shown results,
/// if they are that plan's, are drawn again with it.
#[cfg(feature = "zoning")]
pub(in crate::gui) fn colour_stored() {
    host::on_host(view::results_changed);
}

/// The placement of design `entry_id` in the planner's caliper frame (the caliper turn and the
/// centring of its bounding box) and its caliper width in model units, read from the library
/// (`zoning` builds): what `rough_colour::preview` needs to move a rough's colour zones into a
/// stone's frame. The helpers behind it (`rotate_into_caliper_frame`, `caliper_centre`) are the
/// planner's own and unchanged; `None` for a design that is gone, unreadable or without a solid.
#[cfg(feature = "zoning")]
pub fn design_placement_of(
    entry_id: i64,
) -> Option<(
    indicatrix_cut_core::rough_plan::zoned_plan::DesignPlacement,
    f64,
)> {
    zoning_hooks::design_placement(entry_id)
}

/// The library changed which designs are excluded from the planner (a toggle in the
/// library window): the planner, if it exists, re-reads the list and the counts.
pub(in crate::gui) fn exclusions_changed() {
    host::on_host(exclusions::changed);
}

/// Wires the main window's `RoughPlanModel.open` (the Library menu item), which opens
/// the planner window. Every other callback belongs to the planner window and is
/// registered when that window is first created. Called once, from
/// `gui::build_main_window`.
pub(in crate::gui) fn setup_rough_plan_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let (weak, db, source) = (ui.as_weak(), Arc::clone(db), Arc::clone(source));
    ui.global::<RoughPlanModel>().on_open(move || {
        if let Some(ui) = weak.upgrade() {
            open_planner(&ui, &db, &source);
        }
    });
}
