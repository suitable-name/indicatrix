//! [`super::save`]'s success tail: records the just-written location, refreshes
//! `EditorState`'s saved-generation bookkeeping, toasts the outcome, and reports the
//! catalogue write-back's own result.

use super::{
    CURRENT_NATIVE_PATH,
    autosave::{autosave_path, record_recent_native_file},
    catalogue::CatalogueWriteBack,
};
use crate::{
    EditorModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{editor::state::EditorState, show_toast},
};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Records where a Save/Export just wrote, for the status strip's own persistent
/// "Saved: ..." segment and its click-to-reveal. The strip
/// gets the bare file name (all a 26px strip has room for) and the full path
/// separately, for the Details popup and for the reveal itself -- see
/// `EditorModel.last_saved_path`'s own doc comment. A path with no file-name
/// component cannot come out of a save dialog, but is handled anyway by falling
/// back to the full path rather than clearing the label.
pub(super) fn record_last_saved_path(ui: &MainWindow, path: &Path) {
    let full = path.display().to_string();
    let label = path
        .file_name()
        .map_or_else(|| full.clone(), |name| name.to_string_lossy().into_owned());
    let model = ui.global::<EditorModel>();
    model.set_last_saved_path(full.into());
    model.set_last_saved_label(label.into());
}

/// [`write_native_save`]'s background-thread result, once both the file write and
/// the catalogue write-back (Group 4: "files first then catalogue") have already
/// run -- everything [`finish_save_native_success`] needs to finish on the UI
/// thread. `catalogue` is its own `Result`, independent of the outer one this is
/// wrapped in by [`write_native_save`]: a catalogue failure never means the save
/// itself failed (see [`write_back_to_catalogue`]'s own doc comment), so it is
/// reported on its own rather than folded into a single combined error.
pub(super) struct WriteNativeOutcome {
    pub(super) dest_path: PathBuf,
    pub(super) native_path: PathBuf,
    pub(super) asc_filename: String,
    pub(super) paired: indicatrix_cut_core::native::PairedSave,
    pub(super) catalogue: Result<(i64, CatalogueWriteBack), String>,
    /// `EditorState::generation`'s value at the moment `design` was cloned out
    /// of `state`, well before this write ever started (`NativeSaveContext::
    /// snapshot_generation`'s own doc comment) -- what
    /// [`finish_save_native_success`] actually marks the design clean AT,
    /// instead of whatever `generation` reads once this outcome lands (which
    /// may already include post-click edits this save never saw).
    pub(super) snapshot_generation: u64,
}

/// [`write_native_save`]'s success tail, run back on the UI thread once its
/// background thread reports in -- split out purely to keep that function under
/// clippy's line-count lint. Drops any in-progress autosave (now strictly older
/// than what's actually on disk), records `native_path` as the most recent Open
/// Recent entry and this state's own file (see [`CURRENT_NATIVE_PATH`]'s doc
/// comment), updates `EditorState`'s own saved-generation bookkeeping, and toasts
/// the outcome -- a draft note when
/// [`indicatrix_cut_core::native::PairedSave::draft_reason`] is `Some`, an ordinary
/// success note otherwise -- then reports `outcome.catalogue`'s own result.
pub(super) fn finish_save_native_success(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    outcome: WriteNativeOutcome,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let WriteNativeOutcome {
        dest_path,
        native_path,
        asc_filename,
        paired,
        catalogue,
        snapshot_generation,
    } = outcome;
    // The real save just landed -- any in-progress autosave is now strictly
    // older than what's on disk, so drop it rather than leaving a stale
    // recovery file behind (it never outlives the real save it stood in for).
    let _ = std::fs::remove_file(autosave_path(Some(asc_filename.as_str())));
    record_recent_native_file(ui, &native_path.display().to_string());
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(native_path.clone()));
    // Names the window title (`MainWindow.loaded_design_name`) after the file
    // that was just saved.
    ui.set_loaded_design_name(asc_filename.clone().into());
    // The `.asc` half, not the `.indicatrix.toml`: both land in the same folder
    // (so the reveal is identical either way) and the `.asc` is the one a cutter
    // hands to a machine or another program.
    record_last_saved_path(ui, &dest_path);

    {
        let mut st = state.borrow_mut();
        st.asc_filename = Some(asc_filename);
        st.original_asc_text = Some(paired.asc_text);
        // A draft save is still a real, successful save -- `design` not
        // currently solving does not fail `save_paired` at all (see
        // `PairedSave::draft_reason`'s own doc comment) -- so this is unconditional,
        // not only on the ordinary branch below.
        //
        // `snapshot_generation` (the generation `design` was cloned out of
        // `state` at, before this write even started), never the LIVE
        // `st.generation` read here -- see `WriteNativeOutcome::
        // snapshot_generation`'s own doc comment. Reading the live
        // counter instead let edits made while this save was still
        // resolving/writing read as clean the moment the (already stale) save
        // completed, so the close guard never prompted for them and they were
        // silently lost.
        st.saved_generation = snapshot_generation;
    }
    ui.global::<EditorModel>()
        .set_is_dirty(state.borrow().is_dirty());

    // `draft_reason` is `Some` iff `design` did not currently solve, or had no
    // tiers at all, and this save fell back to a placeholder-mast `.asc` (see
    // `save_paired`'s own doc comment and `indicatrix_cut_core::native::DraftReason`)
    // -- worded so the cutter knows the file IS on disk but is not yet a real cut
    // instruction, rather than seeing a "Cannot save" error. `DraftReason`'s own
    // `Display` already names which of the two happened, so it is not repeated
    // here.
    if let Some(reason) = &paired.draft_reason {
        // This is a critical outcome -- the cutter's `.asc` is a placeholder, not
        // a real cut instruction -- so it gets the same persistent "warning" class
        // as the fingerprint-mismatch toast just below, not an "info" flash that
        // can vanish before it is read.
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
        // The real condition (see
        // `crates/indicatrix-cut-core/src/native/save.rs`'s semantic-equality
        // gate) is that every meet note got resynthesized, which is what the
        // manual (docs/manual/11-saving-and-file-formats.md:57-61) documents --
        // the toast says so explicitly so it does not disagree with the manual on
        // how alarming this is.
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

    // Registers (or updates) this design's own catalogue
    // row -- see `write_back_to_catalogue`'s own doc comment for exactly what gets
    // overwritten versus preserved. Already run, on a background thread, by
    // `write_native_save` (Group 4: "files first then catalogue, results back via
    // the event loop") -- this only reports `catalogue`'s own outcome. Never runs
    // after "Export .asc" (`setup_export_asc_callback`), which keeps its own
    // documented "file only, no database write of any kind" rule.
    report_catalogue_write_back(ui, state, db, source, &dest_path, catalogue);

    // the save this guard's own Save resolution triggered just actually
    // landed -- see `AfterSave`'s own doc comment. Runs last, after this
    // function's own bookkeeping/toasts above, so a listener that hides the
    // window (the close-confirm guard) does so only once every other effect
    // of a successful save has already happened.
    super::notify_save_completed(ui, state);
}

/// Reports [`write_back_to_catalogue`]'s result on the UI thread: adopts the written
/// row as the design's `source_entry_id`, refreshes the library list, and toasts the
/// outcomes the cutter should not miss. `dest_path` names the saved `.asc` for the
/// collision wording.
fn report_catalogue_write_back(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    dest_path: &Path,
    catalogue: Result<(i64, CatalogueWriteBack), String>,
) {
    match catalogue {
        // Another catalogue design already owns this file name, so the catalogue was
        // left untouched. The id is that design's: adopting it as `source_entry_id`
        // would make the next save overwrite it, so it is deliberately not adopted.
        Ok((_, CatalogueWriteBack::UrlCollision { existing_id, title })) => {
            let file = dest_path.file_name().map_or_else(
                || dest_path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            show_toast(
                ui,
                &format!(
                    "Saved to disk; the catalogue already holds a design named '{file}' \
                     (id {existing_id}, \"{title}\"), catalogue not updated. Import it \
                     explicitly to replace."
                ),
                "warning",
            );
        }
        Ok((id, catalogue_outcome)) => {
            // Every save after this one for a previously row-less (or
            // now-row-less, see `CatalogueWriteBack::SourceRowGoneNewRowCreated`)
            // design must update THIS row, never insert a second -- see
            // `EditorState::source_entry_id`'s own doc comment.
            state.borrow_mut().source_entry_id = Some(id);
            crate::gui::library::local::refresh_after_library_change(ui, db, source);
            match catalogue_outcome {
                CatalogueWriteBack::SourceRowGoneNewRowCreated => {
                    show_toast(
                        ui,
                        "This design's catalogue row no longer exists (it may have been \
                         deleted) -- saved as a new catalogue entry instead of updating it.",
                        "info",
                    );
                }
                // The design itself saved fine (this whole function only runs after
                // `write_pair_atomically` already succeeded) -- only the library's
                // cached previews/tilt curves for this row could not be cleared, so
                // the cutter needs to know not to trust them at a glance rather than
                // this failing silently into `tracing::warn!` alone.
                CatalogueWriteBack::UpdatedExisting {
                    stale_cache_invalidation_failed: true,
                } => {
                    show_toast(
                        ui,
                        "Saved, but this design's cached preview image and tilt curves in \
                         the library could not be cleared automatically -- they may still \
                         show the geometry from before this save. Re-export or recompute \
                         them if they look wrong.",
                        "warning",
                    );
                }
                // `UrlCollision` is reported by the arm above and never reaches here.
                CatalogueWriteBack::UpdatedExisting {
                    stale_cache_invalidation_failed: false,
                }
                | CatalogueWriteBack::NewRow
                | CatalogueWriteBack::UrlCollision { .. } => {}
            }
        }
        Err(message) => show_toast(ui, &format!("Catalogue not updated: {message}"), "error"),
    }
}
