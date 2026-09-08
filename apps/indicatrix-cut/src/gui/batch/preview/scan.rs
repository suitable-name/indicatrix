//! The once-per-session "designs with no previews yet" library scan -- see this
//! group's own `mod.rs` doc comment.

use crate::MainWindow;
use indicatrix_vault::{db::sqlite::Database, model::filter::RangeFilter};
use slint::Weak;
use std::{
    sync::{Mutex, PoisonError},
    thread,
};

/// Keyset page size for [`scan_missing_preview_ids`]'s catalogue walk -- large enough
/// to make few round trips against a ~3,187-design catalogue, small enough to keep any
/// one page's work off the calling thread's stack/heap for only a moment.
const SCAN_PAGE_SIZE: i64 = 500;

/// Walks the whole local catalogue via `Database::search_diagrams_page`'s keyset
/// pagination and returns every entry id whose `diagram_previews.preview_generated_at`
/// is still unset -- i.e. every design [`super::spawn_preview_batch`] has never even
/// attempted, per `PreviewImages::generated_at`'s doc comment on why that field (not
/// `front`/`top` being `None`) is the right "never generated" test.
///
/// `indicatrix-vault` has no bulk "which designs lack previews" query of its own, so
/// this does a one-query-per-page-plus-one-point-lookup-per-row walk instead: a few
/// pages of up to [`SCAN_PAGE_SIZE`] rows each, each row a cheap indexed
/// `get_preview_images` point lookup. Intended to run off the UI thread (see
/// [`spawn_missing_preview_scan`]); this function does not itself spawn a thread.
fn scan_missing_preview_ids(db: &Mutex<Database>) -> Vec<i64> {
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
            if guard
                .get_preview_images(item.id)
                .is_ok_and(|p| p.generated_at.is_none())
            {
                missing.push(item.id);
            }
        }
        if (page_len as i64) < SCAN_PAGE_SIZE {
            break;
        }
    }
    missing
}

/// Runs [`scan_missing_preview_ids`] on its own worker thread and reports the result
/// back to `on_result` on the UI event loop: `gui::mod` calls this once per session,
/// shortly after the initial diagram list loads, and shows a confirmation prompt only
/// if the returned list is non-empty.
pub(super) fn spawn_missing_preview_scan(
    ui_weak: Weak<MainWindow>,
    db: std::sync::Arc<Mutex<Database>>,
    on_result: impl FnOnce(&MainWindow, Vec<i64>) + Send + 'static,
) {
    thread::spawn(move || {
        let ids = scan_missing_preview_ids(&db);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| on_result(&ui, ids));
    });
}
