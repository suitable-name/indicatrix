//! Wires the "Import" popup's native pickers to the import pipeline, and runs that
//! pipeline on its own background thread with progress marshalled back to the UI.

use super::{
    confirm::{confirm_collisions_then, count_pending_collisions},
    folder_memory::{
        last_import_directory, remember_import_directory, reset_import_progress,
        spawn_save_folder_rescan,
    },
    pipeline::{ImportOutcome, import_path},
};
use crate::{
    LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{show_toast, tutorial_events::raise},
    settings::SettingsPersister,
};
use indicatrix_editor::guide::viewing_events as events;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

/// Runs [`import_path`] on its own worker thread and marshals every UI update back
/// through `upgrade_in_event_loop` -- the same idiom
/// `bridge::export_thread::spawn_export` + `gui::render::render_export` use for a
/// long-running operation with progress. `db`/`source` are cloned `Arc`s, cheap to
/// move onto the thread.
///
/// A `BusyGuard` local to this closure clears `is_busy` in its `Drop` impl,
/// unconditionally, rather than only from the success closure below: that guarantees
/// `is_busy` is cleared even if a panic somewhere in this thread skips past the normal
/// completion path, so the UI can never get stuck busy forever. A `Drop` guard is used
/// instead of `catch_unwind`-ing the whole closure or resetting on every return path
/// because it can't be skipped by a `continue`/early `return`/panic added later --
/// there is exactly one exit path. (`super::pipeline::catch_file_panic` inside
/// `import_path` is a separate, complementary fix: it stops a bad file from reaching
/// this point as a panic at all, so the batch keeps processing the rest of the files
/// too.)
fn spawn_import(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    source: Arc<Mutex<LibrarySource>>,
    path: PathBuf,
    recurse: bool,
) {
    thread::spawn(move || {
        struct BusyGuard(Weak<MainWindow>);
        impl Drop for BusyGuard {
            fn drop(&mut self) {
                let ui_weak = self.0.clone();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<LibraryModel>().set_is_busy(false);
                });
            }
        }
        let _busy_guard = BusyGuard(ui_weak.clone());

        let progress_ui_weak = ui_weak.clone();
        let ImportOutcome {
            summary: message,
            imported_ids,
            had_failures,
            had_collision,
            had_notes,
        } = import_path(&db, &path, recurse, move |done, total| {
            let _ = progress_ui_weak.upgrade_in_event_loop(move |ui| {
                ui.global::<LibraryModel>().set_import_done(done as i32);
                ui.global::<LibraryModel>().set_import_total(total as i32);
                ui.global::<LibraryModel>()
                    .set_import_progress(done as f32 / total as f32);
                ui.global::<LibraryModel>()
                    .set_status_message(format!("Importing {done} / {total}...").into());
            });
        });
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<LibraryModel>()
                .set_import_result_text(message.clone().into());
            ui.global::<LibraryModel>()
                .set_status_message(message.clone().into());
            // Derived from the actual failure count, not from
            // sniffing `message`'s text -- see `ImportOutcome::had_failures`'s own
            // doc comment for why sniffing the text would misreport a partly-failed
            // batch as a plain success.
            let toast_kind = if had_failures { "error" } else { "success" };
            show_toast(&ui, &message, toast_kind);
            // Keep the import popup open on completion (instead
            // of auto-closing into the toast, where a failure list becomes
            // unreadable within 3.5s) whenever there is something worth reading --
            // a failure list, a collision warning, or a `.gem`/`.gcs` converter
            // note, all already in `message`/`import_result_text` above -- driving
            // `import_dialog.slint`'s `changed importing` handler.
            ui.global::<LibraryModel>()
                .set_import_should_stay_open(had_failures || had_collision || had_notes);
            // For more than one imported id, set
            // `recent_import_filter` to exactly those ids BEFORE
            // `refresh_after_library_change` runs, so the very refresh this import
            // triggers already shows only the just-imported rows -- no separate
            // "show these N" click needed, and no race against the async refresh
            // that a later, separate mutation would have (see
            // `gui::library::search::read_id_filter`'s own doc comment for how this
            // property is cleared again the moment the cutter makes any real
            // search/filter change). The single-id case is left alone: the full
            // list stays visible and `invoke_select_diagram` below opens the one
            // new row directly, which is more useful than narrowing the list to a
            // single row.
            let imported_id_items: Vec<i32> = imported_ids
                .iter()
                .filter_map(|&id| i32::try_from(id).ok())
                .collect();
            ui.global::<LibraryModel>().set_id_filter_label("".into());
            if imported_id_items.len() > 1 {
                ui.global::<LibraryModel>()
                    .set_recent_import_filter(ModelRc::new(VecModel::from(imported_id_items)));
            } else {
                ui.global::<LibraryModel>()
                    .set_recent_import_filter(ModelRc::new(VecModel::from(Vec::<i32>::new())));
            }
            super::super::helpers::refresh_after_library_change(&ui, &db, &source);
            // The common case (a single design file picked via "Choose file...")
            // knows exactly which row it just created; without this, the cutter would
            // have to scroll a possibly-large list looking for their own
            // title. `invoke_select_diagram` re-runs the exact same path a click on
            // the row itself takes (`setup_diagram_selection_and_export_callbacks`,
            // `diagram_list.rs`), so the detail pane opens on it immediately.
            if let [only_id] = imported_ids.as_slice()
                && let Ok(id) = i32::try_from(*only_id)
            {
                ui.global::<LibraryModel>().invoke_select_diagram(id);
            }
            // A tutorial step may wait for designs to be imported.
            if !imported_ids.is_empty() {
                raise(&ui, events::LIBRARY_IMPORTED);
            }
            // Asks whether to generate previews for what was just imported -- shares
            // the same confirm-step dialog as the missing-previews library scan; see
            // `preview::offer_batch_confirmation`'s doc comment. A no-op when nothing
            // was actually imported.
            crate::gui::batch::preview::offer_import_previews(&ui, &imported_ids);
        });
        // `_busy_guard` drops here: on a normal return, right after the completion
        // closure above is enqueued (not necessarily run yet), so the `is_busy` reset
        // is queued right behind it; on an unwind, it drops during that unwind instead.
    });
}

/// Wires up the "Import" popup's native pickers (`import_dialog.slint`'s "Choose
/// file..."/"Choose folder..." buttons).
///
/// Picking a target runs [`count_pending_collisions`] first: with no filename
/// collision against the existing catalogue, [`begin_import`] starts immediately, off
/// the UI thread (see [`spawn_import`]). When at
/// least one candidate WOULD replace an existing row, [`confirm_collisions_then`]
/// shows a synchronous native Yes/No dialog ("up-front confirmation before replacing")
/// before anything is imported; declining leaves the
/// catalogue untouched, same as cancelling the picker itself does. Cancelling the
/// native picker does nothing at all: no import, no error, the popup stays open
/// (`pick_file`/`pick_folder` return `None`, and both handlers just fall through).
/// The folder picker also carries a `bool` -- whether to recurse into subfolders --
/// from `import_dialog.slint`'s toggle at the moment the button was clicked.
///
/// Also kicks off [`spawn_save_folder_rescan`] --
/// "once per session" falls out of this function itself only ever being called once,
/// from `gui::build_main_window`, the same way `gui::batch::preview::setup_preview_batch_callbacks`'s
/// own once-per-session missing-previews scan does.
///
/// Always writes to the LOCAL database regardless of which library is currently being
/// browsed -- import is inherently a local-only operation, so it needs no `source`
/// guard the way rename/delete do; `source` is only threaded through so
/// `super::super::helpers::refresh_after_library_change` keeps showing whichever
/// library was already on screen afterwards.
pub fn setup_import_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let db_file = Arc::clone(db);
    let source_file = Arc::clone(source);
    let settings_file = Arc::clone(settings_store);
    let ui_weak_file = ui.as_weak();
    ui.global::<LibraryModel>().on_pick_asc_file(move || {
        let Some(ui) = ui_weak_file.upgrade() else {
            return;
        };
        // The picker runs off the UI thread via `gui::pickers::pick` -- only its own
        // continuation (collision pre-scan, confirm dialog, import dispatch)
        // runs after; the import itself runs on its own thread regardless.
        let db_file = Arc::clone(&db_file);
        let source_file = Arc::clone(&source_file);
        let settings_file = Arc::clone(&settings_file);
        let ui_weak_file = ui_weak_file.clone();
        crate::gui::pickers::pick(
            &ui,
            crate::gui::pickers::PickerRequest {
                kind: crate::gui::pickers::PickerKind::OpenFile,
                title: None,
                filters: vec![crate::gui::pickers::PickerFilter {
                    label: "Faceting design (.asc, .gem, .gcs)".to_string(),
                    extensions: vec!["asc".to_string(), "gem".to_string(), "gcs".to_string()],
                }],
                default_file_name: None,
                starting_dir: last_import_directory(&settings_file),
            },
            move |ui, path| {
                let Some(path) = path else {
                    return;
                };
                remember_import_directory(&settings_file, path.parent());
                let (collisions, total) = count_pending_collisions(&db_file, &path, false);
                let db_file = Arc::clone(&db_file);
                let source_file = Arc::clone(&source_file);
                confirm_collisions_then(ui, collisions, total, move |ui| {
                    begin_import(ui, &db_file, &source_file, ui_weak_file, path, false);
                });
            },
        );
    });

    let db_folder = Arc::clone(db);
    let source_folder = Arc::clone(source);
    let settings_folder = Arc::clone(settings_store);
    let ui_weak_folder = ui.as_weak();
    // `recurse` is `import_dialog.slint`'s "Include subfolders" toggle, read at the
    // moment "Choose folder..." was clicked -- it must be set before the click since
    // the picker opens (and, absent a collision, the import starts) in this same
    // callback.
    ui.global::<LibraryModel>()
        .on_pick_asc_folder(move |recurse: bool| {
            let Some(ui) = ui_weak_folder.upgrade() else {
                return;
            };
            // Same off-UI-thread picker as `on_pick_asc_file` above.
            let db_folder = Arc::clone(&db_folder);
            let source_folder = Arc::clone(&source_folder);
            let settings_folder = Arc::clone(&settings_folder);
            let ui_weak_folder = ui_weak_folder.clone();
            crate::gui::pickers::pick(
                &ui,
                crate::gui::pickers::PickerRequest {
                    kind: crate::gui::pickers::PickerKind::PickFolder,
                    title: None,
                    filters: Vec::new(),
                    default_file_name: None,
                    starting_dir: last_import_directory(&settings_folder),
                },
                move |ui, path| {
                    let Some(path) = path else {
                        return;
                    };
                    // The chosen folder itself, not its parent: the next import is far
                    // more likely to be another file from inside it than a sibling
                    // folder.
                    remember_import_directory(&settings_folder, Some(path.as_path()));
                    let (collisions, total) = count_pending_collisions(&db_folder, &path, recurse);
                    let db_folder = Arc::clone(&db_folder);
                    let source_folder = Arc::clone(&source_folder);
                    confirm_collisions_then(ui, collisions, total, move |ui| {
                        begin_import(
                            ui,
                            &db_folder,
                            &source_folder,
                            ui_weak_folder,
                            path,
                            recurse,
                        );
                    });
                },
            );
        });

    spawn_save_folder_rescan(ui.as_weak(), Arc::clone(db), Arc::clone(settings_store));
}

/// The actual "start importing now" step -- [`setup_import_callback`]'s pickers call
/// this once the collision confirmation (if any) has cleared.
fn begin_import(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    ui_weak: Weak<MainWindow>,
    path: PathBuf,
    recurse: bool,
) {
    ui.global::<LibraryModel>().set_is_busy(true);
    ui.global::<LibraryModel>()
        .set_status_message("Importing...".into());
    reset_import_progress(ui);
    spawn_import(ui_weak, Arc::clone(db), Arc::clone(source), path, recurse);
}
