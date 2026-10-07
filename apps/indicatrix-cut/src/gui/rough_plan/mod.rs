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

use self::host::open_planner;
pub(in crate::gui) use self::host::{
    close_planner_window, planner_work_at_risk, refresh_links_enabled,
};
use crate::{MainWindow, RoughPlanModel, bridge::library::source::LibrarySource};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

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
