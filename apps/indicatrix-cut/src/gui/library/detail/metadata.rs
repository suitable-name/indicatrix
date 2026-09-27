//! Wires up the detail header's metadata editor modal to the local database.

use super::{local_load::load_diagram_detail, shared::non_empty};
use crate::{
    LibraryModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        library::{
            diagram_list::sync_range_bounds_to_ui_preserving_filters,
            search::refresh_diagram_list_via_source,
        },
        show_toast,
    },
};
use indicatrix_vault::{db::sqlite::Database, model::metadata_update::MetadataUpdate};
use slint::{ComponentHandle, Model, SharedString};
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
/// Title is saved through [`Database::rename_diagram_entry`] -- it lives in
/// `diagram_entries`, not `diagram_details`, and already has its own narrow, correct
/// setter with none of the subset trap `update_diagram_metadata` exists for. Every
/// other field goes through `update_diagram_metadata` in one call, which -- unlike
/// `save_diagram_detail` -- touches only the twelve columns it's given and leaves
/// everything else untouched; see that method's own doc comment for the full story.
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

            let result = {
                let db = db_meta
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.rename_diagram_entry(entry_id, &title).and_then(|()| {
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
                    db.update_diagram_metadata(entry_id, &update)
                })
            };
            match result {
                Ok(()) => {
                    show_toast(&ui, "Metadata updated.", "success");
                    load_diagram_detail(&ui, &db_meta, &render_ctx_meta, entry_id);
                    // Refreshes the visible list/filters the same way
                    // `library::refresh_after_library_change` does for rename/shape --
                    // that helper is private to `gui::library`, so this repeats its
                    // essential two steps (range bounds, then the list query) rather
                    // than reaching into another module's private function. The
                    // `_preserving_filters` variant: a metadata correction
                    // is not a request to clear whatever range filters the cutter had
                    // already narrowed the catalogue to.
                    sync_range_bounds_to_ui_preserving_filters(&ui, &db_meta);
                    let search = ui.global::<LibraryModel>().get_search_text();
                    let shape_idx = ui.global::<LibraryModel>().get_selected_shape_index() as usize;
                    let shape_filter = ui
                        .global::<LibraryModel>()
                        .get_shape_options()
                        .row_data(shape_idx)
                        .unwrap_or_default();
                    let gear_idx = ui.global::<LibraryModel>().get_selected_gear_index() as usize;
                    let gear_filter = ui
                        .global::<LibraryModel>()
                        .get_gear_options()
                        .row_data(gear_idx)
                        .unwrap_or_default();
                    refresh_diagram_list_via_source(
                        &ui,
                        &db_meta,
                        &source_meta,
                        &search,
                        &shape_filter,
                        &gear_filter,
                    );
                }
                Err(e) => show_toast(&ui, &format!("Metadata update failed: {e}"), "error"),
            }
        },
    );
}
