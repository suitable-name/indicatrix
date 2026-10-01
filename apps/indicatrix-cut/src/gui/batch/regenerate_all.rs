//! The Library menu's whole-catalogue actions: regenerate every design's preview
//! images, every design's tilt curves, or both.
//!
//! Each one opens the same confirm step (`preview_batch_dialog.slint`/
//! `tilt_batch_dialog.slint`) every other trigger of those batches uses, worded for
//! regeneration ([`offer_preview_regeneration`]/[`offer_tilt_regeneration`]), so this
//! adds no second copy of either batch.
//!
//! "Both" runs them one after the other rather than together: the two confirm steps
//! would otherwise sit on top of each other. It offers the previews first and holds the
//! tilt ids in [`PENDING_TILT`]; [`preview_dialog_closed`] (called when the preview
//! dialog closes, after a finished batch or a declined offer) then offers the tilt
//! curves. That also holds after a CANCELLED preview batch: Close on its
//! "(cancelled)" summary still leads to the tilt offer. If a tilt batch is already
//! running by then (started from a card menu meanwhile) the offer is skipped with a
//! toast, so it cannot flip the running dialog back to its confirm view. Starting any
//! other Library regeneration drops a held tilt offer ([`clear_pending_tilt`]).

use crate::{
    BatchModel, LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{
        batch::{preview, tilt},
        show_toast,
    },
};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};

thread_local! {
    /// The tilt ids "Regenerate Both" offers once the preview dialog closes.
    /// UI-thread only: every reader and writer is a Slint callback.
    static PENDING_TILT: RefCell<Option<Vec<i64>>> = const { RefCell::new(None) };
}

/// [`preview::offer_batch_confirmation`], worded as a regeneration: the designs may
/// already have previews, which the batch replaces.
pub fn offer_preview_regeneration(ui: &MainWindow, ids: &[i64]) {
    preview::offer_batch_confirmation(ui, ids);
    ui.global::<BatchModel>().set_preview_offer_regenerate(true);
}

/// [`tilt::offer_batch_confirmation`], worded as a regeneration.
pub fn offer_tilt_regeneration(ui: &MainWindow, ids: &[i64]) {
    tilt::offer_batch_confirmation(ui, ids);
    ui.global::<BatchModel>().set_tilt_offer_regenerate(true);
}

/// The preview dialog closed (a finished batch's Close, or "Not now" on its offer):
/// offers the tilt curves a "Regenerate Both" is still holding, if any.
///
/// Also runs after a cancelled preview batch (its Close). When a tilt batch is already
/// running, the held ids are dropped and a toast says so instead of offering: the
/// offer would replace the running dialog's progress view with a confirm step.
pub fn preview_dialog_closed(ui: &MainWindow) {
    if let Some(ids) = PENDING_TILT.with(|pending| pending.borrow_mut().take()) {
        if ui.global::<BatchModel>().get_tilt_batch_running() {
            show_toast(
                ui,
                "A tilt-curve batch is already running -- the tilt half of \"Regenerate Both\" was skipped.",
                "info",
            );
            return;
        }
        offer_tilt_regeneration(ui, &ids);
    }
}

/// Drops the tilt ids a "Regenerate Both" is holding, so a plain Library action started
/// afterwards never inherits its tilt tail.
fn clear_pending_tilt() {
    PENDING_TILT.with(|pending| *pending.borrow_mut() = None);
}

/// Every design in the local catalogue, or `None` after a toast saying why not: a
/// remote-browsed library (whose ids this database does not hold), an empty
/// catalogue, or a failed query.
pub(in crate::gui) fn all_ids_or_toast(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) -> Option<Vec<i64>> {
    if source
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_remote()
    {
        show_toast(
            ui,
            "Switch to the local library to regenerate its designs.",
            "error",
        );
        return None;
    }
    let ids = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .all_entry_ids();
    match ids {
        Ok(ids) if ids.is_empty() => {
            show_toast(ui, "The library has no designs yet.", "info");
            None
        }
        Ok(ids) => Some(ids),
        Err(e) => {
            show_toast(ui, &format!("Could not list the library: {e}"), "error");
            None
        }
    }
}

/// Wires the Library menu's three `LibraryModel.regenerate_all_*` callbacks.
///
/// Called once, from `gui::build_main_window`.
pub(in crate::gui) fn setup_regenerate_all_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let (db_p, source_p, weak_p) = (Arc::clone(db), Arc::clone(source), ui.as_weak());
    ui.global::<LibraryModel>()
        .on_regenerate_all_previews(move || {
            clear_pending_tilt();
            if let Some(ui) = weak_p.upgrade()
                && let Some(ids) = all_ids_or_toast(&ui, &db_p, &source_p)
            {
                offer_preview_regeneration(&ui, &ids);
            }
        });

    let (db_t, source_t, weak_t) = (Arc::clone(db), Arc::clone(source), ui.as_weak());
    ui.global::<LibraryModel>()
        .on_regenerate_all_tilt_curves(move || {
            clear_pending_tilt();
            if let Some(ui) = weak_t.upgrade()
                && let Some(ids) = all_ids_or_toast(&ui, &db_t, &source_t)
            {
                offer_tilt_regeneration(&ui, &ids);
            }
        });

    let (db_b, source_b, weak_b) = (Arc::clone(db), Arc::clone(source), ui.as_weak());
    ui.global::<LibraryModel>()
        .on_regenerate_all_previews_and_tilt_curves(move || {
            if let Some(ui) = weak_b.upgrade()
                && let Some(ids) = all_ids_or_toast(&ui, &db_b, &source_b)
            {
                PENDING_TILT.with(|pending| *pending.borrow_mut() = Some(ids.clone()));
                offer_preview_regeneration(&ui, &ids);
            }
        });
}
