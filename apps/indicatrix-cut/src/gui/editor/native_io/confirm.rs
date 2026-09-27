//! Group 2: the shared in-window write-confirm dialog (`EditorModel.write_confirm_*`)
//! every write path in [`super`] uses instead of a blocking native message dialog --
//! [`decide_write_status`] (pure decision) and [`ask_write_confirm`] (shows the
//! dialog, stashes the continuation).

use crate::{EditorModel, MainWindow};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use slint::ComponentHandle;
use std::cell::RefCell;

// --- Group 2: in-window write confirmations (no more blocking native message dialogs) -

/// [`decide_write_status`]'s outcome -- pure decision from a design's own status,
/// no dialog shown yet.
pub(super) enum StatusDecision {
    /// `design.status()` names no problem -- proceed without asking.
    Fine,
    /// A real problem exists; the cutter must be asked before writing. Carries the
    /// same message the confirm dialog shows (and, if accepted, the message
    /// [`degenerate_marker_header`] stamps into the written file).
    NeedsConfirm(String),
}

/// Decides whether a design can be written based on its status: takes an
/// already-resolved solve (see [`resolve_solved_then`]) instead of solving `design`
/// itself, upholding the principle that the UI thread never solves.
/// `solved`'s `Err` side is the stringified solve error (`MissingAnchor`'s own
/// `Display`, via `DesignSolveError`'s -- see `resolve_solved_then`'s own doc
/// comment).
#[must_use]
pub(super) fn decide_write_status(
    design: &Design,
    solved: Result<&[SolvedTier], &str>,
) -> StatusDecision {
    match solved {
        Ok(solved) => {
            let (message, is_problem) =
                crate::gui::editor::state::status_text_and_is_problem_from_solved(design, solved);
            if is_problem {
                StatusDecision::NeedsConfirm(message)
            } else {
                StatusDecision::Fine
            }
        }
        Err(missing) => StatusDecision::NeedsConfirm(missing.to_string()),
    }
}

/// [`ask_write_confirm`]'s stashed continuation.
struct PendingWriteConfirm {
    on_accept: Box<dyn FnOnce(&MainWindow)>,
    /// The [`AppSettings::suppressed_confirmations`] key this prompt offers
    /// "Don't ask again" under, if any -- read (alongside
    /// `EditorModel.write_confirm_dont_ask`'s own live checkbox state) by
    /// `setup_write_confirm_dialog_callbacks`'s accept handler, to decide whether
    /// to persist the suppression before running [`Self::on_accept`].
    suppress_key: Option<&'static str>,
}

/// Stable [`AppSettings::suppressed_confirmations`] keys for the two suppressible
/// write-confirm prompts. The unsaved-changes/fingerprint-mismatch/gear-remap guards
/// use their own separate `ConfirmActionDialog` mounts (`app.slint`) and are
/// deliberately NOT wired to `show_dont_ask` at all -- a wrong "don't ask again"
/// there risks real data loss, unlike these two (which only ever affect whether a
/// header note gets written).
pub(super) mod confirm_keys {
    /// [`super::super::save::finish_native_save`]/the Export `.asc` path's "this design is not
    /// a closed solid" prompt.
    pub(in crate::gui::editor::native_io) const NOT_CLOSED_SOLID: &str =
        "write_confirm.not_closed_solid";
    /// [`super::super::save::confirm_overwrite_unrelated_native_file_then`]'s "overwrite a
    /// native file that belongs to a different design" prompt.
    pub(in crate::gui::editor::native_io) const OVERWRITE_UNRELATED_NATIVE: &str =
        "write_confirm.overwrite_unrelated_native";
}

/// Reads whether the confirm prompt named `key` is currently suppressed -- a
/// direct settings-file read (not the debounced `SettingsPersister`, which this
/// module has no handle to; same precedent [`record_recent_native_file`]
/// documents on itself for the identical constraint).
fn confirm_is_suppressed(key: &str) -> bool {
    let settings_path = crate::settings::store::default_settings_path();
    crate::settings::store::load_or_default(&settings_path)
        .settings
        .is_confirm_suppressed(key)
}

/// Persists that the confirm prompt named `key` should not be shown again.
///
/// `let _ =` on the write: a failure here (a read-only settings directory, say)
/// only means the NEXT save/export asks again -- never that this save/export
/// itself failed or that any design data was lost, and `record_recent_native_file`
/// already accepts the identical direct-write race/failure mode on this exact
/// settings file for the same reason (no debounced-persister handle in this
/// module).
fn suppress_confirm_permanently(key: &'static str) {
    let settings_path = crate::settings::store::default_settings_path();
    let mut file = crate::settings::store::load_or_default(&settings_path);
    file.settings.suppress_confirm(key);
    let _ = crate::settings::store::save(&settings_path, &file);
}

thread_local! {
    /// The continuation [`ask_write_confirm`] stashed, if the write-confirm dialog
    /// is currently open -- `None` whenever it is closed (the common state), the
    /// same shape [`PENDING_MISMATCH`] uses for its own dialog.
    static PENDING_WRITE_CONFIRM: RefCell<Option<PendingWriteConfirm>> = const { RefCell::new(None) };
}

/// Opens the shared in-window write-confirm dialog (`EditorModel.write_confirm_*`,
/// mounted in `ui/app.slint`) and stashes `on_accept` to run once the cutter
/// accepts. A decline (Cancel) drops it with no toast.
///
/// Reused by `gui::library::local::import`'s "replace existing design(s)?" prompt
/// (see that module's own `confirm_collisions_then`). `EditorModel.write_confirm_*`
/// is a plain global with no notion of which caller is asking, so there is nothing
/// import-specific this dialog needs to know; only this function (and the
/// `PENDING_WRITE_CONFIRM` continuation it stashes) is reachable from outside this
/// module.
///
/// `suppress_key`: `Some` offers "Don't ask again" (`ConfirmActionDialog.
/// show_dont_ask`), persisting the choice via [`suppress_confirm_permanently`] the
/// moment the cutter accepts WITH the checkbox ticked -- and, before even opening
/// the dialog, skips it entirely (runs `on_accept` immediately) if this same key
/// was already suppressed on an earlier run. `None` keeps asking every time,
/// with no checkbox shown at all.
pub(in crate::gui) fn ask_write_confirm(
    ui: &MainWindow,
    heading: &str,
    message: String,
    primary_label: &str,
    suppress_key: Option<&'static str>,
    on_accept: impl FnOnce(&MainWindow) + 'static,
) {
    if suppress_key.is_some_and(confirm_is_suppressed) {
        on_accept(ui);
        return;
    }
    PENDING_WRITE_CONFIRM.with(|cell| {
        *cell.borrow_mut() = Some(PendingWriteConfirm {
            on_accept: Box::new(on_accept),
            suppress_key,
        });
    });
    let model = ui.global::<EditorModel>();
    model.set_write_confirm_heading(heading.into());
    model.set_write_confirm_message(message.into());
    model.set_write_confirm_primary_label(primary_label.into());
    model.set_write_confirm_show_dont_ask(suppress_key.is_some());
    model.set_write_confirm_dont_ask(false);
    model.set_write_confirm_open(true);
}

/// Registers the write-confirm dialog's own two callbacks -- bundled into
/// [`setup_save_native_callback`] (called exactly once, like every other `setup_*`
/// entry point here) rather than given its own, the same reasoning
/// [`setup_dirty_tracking`]/[`setup_autosave_timer`] document on themselves.
pub(super) fn setup_write_confirm_dialog_callbacks(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_write_confirm_accept(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<EditorModel>();
        model.set_write_confirm_open(false);
        let Some(pending) = PENDING_WRITE_CONFIRM.with(RefCell::take) else {
            return;
        };
        // Persist the suppression BEFORE running `on_accept` -- that closure may
        // itself trigger a save that reads this same settings file back (e.g.
        // `record_recent_native_file`), so the write must land first.
        if let Some(key) = pending.suppress_key
            && model.get_write_confirm_dont_ask()
        {
            suppress_confirm_permanently(key);
        }
        crate::gui::editor::stall_guard::stall_guard("write_confirm_accept", || {
            (pending.on_accept)(&ui);
        });
    });
    ui.global::<EditorModel>().on_write_confirm_cancel(move || {
        // The cutter's explicit cancel -- no toast, nothing runs.
        PENDING_WRITE_CONFIRM.with(|cell| *cell.borrow_mut() = None);
    });
}
