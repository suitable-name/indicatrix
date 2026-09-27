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
//! solve, never inline), [`confirm`] (Group 2: the shared in-window write-confirm
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

use std::{cell::RefCell, path::PathBuf};

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
}

// Every symbol below is re-exported at exactly the visibility the pre-split
// `native_io.rs` gave it -- see each submodule's own item for why. A re-export
// can only match or narrow an item's own declared visibility, never widen it, so
// each item is declared `pub(in crate::gui::editor)` (or `pub(in crate::gui)` for
// `ask_write_confirm`) directly at its definition site.
pub(in crate::gui::editor) use autosave::find_leftover_autosave;
pub(in crate::gui) use confirm::ask_write_confirm;
pub(in crate::gui::editor) use export::{
    setup_export_asc_callback, setup_export_cutting_sheet_callback, setup_export_diagram_callback,
};
pub(in crate::gui::editor) use open_native::{
    do_open_native, open_recent_native_path, setup_open_native_callback,
};
pub(in crate::gui::editor) use save::setup_save_native_callback;
pub(in crate::gui::editor) use solve::resolve_solved_then;
