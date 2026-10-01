//! The manually-triggered "designs with no tilt curves yet" library scan -- see this
//! group's own `mod.rs` doc comment for why this one is never run automatically,
//! unlike `gui::batch::preview::scan`.

use crate::MainWindow;
use indicatrix_vault::db::sqlite::Database;
use slint::Weak;
use std::{
    sync::{Arc, Mutex, PoisonError},
    thread,
};

/// Every entry id with no stored tilt curves at all
/// (`Database::entry_ids_missing_tilt_curves`) -- the tilt-curve counterpart of
/// `gui::batch::preview::scan`'s `scan_missing_preview_ids`: one blob-free `LEFT JOIN`
/// query rather than a page walk plus a per-row point lookup. Intended to run
/// off the UI thread -- see [`spawn_missing_tilt_curve_scan`].
fn scan_missing_tilt_curve_ids(db: &Mutex<Database>) -> Vec<i64> {
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry_ids_missing_tilt_curves(|material| {
            crate::bridge::preview_render::cache_fingerprint(
                crate::bridge::preview_render::CacheKind::TiltCurves,
                material,
            )
        })
        .unwrap_or_default()
}

/// Runs [`scan_missing_tilt_curve_ids`] on its own worker thread and reports the result
/// back to `on_result` on the UI event loop.
///
/// Unlike `gui::batch::preview::scan::spawn_missing_preview_scan` (which `gui::mod`
/// calls once, automatically, shortly after every session's diagram list loads), this
/// scan is never launched automatically: at ~72 minutes/catalogue, surprising a user
/// with an unprompted "compute tilt curves for N designs?" offer every session would
/// be far more intrusive than the preview batch's equivalent. This function is called
/// only from the two explicit triggers `super::wiring::setup_tilt_batch_callbacks`
/// wires: the filter panel's "Compute missing tilt curves" button and a session-wide
/// manual re-scan, never on a timer or at startup.
pub(super) fn spawn_missing_tilt_curve_scan(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    on_result: impl FnOnce(&MainWindow, Vec<i64>) + Send + 'static,
) {
    thread::spawn(move || {
        let ids = scan_missing_tilt_curve_ids(&db);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| on_result(&ui, ids));
    });
}
