//! Exporting a design's attached files, from either the local database or a
//! remote worker.

use crate::{
    LibraryModel, MainWindow,
    bridge::library::source::{self as library_source, LibrarySource},
    settings::WorkerSettings,
};
use indicatrix_net::library::{LibraryRequest, LibraryResponse};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Exports one attached file from the LOCAL database, by exact name match, to
/// `dest_path`. Returns a human-readable status line on success.
pub fn export_diagram_file(
    db_mutex: &Arc<Mutex<Database>>,
    entry_id: i64,
    file_name: &str,
    dest_path: &std::path::Path,
) -> Result<String, String> {
    let full_result = {
        let db = match db_mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        db.get_diagram_full(entry_id)
    };

    if let Ok(Some(full)) = full_result
        && let Some(f) = full.attached_files.iter().find(|af| af.name == file_name)
    {
        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(dest_path, &f.content).is_ok() {
            return Ok(format!("Saved '{}' to {}", f.name, dest_path.display()));
        }
    }
    Err(format!("Failed to export file '{file_name}'."))
}

/// Dispatches "export this attachment" to the LOCAL synchronous path ([`export_diagram_file`],
/// which reports through `ui.status_message`/a toast) or a background remote fetch,
/// depending on which library is active.
///
/// Unlike [`export_diagram_file`] this reports its own result directly onto `ui` rather
/// than returning one -- the remote path is unavoidably asynchronous (a `FetchAttachment`
/// round trip), so both branches report the same way for one consistent call
/// convention at the single call site (`gui::diagram_list::setup_diagram_selection_and_export_callbacks`).
///
/// Prompts for a destination with a native Save As dialog, seeded with the
/// attachment's own `file_name` under the existing `./exports/` default directory (if
/// it exists yet), BEFORE dispatching to either branch below -- one prompt shared by
/// both, so cancelling never writes a file either way.
///
/// The picker runs off the UI thread via `gui::pickers::pick`; this function reports
/// through `ui`/a toast rather than a return value, so its one caller,
/// `gui::diagram_list::setup_diagram_selection_and_export_callbacks`, needs no special
/// handling for that.
pub fn export_diagram_file_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    entry_id: i64,
    file_name: &str,
) {
    let default_dir = std::path::Path::new("exports");
    let extension_filter = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_string);
    let db_mutex = Arc::clone(db_mutex);
    let source = Arc::clone(source);
    let entry_id_owned = entry_id;
    let file_name_owned = file_name.to_string();
    crate::gui::pickers::pick(
        ui,
        crate::gui::pickers::PickerRequest {
            kind: crate::gui::pickers::PickerKind::SaveFile,
            title: None,
            filters: extension_filter
                .map(|ext| vec![crate::gui::pickers::PickerFilter::single(ext)])
                .unwrap_or_default(),
            default_file_name: Some(file_name_owned.clone()),
            starting_dir: default_dir.is_dir().then(|| default_dir.to_path_buf()),
        },
        move |ui, dest_path| {
            let Some(dest_path) = dest_path else {
                let msg = "Export cancelled.".to_string();
                ui.global::<LibraryModel>()
                    .set_status_message(msg.clone().into());
                crate::gui::show_toast(ui, &msg, "info");
                return;
            };

            let current = source
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            match current {
                LibrarySource::Local => {
                    match export_diagram_file(
                        &db_mutex,
                        entry_id_owned,
                        &file_name_owned,
                        &dest_path,
                    ) {
                        Ok(msg) => {
                            ui.global::<LibraryModel>()
                                .set_status_message(msg.clone().into());
                            crate::gui::show_toast(ui, &msg, "success");
                        }
                        Err(err) => {
                            ui.global::<LibraryModel>()
                                .set_status_message(err.clone().into());
                            crate::gui::show_toast(ui, &err, "error");
                        }
                    }
                }
                LibrarySource::Remote(worker) => {
                    export_diagram_file_remote(
                        ui,
                        worker,
                        entry_id_owned,
                        &file_name_owned,
                        &dest_path,
                    );
                }
            }
        },
    );
}

/// The remote counterpart of [`export_diagram_file`]. `FetchAttachment` identifies an
/// attachment by id, not name (see `indicatrix_net::library`'s module doc comment), and
/// `FileItem` -- what the attachments tab actually displays -- carries only a name, so
/// this re-fetches the design's metadata first to resolve `file_name` to an attachment
/// id, then fetches that attachment's bytes: two round trips for an occasional,
/// user-initiated export, not a cost paid by browsing itself. `dest_path` is already
/// resolved (the Save As dialog already ran, in [`export_diagram_file_via_source`]) --
/// this never prompts, it only writes.
fn export_diagram_file_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    entry_id: i64,
    file_name: &str,
    dest_path: &std::path::Path,
) {
    let file_name = file_name.to_string();
    let dest_path = dest_path.to_path_buf();
    let worker_for_attachment = worker.clone();
    library_source::spawn_library_request(
        ui.as_weak(),
        worker,
        LibraryRequest::FetchDesign { entry_id },
        move |ui, result| {
            let attachment_id = match result {
                Ok(LibraryResponse::Design(record)) => record
                    .attachments
                    .iter()
                    .find(|f| f.name == file_name)
                    .map(|f| f.id),
                _ => None,
            };
            let Some(attachment_id) = attachment_id else {
                report_export_failure(ui, &file_name);
                return;
            };
            let file_name_for_failure = file_name.clone();
            let dest_path_for_attachment = dest_path.clone();
            library_source::spawn_library_request(
                ui.as_weak(),
                worker_for_attachment,
                LibraryRequest::FetchAttachment { attachment_id },
                move |ui, result| match result {
                    Ok(LibraryResponse::Attachment { name, content }) => {
                        if let Some(parent) = dest_path_for_attachment.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if std::fs::write(&dest_path_for_attachment, &content).is_ok() {
                            let msg =
                                format!("Saved '{name}' to {}", dest_path_for_attachment.display());
                            ui.global::<LibraryModel>()
                                .set_status_message(msg.clone().into());
                            crate::gui::show_toast(ui, &msg, "success");
                        } else {
                            report_export_failure(ui, &file_name_for_failure);
                        }
                    }
                    _ => report_export_failure(ui, &file_name_for_failure),
                },
            );
        },
    );
}

fn report_export_failure(ui: &MainWindow, file_name: &str) {
    let msg = format!("Failed to export file '{file_name}'.");
    ui.global::<LibraryModel>()
        .set_status_message(msg.clone().into());
    crate::gui::show_toast(ui, &msg, "error");
}
