//! Saved plans: the rough model, the settings and the ticked layouts of the planner,
//! stored in the library database and exchanged as `.indicatrix-rough.toml` files.
//!
//! - [`dto`], [`convert`], [`checks`] and [`format`] are the file format: documents,
//!   validation, cross-checks and the header-first loader;
//! - [`store`] is the database table;
//! - [`staleness`] compares the stored designs with the library;
//! - [`list`], [`save`], [`open`] and [`transfer`] are the window's flows: the saved
//!   list, saving, opening and export/import. All database and file work runs on a
//!   worker thread ([`spawn_task`]); only the results are applied on the UI thread.

pub mod checks;
pub mod convert;
pub mod dto;
#[cfg(test)]
mod fixtures;
pub mod format;
mod hull_base;
mod list;
pub mod naming;
mod open;
mod save;
pub mod staleness;
pub mod store;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_checks;
#[cfg(test)]
mod tests_db;
#[cfg(test)]
mod tests_mesh;
#[cfg(test)]
mod tests_save;
#[cfg(test)]
mod tests_shape;
mod transfer;

use self::dto::DesignShape;
use super::host::{Host, on_host};
use crate::RoughPlanModel;
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    any::Any,
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::warn;

/// The shapes a loaded plan came with, for as long as its layouts are on screen: saving
/// them again must keep the shapes they were planned for, not today's.
struct LoadedFingerprints {
    /// The stored plan the layouts were loaded from.
    plan_id: i64,
    /// The stamp of the library the plan's file came from, which a re-save keeps: the
    /// entry ids the file still carries (designs that were not found here) belong to
    /// that library, not to this one.
    library_id: Option<u32>,
    /// The stored shape of each design, by the entry id the layouts now use.
    shapes: BTreeMap<i64, DesignShape>,
}

/// The banner of a plan that was just opened, kept until its rows are on screen.
struct PendingBanner {
    /// The stored plan the banner belongs to.
    plan_id: i64,
    /// The banner line.
    text: String,
}

/// The saved plans' UI-thread state.
#[derive(Default)]
pub(super) struct SavedState {
    /// Bumped by every list refresh; an older answer is dropped.
    list_seq: u64,
    /// Bumped by every open or import; an older answer is dropped.
    open_seq: u64,
    /// Whether the name row that is open saves the ticked layouts only.
    selected_only: bool,
    /// The stored shapes of the loaded plan, if one is on screen.
    loaded: Option<LoadedFingerprints>,
    /// The banner to show when the opened plan's rows arrive.
    pending_banner: Option<PendingBanner>,
}

/// Registers the save, open, rename, delete, export and import callbacks on the planner
/// window and fills the saved list.
pub(super) fn setup_saved_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_begin_save(|selected_only| on_host(|host| save::begin_save(host, selected_only)));
    model.on_confirm_save(|| on_host(save::confirm_save));
    model.on_open_saved(|id| on_host(|host| open::open_saved(host, i64::from(id))));
    model.on_rename_saved(|id, name| {
        on_host(|host| list::rename_saved(host, i64::from(id), &name));
    });
    model.on_delete_saved(|id| on_host(|host| list::delete_saved(host, i64::from(id))));
    model.on_export_saved(|id| on_host(|host| transfer::export_saved(host, i64::from(id))));
    model.on_import_saved(|| on_host(transfer::import_saved));
    list::refresh_list(host);
}

/// A readable message for a `catch_unwind` payload.
pub(in crate::gui::rough_plan) fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Runs `work` on a worker thread with the database handle, then `done` with its result
/// on the UI thread (skipped if the planner window is gone by then).
///
/// A panic in `work` does not take the thread's answer with it: `done` gets it as an
/// error. A message is shown in the window if the thread cannot be started.
fn spawn_task<T: Send + 'static>(
    host: &Host,
    work: impl FnOnce(&Mutex<Database>) -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&Rc<Host>, Result<T, String>) + Send + 'static,
) {
    let db = Arc::clone(&host.db);
    let weak = host.window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("rough-saved".to_string())
        .spawn(move || {
            let value = catch_unwind(AssertUnwindSafe(|| work(&db))).unwrap_or_else(|payload| {
                let message = panic_message(&*payload);
                warn!("Rough planner: a saved-plan task panicked: {message}");
                Err(format!("Internal error: {message}"))
            });
            let _ = weak.upgrade_in_event_loop(move |_window| {
                on_host(|host| done(host, value));
            });
        });
    if let Err(error) = spawned {
        warn!("Rough planner: could not start a background task: {error}");
        host.window
            .global::<RoughPlanModel>()
            .set_error_text(format!("Could not start a background task: {error}").into());
    }
}

/// Shows `message` as the window's error line.
pub(in crate::gui::rough_plan) fn show_error(host: &Host, message: &str) {
    host.window
        .global::<RoughPlanModel>()
        .set_error_text(message.into());
}

/// Shows `message` as the window's neutral status line.
pub(in crate::gui::rough_plan) fn show_status(host: &Host, message: &str) {
    let model = host.window.global::<RoughPlanModel>();
    model.set_status_text(message.into());
    model.set_status_level(0);
}

/// Shows `message` as the window's status line in the warning color (a staleness note).
fn show_warning(host: &Host, message: &str) {
    let model = host.window.global::<RoughPlanModel>();
    model.set_status_text(message.into());
    model.set_status_level(1);
}

/// Tells the user something finished: a toast on the main window and the window's status
/// line (the planner window may be covering the main one).
pub(in crate::gui::rough_plan) fn announce(host: &Host, message: &str) {
    if let Some(main) = host.main.upgrade() {
        crate::gui::show_toast(&main, message, "success");
    }
    show_status(host, message);
}
