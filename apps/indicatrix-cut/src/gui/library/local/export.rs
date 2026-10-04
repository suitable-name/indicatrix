//! Exporting a local design back out to an `.asc` file -- the "Export .asc" half of
//! `gui::library::local` (see this group's own `mod.rs`) -- or, experimentally, to
//! a Gem Cut Studio `.gcs` file; import and organize live in their own sibling
//! modules.

use crate::{LibraryModel, MainWindow, bridge::library::source::LibrarySource, gui::show_toast};
use indicatrix_vault::{db::sqlite::Database, local};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// How many of `full`'s schedule rows are concave tiers (rows carrying a tool).
fn concave_row_count(full: &indicatrix_vault::model::entry::FullDiagramRecord) -> usize {
    full.angle_settings
        .iter()
        .filter(|row| row.tool.is_some())
        .count()
}

/// What exporting `full` as a tier list loses, as one confirm-dialog paragraph; `None`
/// for a planar design. The same words the editor's export says
/// (`ManufacturabilityWarning::ConcaveTiersOmittedFromExport`), so the two never drift.
fn concave_export_notice(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<String> {
    let count = concave_row_count(full);
    (count > 0).then(|| {
        indicatrix_cut_core::ManufacturabilityWarning::ConcaveTiersOmittedFromExport { count }
            .to_string()
    })
}

/// `full`'s schedule rows split for a reconstructed `.asc`: the flat rows (the tier
/// list) and one footnote pair per concave row, in the shape
/// `Design::append_concave_footnotes` writes (facet line, then the tool line). Without
/// the split a concave row would be written as a flat tier with a placeholder mast, a
/// facet the stone does not have.
fn split_concave_rows(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> (
    Vec<indicatrix_vault::model::angle::AngleSetting>,
    Vec<String>,
) {
    let single_line = |text: &str| -> String {
        text.split(['\r', '\n'])
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_owned()
    };
    let mut flat = Vec::with_capacity(full.angle_settings.len());
    let mut footnotes = Vec::new();
    for row in &full.angle_settings {
        if row.tool.is_none() {
            flat.push(row.clone());
            continue;
        }
        footnotes.push(single_line(&format!(
            "{}  {}  {}  {}",
            row.facet, row.angle, row.index, row.notes
        )));
        if let Some(line) = &row.tool_line {
            footnotes.push(single_line(line));
        }
    }
    (flat, footnotes)
}

/// Runs `proceed` straight away for a planar design, and after the cutter accepts the
/// loss of the concave tiers otherwise -- before any picker opens, so declining writes
/// nothing.
fn confirm_concave_loss_then(
    ui: &MainWindow,
    notice: Option<String>,
    proceed: impl FnOnce(&MainWindow) + 'static,
) {
    match notice {
        None => proceed(ui),
        Some(notice) => crate::gui::editor::native_io::ask_write_confirm(
            ui,
            "Concave tiers are not part of this format",
            format!("{notice}\n\nExport anyway?"),
            "Export Anyway",
            None,
            proceed,
        ),
    }
}

/// Wires up "Export .asc": for a design with an original `.asc` attachment, exports
/// the stored attachment byte-for-byte (delegates to
/// `gui::library::detail::export_diagram_file`, the same path the Attachments tab's
/// per-file export button already uses). What is stored is the file as it was imported
/// (`indicatrix_vault::local::import_asc_bytes` keeps the raw bytes, so a Windows-1252
/// file exports as Windows-1252); a design imported by an earlier version holds the UTF-8
/// text it was decoded to instead, and exports that. Every reader of the attachment
/// decodes it with `indicatrix_formats::asc::decode_asc_bytes`, which accepts both.
/// Otherwise rebuilds one from the stored angle-settings table via
/// `indicatrix_vault::local::reconstruct_asc_schedule` + `indicatrix_formats::asc::to_asc_string`
/// -- see that function's doc comment on why the result is marked `RECONSTRUCTED`.
///
/// Refuses outright while a remote library is being browsed -- same reasoning as
/// `super::organize::setup_rename_callback`/`setup_delete_callback`: the selected
/// entry id names a row in the remote catalogue, and `Database::get_diagram_full`
/// below must never be called with it against the LOCAL database.
///
/// Also wires up the experimental "Export as Gem Cut Studio (.gcs)..." beside it
/// (see [`setup_export_gcs_callback`]).
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

        // Whether a stored `.asc` attachment exists (exported unchanged, same path
        // `gui::library::detail::export_diagram_file` uses for the
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
        let default_file_name = existing_name.clone().unwrap_or_else(|| {
            // A `.gem`/`.gcs`-only design exports its converted cutting
            // instructions under `<stem>.asc` (see `finish_export_asc`).
            local::design_attachment_position(full.attached_files.iter().map(|f| f.name.as_str()))
                .map_or_else(
                    || format!("{}.asc", sanitize_filename(&full.title)),
                    |(position, _)| {
                        local::converted_asc_file_name(&full.attached_files[position].name)
                    },
                )
        });

        // The save-as picker runs off the UI thread via `gui::pickers::pick` --
        // everything below (the stored-bytes export or the reconstruct-and-write
        // path) stays synchronous, running once the picker's continuation lands
        // back on the UI thread.
        let default_dir = std::path::Path::new("exports");
        let db_for_pick = Arc::clone(&db_export);
        // Only the reconstruction loses anything: a stored `.asc` is exported as it
        // is (its concave tiers already are footnotes) and a `.gem`/`.gcs` cannot
        // hold a concave tier to begin with.
        let notice = (existing_name.is_none()
            && local::design_attachment_position(
                full.attached_files.iter().map(|f| f.name.as_str()),
            )
            .is_none())
        .then(|| concave_export_notice(&full))
        .flatten();
        confirm_concave_loss_then(&ui, notice, move |ui| {
            crate::gui::pickers::pick(
                ui,
                crate::gui::pickers::PickerRequest {
                    kind: crate::gui::pickers::PickerKind::SaveFile,
                    title: None,
                    filters: vec![crate::gui::pickers::PickerFilter {
                        label: ".asc design".to_string(),
                        extensions: vec!["asc".to_string()],
                    }],
                    default_file_name: Some(default_file_name),
                    starting_dir: default_dir.is_dir().then(|| default_dir.to_path_buf()),
                },
                move |ui, dest_path| {
                    finish_export_asc(
                        ui,
                        &db_for_pick,
                        entry_id,
                        &full,
                        existing_name,
                        dest_path,
                    );
                },
            );
        });
    });

    setup_export_gcs_callback(ui, db, source);
}

/// "Export as Gem Cut Studio (.gcs)..." (experimental) for the selected catalogue
/// design, registered by [`setup_export_asc_callback`] beside its `.asc`
/// counterpart and refusing a remote library for the same reason. The file text
/// comes from [`gcs_export_for_record`]; a design it cannot build is reported
/// before any picker opens.
fn setup_export_gcs_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_export = Arc::clone(db);
    let source_export = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_export_gcs(move || {
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
                "Switch to the local library to export a .gcs file.",
                "error",
            );
            return;
        }
        let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
        if entry_id < 0 {
            show_toast(&ui, "No diagram selected for export.", "error");
            return;
        }
        let full = db_export
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_diagram_full(i64::from(entry_id));
        let Ok(Some(full)) = full else {
            show_toast(&ui, "Diagram detail not found.", "error");
            return;
        };
        let export = match gcs_export_for_record(&full) {
            Ok(export) => export,
            Err(message) => {
                show_toast(&ui, &message, "error");
                return;
            }
        };
        let default_dir = std::path::Path::new("exports");
        // An original `.gcs` attachment is written byte for byte; one rebuilt from the
        // design's schedule has no place for a concave tier.
        let notice = (!export.is_original)
            .then(|| concave_export_notice(&full))
            .flatten();
        confirm_concave_loss_then(&ui, notice, move |ui| {
            crate::gui::pickers::pick(
                ui,
                crate::gui::pickers::PickerRequest {
                    kind: crate::gui::pickers::PickerKind::SaveFile,
                    title: None,
                    filters: vec![crate::gui::pickers::PickerFilter {
                        label: "Gem Cut Studio design (.gcs)".to_string(),
                        extensions: vec!["gcs".to_string()],
                    }],
                    default_file_name: Some(export.file_name.clone()),
                    starting_dir: default_dir.is_dir().then(|| default_dir.to_path_buf()),
                },
                move |ui, dest_path| finish_export_gcs(ui, &export, dest_path),
            );
        });
    });
}

/// What the catalogue's `.gcs` export writes for one design.
#[derive(Debug)]
struct GcsExport {
    /// The file name the save dialog proposes.
    file_name: String,
    /// The file's bytes.
    bytes: Vec<u8>,
    /// `true` when `bytes` is the design's own original `.gcs` attachment,
    /// `false` when it was written from the attached `.asc`.
    is_original: bool,
}

/// The `.gcs` file for a catalogue design: its own original `.gcs` attachment
/// byte-for-byte when it was imported from one, otherwise the cutting
/// instructions of its design-file attachment ([`design_attachment_as_asc`]: a
/// `.asc`, else a `.gem`, read as text with `indicatrix_formats::asc::decode_asc_bytes`
/// so a Windows-1252 `.asc` works) written by `indicatrix_formats::gcs::to_gcs_string`
/// (experimental).
///
/// # Errors
///
/// A toast-ready message when the design has no design-file attachment (the
/// angle-table reconstruction has no mast distances, and a `.gcs` needs the
/// solid), when that attachment does not read or parse, or when the writer
/// refuses the schedule.
fn gcs_export_for_record(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Result<GcsExport, String> {
    if let Some(original) = full.attached_files.iter().find(|f| {
        local::DesignFileKind::from_file_name(&f.name) == Some(local::DesignFileKind::Gcs)
    }) {
        return Ok(GcsExport {
            file_name: original.name.clone(),
            bytes: original.content.clone(),
            is_original: true,
        });
    }
    let file = design_attachment_as_asc(full).unwrap_or_else(|| {
        Err(
            "This design has no attached .asc or .gem file, so its mast distances are unknown \
             and no .gcs can be built from it. Load it in the editor, solve it, and use Export \
             as Gem Cut Studio (.gcs) there."
                .to_string(),
        )
    })?;
    let schedule = indicatrix_formats::asc::parse_asc(&file.asc_text).map_err(|e| {
        format!(
            "The attached '{}' could not be read: {e}",
            file.asc_file_name
        )
    })?;
    let gcs = indicatrix_formats::gcs::to_gcs_string(&schedule)
        .map_err(|e| format!("Cannot write this design as .gcs: {e}"))?;
    Ok(GcsExport {
        file_name: std::path::Path::new(&file.asc_file_name)
            .with_extension("gcs")
            .to_string_lossy()
            .into_owned(),
        bytes: gcs.into_bytes(),
        is_original: false,
    })
}

/// `full`'s design-file attachment, picked by
/// `indicatrix_vault::local::design_attachment_position` (the first `.asc`, else
/// the first `.gem`, else the first `.gcs` -- the rule every reader of a stored
/// design shares) and read as `.asc` text inside [`super::catch_file_panic`], the
/// importer's per-file guard.
///
/// `None` when the record has no design-file attachment; `Some(Err)` (a ready
/// message naming the attachment) when a `.gem`/`.gcs` does not read.
fn design_attachment_as_asc(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<Result<local::DesignFileText, String>> {
    let (position, kind) =
        local::design_attachment_position(full.attached_files.iter().map(|f| f.name.as_str()))?;
    let attached = &full.attached_files[position];
    let read = super::catch_file_panic(std::panic::AssertUnwindSafe(|| {
        local::design_file_to_asc_text(&attached.name, kind, &attached.content)
    }));
    Some(match read {
        Ok(result) => result.map_err(|e| format!("'{}': {e}", attached.name)),
        Err(panic_msg) => Err(format!("'{}': internal error: {panic_msg}", attached.name)),
    })
}

/// [`setup_export_gcs_callback`]'s picker continuation: writes `export` to
/// `dest_path` (`None` is a cancelled picker, reported like the `.asc` export's).
fn finish_export_gcs(ui: &MainWindow, export: &GcsExport, dest_path: Option<std::path::PathBuf>) {
    let Some(dest_path) = dest_path else {
        let msg = "Export cancelled.".to_string();
        ui.global::<LibraryModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "info");
        return;
    };
    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&dest_path, &export.bytes) {
        Ok(()) => {
            let msg = if export.is_original {
                format!(
                    "Exported the original .gcs file to {}.",
                    dest_path.display()
                )
            } else {
                format!(
                    "Exported {} (experimental Gem Cut Studio file -- check it in Gem Cut \
                     Studio before cutting from it).",
                    dest_path.display()
                )
            };
            ui.global::<LibraryModel>()
                .set_status_message(msg.clone().into());
            show_toast(ui, &msg, "success");
        }
        Err(e) => show_toast(
            ui,
            &format!("Failed to write {}: {e}", dest_path.display()),
            "error",
        ),
    }
}

/// Writes a `.gem`/`.gcs` design's converted cutting instructions
/// ([`design_attachment_as_asc`]) to `dest_path` for "Export .asc", or reports
/// why the attachment could not be read.
fn write_converted_asc(
    ui: &MainWindow,
    converted: Result<local::DesignFileText, String>,
    dest_path: &std::path::Path,
) {
    let file = match converted {
        Ok(file) => file,
        Err(e) => {
            show_toast(
                ui,
                &format!("This diagram's design file could not be read: {e}"),
                "error",
            );
            return;
        }
    };
    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(dest_path, file.asc_text) {
        Ok(()) => {
            let msg = format!(
                "Exported the cutting instructions converted from this design's .gem/.gcs \
                 file to {}.",
                dest_path.display()
            );
            ui.global::<LibraryModel>()
                .set_status_message(msg.clone().into());
            show_toast(ui, &msg, "success");
        }
        Err(e) => show_toast(
            ui,
            &format!("Failed to write {}: {e}", dest_path.display()),
            "error",
        ),
    }
}

/// [`setup_export_asc_callback`]'s own picker continuation -- split out purely
/// to keep that function under clippy's function-length lint. `dest_path` is
/// `None` for a cancelled/dismissed picker; every other parameter is exactly
/// what the callback body already had in scope before this split.
fn finish_export_asc(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    entry_id: i32,
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
    existing_name: Option<String>,
    dest_path: Option<std::path::PathBuf>,
) {
    let Some(dest_path) = dest_path else {
        let msg = "Export cancelled.".to_string();
        ui.global::<LibraryModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "info");
        return;
    };

    if let Some(name) = existing_name {
        match crate::gui::library::detail::export_diagram_file(
            db,
            i64::from(entry_id),
            &name,
            &dest_path,
        ) {
            Ok(msg) => {
                ui.global::<LibraryModel>()
                    .set_status_message(msg.clone().into());
                show_toast(ui, &msg, "success");
            }
            Err(e) => show_toast(ui, &e, "error"),
        }
        return;
    }
    // No `.asc` of its own, but a `.gem`/`.gcs`: its converted cutting
    // instructions carry the real masts the angle-table reconstruction lacks.
    if let Some(converted) = design_attachment_as_asc(full) {
        write_converted_asc(ui, converted, &dest_path);
        return;
    }

    let (flat_rows, concave_footnotes) = split_concave_rows(full);
    let schedule = match local::reconstruct_asc_schedule(
        &full.title,
        full.refractive_index.as_deref(),
        full.index_gear.as_deref(),
        &flat_rows,
    ) {
        Ok(Some(mut schedule)) => {
            schedule.footnotes.extend(concave_footnotes);
            schedule
        }
        Ok(None) => {
            show_toast(
                ui,
                "No cutting-instructions data to export for this diagram.",
                "error",
            );
            return;
        }
        Err(e) => {
            show_toast(
                ui,
                &format!("This diagram's cutting-instructions data could not be read: {e}"),
                "error",
            );
            return;
        }
    };

    let text = match indicatrix_formats::asc::to_asc_string(&schedule) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Cannot write this schedule as .asc: {e}"),
                "error",
            );
            return;
        }
    };
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
            ui.global::<LibraryModel>()
                .set_status_message(msg.clone().into());
            show_toast(ui, &msg, "success");
        }
        Err(e) => show_toast(
            ui,
            &format!("Failed to write {}: {e}", dest_path.display()),
            "error",
        ),
    }
}

/// Strips characters Windows (and, incidentally, every other common filesystem)
/// disallows in a file name, so an arbitrary design title is always a safe file name.
///
/// `pub`, not private (`export` is itself a private module, so this stays
/// crate-internal regardless -- `clippy::redundant_pub_crate` prefers plain `pub`
/// here over `pub(crate)` for exactly that reason): `gui::editor::native_io`'s own
/// Export/Save-Native dialogs reuse this exact rule for a slugged-first-
/// header default file name, so a title with a `:` or `/` in it (a real `GemCad`
/// header, e.g. "Round Brilliant: 57 facets") sanitizes identically whichever dialog
/// offered it.
pub fn sanitize_filename(title: &str) -> String {
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
    use indicatrix_vault::model::{entry::FullDiagramRecord, file::AttachedFile};

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

    /// A catalogue record with no metadata and the given attachments.
    fn record_with(attached_files: Vec<AttachedFile>) -> FullDiagramRecord {
        FullDiagramRecord {
            entry_id: 1,
            title: "Test Design".to_string(),
            url: "local://test.asc".to_string(),
            design_id: None,
            page_url: String::new(),
            diagram_image_name: None,
            diagram_image_data: None,
            competition_diagram: None,
            lw_ratio: None,
            refractive_index: None,
            index_gear: None,
            volume: None,
            facets_count: None,
            shape: None,
            designer_info: None,
            hw_ratio: None,
            tw_ratio: None,
            uw_ratio: None,
            pw_ratio: None,
            cw_ratio: None,
            symmetry_order: None,
            mirror_symmetry: None,
            designer: None,
            source_citation: None,
            pdf_file: None,
            gem_file: None,
            shape_category: None,
            angle_settings: Vec::new(),
            attached_files,
        }
    }

    /// The standard round brilliant's cutting instructions as `.asc` text, from
    /// the first New Design template.
    fn standard_round_brilliant_asc() -> (String, usize) {
        let spec = indicatrix_cut_core::templates::TEMPLATES
            .first()
            .expect("TEMPLATES is non-empty");
        assert_eq!(spec.name, "Standard Round Brilliant");
        let design = indicatrix_cut_core::design::Design::new(
            indicatrix_cut_core::preform::PreformSpec::cylinder(
                spec.gear_teeth.unsigned_abs() as usize,
                1.5,
                1.0,
                1.5,
            ),
            spec.schedule_meta(),
            spec.tiers(),
        );
        let schedule = design
            .to_asc_schedule()
            .expect("the standard round brilliant solves");
        let text = indicatrix_formats::asc::to_asc_string(&schedule).expect("writes as .asc");
        (text, schedule.tiers.len())
    }

    /// A `.asc` attachment holding the original Windows-1252 bytes (not valid UTF-8)
    /// reads as text through the reader every design-file consumer here shares.
    #[test]
    fn a_windows_1252_asc_attachment_reads_as_text() {
        let raw = b"GemCad 5.0\ng 96 0.0\ny 6 y\nI 1.72\nH Caf\xE9 \xB0\na 41.000000 0.5 92 n T\n"
            .to_vec();
        assert!(std::str::from_utf8(&raw).is_err());
        let full = record_with(vec![AttachedFile {
            name: "cafe.asc".to_string(),
            url: String::new(),
            content: raw,
        }]);
        let file = design_attachment_as_asc(&full)
            .expect("the record has a design file")
            .expect("a .asc always reads as text");
        assert_eq!(file.asc_file_name, "cafe.asc");
        assert!(
            file.asc_text.contains("Caf\u{e9} \u{b0}"),
            "{:?}",
            file.asc_text
        );
    }

    /// The catalogue `.gcs` export of the standard round brilliant parses back
    /// with the `.gcs` reader and keeps every tier.
    #[test]
    fn gcs_export_of_the_standard_round_brilliant_record_parses_back() {
        let (asc_text, tier_count) = standard_round_brilliant_asc();
        let full = record_with(vec![AttachedFile {
            name: "round.asc".to_string(),
            url: String::new(),
            content: asc_text.into_bytes(),
        }]);
        let export = gcs_export_for_record(&full).expect("a solved design exports");
        assert_eq!(export.file_name, "round.gcs");
        assert!(!export.is_original);
        let parsed =
            indicatrix_formats::gcs::parse_gcs_bytes(&export.bytes).expect("the .gcs parses back");
        assert_eq!(parsed.tiers.len(), tier_count);
    }

    /// A design imported from a `.gcs` exports that original file unchanged.
    #[test]
    fn gcs_export_prefers_the_original_gcs_attachment() {
        let full = record_with(vec![
            AttachedFile {
                name: "octabar.asc".to_string(),
                url: String::new(),
                content: b"GemCad 5.0\n".to_vec(),
            },
            AttachedFile {
                name: "octabar.gcs".to_string(),
                url: String::new(),
                content: b"<GemCutStudio version=\"1000\"/>".to_vec(),
            },
        ]);
        let export = gcs_export_for_record(&full).expect("the original is exported");
        assert!(export.is_original);
        assert_eq!(export.file_name, "octabar.gcs");
        assert_eq!(export.bytes, b"<GemCutStudio version=\"1000\"/>");
    }

    /// Without an attached `.asc` there are no masts, so no `.gcs` is built.
    #[test]
    fn gcs_export_refuses_a_design_with_no_attached_asc() {
        let err = gcs_export_for_record(&record_with(Vec::new()))
            .expect_err("no attached .asc means no masts");
        assert!(err.contains("no attached .asc"), "{err}");
    }
}
