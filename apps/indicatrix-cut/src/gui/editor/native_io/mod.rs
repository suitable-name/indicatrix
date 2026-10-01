//! File-based import/export for the Edit tab: exporting the edited schedule as a
//! plain `.asc` ([`export::setup_export_asc_callback`]), and the paired native
//! `.indicatrix.toml` save/open ([`save::setup_save_native_callback`]/
//! [`open_native::setup_open_native_callback`]; legacy `.gemcut.toml` sidecars still
//! open). See this group's own `mod.rs` doc comment.
//!
//! # Module split
//!
//! Three groups of shared infrastructure every write/export/autosave/open path
//! below uses: [`solve`] (Group 1: a matching cached solve, else a background
//! solve, never inline -- `resolve_solve_at` for the generation-tagged write
//! paths with a typed failure, `resolve_solved_then` for the plain one), [`confirm`] (Group 2: the shared in-window write-confirm
//! dialog, no blocking native message dialogs), and [`picker`] (Group 3: file
//! pickers off the UI thread). [`save_helpers`]/[`catalogue`] hold the save-side
//! helpers (custom-material snapshot, degenerate-marker header, catalogue
//! write-back). Then one module per write path: [`export`] ("Export .asc"/"Export
//! Cutting Sheet"/"Export Diagram"), [`save`]/[`save_finish`]/[`atomic_write`]
//! ("Save Native"'s dialog wiring, its UI-thread success tail, and the
//! stage-then-rename disk write), [`autosave`] (Group 4: the two-minute recovery
//! snapshot), and [`open_native`]/[`open_picker`]/[`open_commit`] ("Open Native"'s
//! entry points, its file-reading/parsing, and committing a loaded design into
//! `EditorState`). Tests live in their own [`tests`] module.

mod atomic_write;
mod autosave;
mod catalogue;
mod confirm;
mod export;
mod open_commit;
mod open_native;
mod open_picker;
mod picker;
mod save;
mod save_finish;
mod save_helpers;
mod solve;
#[cfg(test)]
mod tests;

use crate::{MainWindow, gui::editor::state::EditorState};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

// Re-exported at the SAME `pub(in crate::gui)` restriction `AfterSave` is already
// declared with at its own definition site (`state::core`) -- `gui::window_close`
// is a sibling of `gui::editor`, not a descendant, and cannot write
// `crate::gui::editor::state::AfterSave` directly (`state` itself is a private
// module, visible only within `editor` and its own descendants). Naming it here
// instead works because `native_io` (a descendant of `editor`) already has
// `pub(in crate::gui)` visibility of its own (see `ask_write_confirm`'s matching
// comment on the re-export block below) and CAN see `state::AfterSave` from
// inside `editor`'s subtree.
pub(in crate::gui) use crate::gui::editor::state::AfterSave;

thread_local! {
    /// The native `.indicatrix.toml` path this `EditorState` was last loaded from or
    /// saved to, if any -- `native_path` in [`save::setup_save_native_callback`]
    /// is DERIVED from whatever `.asc` name the cutter just picked
    /// ([`indicatrix_cut_core::native_path_for_asc`]), never itself chosen through the
    /// native save dialog, so the OS's own "this file already exists" prompt (which
    /// covers only the `.asc` half) never sees it. Comparing against this lets a Save
    /// tell "the same design's own sidecar, safe to overwrite" (already covered by
    /// [`atomic_write::write_pair_atomically`]'s own `.bak` backup) apart from "some
    /// unrelated design's sidecar that happens to share this `.asc` name," which gets
    /// an explicit native OS confirm instead. A plain `RefCell`, not part of
    /// `EditorState` itself: it names a location on disk, not design data, so it must
    /// not be reset by [`crate::gui::editor::state::EditorState::replace_wholesale`]
    /// the way every other field on that struct is -- see that method's own doc
    /// comment. Shared across this whole module tree (save/save_finish/open_native/
    /// open_commit all read or write it), so it lives here rather than in any one
    /// submodule.
    static CURRENT_NATIVE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };

    /// The live `EditorState`, stashed the one time [`save::setup_save_native_callback`]
    /// runs (there is only ever one `EditorState` for the app's lifetime) so
    /// [`request_save_then_close`] can reach it from `gui::window_close`'s
    /// close-confirm guard -- a sibling of `gui::editor`, not a descendant, so it
    /// cannot hold this crate's one `Rc<RefCell<EditorState>>` itself without
    /// `main_window.rs`'s own setup call threading it through as a new parameter.
    /// Mirrors `callbacks::tier_actions::lifecycle::REMOTE_LOAD_TARGET`'s identical
    /// trick, for the identical reason (a plain `Rc` is not `Send` and cannot become
    /// a new parameter on a call site outside this module).
    static LIVE_EDITOR_STATE: RefCell<Option<Rc<RefCell<EditorState>>>> = const { RefCell::new(None) };

    /// Listeners [`notify_save_completed`] calls, in registration order, every time
    /// a native save just finished successfully -- see [`on_save_completed`]'s own
    /// doc comment.
    static SAVE_COMPLETED_LISTENERS: RefCell<Vec<SaveCompletedListener>> =
        RefCell::new(Vec::new());
}

/// One [`on_save_completed`] listener, boxed so [`SAVE_COMPLETED_LISTENERS`] can hold
/// a heterogeneous list of them.
type SaveCompletedListener = Box<dyn Fn(&MainWindow, &Rc<RefCell<EditorState>>)>;

/// Stashes `state` for [`request_save_then_close`] to reach later -- called once,
/// from [`save::setup_save_native_callback`].
pub(super) fn remember_editor_state(state: &Rc<RefCell<EditorState>>) {
    LIVE_EDITOR_STATE.with(|cell| *cell.borrow_mut() = Some(Rc::clone(state)));
}

/// `gui::window_close`'s close-confirm "Save" handler's own entry point: marks
/// [`EditorState::after_save`] as [`AfterSave::CloseWindow`] before that handler
/// invokes `EditorModel.save_native` -- see [`AfterSave`]'s own doc comment
/// for why this replaces a synchronous `is_dirty` check run right after that call
/// returns. A silent no-op if [`remember_editor_state`] has not run yet, which
/// cannot happen once `setup_editor_callbacks` has (see that function's own call
/// order) -- by the time a cutter can close the window at all, it already has.
pub(in crate::gui) fn request_save_then_close() {
    LIVE_EDITOR_STATE.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            state.borrow_mut().after_save = Some(AfterSave::CloseWindow);
        }
    });
}

/// Registers a listener [`notify_save_completed`] calls every time a native save
/// just finished successfully (draft or real alike -- both actually wrote a file).
///
/// Exists for [`AfterSave`]: the window-close guard and the New/Load
/// Selected/Open Native unsaved-changes guard each need to run something once an
/// otherwise-asynchronous Save actually lands, but neither owns the other's own
/// render/preview/settings handles, and threading every caller's own parameters
/// through the whole write path (`save::setup_save_native_callback` ->
/// `finish_native_save` -> `write_native_save` -> its background thread's
/// `upgrade_in_event_loop` hop -> [`save_finish::finish_save_native_success`])
/// would touch call sites this lane's file list does not include. Each guard
/// registers its own listener instead, closing over whatever it already has.
///
/// A listener that does not recognize the `after_save` value it is handed (it
/// belongs to the OTHER guard, or there is none right now) must leave it exactly
/// as found rather than dropping it via a plain `Option::take`/// `gui::window_close::setup_close_confirm_callbacks`'s and
/// `callbacks::tier_actions::lifecycle::setup_unsaved_guard_dispatch`'s matching
/// "put it back" comment for why: only one listener may ever actually consume a
/// given value, and a listener registered first must not silently steal a value
/// meant for the other.
pub(in crate::gui) fn on_save_completed(
    listener: impl Fn(&MainWindow, &Rc<RefCell<EditorState>>) + 'static,
) {
    SAVE_COMPLETED_LISTENERS.with(|cell| cell.borrow_mut().push(Box::new(listener)));
}

/// [`save_finish::finish_save_native_success`]'s own call into every
/// [`on_save_completed`] listener -- see that function's own doc comment.
pub(super) fn notify_save_completed(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    SAVE_COMPLETED_LISTENERS.with(|cell| {
        for listener in &*cell.borrow() {
            listener(ui, state);
        }
    });
}

// Every symbol below is re-exported at exactly the visibility the pre-split
// `native_io.rs` gave it -- see each submodule's own item for why. A re-export
// can only match or narrow an item's own declared visibility, never widen it, so
// each item is declared `pub(in crate::gui::editor)` (or `pub(in crate::gui)` for
// `ask_write_confirm`) directly at its definition site.
pub(in crate::gui::editor) use autosave::{delete_leftover_autosave, find_leftover_autosave};
pub(in crate::gui) use confirm::ask_write_confirm;
pub(in crate::gui::editor) use export::{
    setup_export_asc_callback, setup_export_cutting_sheet_callback, setup_export_diagram_callback,
};
pub(in crate::gui::editor) use open_native::{
    do_open_native, open_recent_native_path, setup_open_native_callback,
};
pub(in crate::gui::editor) use save::setup_save_native_callback;
pub(in crate::gui::editor) use solve::resolve_solved_then;
