//! File-based import/export for the Edit tab: exporting the edited schedule as a
//! plain `.asc` ([`setup_export_asc_callback`]), and the paired native
//! `.indicatrix.toml` save/open ([`setup_save_native_callback`]/
//! [`setup_open_native_callback`]; legacy `.gemcut.toml` sidecars still open). See
//! this group's own `mod.rs` doc comment.

use super::{
    callbacks::clear_analysis_results,
    state::{EditorState, PendingUnsavedAction, PushedScratch},
    view::refresh_all,
};
use crate::{
    EditorModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix_cut_core::{
    Design, FingerprintCheck, History, LoadPairedResult, NativeDesignFile, TierOverlay,
    load_paired, native_path_for_asc, save_paired,
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tracing::warn;

/// The file name an Export/Save Native dialog should default to -- Item 176: neither
/// dialog previously read anything about the design actually open, so ten exports in
/// a row all proposed the identical literal `"edited_design.asc"`. Prefers
/// `asc_filename` (this design's own recorded/last-saved name -- see
/// [`super::state::EditorState::asc_filename`]'s own doc comment) when set, else a
/// sanitized version of the schedule's own first free-text header line (the `GemCad`
/// convention for a design's title/description), else `"edited_design.asc"` for a
/// design with neither (a brand-new "New Design" with no header typed yet). Shared by
/// [`setup_export_asc_callback`] and [`setup_save_native_callback`] so the two
/// dialogs never again drift apart the way the audit that raised this item found them
/// (Export hard-coded, Save Native already read `asc_filename`).
fn suggested_file_name(st: &EditorState) -> String {
    if let Some(name) = &st.asc_filename {
        return name.clone();
    }
    match st.design.meta.headers.first() {
        Some(header) if !header.trim().is_empty() => {
            format!(
                "{}.asc",
                crate::gui::library::local::sanitize_filename(header)
            )
        }
        _ => "edited_design.asc".to_string(),
    }
}

/// Seeds `dialog`'s starting directory from `./exports` when that folder exists --
/// the same convention `gui::library::local::export::setup_export_asc_callback`/
/// `gui::library::detail::export_diagram_file_via_source` already use (and the one
/// `docs/manual/11-saving-and-file-formats.md` promises), which neither of this
/// module's own dialogs previously followed (Item 176). A no-op (returns `dialog`
/// unchanged) when the folder doesn't exist, so the OS's own last-used-directory
/// memory still applies for a cutter who has never created one.
fn seed_default_export_dir(dialog: rfd::FileDialog) -> rfd::FileDialog {
    let default_dir = std::path::Path::new("exports");
    if default_dir.is_dir() {
        dialog.set_directory(default_dir)
    } else {
        dialog
    }
}

/// [`confirm_status_before_write`]'s outcome.
enum StatusConfirm {
    /// `design.status()` names no problem -- write unchanged.
    Fine,
    /// A real problem existed and the cutter chose to proceed anyway -- write, but
    /// stamp this message into the schedule's own headers first (see
    /// [`degenerate_marker_header`]) so the file itself carries the same warning the
    /// confirmation dialog just showed.
    ConfirmedWithReason(String),
    /// A real problem existed and the cutter declined.
    Declined,
}

/// The leading header [`StatusConfirm::ConfirmedWithReason`] stamps into a written
/// schedule -- Item 182's "the file itself carries no hint." A distinct marker from
/// [`indicatrix_formats::asc::mark_reconstructed`]'s own "RECONSTRUCTED" line: that one is
/// specific to an angle-table placeholder reconstruction (Item 82), a different
/// situation from a design that solves but does not close a real stone.
const NOT_CLOSED_SOLID_MARKER: &str = "NOT A CLOSED SOLID";

/// Builds the header line itself, or `None` when `headers` already starts with one
/// (never stamped twice, mirroring `mark_reconstructed`'s own idempotence).
fn degenerate_marker_header(headers: &[String], message: &str) -> Option<String> {
    let already_marked = headers
        .first()
        .is_some_and(|h| h.starts_with(NOT_CLOSED_SOLID_MARKER));
    (!already_marked).then(|| format!("{NOT_CLOSED_SOLID_MARKER} -- {message}"))
}

/// Item 182: a design that solves but does not close a real stone (every mast `0.0`
/// after an angle-table placeholder load is the common way this happens, but a
/// half-built schedule mid-session hits it too) used to export/save as a
/// normal-looking file with no signal beyond a validation banner the cutter may have
/// scrolled past. Checks `design.status()` -- reusing the Edit tab's own
/// validation-banner wording (`state::status_text_and_is_problem`) so this asks about
/// exactly the same problem the cutter already saw on screen, never a second,
/// differently-worded description of it -- and if it names a real problem, asks via a
/// native OS Yes/No dialog (same reasoning as [`setup_save_native_callback`]'s own
/// overwrite confirmation, Item 171: neither of this lane's `.slint` files owns a
/// place to put a new modal for a decision `native_io.rs` alone knows needs asking).
fn confirm_status_before_write(design: &Design, action: &str) -> StatusConfirm {
    let (message, is_problem) = super::state::status_text_and_is_problem(design);
    if !is_problem {
        return StatusConfirm::Fine;
    }
    let choice = rfd::MessageDialog::new()
        .set_title("This design is not a closed solid")
        .set_description(format!(
            "{message}\n\n{action} anyway? The written file will note this in its own header."
        ))
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    if choice == rfd::MessageDialogResult::Yes {
        StatusConfirm::ConfirmedWithReason(message)
    } else {
        StatusConfirm::Declined
    }
}

/// Stamps (or clears) `footnotes`' own recorded source-catalogue-row marker before a
/// `.asc` is written -- CAD audit item 186's "provenance from a recorded id, never a
/// title/filename guess." Removes any previous stamp first (idempotent, same
/// reasoning as [`degenerate_marker_header`]) so a design that changes source row
/// (Save Native creating its very first row) or loses one (the row was deleted) never
/// carries two stamps, or a stale one, across a later save. `gui::library::local::
/// import::save_imported_design` is the reader: a `.asc` re-imported later recovers
/// `source_entry_id` from exactly this line via
/// [`indicatrix_vault::local::parse_source_entry_footnote`] and records it as
/// `diagram_entries.derived_from_entry_id`, turning what would otherwise be a second
/// same-titled row into a recorded version of the original.
///
/// Called from both "Export .asc" and "Save Native": a bare Export with no native
/// sidecar at all is exactly item 186's "export-then-reimport" scenario, so the plain
/// `.asc` itself has to carry this, not only the native sidecar (which already
/// records everything else about this design, but is never attached to a plain
/// Export).
fn stamp_source_entry_footnote(footnotes: &mut Vec<String>, source_entry_id: Option<i64>) {
    footnotes.retain(|f| !f.starts_with(indicatrix_vault::local::SOURCE_ENTRY_FOOTNOTE_PREFIX));
    if let Some(id) = source_entry_id {
        footnotes.push(indicatrix_vault::local::format_source_entry_footnote(id));
    }
}

/// [`write_back_to_catalogue`]'s outcome, for [`finish_save_native_success`]'s own
/// toast wording -- the ordinary "updated existing row" case gets no extra toast at
/// all (the file-save toast already said enough), while the other two are surprising
/// enough on their own to call out.
enum CatalogueWriteBack {
    /// The design's own known `source_entry_id` still named a real row -- updated in
    /// place.
    UpdatedExisting,
    /// This design had no known source row -- a brand-new one was inserted.
    NewRow,
    /// This design's known `source_entry_id` no longer named a real row (deleted
    /// while the design was open) -- a brand-new row was inserted instead of an
    /// update, same as [`Self::NewRow`], but worth telling the cutter about.
    SourceRowGoneNewRowCreated,
}

/// CAD audit items 92/96/186's catalogue write-back -- called by
/// [`finish_save_native_success`] once [`write_pair_atomically`] has already
/// succeeded (never before: the files on disk are the design of record, so a
/// catalogue-write failure here must never be read as "the save failed").
///
/// Builds the exact same [`indicatrix_vault::local::ImportedAsc`] an Import of these
/// same two just-written files would build
/// ([`indicatrix_vault::local::import_asc`]) plus the same measured proportions/shape
/// an import derives ([`crate::gui::library::local::apply_measured_metadata`]) --
/// this is deliberately the SAME parse-and-measure path, not a second, independently
/// maintained one, so a design's catalogue row always describes it exactly as
/// re-importing the same two files would.
///
/// # What gets overwritten versus preserved
///
/// - `source_entry_id: Some(id)`, `id` still a real row: `angle_settings_table`/
///   `attached_files` (the `.asc` + native sidecar) and every geometry-derived
///   column (`refractive_index`/`index_gear`/`facets_count`/`symmetry_order`/
///   `mirror_symmetry`/the measured `lw`/`hw`/`cw`/`pw`/`volume` ratios/`shape`) are
///   always replaced with this save's own fresh values. Everything else --
///   `designer_info`, a hand-corrected `shape` override, the competition/citation/
///   scrape-only columns -- is merged forward from the existing row first
///   ([`crate::gui::library::local::merge_reimport_metadata`], the SAME rule CAD
///   audit item 97 already applies to a `.asc` re-import), so a cutter's hand-typed
///   metadata survives a Save exactly as it survives an Import. `title` is left
///   untouched entirely (`Database::update_diagram_entry_url` never touches it --
///   same precedent as `Database::update_diagram_metadata`): a title is something a
///   cutter hand-corrects, never something a geometry write-back should silently
///   rename. Previews and tilt curves are invalidated (deleted, to be regenerated on
///   demand) -- they describe the geometry as it was before this save, same as a
///   `.asc` re-import collision already does.
/// - `source_entry_id: Some(id)`, but `id` no longer names a real row (deleted while
///   this design was open): falls through to the next case, exactly as if
///   `source_entry_id` had been `None`, so this save still lands somewhere instead
///   of silently failing or resurrecting a deleted row.
/// - `source_entry_id: None`: this design has never been saved to the catalogue --
///   inserts a brand-new row (`Database::save_diagram_entry` + `save_diagram_detail`,
///   no merge: there is nothing existing to preserve).
///
/// Returns the row's id on success, so the caller can write it back into
/// [`EditorState::source_entry_id`] -- every save after the FIRST one for a
/// previously row-less design must update that SAME new row, never insert a second.
///
/// # Errors
///
/// A ready-to-toast message. A failure here never rolls back the files
/// [`write_pair_atomically`] already wrote -- the design is safely on disk either
/// way; this only affects whether the library list reflects it yet.
fn write_back_to_catalogue(
    db: &Arc<Mutex<Database>>,
    source_entry_id: Option<i64>,
    asc_filename: &str,
    asc_text: &str,
    native_filename: &str,
    native_toml: &str,
) -> Result<(i64, CatalogueWriteBack), String> {
    let mut parsed = indicatrix_vault::local::import_asc(
        asc_filename,
        asc_text,
        Some((native_filename, native_toml.as_bytes())),
    )
    .map_err(|e| format!("Saved to disk, but could not update the catalogue: {e}"))?;
    crate::gui::library::local::apply_measured_metadata(&mut parsed.detail);

    // The lock covers the database work and nothing else: the caller goes on to
    // toast and refresh the library list, and holding the catalogue mutex across
    // that would serialise it against every other reader.
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_back_locked(&db, source_entry_id, &mut parsed)
}

/// [`write_back_to_catalogue`]'s database half, with the lock already taken --
/// split out so the guard's scope is exactly this call rather than the remainder of
/// its caller.
///
/// Returns the row id written and which of the three cases happened: the design's
/// own row updated in place, its row found missing and a fresh one inserted, or a
/// first-ever insert for a design that had no row.
fn write_back_locked(
    db: &Database,
    source_entry_id: Option<i64>,
    parsed: &mut indicatrix_vault::local::ImportedAsc,
) -> Result<(i64, CatalogueWriteBack), String> {
    if let Some(existing_id) = source_entry_id {
        match db.get_diagram_full(existing_id) {
            Ok(Some(existing)) => {
                crate::gui::library::local::merge_reimport_metadata(&mut parsed.detail, &existing);
                db.update_diagram_entry_url(existing_id, &parsed.entry.url)
                    .map_err(|e| e.to_string())?;
                db.save_diagram_detail(&parsed.detail, existing_id)
                    .map_err(|e| e.to_string())?;
                if let Err(e) = db.delete_preview_images(existing_id) {
                    warn!(
                        "Save Native: failed to invalidate stale preview cache for entry \
                         #{existing_id}: {e}"
                    );
                }
                if let Err(e) = db.delete_tilt_curves(existing_id) {
                    warn!(
                        "Save Native: failed to invalidate stale tilt-curve cache for entry \
                         #{existing_id}: {e}"
                    );
                }
                return Ok((existing_id, CatalogueWriteBack::UpdatedExisting));
            }
            Ok(None) => {
                // Source row deleted while this design was open -- fall through to
                // insert a fresh row below, rather than updating nothing or erroring.
            }
            Err(e) => {
                return Err(format!(
                    "Saved to disk, but could not read this design's catalogue row to \
                     update it: {e}"
                ));
            }
        }
    }

    let new_id = db
        .save_diagram_entry(&parsed.entry, indicatrix_vault::local::LOCAL_SOURCE_ID)
        .map_err(|e| e.to_string())?;
    db.save_diagram_detail(&parsed.detail, new_id)
        .map_err(|e| e.to_string())?;
    let outcome = if source_entry_id.is_some() {
        CatalogueWriteBack::SourceRowGoneNewRowCreated
    } else {
        CatalogueWriteBack::NewRow
    };
    Ok((new_id, outcome))
}

/// "Export Cutting Sheet": the printable HTML sheet (CAD audit item 109).
///
/// Solves here rather than reusing a cached solve: the sheet states masts a cutter
/// will set a mast gauge to, so it must describe the schedule as it is right now,
/// not as it was when something last happened to solve it. A design that does not
/// solve has no masts to print, and says so rather than printing zeros.
pub(super) fn setup_export_cutting_sheet_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_cutting_sheet(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let (design, base_name) = {
            let st = state.borrow();
            (st.design.clone(), suggested_file_name(&st))
        };
        let solved = match design.solve() {
            Ok(solved) => solved,
            Err(missing) => {
                show_toast(
                    &ui,
                    &format!("Cannot build a cutting sheet: {missing}."),
                    "error",
                );
                return;
            }
        };
        let base = base_name.trim_end_matches(".asc");
        match super::cut_sheet::write_cutting_sheet_html(&design, &solved, base) {
            Ok(Some(path)) => show_toast(
                &ui,
                &format!("Wrote cutting sheet to {}", path.display()),
                "success",
            ),
            // A dismissed dialog is the cutter's own cancel -- no toast (item 244).
            Ok(None) => {}
            Err(message) => show_toast(&ui, &message, "error"),
        }
    });
}

/// "Export Diagram": the 2D crown/pavilion/profile drawing as a PNG (CAD audit
/// item 214) -- the same render the cutting sheet embeds, written on its own.
pub(super) fn setup_export_diagram_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_diagram(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let (design, base_name) = {
            let st = state.borrow();
            (st.design.clone(), suggested_file_name(&st))
        };
        let solved = match design.solve() {
            Ok(solved) => solved,
            Err(missing) => {
                show_toast(
                    &ui,
                    &format!("Cannot draw this design: {missing}."),
                    "error",
                );
                return;
            }
        };
        let base = base_name.trim_end_matches(".asc");
        match super::cut_sheet::write_diagram_png(&design, &solved, base) {
            Ok(Some(path)) => {
                show_toast(
                    &ui,
                    &format!("Wrote diagram to {}", path.display()),
                    "success",
                );
            }
            Ok(None) => {}
            Err(message) => show_toast(&ui, &message, "error"),
        }
    });
}

/// "Export .asc": writes the edited schedule (never the catalogue's own, unedited
/// one -- see this group's `mod.rs` doc comment) to a user-chosen path via
/// `indicatrix_formats::to_asc_string`. File only, no database write of any kind --
/// the catalogue stays read-only on this path.
pub(super) fn setup_export_asc_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_asc(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // Snapshot the schedule text, then drop the lock before the blocking native
        // file dialog -- same "don't hold a mutex across a blocking dialog call"
        // discipline every other export/import path in this crate follows (see e.g.
        // `gui::library::local::export::setup_export_asc_callback`). `to_asc_schedule`
        // is now fallible (it solves every tier's mast -- see
        // `Design::to_asc_schedule`'s doc comment): a `MissingAnchor` here means the
        // same "add a Scale Reference tier" problem the validation banner already
        // reports, so it is surfaced the same way (a toast) rather than exporting a
        // schedule with fabricated masts.
        let (mut schedule, default_file_name, design, used_placeholder) = {
            let st = state.borrow();
            let mut schedule = match st.design.to_asc_schedule() {
                Ok(schedule) => schedule,
                Err(missing) => {
                    show_toast(&ui, &format!("Cannot export: {missing}."), "error");
                    return;
                }
            };
            // CAD audit item 186: recorded so a LATER re-import of this exact file
            // can match it back to its source row instead of creating a second,
            // same-titled duplicate -- see `stamp_source_entry_footnote`'s own doc
            // comment.
            stamp_source_entry_footnote(&mut schedule.footnotes, st.source_entry_id);
            (
                schedule,
                suggested_file_name(&st),
                st.design.clone(),
                st.used_placeholder,
            )
        };
        // Item 82: same marker Save Native writes, for the plain-export path -- a
        // schedule whose masts were fabricated by the angle-table reconstruction
        // must say so in the file itself, not only in the app that wrote it.
        if used_placeholder {
            indicatrix_formats::asc::mark_reconstructed(
                &mut schedule,
                "angle-table reconstruction, no attached .asc",
            );
        }
        // Item 182: `to_asc_schedule` succeeding only means every tier solved SOME
        // mast, never that those masts actually close a real solid -- see
        // `confirm_status_before_write`'s own doc comment.
        match confirm_status_before_write(&design, "Export") {
            StatusConfirm::Fine => {}
            StatusConfirm::ConfirmedWithReason(message) => {
                if let Some(header) = degenerate_marker_header(&schedule.headers, &message) {
                    schedule.headers.insert(0, header);
                }
            }
            // Item 179: a declined confirmation is the cutter's own explicit
            // cancel, same reasoning as the dismissed-picker case just below
            // (cad_todo.md #244) -- no toast needed for it either.
            StatusConfirm::Declined => {
                return;
            }
        }
        let text = indicatrix_formats::asc::to_asc_string(&schedule);

        let dest_path = seed_default_export_dir(
            rfd::FileDialog::new()
                .set_file_name(&default_file_name)
                .add_filter(".asc design", &["asc"]),
        )
        .save_file();
        // cad_todo.md #244: a dismissed file dialog is the cutter's own deliberate
        // cancel -- no toast needed to confirm an action they just performed, and
        // the old one could silently replace an error toast still waiting to be
        // read (see `gui::mod`'s own auto-dismiss scheduling).
        let Some(dest_path) = dest_path else {
            return;
        };

        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&dest_path, text) {
            Ok(()) => {
                record_last_saved_path(&ui, &dest_path);
                show_toast(
                    &ui,
                    &format!("Exported edited schedule to {}", dest_path.display()),
                    "success",
                );
            }
            Err(e) => show_toast(
                &ui,
                &format!("Failed to write {}: {e}", dest_path.display()),
                "error",
            ),
        }
    });
}

/// "Save Native": writes the design's `.indicatrix.toml` sidecar ALONGSIDE a
/// real `.asc` export -- never instead of one. That pairing is the module's own hard
/// rule (see `indicatrix_cut_core::native`'s module doc comment): a design must never exist only
/// in a format its author can be locked out of, so this always writes both files,
/// never the native file alone.
///
/// `indicatrix_cut_core::save_paired` decides whether the `.asc` half can stay byte-identical to
/// `state.original_asc_text` (an untouched design, or one whose only edits were
/// `girdle_diameter_mm`/`material`/`preform` -- none of which round-trip into `.asc`
/// at all) or must be freshly regenerated -- see that function's own doc comment.
/// `state.asc_filename`/`original_asc_text` are `Some` only when this design's
/// schedule came from a real `.asc` on disk (a catalogue attachment via "Load
/// Selected", or a prior Save/Open Native -- see `EditorState::asc_filename`'s own
/// doc comment); `None` (a brand-new "New" design, or the angle-table placeholder
/// reconstruction) always regenerates, exactly like "Export .asc" already does.
///
/// Both fields are refreshed from what was ACTUALLY just written on success, so a
/// second Save right after the first preserves ITS OWN output rather than reopening
/// the preservation question against stale text.
///
/// Also wires up [`setup_dirty_tracking`] -- see that function's own doc comment for
/// why it is bundled in here rather than exported as its own `setup_*` entry point.
/// Item 171: `native_path` is derived from `dest_path` (whatever `.asc` name the
/// cutter just picked through the OS's own save dialog, which already prompts for
/// THAT file on its own), never itself offered to the OS -- so a sidecar belonging to
/// a completely different design, sitting at this same derived path, would otherwise
/// be replaced with no prompt at all. Skipped (returns `true` with no dialog) when
/// `native_path` doesn't exist yet (nothing to overwrite) or already names this exact
/// state's own last save/open (re-saving your own file needs no confirmation --
/// `write_pair_atomically`'s own `.bak` step already covers that case). Split out of
/// [`setup_save_native_callback`] purely to keep that function under clippy's
/// line-count lint.
fn confirm_overwrite_unrelated_native_file(native_path: &Path) -> bool {
    let is_own_file =
        CURRENT_NATIVE_PATH.with(|cell| cell.borrow().as_deref() == Some(native_path));
    if !native_path.is_file() || is_own_file {
        return true;
    }
    let choice = rfd::MessageDialog::new()
        .set_title("Overwrite existing file?")
        .set_description(format!(
            "'{}' already exists and belongs to a different save (or a design this one \
             was never opened from). Overwriting it replaces that design's native \
             sidecar with this one's.",
            native_path.display()
        ))
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    choice == rfd::MessageDialogResult::Yes
}

/// Item 80: this design's own already-known save location, when it was previously
/// saved to or opened from a real native pair -- Ctrl+S/"Save Native" used to always
/// reopen the Save-As dialog, even seconds after the cutter had just chosen a
/// location for it. `Some` only once BOTH halves of "we already know exactly where
/// this design lives" are true: [`CURRENT_NATIVE_PATH`] (the sidecar -- see its own
/// doc comment) and `asc_filename` (the paired `.asc`'s bare name, which
/// [`finish_save_native_success`] always records alongside it, in the very same
/// directory). `None` for a design that has never been saved/opened as a native pair
/// at all -- a brand-new "New" design, the angle-table placeholder reconstruction, or
/// a plain `.asc` opened with no sidecar ([`open_plain_asc`] clears
/// `CURRENT_NATIVE_PATH` precisely so this never fires for one) -- which still falls
/// through to [`save_native_via_dialog`]'s ordinary Save-As behaviour, unchanged.
fn known_save_target(st: &EditorState) -> Option<(PathBuf, PathBuf)> {
    let native_path = CURRENT_NATIVE_PATH.with(|cell| cell.borrow().clone())?;
    let asc_filename = st.asc_filename.as_deref()?;
    Some((native_path.with_file_name(asc_filename), native_path))
}

/// Item 80's quick save: writes straight to `dest_path`/`native_path` (both already
/// known -- see [`known_save_target`]) with no file dialog and no
/// [`confirm_overwrite_unrelated_native_file`] prompt (this IS this state's own file,
/// by construction of how the caller obtained these two paths). Otherwise identical
/// to [`save_native_via_dialog`]'s own tail: the same degenerate-status confirmation,
/// the same atomic pair write, the same success/failure reporting.
fn quick_save_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    dest_path: &Path,
    native_path: &Path,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let (
        mut design,
        asc_filename,
        original_asc_text,
        printed_proportions,
        source_entry_id,
        used_placeholder,
    ) = {
        let st = state.borrow();
        (
            st.design.clone(),
            // `known_save_target` already checked this is `Some`.
            st.asc_filename.clone().unwrap_or_default(),
            st.original_asc_text.clone(),
            st.printed_proportions,
            st.source_entry_id,
            st.used_placeholder,
        )
    };
    // CAD audit item 186 -- see `stamp_source_entry_footnote`'s own doc comment.
    stamp_source_entry_footnote(&mut design.meta.footnotes, source_entry_id);
    match confirm_status_before_write(&design, "Save") {
        StatusConfirm::Fine => {}
        StatusConfirm::ConfirmedWithReason(message) => {
            if let Some(header) = degenerate_marker_header(&design.meta.headers, &message) {
                design.meta.headers.insert(0, header);
            }
        }
        // Item 179: the cutter's own explicit cancel -- no toast (cad_todo.md #244).
        StatusConfirm::Declined => {
            return;
        }
    }

    // Item 82: a design reconstructed from a catalogue's bare angle table has every
    // mast fabricated as `0.0`. `save_paired` stamps
    // `indicatrix_formats::asc::mark_reconstructed` when told so, which is what stops
    // a file that looks like a real cut instruction from passing for one.
    let placeholder_note =
        used_placeholder.then_some("angle-table reconstruction, no attached .asc");
    let paired = match save_paired(
        &design,
        asc_filename.clone(),
        original_asc_text.as_deref(),
        placeholder_note,
        printed_proportions.as_ref(),
    ) {
        Ok(paired) => paired,
        Err(e) => {
            show_toast(ui, &format!("Cannot save: {e}"), "error");
            return;
        }
    };

    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(message) = write_pair_atomically(
        dest_path,
        native_path,
        &paired.asc_text,
        &paired.native_toml,
    ) {
        show_toast(ui, &message, "error");
        return;
    }

    finish_save_native_success(
        ui,
        state,
        SavedPaths {
            dest_path,
            native_path,
            asc_filename,
        },
        paired,
        db,
        source,
    );
}

/// Item 80's Save-As: always shows the native save dialog, exactly like every
/// "Save Native"/Ctrl+S press used to, unconditionally, before this item's fix. Now
/// reached two ways: as [`setup_save_native_callback`]'s own fallback, for a design
/// [`known_save_target`] has no answer for yet, and as `EditorModel.save_native_as`'s
/// entire body (a brand-new callback -- see this module's own handoff note for the
/// exact `ui/models/editor.slint`/`ui/app.slint` additions this depends on) for a
/// cutter who explicitly wants to save the current design to a DIFFERENT file.
fn save_native_via_dialog(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    // Snapshot, then drop the borrow before the blocking native file dialog --
    // same discipline `setup_export_asc_callback` follows.
    let (
        mut design,
        default_asc_name,
        original_asc_text,
        printed_proportions,
        source_entry_id,
        used_placeholder,
    ) = {
        let st = state.borrow();
        (
            st.design.clone(),
            suggested_file_name(&st),
            st.original_asc_text.clone(),
            st.printed_proportions,
            st.source_entry_id,
            st.used_placeholder,
        )
    };
    // CAD audit item 186 -- see `stamp_source_entry_footnote`'s own doc comment.
    stamp_source_entry_footnote(&mut design.meta.footnotes, source_entry_id);
    // Item 182: same check `setup_export_asc_callback` runs, applied to `design`
    // itself (rather than post-processing `paired.asc_text` afterward) so the
    // stamped header is baked in before `save_paired` computes the native
    // sidecar's fingerprint against the final bytes -- mutating the text
    // afterward would leave that fingerprint describing bytes that were never
    // actually written.
    match confirm_status_before_write(&design, "Save") {
        StatusConfirm::Fine => {}
        StatusConfirm::ConfirmedWithReason(message) => {
            if let Some(header) = degenerate_marker_header(&design.meta.headers, &message) {
                design.meta.headers.insert(0, header);
            }
        }
        // Item 179: the cutter's own explicit cancel -- no toast (cad_todo.md #244).
        StatusConfirm::Declined => {
            return;
        }
    }

    let dest_path = seed_default_export_dir(
        rfd::FileDialog::new()
            .set_file_name(&default_asc_name)
            .add_filter(".asc design", &["asc"]),
    )
    .save_file();
    // cad_todo.md #244: see the matching comment on `setup_export_asc_callback`'s
    // own save-picker cancel, above -- a dismissed dialog needs no toast.
    let Some(dest_path) = dest_path else {
        return;
    };
    let asc_filename = dest_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(default_asc_name);
    let native_path = native_path_for_asc(&dest_path);

    if !confirm_overwrite_unrelated_native_file(&native_path) {
        // Item 179: the cutter's own explicit cancel -- no toast (cad_todo.md #244).
        return;
    }

    // Item 82 -- see `quick_save_native`'s matching comment.
    let placeholder_note =
        used_placeholder.then_some("angle-table reconstruction, no attached .asc");
    let paired = match save_paired(
        &design,
        asc_filename.clone(),
        original_asc_text.as_deref(),
        placeholder_note,
        printed_proportions.as_ref(),
    ) {
        Ok(paired) => paired,
        Err(e) => {
            show_toast(ui, &format!("Cannot save: {e}"), "error");
            return;
        }
    };

    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Both files are staged under temp sibling names and only renamed into place
    // once BOTH are known-good on disk -- Item 99: a read-only folder, a full
    // disk or an antivirus lock must never leave a real `.asc` on disk with no
    // native sidecar to reopen it with (see this module's own hard rule, above).
    if let Err(message) = write_pair_atomically(
        &dest_path,
        &native_path,
        &paired.asc_text,
        &paired.native_toml,
    ) {
        show_toast(ui, &message, "error");
        return;
    }

    finish_save_native_success(
        ui,
        state,
        SavedPaths {
            dest_path: &dest_path,
            native_path: &native_path,
            asc_filename,
        },
        paired,
        db,
        source,
    );
}

/// # Handoff
///
/// `db`/`source` are new parameters -- CAD audit items 92/96: Save Native's own
/// catalogue write-back needs them (see [`write_back_to_catalogue`]). Both are
/// already threaded into `gui::editor::setup_editor_callbacks` (`gui::editor::mod`,
/// a hub file this lane doesn't own); that function's own call site
/// (`native_io::setup_save_native_callback(ui, &state);`) needs updating to
/// `native_io::setup_save_native_callback(ui, &state, db, source);` -- both already
/// in scope there as `setup_editor_callbacks`'s own `db: &Arc<Mutex<Database>>`/
/// `source: &Arc<Mutex<LibrarySource>>` parameters (see that function's call to
/// `callbacks::setup_load_selected_callback` a few lines above for the identical
/// pattern already in use).
pub(super) fn setup_save_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    setup_dirty_tracking(ui, state);
    setup_autosave_timer(ui, state);
    let state_save = Rc::clone(state);
    let db_save = Arc::clone(db);
    let source_save = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_save_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // Item 80: quick-save straight to the design's own known location when one
        // exists, so Ctrl+S/"Save Native" stop being a Save-As round trip on every
        // press -- only a design with no known location yet (see
        // `known_save_target`'s own doc comment) still shows the dialog.
        let known = {
            let st = state_save.borrow();
            known_save_target(&st)
        };
        match known {
            Some((dest_path, native_path)) => {
                quick_save_native(
                    &ui,
                    &state_save,
                    &dest_path,
                    &native_path,
                    &db_save,
                    &source_save,
                );
            }
            None => save_native_via_dialog(&ui, &state_save, &db_save, &source_save),
        }
    });

    // "Save Native As...": Item 80's other half -- always the dialog, even when a
    // known location exists, for a cutter who explicitly wants this design written
    // to a DIFFERENT file. The plain `save_native` above deliberately no longer
    // offers that choice, which is exactly why this exists.
    let state_save_as = Rc::clone(state);
    let db_save_as = Arc::clone(db);
    let source_save_as = Arc::clone(source);
    let ui_weak_as = ui.as_weak();
    ui.global::<EditorModel>().on_save_native_as(move || {
        let Some(ui) = ui_weak_as.upgrade() else {
            return;
        };
        save_native_via_dialog(&ui, &state_save_as, &db_save_as, &source_save_as);
    });
}

/// Records where a Save/Export just wrote, for the status strip's own persistent
/// "Saved: ..." segment and its click-to-reveal (CAD audit item 192). The strip
/// gets the bare file name (all a 26px strip has room for) and the full path
/// separately, for the Details popup and for the reveal itself -- see
/// `EditorModel.last_saved_path`'s own doc comment. A path with no file-name
/// component cannot come out of a save dialog, but is handled anyway by falling
/// back to the full path rather than clearing the label.
fn record_last_saved_path(ui: &MainWindow, path: &Path) {
    let full = path.display().to_string();
    let label = path
        .file_name()
        .map_or_else(|| full.clone(), |name| name.to_string_lossy().into_owned());
    let model = ui.global::<EditorModel>();
    model.set_last_saved_path(full.into());
    model.set_last_saved_label(label.into());
}

/// [`setup_save_native_callback`]'s success tail -- split out purely to keep that
/// function under clippy's line-count lint. Drops any in-progress autosave (now
/// strictly older than what's actually on disk), records `native_path` as the most
/// recent Open Recent entry and this state's own file (see [`CURRENT_NATIVE_PATH`]'s
/// doc comment), updates `EditorState`'s own saved-generation bookkeeping, and toasts
/// the outcome -- a draft note when [`indicatrix_cut_core::native::PairedSave::draft_reason`]
/// is `Some`, an ordinary success note otherwise.
/// The pair of files a successful save just wrote, and the name it wrote them
/// under -- grouped because they always travel together (see
/// `setup_save_native_callback`'s own doc comment on why a `.indicatrix.toml` is
/// never written without its `.asc`).
struct SavedPaths<'a> {
    dest_path: &'a Path,
    native_path: &'a Path,
    asc_filename: String,
}

fn finish_save_native_success(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    saved: SavedPaths<'_>,
    paired: indicatrix_cut_core::native::PairedSave,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let SavedPaths {
        dest_path,
        native_path,
        asc_filename,
    } = saved;
    // The real save just landed -- any in-progress autosave is now strictly
    // older than what's on disk, so drop it rather than leaving a stale
    // recovery file behind (it never outlives the real save it stood in for).
    let _ = std::fs::remove_file(autosave_path(Some(asc_filename.as_str())));
    record_recent_native_file(ui, &native_path.display().to_string());
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(native_path.to_path_buf()));
    // cad_todo.md #248: names the window title (`MainWindow.loaded_design_name`)
    // after the file that was just saved.
    ui.set_loaded_design_name(asc_filename.clone().into());
    // The `.asc` half, not the `.indicatrix.toml`: both land in the same folder
    // (so the reveal is identical either way) and the `.asc` is the one a cutter
    // hands to a machine or another program.
    record_last_saved_path(ui, dest_path);

    // Cloned before the moves just below hand `asc_filename`/`paired.asc_text` off
    // to `EditorState` -- `write_back_to_catalogue` (CAD audit items 92/96/186,
    // called after this block) needs its own copies of exactly the same text/name
    // that was just written to disk.
    let asc_filename_for_catalogue = asc_filename.clone();
    let asc_text_for_catalogue = paired.asc_text.clone();
    let native_toml_for_catalogue = paired.native_toml.clone();
    let native_filename_for_catalogue = native_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    {
        let mut st = state.borrow_mut();
        st.asc_filename = Some(asc_filename);
        st.original_asc_text = Some(paired.asc_text);
        // A draft save is still a real, successful save -- `design` not
        // currently solving no longer fails `save_paired` at all (see
        // `PairedSave::draft_reason`'s own doc comment) -- so this is unconditional,
        // not only on the ordinary branch below.
        st.saved_generation = st.generation.load(Ordering::Relaxed);
    }
    ui.global::<EditorModel>()
        .set_is_dirty(state.borrow().is_dirty());

    // `draft_reason` is `Some` iff `design` did not currently solve, or had no
    // tiers at all, and this save fell back to a placeholder-mast `.asc` (see
    // `save_paired`'s own doc comment and `indicatrix_cut_core::native::DraftReason`)
    // -- worded so the cutter knows the file IS on disk but is not yet a real cut
    // instruction, never the old (now-impossible) "Cannot save" error path this
    // replaced. `DraftReason`'s own `Display` already names which of the two
    // happened, so it is not repeated here.
    if let Some(reason) = &paired.draft_reason {
        // Item 179: this is exactly the "critical outcome" class the finding names --
        // the cutter's `.asc` is a placeholder, not a real cut instruction -- so it
        // gets the same persistent "warning" class as the fingerprint-mismatch toast
        // just below, not an "info" flash that can vanish before it is read.
        show_toast(
            ui,
            &format!(
                "Saved '{}' as a draft ({reason}). Add a scale-reference tier to \
                 finish it.",
                dest_path.display()
            ),
            "warning",
        );
    } else {
        // Item 222: the neutral wording used to imply a plain re-serialize; the real
        // condition (see `crates/indicatrix-cut-core/src/native/save.rs`'s
        // semantic-equality gate) is that every meet note got resynthesized, which is
        // what the manual (docs/manual/11-saving-and-file-formats.md:57-61) already
        // documents -- so the toast now says so explicitly instead of leaving the
        // manual and the toast disagreeing on how alarming this is.
        let asc_note = if paired.asc_preserved {
            "unchanged .asc preserved"
        } else {
            ".asc regenerated (meet notes rewritten)"
        };
        show_toast(
            ui,
            &format!(
                "Saved '{}' and '{}' ({asc_note}).",
                dest_path.display(),
                native_path.display()
            ),
            "success",
        );
    }

    // CAD audit items 92/96/186: registers (or updates) this design's own catalogue
    // row -- see `write_back_to_catalogue`'s own doc comment for exactly what gets
    // overwritten versus preserved. Runs after every successful Save Native
    // (`quick_save_native` and `save_native_via_dialog` both funnel through here),
    // never after "Export .asc" (`setup_export_asc_callback`), which keeps its own
    // documented "file only, no database write of any kind" rule.
    let source_entry_id = state.borrow().source_entry_id;
    match write_back_to_catalogue(
        db,
        source_entry_id,
        &asc_filename_for_catalogue,
        &asc_text_for_catalogue,
        &native_filename_for_catalogue,
        &native_toml_for_catalogue,
    ) {
        Ok((id, outcome)) => {
            // Every save after this one for a previously row-less (or
            // now-row-less, see `CatalogueWriteBack::SourceRowGoneNewRowCreated`)
            // design must update THIS row, never insert a second -- see
            // `EditorState::source_entry_id`'s own doc comment.
            state.borrow_mut().source_entry_id = Some(id);
            crate::gui::library::local::refresh_after_library_change(ui, db, source);
            if matches!(outcome, CatalogueWriteBack::SourceRowGoneNewRowCreated) {
                show_toast(
                    ui,
                    "This design's catalogue row no longer exists (it may have been \
                     deleted) -- saved as a new catalogue entry instead of updating it.",
                    "info",
                );
            }
        }
        Err(message) => show_toast(ui, &format!("Catalogue not updated: {message}"), "error"),
    }
}

/// Writes `asc_text`/`native_toml` to `dest_path`/`native_path` as one
/// atomic-as-possible pair -- Item 99: a read-only folder, a full disk, or an
/// antivirus lock must never leave a plausible-looking `.asc` on disk with its
/// authored constraints nowhere to be found (this module's own stated rule, see
/// [`setup_save_native_callback`]'s doc comment).
///
/// Both files are staged under same-directory temp sibling names first ([`temp_sibling`]
/// -- always the same filesystem as the real target, so the rename that follows is a
/// cheap, effectively-atomic same-volume operation, never one that could silently
/// fall back to copy+delete across volumes). Before either temp file is renamed into
/// place, any file ALREADY at that destination is copied to a `.bak` sibling first
/// ([`backup_existing`]) -- Item 78: a bad save (or this very save, if the design
/// regressed since the last one) must never overwrite the only copy of a design that
/// was already on disk. Only once both backups (when needed) and both writes have
/// already succeeded are the real renames attempted. The one residual failure window
/// -- the second rename failing after the first already landed -- is reported as
/// exactly that (`.asc` on disk, sidecar not yet written) rather than silently
/// claimed as a full success.
///
/// # Errors
///
/// A ready-to-toast message naming exactly what's on disk afterward.
fn write_pair_atomically(
    dest_path: &Path,
    native_path: &Path,
    asc_text: &str,
    native_toml: &str,
) -> Result<(), String> {
    let tmp_asc = temp_sibling(dest_path);
    let tmp_native = temp_sibling(native_path);

    if let Err(e) = std::fs::write(&tmp_asc, asc_text) {
        let _ = std::fs::remove_file(&tmp_asc);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            dest_path.display()
        ));
    }
    if let Err(e) = std::fs::write(&tmp_native, native_toml) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            native_path.display()
        ));
    }
    if let Err(message) = backup_existing(dest_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(message);
    }
    if let Err(message) = backup_existing(native_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(message);
    }
    if let Err(e) = std::fs::rename(&tmp_asc, dest_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(format!(
            "Failed to finalize {}: {e}. Nothing was saved.",
            dest_path.display()
        ));
    }
    if let Err(e) = std::fs::rename(&tmp_native, native_path) {
        return Err(format!(
            "Saved '{}' but failed to finalize its native sidecar {}: {e}. The .asc is on \
             disk without its native sidecar -- re-save once the problem is fixed.",
            dest_path.display(),
            native_path.display()
        ));
    }
    Ok(())
}

/// A same-directory temp sibling of `path`, used to stage a write before the final
/// atomic-as-possible rename -- see [`write_pair_atomically`]. Always a sibling
/// (never `std::env::temp_dir()`), so the rename that follows never crosses
/// filesystems.
fn temp_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.tmp"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".tmp");
            s
        },
    );
    path.with_file_name(file_name)
}

/// `path`'s `.bak` sibling -- e.g. `design.asc` -> `design.asc.bak`, `design.
/// indicatrix.toml` -> `design.indicatrix.toml.bak`. One generation of backup only:
/// a second save in a row overwrites the `.bak` from the first, matching "keep the
/// PREVIOUS version" rather than an ever-growing history.
fn backup_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.bak"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".bak");
            s
        },
    );
    path.with_file_name(file_name)
}

/// Item 78 ("never overwrite the only copy"): copies `path` to its [`backup_sibling`]
/// before [`write_pair_atomically`] renames a freshly staged temp file over it. A
/// no-op (`Ok(())`) when `path` doesn't exist yet -- a design's first save has
/// nothing to back up. Copies rather than renames `path` itself: `path` is left
/// completely untouched by this step either way, so a failed backup aborts the whole
/// save (see [`write_pair_atomically`]) without having disturbed the file that was
/// already there.
///
/// # Errors
///
/// A ready-to-toast message naming the file that could not be backed up.
fn backup_existing(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let backup = backup_sibling(path);
    std::fs::copy(path, &backup).map_err(|e| {
        format!(
            "Failed to back up {} to {} before overwriting it: {e}. Nothing was saved.",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}

/// Wires `EditorModel.recompute_dirty` -- called by `changed tiers` in
/// `ui/models/editor.slint` every time ANY edit path (this app's own tier-editing
/// callbacks, but also Deep Solve/Optimize Apply, Adopt, and Retarget Apply, none of
/// which this module owns) rebuilds the tier list -- so [`EditorState::is_dirty`]
/// stays live without a `set_is_dirty` call at every one of those sites. Bundled into
/// [`setup_save_native_callback`] (called exactly once, like every other `setup_*`
/// entry point here) rather than given its own -- `callbacks::mod` only re-exports
/// entry points by name, and this one has no Slint button of its own to answer to.
fn setup_dirty_tracking(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_recompute_dirty(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // `try_borrow`, never `borrow`: Slint runs `changed tiers` synchronously
        // from inside `set_tiers`, and the paths that push the tier list are
        // normally holding `state.borrow_mut()` while they do it (`do_new_design_
        // create`, `apply_loaded_design`, and every ordinary edit callback). A plain
        // `borrow()` panicked there with "already mutably borrowed".
        //
        // Skipping is safe rather than merely non-fatal: `view::
        // push_tier_list_and_undo_redo` -- the only thing that calls `set_tiers` --
        // pushes `is_dirty` itself from the `&EditorState` it already holds, on
        // every one of those paths. This handler exists for the pushes that do NOT
        // come through there, where nothing is borrowed and the read succeeds.
        if let Ok(st) = state.try_borrow() {
            ui.global::<EditorModel>().set_is_dirty(st.is_dirty());
        }
    });
}

/// How often the autosave timer checks [`EditorState::is_dirty`] and, if `true`,
/// writes a recovery snapshot -- Item 78's "timer-based autosave." Two minutes:
/// frequent enough that a crash loses at most a couple of minutes of edits,
/// infrequent enough that it never shows up as a hitch even on a slow disk or a
/// large design. Gated on `is_dirty` (never writes while nothing has changed) and
/// never touches the cutter's own save path -- see [`autosave_path`]/
/// [`run_autosave_tick`].
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);

thread_local! {
    /// The native `.indicatrix.toml` path this `EditorState` was last loaded from or
    /// saved to, if any -- Item 171: `native_path` in [`setup_save_native_callback`]
    /// is DERIVED from whatever `.asc` name the cutter just picked
    /// ([`indicatrix_cut_core::native_path_for_asc`]), never itself chosen through the
    /// native save dialog, so the OS's own "this file already exists" prompt (which
    /// covers only the `.asc` half) never sees it. Comparing against this lets a Save
    /// tell "the same design's own sidecar, safe to overwrite" (already covered by
    /// [`write_pair_atomically`]'s own `.bak` backup) apart from "some unrelated
    /// design's sidecar that happens to share this `.asc` name," which gets an
    /// explicit native OS confirm instead. A plain `RefCell`, not part of
    /// `EditorState` itself: it names a location on disk, not design data, so it must
    /// not be reset by [`EditorState::replace_wholesale`] the way every other field on
    /// that struct is -- see that method's own doc comment.
    static CURRENT_NATIVE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

thread_local! {
    /// Keeps the autosave `slint::Timer` alive for the life of the window -- a
    /// `Timer` stops as soon as its value is dropped, and this module owns no
    /// longer-lived struct to park the handle in (unlike `gui::mod`'s own
    /// `remote_rendering_timer` field). Mirrors `retarget_actions::RETARGET_ASYNC`'s
    /// own reasoning for using a `thread_local!` here: Slint's event loop is
    /// single-threaded, so this is sound without any real synchronization.
    static AUTOSAVE_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Starts the autosave timer -- bundled into [`setup_save_native_callback`] (called
/// exactly once, like every other `setup_*` entry point here) rather than given its
/// own, the same reasoning [`setup_dirty_tracking`] documents on itself.
///
/// # Recovery
///
/// The autosave lands at [`autosave_path`]: `<name>.indicatrix.autosave.toml` inside
/// this app's OWN settings directory (`settings::store::default_settings_path`'s
/// parent directory), never beside the cutter's real save location and never the
/// cutter's real `.asc`/native pair (this design's own path isn't even tracked
/// today -- see this module's own handoff note on that). After a crash, a cutter
/// recovers by choosing File > Open Native and browsing to that file directly; it
/// opens exactly like any other native design file (no `.asc` sidecar needed --
/// `read_picked_asc`'s own "no sidecar" fallback never applies here since a `.toml`
/// is picked directly). A successful real Save Native deletes the autosave file
/// (see that callback's own body) so a stale one never outlives the real save it
/// was standing in for.
fn setup_autosave_timer(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, AUTOSAVE_INTERVAL, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        run_autosave_tick(&ui, &state);
    });
    AUTOSAVE_TIMER.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// One autosave timer tick: writes a recovery snapshot iff [`EditorState::is_dirty`]
/// -- see [`setup_autosave_timer`]'s own doc comment. Reuses
/// [`indicatrix_cut_core::save_paired`] (the same builder Save Native itself calls)
/// purely for its draft-save fallback (an unsolved or tier-less design must still
/// produce recoverable TOML, not an error) -- only `native_toml` is ever written;
/// `save_paired`'s own `asc_text` is discarded, so this never touches the cutter's
/// real `.asc` file, or any file the cutter chose at all.
fn run_autosave_tick(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let (design, asc_filename, original_asc_text, printed_proportions) = {
        let st = state.borrow();
        if !st.is_dirty() {
            return;
        }
        (
            st.design.clone(),
            st.asc_filename.clone(),
            st.original_asc_text.clone(),
            st.printed_proportions,
        )
    };
    let Ok(saved) = save_paired(
        &design,
        format!("{}.asc", autosave_base_name(asc_filename.as_deref())),
        original_asc_text.as_deref(),
        None,
        printed_proportions.as_ref(),
    ) else {
        // `SaveError::Toml` is unreachable in practice (see its own doc comment) and
        // there is nothing actionable to toast for a silent background autosave.
        return;
    };
    let path = autosave_path(asc_filename.as_deref());
    if let Err(e) = write_autosave(&path, &saved.native_toml) {
        show_toast(
            ui,
            &format!("Autosave to {} failed: {e}", path.display()),
            "error",
        );
    }
}

/// The base name an autosave is filed under: the design's own `asc_filename` (its
/// bare `.asc` name) with the extension stripped, or `"untitled"` for a design never
/// yet paired with one (a brand-new design, or the angle-table placeholder
/// reconstruction).
fn autosave_base_name(asc_filename: Option<&str>) -> String {
    let name = asc_filename.unwrap_or("untitled");
    name.strip_suffix(".asc").unwrap_or(name).to_string()
}

/// Where [`run_autosave_tick`] writes: `<name>.indicatrix.autosave.toml` inside this
/// app's own settings directory, falling back to the OS temp directory on the rare
/// platform where even that can't be resolved -- either way, never a location the
/// cutter chose or the design's own paired files live at.
fn autosave_path(asc_filename: Option<&str>) -> PathBuf {
    let dir = crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    dir.join(format!(
        "{}.indicatrix.autosave.toml",
        autosave_base_name(asc_filename)
    ))
}

/// Item 78's still-open half: "no startup check for a leftover autosave file and no
/// restore-on-launch prompt exists -- a crash still loses the session in practice
/// even though the autosave file itself is written." This is that check, exposed for
/// `gui::editor::mod`'s startup sequence (NOT this lane's file -- see this module's
/// own handoff note) to call before [`EditorState::fresh`] replaces whatever the
/// previous session had.
///
/// Scans [`autosave_path`]'s own directory (never recurses -- there is nothing to
/// recurse into, every autosave lands flat in this one app-settings directory) for
/// any `*.indicatrix.autosave.toml` file and returns the most recently modified one,
/// if any. Does not open or delete anything itself: a leftover file only means SOME
/// past session was dirty when it stopped ticking (a real crash, or simply the
/// window closing between one 120-second tick and the next), never that the design
/// in it is still wanted -- the caller decides whether to offer it and, either way,
/// to remove it afterward (matching [`setup_save_native_callback`]'s own
/// successful-save cleanup, just above).
#[must_use]
pub(super) fn find_leftover_autosave() -> Option<PathBuf> {
    let dir = crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".indicatrix.autosave.toml"))
        })
        .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
}

/// Writes `native_toml` to `path` via the same stage-then-rename discipline
/// [`write_pair_atomically`] uses, so a crash mid-autosave-write can never leave a
/// half-written, corrupt recovery file behind either.
///
/// # Errors
///
/// A ready-to-toast message.
fn write_autosave(path: &Path, native_toml: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, native_toml).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Item 78's "Open Recent" write side: records `native_path_display` as the
/// most-recently-used native file in the settings store
/// ([`crate::settings::model::AppSettings::record_recent_native_file`]) and
/// refreshes `MainWindow.recent_native_files` so File > Open Recent reflects it
/// immediately, with no restart needed. Called after every successful Save Native
/// and Open Native (both the picker and File > Open Recent itself, since both funnel
/// through [`commit_loaded_native`]) -- never for [`open_plain_asc`]'s bare-`.asc`
/// path, which has no native file to name.
///
/// Reads/writes the settings file directly via [`crate::settings::store`] rather
/// than through the debounced [`crate::settings::SettingsPersister`]: this group has
/// no handle to it (`gui::editor::setup_editor_callbacks` doesn't thread one in, and
/// adding one is a `gui::editor::mod`/`gui::mod` change outside this lane's file
/// ownership). This is a real, if narrow, race as a result: a settings change still
/// inside the persister's ~600ms debounce window when this runs writes its own
/// (older, recent-files-less) in-memory snapshot over this one the next time it
/// flushes, silently dropping the just-recorded entry. Accepted rather than leaving
/// the whole feature unwired -- the consequence is losing one recent-files entry
/// occasionally, never any design data, and the entry reappears next save/open
/// anyway.
fn record_recent_native_file(ui: &MainWindow, native_path_display: &str) {
    let settings_path = crate::settings::store::default_settings_path();
    let mut file = crate::settings::store::load_or_default(&settings_path);
    file.settings
        .record_recent_native_file(native_path_display.to_string());
    let _ = crate::settings::store::save(&settings_path, &file);
    ui.set_recent_native_files(ModelRc::new(VecModel::from(
        file.settings
            .recent_native_files
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}

/// "Open Native": loads a `.indicatrix.toml` (or legacy `.gemcut.toml`) sidecar
/// together with its paired `.asc` via `indicatrix_cut_core::load_paired`. Replaces the whole editor state the same way
/// "New"/"Load Selected" do (fresh `History`, no printed proportions -- a locally
/// opened native file has no catalogue row to verify Deep Solve against, exactly
/// like a brand-new design).
///
/// Checks [`EditorState::is_dirty`] BEFORE doing anything else -- including before
/// showing the native-file picker -- exactly like `setup_new_design_create_callback`/
/// `setup_load_selected_callback` do for New/Load Selected: asking "keep unsaved
/// changes?" only after making the user pick a file would be backwards. A dirty
/// design stashes [`PendingUnsavedAction::OpenNative`] and opens the guard dialog
/// instead of proceeding; [`setup_unsaved_guard_dispatch`]
/// (`gui::editor::callbacks::tier_actions`) resumes by calling [`do_open_native`]
/// again once Save/Discard is chosen, which re-shows the picker from scratch.
///
/// Also registers the fingerprint-mismatch dialog's three callbacks
/// ([`PENDING_MISMATCH`]) -- bundled in here rather than a separate `setup_*` for the
/// same reason [`setup_dirty_tracking`] is bundled into [`setup_save_native_callback`]:
/// this is the one `setup_*` entry point Open Native's own wiring has to answer to.
pub(super) fn setup_open_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
) {
    setup_mismatch_dialog_callbacks(ui, state, render_ctx, preview_state, solid_last_solved);

    let state_open = Rc::clone(state);
    let render_ctx_open = Arc::clone(render_ctx);
    let preview_state_open = Arc::clone(preview_state);
    let solid_last_solved_open = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_open_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if state_open.borrow().is_dirty() {
            state_open.borrow_mut().pending_unsaved_action = Some(PendingUnsavedAction::OpenNative);
            ui.global::<EditorModel>().set_unsaved_dialog_message(
                "Opening a native file will discard the current design's unsaved changes.".into(),
            );
            ui.global::<EditorModel>().set_unsaved_dialog_open(true);
            return;
        }
        do_open_native(
            &ui,
            &state_open,
            &render_ctx_open,
            &preview_state_open,
            &solid_last_solved_open,
        );
    });

    // File > Open Recent (`ui/app.slint`'s `MainWindow.open_recent_native_file`) --
    // a root-component callback rather than an `EditorModel` one, since
    // `recent_native_files`/`open_recent_native_file` are declared directly on
    // `MainWindow` (`ui/app.slint`, this lane's own file) rather than on
    // `EditorModel` (`ui/models/editor.slint`, owned elsewhere). Opening an entry
    // re-records it via [`record_recent_native_file`] (inside
    // [`commit_loaded_native`], which this path shares with the ordinary picker),
    // so using a recent file also bumps it back to the front. Bundled into this
    // `setup_*` entry point for the same reason `setup_mismatch_dialog_callbacks`
    // is, immediately above.
    let state_recent = Rc::clone(state);
    let render_ctx_recent = Arc::clone(render_ctx);
    let preview_state_recent = Arc::clone(preview_state);
    let solid_last_solved_recent = Arc::clone(solid_last_solved);
    let ui_weak_recent = ui.as_weak();
    ui.on_open_recent_native_file(move |path| {
        let Some(ui) = ui_weak_recent.upgrade() else {
            return;
        };
        open_recent_native_path(
            &ui,
            &state_recent,
            &render_ctx_recent,
            &preview_state_recent,
            &solid_last_solved_recent,
            PathBuf::from(path.as_str()),
        );
    });
}

/// File > Open Recent's own click handler -- loads `native_path` directly (no file
/// picker) via the exact same [`read_native_pair`]/[`open_native_pair`] path
/// [`do_open_native`] uses for a picked `.toml`. Guards on
/// [`EditorState::is_dirty`] like every other destructive replace-the-design entry
/// point in this module, but -- unlike [`setup_open_native_callback`]'s own picker
/// path -- does not yet resume through the Save/Discard/Cancel dialog on a dirty
/// design: `PendingUnsavedAction` has no variant carrying a specific path to resume
/// at (only `OpenNative`, which re-shows the picker from scratch), and that enum is
/// owned by `state/mod.rs`. Refusing with a toast instead is safe (nothing is lost)
/// though less smooth; see this module's own handoff note for the exact
/// `PendingUnsavedAction` variant a future change could add to close this gap.
///
/// `pub(super)` (rather than private) so `gui::editor::mod`'s startup sequence can
/// reuse it for Item 95's "reopen last design" -- `AppSettings::recent_native_files`
/// already carries the most-recently-used path first; this is the same load path
/// "File > Open Recent" itself uses to open it.
pub(super) fn open_recent_native_path(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    native_path: PathBuf,
) {
    if state.borrow().is_dirty() {
        show_toast(
            ui,
            "Save or discard the current design's unsaved changes before opening a recent file.",
            "error",
        );
        return;
    }
    let Some((parsed_native, asc_text, native_text)) = read_native_pair(ui, &native_path) else {
        return;
    };
    open_native_pair(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
        PickedPair {
            native_path,
            parsed_native,
            asc_text,
            native_text,
        },
    );
}

/// The native-load fingerprint-mismatch choice's stashed inputs -- everything
/// [`PENDING_MISMATCH`]'s three resolution callbacks need to either recommit the
/// already-loaded (overlay-skipped) design or re-run [`load_paired`] with
/// `apply_overlay_on_mismatch: true`. Deliberately just the two source texts plus the
/// display path/bare filename, not the whole [`indicatrix_cut_core::LoadPairedResult`]
/// or parsed [`indicatrix_cut_core::NativeDesignFile`] -- re-parsing both (cheap: this
/// only happens once, on the user's own explicit click) is simpler than keeping a
/// second, slightly-different-shaped snapshot of the same two files in sync.
struct PendingMismatch {
    native_path_display: String,
    asc_filename: String,
    asc_text: String,
    native_text: String,
}

thread_local! {
    /// Stashed by [`do_open_native`] the moment it sees
    /// [`indicatrix_cut_core::TierOverlay::SkippedFingerprintMismatch`], read back by
    /// whichever of [`setup_mismatch_dialog_callbacks`]'s three handlers the user
    /// picks. `None` whenever the mismatch dialog is closed (the common state) --
    /// mirrors `tier_actions::REMOTE_LOAD_TARGET`'s own `thread_local!` shape, though
    /// for a different reason: this is plain, non-`Send` local data with nothing
    /// forcing a `thread_local!` on its own, but it still needs to outlive the single
    /// `do_open_native` call that stashes it, across however long the user takes to
    /// click a button, which a local variable can't do.
    static PENDING_MISMATCH: RefCell<Option<PendingMismatch>> = const { RefCell::new(None) };
}

/// What the "Open Native" picker actually returned -- see [`setup_open_native_callback`]'s
/// own doc comment for why the same picker now accepts a bare `.asc` too (Item 89: a
/// cutter handed a plain `GemCAD` file by email should not have to import it into the
/// catalogue database first just to look at it).
enum PickedNative {
    /// A real native+`.asc` pair, ready for [`load_paired`]. Boxed: [`PickedPair`]
    /// carries a whole parsed [`NativeDesignFile`] plus both source texts, several
    /// times larger than [`Self::AscOnly`]'s bare path+text
    /// (`clippy::large_enum_variant`).
    Pair(Box<PickedPair>),
    /// A directly-picked bare `.asc` with no native sidecar found next to it --
    /// nothing [`load_paired`] has any use for (no per-tier overlay, no fingerprint,
    /// no draft flag); committed straight through
    /// `gui::editor::loading::design_from_asc_text` instead, in [`open_plain_asc`].
    AscOnly { asc_path: PathBuf, asc_text: String },
}

/// [`PickedNative::Pair`]'s payload -- bundled into its own struct (rather than four
/// more parameters on [`open_native_pair`]) purely to keep that function under
/// clippy's argument-count lint, the same reasoning [`LoadedNativeOutcome`] uses.
struct PickedPair {
    native_path: PathBuf,
    parsed_native: NativeDesignFile,
    asc_text: String,
    native_text: String,
}

/// Reads/parses a native file already known to live at `native_path`, plus its
/// recorded paired `.asc` -- split out of [`read_picked_native`]/[`read_picked_asc`]
/// purely to keep both under clippy's line-count lint. `None` on any read/parse
/// failure, each already toasted here before returning;
/// `Some((parsed_native, asc_text, native_text))` on success.
fn read_native_pair(
    ui: &MainWindow,
    native_path: &Path,
) -> Option<(NativeDesignFile, String, String)> {
    let native_text = match std::fs::read_to_string(native_path) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Failed to read {}: {e}", native_path.display()),
                "error",
            );
            return None;
        }
    };
    // The native file's own recorded `asc_filename` (a bare file name -- see
    // `indicatrix_cut_core::NativeDesignFile::asc_filename`'s own doc comment) is read
    // AFTER parsing the chosen file, since that field is the authoritative pointer to
    // the real paired file once a native file has actually been parsed; this never
    // guesses at a sibling `.asc` name the way `indicatrix_cut_core::asc_path_for_native`
    // does for a picker's initial directory (there is no picker here to seed -- the
    // native file's own directory plus its own recorded name is exact).
    let parsed_native = match indicatrix_cut_core::native::parse_toml_string(&native_text) {
        Ok(n) => n,
        Err(e) => {
            show_toast(
                ui,
                &format!(
                    "'{}' is not a valid native design file: {e}",
                    native_path.display()
                ),
                "error",
            );
            return None;
        }
    };
    let asc_path = native_path.with_file_name(&parsed_native.asc_filename);
    let asc_text =
        resolve_paired_asc_text(ui, native_path, &asc_path, &parsed_native.asc_filename)?;
    Some((parsed_native, asc_text, native_text))
}

/// Reads the paired `.asc`'s text, recovering from a moved/renamed file (common
/// after `GemCad`'s own Save As -- Item 177) instead of giving up outright the way
/// this used to. Tries, in order: `recorded_asc_path` (the native file's own authoritative
/// `asc_filename`, exact when it still holds); [`indicatrix_cut_core::asc_path_for_native`]'s
/// naming guess (only meaningful when it names a DIFFERENT path -- most native files'
/// own recorded name already matches the guess, so this rarely fires on its own); and
/// finally an explicit "Locate the paired .asc" picker, filtered to `*.asc`, so a
/// cutter who renamed or moved the file can point at it directly rather than
/// hand-editing the sidecar's TOML. `None` (already toasted) only once all three have
/// failed or the picker was cancelled.
fn resolve_paired_asc_text(
    ui: &MainWindow,
    native_path: &Path,
    recorded_asc_path: &Path,
    recorded_asc_filename: &str,
) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(recorded_asc_path) {
        return Some(text);
    }
    if let Some(guessed) = indicatrix_cut_core::asc_path_for_native(native_path)
        && guessed != recorded_asc_path
        && let Ok(text) = std::fs::read_to_string(&guessed)
    {
        return Some(text);
    }
    show_toast(
        ui,
        &format!(
            "'{}' names a paired .asc file '{recorded_asc_filename}', but it could not be found \
             next to it. Locate it to continue.",
            native_path.display()
        ),
        "info",
    );
    let Some(located) = rfd::FileDialog::new()
        .set_title("Locate the paired .asc")
        .add_filter(".asc design", &["asc"])
        .pick_file()
    else {
        // Item 179: the cutter's own explicit cancel -- no toast (cad_todo.md #244).
        return None;
    };
    match std::fs::read_to_string(&located) {
        Ok(text) => Some(text),
        Err(e) => {
            show_toast(
                ui,
                &format!("Failed to read {}: {e}", located.display()),
                "error",
            );
            None
        }
    }
}

/// A directly-picked `.asc` file: checks for a sibling native sidecar first (via
/// `native_path_for_asc`), so a pair still opens as a full pair even when picked by
/// its `.asc` half -- preserving the authored meet constraints/detached facets a bare
/// `.asc` re-import would otherwise drop entirely. Falls back to
/// [`PickedNative::AscOnly`] only when no sidecar file exists at all; a sidecar that
/// exists but fails to read/parse is a real error (already toasted by
/// [`read_native_pair`]), not silently skipped.
fn read_picked_asc(ui: &MainWindow, asc_path: &Path) -> Option<PickedNative> {
    let native_path = native_path_for_asc(asc_path);
    if native_path.is_file() {
        let (parsed_native, asc_text, native_text) = read_native_pair(ui, &native_path)?;
        return Some(PickedNative::Pair(Box::new(PickedPair {
            native_path,
            parsed_native,
            asc_text,
            native_text,
        })));
    }

    let asc_text = match std::fs::read_to_string(asc_path) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Failed to read {}: {e}", asc_path.display()),
                "error",
            );
            return None;
        }
    };
    Some(PickedNative::AscOnly {
        asc_path: asc_path.to_path_buf(),
        asc_text,
    })
}

/// Shows the "Open Native" picker (now accepting `.toml` OR `.asc` -- Item 89) and
/// reads whichever one the cutter picked. `None` on cancellation or any read/parse
/// failure, each already toasted before returning.
fn read_picked_native(ui: &MainWindow) -> Option<PickedNative> {
    // Item 195: a bare `["toml"]` filter listed every `.toml` in the folder --
    // `Cargo.toml` included -- because the real extension is the compound
    // `indicatrix.toml` (legacy `gemcut.toml`), which a single short extension can't
    // express. `rfd` turns each extension into a literal `*.{ext}` glob (see its own
    // Windows backend), so passing the FULL suffix here (not just `"toml"`) produces
    // `*.indicatrix.toml`/`*.gemcut.toml` patterns that actually exclude an unrelated
    // `.toml` file -- this leading filter is what the OS picker pre-selects. The
    // broad `toml` filter stays available as a second, explicitly-labelled choice
    // for a hand-edited or otherwise irregularly-named sidecar the first filter would
    // hide.
    let Some(picked_path) = rfd::FileDialog::new()
        .set_title("Open Native Design")
        .add_filter(
            "Indicatrix native design",
            &[
                indicatrix_cut_core::native::NATIVE_EXTENSION_SUFFIX,
                indicatrix_cut_core::native::LEGACY_NATIVE_EXTENSION_SUFFIX,
            ],
        )
        .add_filter("All TOML files", &["toml"])
        .add_filter(".asc design", &["asc"])
        .pick_file()
    else {
        // cad_todo.md #244: a dismissed file dialog is the cutter's own deliberate
        // cancel -- no toast needed, and the old one could silently replace an
        // error toast still waiting to be read.
        return None;
    };

    if picked_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("asc"))
    {
        return read_picked_asc(ui, &picked_path);
    }

    let (parsed_native, asc_text, native_text) = read_native_pair(ui, &picked_path)?;
    Some(PickedNative::Pair(Box::new(PickedPair {
        native_path: picked_path,
        parsed_native,
        asc_text,
        native_text,
    })))
}

/// The actual "Open Native" work -- see [`setup_open_native_callback`]'s own doc
/// comment for why the unsaved-changes guard runs before this is ever called, not
/// inside it. Reads whichever file the cutter picked via [`read_picked_native`], then
/// dispatches to [`open_native_pair`] (a real pair) or [`open_plain_asc`] (a bare
/// `.asc`, no sidecar).
pub(in crate::gui::editor) fn do_open_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
) {
    match read_picked_native(ui) {
        Some(PickedNative::Pair(picked)) => {
            open_native_pair(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                *picked,
            );
        }
        Some(PickedNative::AscOnly { asc_path, asc_text }) => open_plain_asc(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            &asc_path,
            &asc_text,
        ),
        None => {}
    }
}

/// The real native+`.asc` pair path -- split out of [`do_open_native`] purely to keep
/// that function under clippy's line-count/argument-count lints. `false` passed to
/// [`load_paired`]: never silently keep a per-tier overlay whose fingerprint no
/// longer matches the `.asc` it would be applied against -- see [`TierOverlay`]'s own
/// doc comment. A `SkippedFingerprintMismatch` result pauses on the mismatch dialog
/// instead of committing it; every other outcome (including the defensive-only
/// `SkippedTierCountMismatch`, never expected in practice per its own doc comment)
/// commits immediately via [`commit_loaded_native`].
fn open_native_pair(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    picked: PickedPair,
) {
    let PickedPair {
        native_path,
        parsed_native,
        asc_text,
        native_text,
    } = picked;
    match load_paired(&asc_text, &native_text, false) {
        Ok(loaded) => {
            if matches!(loaded.tier_overlay, TierOverlay::SkippedFingerprintMismatch) {
                // Only offered when the native file's own tier count still agrees
                // with the freshly imported `.asc` -- see
                // `EditorModel.mismatch_dialog_can_apply`'s own doc comment. Recomputed
                // here from `loaded.design.tiers` (the `.asc`-derived tier list, before
                // any overlay) rather than trusted from `loaded.tier_overlay` itself,
                // since `SkippedFingerprintMismatch` alone doesn't distinguish the two.
                let can_apply = parsed_native.tiers.len() == loaded.design.tiers.len();
                PENDING_MISMATCH.with(|cell| {
                    *cell.borrow_mut() = Some(PendingMismatch {
                        native_path_display: native_path.display().to_string(),
                        asc_filename: parsed_native.asc_filename,
                        asc_text,
                        native_text,
                    });
                });
                ui.global::<EditorModel>()
                    .set_mismatch_dialog_can_apply(can_apply);
                ui.global::<EditorModel>().set_mismatch_dialog_open(true);
                return;
            }
            let is_mismatch = !matches!(loaded.fingerprint, FingerprintCheck::Match);
            commit_loaded_native(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                LoadedNativeOutcome {
                    loaded,
                    native_path_display: native_path.display().to_string(),
                    asc_filename: parsed_native.asc_filename,
                    asc_text,
                    is_mismatch,
                },
            );
        }
        Err(e) => show_toast(ui, &format!("Cannot open: {e}"), "error"),
    }
}

/// The bare-`.asc`-with-no-sidecar path (Item 89) -- builds the design exactly the
/// way `gui::editor::loading::design_from_asc_text` already does for a catalogue
/// attachment (no meet-intent overlay, no fingerprint, no draft flag: there is no
/// native file at all), then replaces `state` wholesale via [`finish_state_replace`],
/// the same tail [`commit_loaded_native`] runs for the paired case.
fn open_plain_asc(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    asc_path: &Path,
    asc_text: &str,
) {
    let file_name = asc_path.file_name().map_or_else(
        || "design.asc".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match super::loading::design_from_asc_text(&file_name, asc_text, None) {
        Ok(loaded) => {
            // A bare `.asc` has no native sidecar at all -- see `CURRENT_NATIVE_PATH`'s
            // own doc comment. Cleared rather than left at whatever the PREVIOUS
            // design's own save/open set it to, so a later Save Native here is never
            // mistaken for "re-saving that unrelated design's own file."
            CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = None);
            // `replace_wholesale`, not a plain `*state.borrow_mut() = ...`: carries
            // this state's own `generation` `Arc` across the replacement (and bumps
            // it) instead of handing back a brand-new one, so a background Deep
            // Solve/Optimize/auto-solve dispatched against the design being replaced
            // still observes the change -- see that method's own doc comment
            // (`state/mod.rs`) and `tier_actions::do_new_design_create`'s matching
            // comment for the same reasoning applied to New/Load Selected.
            state.borrow_mut().replace_wholesale(EditorState {
                design: loaded.design,
                history: History::new(),
                printed_proportions: None,
                generation: Arc::new(AtomicU64::new(0)),
                design_epoch: Arc::new(AtomicU64::new(0)),
                saved_generation: 0,
                pending_unsaved_action: None,
                deep_solve: None,
                optimize: None,
                pending_optimize: Arc::new(Mutex::new(None)),
                asc_filename: loaded.asc_filename,
                original_asc_text: loaded.original_asc_text,
                pending_gear_remap: None,
                pending_retarget: None,
                multi_selected: std::collections::BTreeSet::new(),
                last_pushed_scratch: RefCell::new(PushedScratch::default()),
                // CAD audit items 92/96: Open Native (this path and the paired-load
                // one below) has no catalogue row of its own -- it loaded from a
                // file the cutter picked directly, not from a library selection --
                // so there is nothing here for a later Save Native to write back
                // to. `gui::editor::callbacks::tier_actions::setup_load_selected_callback`'s
                // local branch is the one place this is ever `Some`.
                source_entry_id: None,
                // Item 82: a native/plain-`.asc` open carries a real recorded
                // schedule, never the angle-table reconstruction fallback.
                used_placeholder: false,
            });
            finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
            show_toast(
                ui,
                &format!(
                    "Loaded '{}' (plain .asc, no native sidecar found -- authored meet \
                     constraints and detached facets are not available).",
                    asc_path.display()
                ),
                "success",
            );
        }
        Err(e) => show_toast(ui, &format!("Cannot open: {e}"), "error"),
    }
}

/// The tail every "replace `EditorState` wholesale" open path shares, once the new
/// state is already stored: refresh the viewport/panel from it, then reset selection
/// unconditionally (a previously selected tier index now names, at best, an
/// unrelated row in whatever design just replaced it) and mark the fresh state clean.
/// Shared by [`commit_loaded_native`] (a native+`.asc` pair) and [`open_plain_asc`] (a
/// bare `.asc`) so the "reset selection, mark clean" sequence is written exactly once
/// for both -- the same reset `gui::editor::callbacks::tier_actions::apply_loaded_design`/
/// `setup_new_design_create_callback` run for Load Selected/New.
fn finish_state_replace(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    state: &Rc<RefCell<EditorState>>,
) {
    // A Deep Solve or Optimize verdict describes the design that was just replaced,
    // so it must not outlive it -- see `clear_analysis_results`' own doc comment.
    clear_analysis_results(ui);
    let st = state.borrow();
    refresh_all(ui, render_ctx, preview_state, solid_last_solved, &st);
    // cad_todo.md #248: names the window title (`MainWindow.loaded_design_name`)
    // after whatever design this replace just loaded -- covers both `open_native_pair`
    // and `open_plain_asc`, the two callers of this shared tail. Falls back to empty
    // (bare "Indicatrix Cut") only in the defensive case where `asc_filename` was
    // somehow never set, which neither caller actually does.
    ui.set_loaded_design_name(st.asc_filename.clone().unwrap_or_default().into());
    drop(st);
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    let pulse = ui.global::<EditorModel>().get_form_reset_pulse();
    ui.global::<EditorModel>()
        .set_form_reset_pulse(pulse.wrapping_add(1));
    // Explicit rather than left to `EditorModel.recompute_dirty`'s reactive `changed
    // tiers` hook alone (`refresh_all` above does reassign `tiers`, so that hook would
    // catch this too) -- a freshly replaced `EditorState` is clean by construction
    // (`saved_generation`/`generation` both start at `0`), and saying so directly
    // here is one line, easier to verify than tracing through the Slint side.
    ui.global::<EditorModel>().set_is_dirty(false);
}

/// Bundles [`commit_loaded_native`]'s per-call payload -- kept as one struct (rather
/// than five more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning `tier_actions::LoadedDesignOutcome` uses.
struct LoadedNativeOutcome {
    loaded: LoadPairedResult,
    /// What the toast calls the file -- the picked native file's own display path.
    native_path_display: String,
    asc_filename: String,
    asc_text: String,
    /// Drives the toast's class: `"warning"` (which `gui::show_toast` never
    /// auto-dismisses, same as `"error"`, but is coloured and captioned as a note
    /// rather than a failure -- Item 179) rather than `"info"`'s 3.5-second flash,
    /// since a mismatch means something about this design's authored intent may not
    /// have made the round trip -- worth a permanent, plainly-worded note, not a
    /// flash a cutter can miss mid-click, and not styled as an error when nothing
    /// actually failed.
    is_mismatch: bool,
}

/// Replaces `state` wholesale with `outcome.loaded`'s design and pushes the result
/// into the panel/viewport -- the shared tail [`do_open_native`]'s clean path and both
/// of [`setup_mismatch_dialog_callbacks`]'s committing branches (Apply Anyway/Use .asc
/// Only) all funnel through, so the "replace state, reset selection, report the
/// outcome" sequence is written exactly once.
fn commit_loaded_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    outcome: LoadedNativeOutcome,
) {
    let LoadedNativeOutcome {
        loaded,
        native_path_display,
        asc_filename,
        asc_text,
        is_mismatch,
    } = outcome;
    record_recent_native_file(ui, &native_path_display);
    // Item 171: this design's OWN sidecar just landed here -- see
    // `CURRENT_NATIVE_PATH`'s own doc comment for why a later Save Native re-writing
    // this exact path needs no overwrite confirmation.
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(PathBuf::from(&native_path_display)));
    let fingerprint_note = loaded.fingerprint.to_string();
    let overlay_note = loaded.tier_overlay.to_string();
    // Item 83 (minimum viable): a material name this build can't resolve on its own
    // silently becomes Diamond once `MaterialSelection::resolve` runs -- see
    // `MaterialResolution`'s own doc comment for why this is a heuristic, not a
    // guarantee. Surfaced here rather than swallowed, so at least the open toast
    // says so instead of nothing anywhere ever mentioning the substitution.
    let material_note = matches!(
        loaded.material_resolution,
        indicatrix_cut_core::native::MaterialResolution::Unresolved
    )
    .then(|| format!(" {}", loaded.material_resolution));
    // Item 181: `format_version` was written but never checked -- a sidecar from a
    // newer build could carry fields this one silently drops into `unknown` and
    // re-serializes on the next save (quietly degrading it further each round trip).
    // Warned rather than refused: every named field here already tolerates being
    // absent, so the design itself still loaded fine.
    let newer_version_note = loaded.written_by_newer_version.then(|| {
        " This file was written by a newer version of Indicatrix Cut; some settings \
          may not have been understood and could be lost on your next save."
            .to_string()
    });
    let is_mismatch = is_mismatch || material_note.is_some() || loaded.written_by_newer_version;

    // `replace_wholesale`, not a plain `*state.borrow_mut() = ...` -- see
    // `open_plain_asc`'s matching comment and `EditorState::replace_wholesale`'s own
    // doc comment (`state/mod.rs`) for why: this carries the OLD `generation` `Arc`
    // (and bumps it) across the replacement so a background Deep Solve/Optimize/
    // auto-solve dispatched against the design being replaced still observes the
    // change instead of comparing against a counter nobody increments anymore.
    // Item 178: restored from the sidecar's own `[source]` table (written by an
    // earlier Save Native -- see `EditorState::printed_proportions`'s own doc
    // comment) rather than hard-coded `None`, so Deep Solve still has printed figures
    // to verify against after a Save Native/Open Native round trip, not only on this
    // design's very first "Load Selected" from the catalogue. Still `None` for a
    // sidecar saved before Item 178, or one for a design never loaded from a
    // catalogue row at all.
    let printed_proportions = loaded.printed_proportions;
    state.borrow_mut().replace_wholesale(EditorState {
        design: loaded.design,
        history: History::new(),
        printed_proportions,
        generation: Arc::new(AtomicU64::new(0)),
        design_epoch: Arc::new(AtomicU64::new(0)),
        saved_generation: 0,
        pending_unsaved_action: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        asc_filename: Some(asc_filename),
        original_asc_text: Some(asc_text),
        pending_gear_remap: None,
        pending_retarget: None,
        multi_selected: std::collections::BTreeSet::new(),
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        // CAD audit items 92/96: same reasoning as the plain-`.asc` Open Native path
        // above -- the native sidecar's own `[source]` table (`printed_proportions`,
        // just above) carries a catalogue row's PRINTED proportions, but not which
        // row it was loaded from, so there is nothing here to write back to either.
        source_entry_id: None,
        // Item 82: a native/plain-`.asc` open carries a real recorded
        // schedule, never the angle-table reconstruction fallback.
        used_placeholder: false,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    show_toast(
        ui,
        &format!(
            "Loaded '{native_path_display}': {fingerprint_note}; {overlay_note}.{}{}",
            material_note.unwrap_or_default(),
            newer_version_note.unwrap_or_default()
        ),
        if is_mismatch { "warning" } else { "success" },
    );
}

/// The fingerprint-mismatch dialog's three resolution callbacks -- see
/// [`PendingMismatch`]/[`PENDING_MISMATCH`]. Registered once, from
/// [`setup_open_native_callback`], since only Open Native can ever populate
/// [`PENDING_MISMATCH`] in the first place.
fn setup_mismatch_dialog_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
) {
    let state_apply = Rc::clone(state);
    let render_ctx_apply = Arc::clone(render_ctx);
    let preview_state_apply = Arc::clone(preview_state);
    let solid_last_solved_apply = Arc::clone(solid_last_solved);
    let ui_weak_apply = ui.as_weak();
    ui.global::<EditorModel>()
        .on_mismatch_dialog_apply_anyway(move || {
            let Some(ui) = ui_weak_apply.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_mismatch_dialog_open(false);
            let Some(pending) = PENDING_MISMATCH.with(RefCell::take) else {
                return;
            };
            // `true`: the cutter just explicitly asked for the sidecar's meets to
            // apply despite the mismatch -- see `TierOverlay::AppliedDespiteMismatch`.
            match load_paired(&pending.asc_text, &pending.native_text, true) {
                Ok(loaded) => commit_loaded_native(
                    &ui,
                    &state_apply,
                    &render_ctx_apply,
                    &preview_state_apply,
                    &solid_last_solved_apply,
                    LoadedNativeOutcome {
                        loaded,
                        native_path_display: pending.native_path_display,
                        asc_filename: pending.asc_filename,
                        asc_text: pending.asc_text,
                        is_mismatch: true,
                    },
                ),
                Err(e) => show_toast(&ui, &format!("Cannot open: {e}"), "error"),
            }
        });

    let state_asc_only = Rc::clone(state);
    let render_ctx_asc_only = Arc::clone(render_ctx);
    let preview_state_asc_only = Arc::clone(preview_state);
    let solid_last_solved_asc_only = Arc::clone(solid_last_solved);
    let ui_weak_asc_only = ui.as_weak();
    ui.global::<EditorModel>()
        .on_mismatch_dialog_asc_only(move || {
            let Some(ui) = ui_weak_asc_only.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_mismatch_dialog_open(false);
            let Some(pending) = PENDING_MISMATCH.with(RefCell::take) else {
                return;
            };
            // `false` again: the cutter chose to keep the mismatch's overlay SKIPPED,
            // i.e. `TierOverlay::SkippedFingerprintMismatch` -- every authored meet
            // constraint and detached facet the sidecar carried is dropped, kept only
            // to whatever the plain `.asc` itself encodes.
            match load_paired(&pending.asc_text, &pending.native_text, false) {
                Ok(loaded) => commit_loaded_native(
                    &ui,
                    &state_asc_only,
                    &render_ctx_asc_only,
                    &preview_state_asc_only,
                    &solid_last_solved_asc_only,
                    LoadedNativeOutcome {
                        loaded,
                        native_path_display: pending.native_path_display,
                        asc_filename: pending.asc_filename,
                        asc_text: pending.asc_text,
                        is_mismatch: true,
                    },
                ),
                Err(e) => show_toast(&ui, &format!("Cannot open: {e}"), "error"),
            }
        });

    ui.global::<EditorModel>()
        .on_mismatch_dialog_cancel(move || {
            PENDING_MISMATCH.with(|cell| *cell.borrow_mut() = None);
        });
}

#[cfg(test)]
mod tests {
    use super::{NOT_CLOSED_SOLID_MARKER, degenerate_marker_header};
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{
        ConstraintTier, Design, FreshDesignSpec, MaterialSelection, PreformSpec, load_paired,
        save_paired,
    };

    /// Material name/RI override, gear, symmetry and mirror all round-trip through
    /// the exact pair of functions
    /// [`setup_save_native_callback`]/[`setup_open_native_callback`] call
    /// (`indicatrix_cut_core::save_paired`/`load_paired`) -- verified here directly rather
    /// than trusted, since this crate's own wiring exercises gear/symmetry/mirror
    /// persistence only through the editor's own "New Design"/design-settings forms,
    /// not through a dedicated round-trip test. `gear`/`symmetry`/
    /// `mirror` round-trip through the paired `.asc`'s own header (already
    /// exercised, indirectly, by every existing "Open Native" test in
    /// `indicatrix_cut_core::native`); `material`/`refractive_index_override` round-trip
    /// through the native sidecar's `[material]` table (already
    /// unit-tested in `indicatrix_cut_core::native` directly) -- this test's own
    /// value is confirming the ONE combination this app actually writes (a
    /// design with all four set together, via the same `save_paired`/
    /// `load_paired` this module's own callbacks call) survives intact.
    #[test]
    fn gear_symmetry_mirror_and_material_all_round_trip_through_save_and_open() {
        let spec = FreshDesignSpec {
            gear_teeth: 80,
            symmetry_order: 5,
            mirror: false,
            material: MaterialSelection {
                name: Some("Quartz".to_string()),
                specific_gravity_override: Some(2.65),
                refractive_index_override: Some(1.55),
            },
            preform: PreformSpec::cylinder(80, 1.4, 1.0, 1.3),
        };
        let mut design = Design::fresh_from_spec(spec);
        // A schedule with zero tiers exports (and re-solves) fine, but
        // `indicatrix_formats::asc::parse_asc` refuses to parse an `.asc` with no
        // facet ('a') records at all -- one real, anchored tier is what a
        // saved design would actually look like.
        design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 16.0, 32.0, 48.0, 64.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let saved = save_paired(&design, "roundtrip.asc", None, None, None)
            .expect("a fresh design must save");
        let loaded =
            load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");

        assert_eq!(loaded.design.meta.gear_teeth, 80);
        assert_eq!(loaded.design.meta.symmetry_order, 5);
        assert!(!loaded.design.meta.mirror);
        assert_eq!(loaded.design.material.name.as_deref(), Some("Quartz"));
        assert_eq!(loaded.design.material.specific_gravity_override, Some(2.65));
        assert_eq!(loaded.design.material.refractive_index_override, Some(1.55));
        // The effective RI actually written to `.asc`'s `I` line -- confirms
        // the override, not just the raw field, made the round trip in a way
        // that would show up in the exported schedule too.
        assert!((loaded.design.effective_refractive_index() - 1.55).abs() < 1e-9);
    }

    // --- degenerate_marker_header (Item 182) ---

    #[test]
    fn degenerate_marker_header_stamps_the_reason_when_absent() {
        let headers: Vec<String> = vec!["GemCad 5.0".to_string()];
        let header = degenerate_marker_header(&headers, "Degenerate: only 2 distinct vertices.")
            .expect("no existing marker -- must stamp one");
        assert!(header.starts_with(NOT_CLOSED_SOLID_MARKER));
        assert!(header.contains("Degenerate: only 2 distinct vertices."));
    }

    #[test]
    fn degenerate_marker_header_never_stamps_twice() {
        let headers = vec![format!("{NOT_CLOSED_SOLID_MARKER} -- already noted")];
        assert!(degenerate_marker_header(&headers, "a different message").is_none());
    }
}
