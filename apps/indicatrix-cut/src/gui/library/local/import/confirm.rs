//! The filename-collision confirmation flow: a cheap pre-scan of what an import
//! would replace, and the up-front Yes/No prompt shown when it would replace
//! anything at all.

use super::scan::collect_import_candidates;
use crate::MainWindow;
use indicatrix_vault::db::sqlite::Database;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// Filename-only pre-scan of what `super::pipeline::import_path` would find at
/// `path` -- how many of its candidate `.asc` files already share a name with a row
/// already in `db` (returned first), out of how many candidates total (returned
/// second). Uses the same `local://<file_name>` collision test the pipeline's own
/// save step applies later, since that url is derivable from the
/// file name alone -- no file is read or parsed here, so this costs only the
/// directory listing [`collect_import_candidates`] needs regardless.
///
/// `(0, 0)` for an unreadable path/empty folder: `import_path` reports that failure
/// itself once the import actually runs, so this pre-scan does not need to duplicate
/// it.
pub(super) fn count_pending_collisions(
    db: &Arc<Mutex<Database>>,
    path: &Path,
    recurse: bool,
) -> (usize, usize) {
    let (candidates, _gem_gcs_skipped) =
        collect_import_candidates(path, recurse).unwrap_or_default();
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let collisions = candidates
        .iter()
        .filter(|p| {
            let file_name = p.file_name().map_or_else(
                || "unknown.asc".to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            db.has_detail_for_entry_url(&format!("local://{file_name}"))
                .unwrap_or(false)
        })
        .count();
    (collisions, candidates.len())
}

/// When `collisions` is non-zero, opens the SAME in-window write-confirm dialog
/// `gui::editor::native_io::ask_write_confirm` already gives Save Native's own
/// prompts, naming how many of `total` candidate files would replace an existing
/// catalogue row, and runs `proceed` once the cutter accepts. Declining runs
/// nothing at all -- the catalogue stays untouched, exactly like cancelling the
/// picker itself does. Runs `proceed` immediately (synchronously) when
/// `collisions == 0`, the common case with nothing to ask about.
///
/// Reuses `EditorModel.write_confirm_*` (`ui/models/editor.slint`), a plain global
/// with no notion of which part of the app is asking, so no new Slint callback is
/// needed -- only `native_io::ask_write_confirm` itself becoming reachable from here
/// (see that function's own doc comment).
pub(super) fn confirm_collisions_then(
    ui: &MainWindow,
    collisions: usize,
    total: usize,
    proceed: impl FnOnce(&MainWindow) + 'static,
) {
    if !collisions_need_confirmation(collisions) {
        proceed(ui);
        return;
    }
    crate::gui::editor::native_io::ask_write_confirm(
        ui,
        "Replace existing design(s)?",
        format!(
            "{collisions} of {total} file(s) have the same name as a design already in \
             the catalogue and will REPLACE it (filename-only match, not a content \
             comparison). Continue?"
        ),
        "Continue",
        // Not suppressible -- unlike
        // `native_io::confirm_keys`'s two prompts (which only ever change whether a
        // header note gets written), silencing this one would let a cutter
        // permanently stop being warned before a catalogue row gets overwritten.
        None,
        proceed,
    );
}

/// [`confirm_collisions_then`]'s pure decision -- whether ANY collision means the
/// dialog is needed at all. Pulled out so a test can call it directly, matching
/// `gui::editor::native_io::decide_write_status`'s own "pure decision, `ui`-touching
/// action kept separate" split -- `confirm_collisions_then` itself is not
/// exercised end-to-end here, since `ask_write_confirm` needs a live
/// `slint::ComponentHandle` (a real `MainWindow`) this crate's test environment
/// cannot start (the same constraint `solve_service`/`edit_intent`'s own test
/// modules document).
#[must_use]
pub(super) const fn collisions_need_confirmation(collisions: usize) -> bool {
    collisions > 0
}
