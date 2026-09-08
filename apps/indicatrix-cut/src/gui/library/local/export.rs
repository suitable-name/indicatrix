//! Exporting a local design back out to an `.asc` file -- the "Export .asc" half of
//! `gui::library::local` (see this group's own `mod.rs`); import and organize live in
//! their own sibling modules.

use crate::{LibraryModel, MainWindow, bridge::library::source::LibrarySource, gui::show_toast};
use indicatrix_vault::{db::sqlite::Database, local};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Wires up "Export .asc": for a design with an original `.asc` attachment, exports
/// that file byte-for-byte (delegates to `gui::library::detail::export_diagram_file`,
/// the same path the Attachments tab's per-file export button already uses).
/// Otherwise rebuilds one from the stored angle-settings table via
/// `indicatrix_vault::local::reconstruct_asc_schedule` + `indicatrix_formats::asc::to_asc_string`
/// -- see that function's doc comment on why the result is marked `RECONSTRUCTED`.
///
/// Refuses outright while a remote library is being browsed -- same reasoning as
/// `super::organize::setup_rename_callback`/`setup_delete_callback`: the selected
/// entry id names a row in the remote catalogue, and `Database::get_diagram_full`
/// below must never be called with it against the LOCAL database.
pub fn setup_export_asc_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_export = Arc::clone(db);
    let source_export = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_export_asc(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if source_export
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_remote()
        {
            show_toast(
                &ui,
                "Switch to the local library to export a reconstructed .asc (use the Attachments tab to download a remote design's own files).",
                "error",
            );
            return;
        }
        let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
        if entry_id < 0 {
            show_toast(&ui, "No diagram selected for export.", "error");
            return;
        }

        let full = {
            let db = db_export
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            db.get_diagram_full(i64::from(entry_id))
        };
        let Ok(Some(full)) = full else {
            show_toast(&ui, "Diagram detail not found.", "error");
            return;
        };

        // Whether an already-parsed `.asc` attachment exists (byte-for-byte export,
        // same path `gui::library::detail::export_diagram_file` uses for the
        // Attachments tab) or one has to be reconstructed below, both branches now
        // write to a user-chosen destination rather than silently into `./exports/`
        // -- one Save As dialog shared by both, prompted BEFORE either does any work,
        // so cancelling writes nothing either way (same cancel rule as
        // `gui::library::detail::export_diagram_file_via_source`).
        let existing_name = full
            .attached_files
            .iter()
            .find(|f| f.name.to_lowercase().ends_with(".asc"))
            .map(|f| f.name.clone());
        let default_file_name =
            existing_name.clone().unwrap_or_else(|| format!("{}.asc", sanitize_filename(&full.title)));

        let mut dialog = rfd::FileDialog::new()
            .set_file_name(&default_file_name)
            .add_filter(".asc design", &["asc"]);
        let default_dir = std::path::Path::new("exports");
        if default_dir.is_dir() {
            dialog = dialog.set_directory(default_dir);
        }
        // Blocking `rfd::FileDialog`, invoked directly on the Slint UI/event-loop
        // thread -- see `apps/indicatrix-cut/Cargo.toml`'s `rfd` dependency comment for
        // why that's the supported way to call it here.
        let Some(dest_path) = dialog.save_file() else {
            let msg = "Export cancelled.".to_string();
            ui.global::<LibraryModel>().set_status_message(msg.clone().into());
            show_toast(&ui, &msg, "info");
            return;
        };

        if let Some(name) = existing_name {
            match crate::gui::library::detail::export_diagram_file(
                &db_export,
                i64::from(entry_id),
                &name,
                &dest_path,
            ) {
                Ok(msg) => {
                    ui.global::<LibraryModel>().set_status_message(msg.clone().into());
                    show_toast(&ui, &msg, "success");
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
            return;
        }

        let Some(schedule) = local::reconstruct_asc_schedule(
            &full.title,
            full.refractive_index.as_deref(),
            full.index_gear.as_deref(),
            &full.angle_settings,
        ) else {
            show_toast(&ui, "No cutting-schedule data to export for this diagram.", "error");
            return;
        };

        let text = indicatrix_formats::asc::to_asc_string(&schedule);
        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&dest_path, text) {
            Ok(()) => {
                let msg = format!(
                    "Exported reconstructed schedule to {} \
                     (mast distances are placeholders -- see the file's RECONSTRUCTED header).",
                    dest_path.display()
                );
                ui.global::<LibraryModel>().set_status_message(msg.clone().into());
                show_toast(&ui, &msg, "success");
            }
            Err(e) => show_toast(
                &ui,
                &format!("Failed to write {}: {e}", dest_path.display()),
                "error",
            ),
        }
    });
}

/// Strips characters Windows (and, incidentally, every other common filesystem)
/// disallows in a file name, so an arbitrary design title is always a safe file name.
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "design".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_replaces_reserved_characters() {
        assert_eq!(
            sanitize_filename("Round: Trichecker/12 \"Special\""),
            "Round_ Trichecker_12 _Special_"
        );
    }

    #[test]
    fn sanitize_filename_falls_back_for_empty_or_blank_titles() {
        assert_eq!(sanitize_filename(""), "design");
        assert_eq!(sanitize_filename("   "), "design");
    }
}
