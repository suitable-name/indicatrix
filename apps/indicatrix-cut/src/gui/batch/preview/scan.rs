//! The once-per-session "designs with no previews yet" library scan -- see this
//! group's own `mod.rs` doc comment.

use crate::MainWindow;
use indicatrix_vault::db::sqlite::Database;
use slint::Weak;
use std::{
    sync::{Mutex, PoisonError},
    thread,
};

/// Every entry id whose cached preview is missing, has either view (front or top)
/// missing, or was rendered with other parameters than the current ones, per `Database::entry_ids_missing_previews`'s
/// fingerprint comparison (`preview_size`/`preview_spp` feed the current
/// fingerprint) -- one blob-free `LEFT JOIN` query rather than a page walk plus a
/// per-row point lookup. The walk this replaced (`search_diagrams_page` plus a `get_preview_images`
/// call per row) loaded both cached 256x256 preview PNGs for every entry in the
/// catalogue just to read `generated_at.is_none()` off each one -- measured at 4.4s and
/// 447MB for one pass over the real ~3,299-entry catalogue. Intended to run off
/// the UI thread (see [`spawn_missing_preview_scan`]); this function does not itself
/// spawn a thread.
fn scan_missing_preview_ids(db: &Mutex<Database>, preview_size: u32, preview_spp: u32) -> Vec<i64> {
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry_ids_missing_previews(|material| {
            crate::bridge::preview_render::cache_fingerprint(
                crate::bridge::preview_render::CacheKind::Preview {
                    size: preview_size,
                    spp: preview_spp,
                    max_bounces: super::engine::PREVIEW_MAX_BOUNCES,
                },
                material,
            )
        })
        .unwrap_or_default()
}

/// Runs [`scan_missing_preview_ids`] on its own worker thread and reports the result
/// back to `on_result` on the UI event loop: `gui::mod` calls this once per session,
/// shortly after the initial diagram list loads, and shows a confirmation prompt only
/// if the returned list is non-empty.
pub(super) fn spawn_missing_preview_scan(
    ui_weak: Weak<MainWindow>,
    db: std::sync::Arc<Mutex<Database>>,
    preview_size: u32,
    preview_spp: u32,
    on_result: impl FnOnce(&MainWindow, Vec<i64>) + Send + 'static,
) {
    thread::spawn(move || {
        let ids = scan_missing_preview_ids(&db, preview_size, preview_spp);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| on_result(&ui, ids));
    });
}
