//! Wires up the detail header's metadata editor modal to the local database.

use super::{local_load::load_diagram_detail, shared::non_empty};
use crate::{
    EditorModel, LibraryModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{library::local::refresh_after_library_change, show_toast},
};
use indicatrix_vault::{db::sqlite::Database, model::metadata_update::MetadataUpdate};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

/// Wires up the detail header's metadata editor modal (see
/// `metadata_editor_dialog.slint`) -> `save_metadata`.
///
/// Local designs only, same guard and same reasoning as
/// `library::setup_rename_callback`/`setup_set_shape_callback`: a remote-sourced
/// `selected_entry_id` names a row in a REMOTE server's catalogue, not this process's
/// local database, and the library-sync protocol is read-only besides.
/// `detail_header.slint` additionally only offers the editor for `root.detail.is_local`,
/// so this is a backstop, not the only thing standing between a remote id and the
/// local database.
///
/// Title and metadata are saved together through
/// [`Database::rename_and_update_metadata`], one transaction. The title lives in
/// `diagram_entries`, not `diagram_details`, and is renamed with none of the subset
/// trap `update_diagram_metadata` exists for. Every other field goes through that
/// method's `UPDATE`, which -- unlike `save_diagram_detail` -- touches only the twelve
/// columns it's given and leaves everything else untouched; see its own doc comment for
/// the full story.
///
/// On success, reloads the design from the database via
/// [`super::local_load::load_diagram_detail`] rather than hand-patching
/// `current_detail`'s dozen fields in place -- the reload picks up SQLite's own
/// numeric normalisation (e.g. `"1.760"` reads back `"1.76"`) and this module's own
/// 3-decimal display formatting for exactly the same reason opening the design fresh
/// would, with one implementation instead of two that could drift apart. Also
/// re-reconstructs the 3D viewport's planes, which matters when the edit changed
/// `shape` (the one field here that feeds `reconstruct_planes`' emerald/baguette/rect
/// special case).
pub fn setup_save_metadata_callback(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let db_meta = Arc::clone(db_mutex);
    let source_meta = Arc::clone(source);
    let render_ctx_meta = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_save_metadata(
        move |title: SharedString,
              designer: SharedString,
              shape: SharedString,
              refractive_index: SharedString,
              index_gear: SharedString,
              facets_count: SharedString,
              symmetry_order: SharedString,
              mirror_symmetry: bool,
              lw_ratio: SharedString,
              hw_ratio: SharedString,
              cw_ratio: SharedString,
              pw_ratio: SharedString,
              volume: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_meta
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to edit a design's metadata.",
                    "error",
                );
                return;
            }
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                return;
            }
            let entry_id = i64::from(entry_id);

            let update = MetadataUpdate {
                designer_info: non_empty(&designer),
                shape: non_empty(&shape),
                refractive_index: non_empty(&refractive_index),
                index_gear: non_empty(&index_gear),
                facets_count: non_empty(&facets_count),
                symmetry_order: non_empty(&symmetry_order),
                mirror_symmetry: Some(mirror_symmetry),
                lw_ratio: non_empty(&lw_ratio),
                hw_ratio: non_empty(&hw_ratio),
                cw_ratio: non_empty(&cw_ratio),
                pw_ratio: non_empty(&pw_ratio),
                volume: non_empty(&volume),
            };
            // Title and metadata in one transaction: a rejected numeric field must not
            // leave the new title applied while the toast below reports a failure.
            //
            // The write, the three cache deletes and everything after run on a worker
            // (the same shape as `refresh_after_library_change`): they used to run on
            // the UI thread behind the shared database lock, followed by two more
            // synchronous reads. Only the toast, the dirty flag and the reloads that
            // touch `ui` come back through the event loop.
            let db = Arc::clone(&db_meta);
            let source = Arc::clone(&source_meta);
            let render_ctx = Arc::clone(&render_ctx_meta);
            let ui_weak = ui.as_weak();
            let title = title.to_string();
            std::thread::spawn(move || {
                let result = {
                    let guard = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let result = guard.rename_and_update_metadata(entry_id, &title, &update);
                    if result.is_ok() {
                        invalidate_derived_caches(&guard, entry_id);
                    }
                    drop(guard);
                    result.map_err(|e| e.to_string())
                };
                // The queued closure holds the vault weakly, so a window closed meanwhile
                // does not keep the database file open until the event loop runs it.
                let db_weak = Arc::downgrade(&db);
                drop(db);
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    if let Some(db) = db_weak.upgrade() {
                        finish_save_metadata(&ui, &db, &source, &render_ctx, entry_id, result);
                    }
                });
            });
        },
    );
}

/// The UI half of the metadata save, run from the event loop once the worker's write
/// has finished: the toast, the editor's dirty flag, the detail reload and the list /
/// range refresh (the latter on its own worker, through
/// [`refresh_after_library_change`], whose range handling preserves the cutter's
/// narrowed range filters -- a metadata correction is not a request to clear them).
fn finish_save_metadata(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    result: Result<(), String>,
) {
    match result {
        Ok(()) => {
            show_toast(ui, "Metadata updated.", "success");
            // When this is the design open in the editor, its file now lags the
            // row: mark it unsaved so the next Save writes the edit.
            if crate::gui::editor::native_io::mark_design_row_edited(entry_id) {
                ui.global::<EditorModel>().set_is_dirty(true);
            }
            // The selection may have moved while the worker wrote; reloading the old
            // row then would put its header over the newly selected design.
            if i64::from(ui.global::<LibraryModel>().get_selected_entry_id()) == entry_id {
                load_diagram_detail(ui, db, render_ctx, entry_id);
            }
            refresh_after_library_change(ui, db, source);
        }
        Err(e) => show_toast(ui, &format!("Metadata update failed: {e}"), "error"),
    }
}

/// Deletes `entry_id`'s cached previews, tilt curves and solid extents after a metadata
/// edit: the edited fields (shape, index, gear) feed those renders and sweeps, so the
/// cached results no longer describe the design. Logged, not propagated: the edit
/// itself succeeded.
fn invalidate_derived_caches(db: &Database, entry_id: i64) {
    if let Err(e) = db.delete_preview_images(entry_id) {
        tracing::warn!("Metadata edit: failed to drop cached previews for #{entry_id}: {e}");
    }
    if let Err(e) = db.delete_tilt_curves(entry_id) {
        tracing::warn!("Metadata edit: failed to drop cached tilt curves for #{entry_id}: {e}");
    }
    if let Err(e) = db.delete_solid_extents(entry_id) {
        tracing::warn!("Metadata edit: failed to drop cached solid extents for #{entry_id}: {e}");
    }
}
