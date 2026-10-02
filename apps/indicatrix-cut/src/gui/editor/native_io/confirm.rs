//! Group 2: the shared in-window write-confirm dialog (`EditorModel.write_confirm_*`)
//! every write path in [`super`] uses instead of a blocking native message dialog --
//! [`decide_write_status`] (pure decision) and [`ask_write_confirm`] (shows the
//! dialog, stashes the continuation).

use crate::{
    EditorModel, MainWindow, gui::editor::state::EditorState, settings::SettingsPersister,
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc};
use tracing::warn;

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

/// Stable [`AppSettings::suppressed_confirmations`] keys for the suppressible
/// write-confirm prompt. The unsaved-changes/fingerprint-mismatch/gear-remap guards
/// use their own separate `ConfirmActionDialog` mounts (`app.slint`) and are
/// deliberately NOT wired to `show_dont_ask` at all -- a wrong "don't ask again"
/// there risks real data loss, unlike this one (which only ever affects whether a
/// header note gets written).
pub(super) mod confirm_keys {
    /// [`super::super::save::finish_native_save`]/the Export `.asc` path's "this design is not
    /// a closed solid" prompt.
    pub(in crate::gui::editor::native_io) const NOT_CLOSED_SOLID: &str =
        "write_confirm.not_closed_solid";
}

/// Reads whether the confirm prompt named `key` is currently suppressed, from the
/// application's [`SettingsPersister`] (the handle installed by `gui::main_window`; see
/// [`SettingsPersister::install_for_this_thread`]) so a suppression chosen earlier in
/// this same session counts immediately. With no persister installed nothing is
/// suppressed, so the prompt is shown.
fn confirm_is_suppressed(key: &str) -> bool {
    SettingsPersister::installed_for_this_thread()
        .is_some_and(|persister| persister.snapshot().settings.is_confirm_suppressed(key))
}

/// Records that the confirm prompt named `key` should not be shown again.
///
/// The choice goes through the [`SettingsPersister`], never straight into the
/// settings file: the persister's in-memory snapshot is what every close path flushes
/// over the file, so a suppression written around it would be erased on exit. The
/// in-memory update is synchronous (the very next `confirm_is_suppressed` sees it);
/// only the disk write is debounced, and a failed write only means the NEXT
/// save/export asks again -- never that this save/export itself failed or that any
/// design data was lost.
fn suppress_confirm_permanently(key: &'static str) {
    let Some(persister) = SettingsPersister::installed_for_this_thread() else {
        warn!("No settings persister is installed; not suppressing the prompt `{key}`");
        return;
    };
    persister.update(|file| file.settings.suppress_confirm(key));
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
pub(super) fn setup_write_confirm_dialog_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
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
        // Record the suppression BEFORE running `on_accept` -- that closure may itself
        // trigger a save whose settings snapshot (e.g. `record_recent_native_file`) must
        // already include it; the persister's in-memory update is synchronous.
        if let Some(key) = pending.suppress_key
            && model.get_write_confirm_dont_ask()
        {
            suppress_confirm_permanently(key);
        }
        crate::gui::editor::stall_guard::stall_guard("write_confirm_accept", || {
            (pending.on_accept)(&ui);
        });
    });
    let state_cancel = Rc::clone(state);
    ui.global::<EditorModel>().on_write_confirm_cancel(move || {
        // The cutter's explicit cancel -- no toast, nothing runs.
        PENDING_WRITE_CONFIRM.with(|cell| *cell.borrow_mut() = None);
        // This dialog is shared with `gui::library::local::import`'s own
        // "replace existing design(s)?" prompt (see this module's own doc
        // comment), which never touches `after_save` at all -- clearing it
        // unconditionally here is still correct: a Save/Export flow is the
        // only thing that ever SETS it, and the two dialogs cannot be open at
        // once (both are modal), so there is nothing else this could
        // possibly clobber. Left dangling otherwise, a stashed `after_save`
        // would make some LATER, unrelated successful save spuriously close
        // the window or resume a stale pending action; see `AfterSave`'s own doc
        // comment.
        state_cancel.borrow_mut().after_save = None;
    });
}
