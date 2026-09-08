//! The manually-triggered "designs with no tilt curves yet" library scan -- see this
//! group's own `mod.rs` doc comment for why this one is never run automatically,
//! unlike `gui::batch::preview::scan`.

use crate::MainWindow;
use indicatrix_vault::{db::sqlite::Database, model::filter::RangeFilter};
use slint::Weak;
use std::{
    sync::{Arc, Mutex, PoisonError},
    thread,
};

/// Keyset page size for [`scan_missing_tilt_curve_ids`]'s catalogue walk -- matches
/// `gui::batch::preview::scan`'s `SCAN_PAGE_SIZE` reasoning (few round trips against a
/// ~3,187-design catalogue, each page cheap).
const SCAN_PAGE_SIZE: i64 = 500;

/// Walks the whole local catalogue via `Database::search_diagrams_page`'s keyset
/// pagination and returns every entry id with no stored tilt curves at all
/// (`Database::has_tilt_curves` false) -- the tilt-curve counterpart of
/// `gui::batch::preview::scan`'s `scan_missing_preview_ids`, same
/// one-query-per-page-plus-one-point-lookup-per-row approach since `indicatrix-vault`
/// has no bulk "which designs lack curves" query of its own. Intended to run off the
/// UI thread -- see [`spawn_missing_tilt_curve_scan`].
fn scan_missing_tilt_curve_ids(db: &Mutex<Database>) -> Vec<i64> {
    let mut missing = Vec::new();
    let mut after_id: Option<i64> = None;
    loop {
        let page = {
            let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
            guard.search_diagrams_page(
                "",
                "All",
                "All",
                &RangeFilter::default(),
                after_id,
                SCAN_PAGE_SIZE,
            )
        };
        let Ok(page) = page else { break };
        let page_len = page.len();
        if page_len == 0 {
            break;
        }
        after_id = page.last().map(|item| item.id);
        for item in &page {
            let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.has_tilt_curves(item.id).is_ok_and(|has| !has) {
                missing.push(item.id);
            }
        }
        if (page_len as i64) < SCAN_PAGE_SIZE {
            break;
        }
    }
    missing
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
