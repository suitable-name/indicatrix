//! "Open": loads a `.isdd` design file (or a legacy `.indicatrix` one, read exactly
//! the same), a bare `.asc` or a `.gem`/`.gcs`, replacing the whole editor state the
//! same way "New"/"Load Selected" do. [`setup_open_native_callback`]/
//! [`open_recent_native_path`] are this group's entry points; [`do_open_native`]
//! dispatches a picked file to [`super::open_commit`]'s replace-state paths.

use super::{
    open_commit::{
        DesignFileLoad, ReplaceGuard, open_converted_design, open_design_file, open_plain_asc,
    },
    open_picker::{PickedNative, pick_design_then, read_picked_path_then},
};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::state::{EditorState, PendingUnsavedAction},
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// "Open": loads a `.isdd` design file (or a legacy `.indicatrix` one), a `.asc` or a
/// `.gem`/`.gcs`. Replaces the
/// whole editor state the same way "New"/"Load Selected" do (fresh `History`, no
/// printed proportions unless the file carries them -- a locally opened file has no
/// catalogue row to verify Deep Solve against, exactly like a brand-new design).
///
/// Checks [`EditorState::is_dirty`] BEFORE doing anything else -- including before
/// showing the file picker -- exactly like `setup_new_design_create_callback`/
/// `setup_load_selected_callback` do for New/Load Selected: asking "keep unsaved
/// changes?" only after making the user pick a file would be backwards. A dirty
/// design stashes [`PendingUnsavedAction::OpenNative`] and opens the guard dialog
/// instead of proceeding; [`setup_unsaved_guard_dispatch`]
/// (`gui::editor::callbacks::tier_actions`) resumes by calling [`do_open_native`]
/// again once Save/Discard is chosen, which re-shows the picker from scratch. The
/// picker and the file read are asynchronous and the editor stays interactive, so
/// [`do_open_native`] re-checks for edits made since that decision
/// ([`ReplaceGuard`]) when the pick lands, before anything is replaced.
///
/// Also registers File > Open Recent's callback -- bundled in here rather than a
/// separate `setup_*` for the same reason [`setup_dirty_tracking`] is bundled into
/// [`setup_save_native_callback`]: this is the one `setup_*` entry point Open's own
/// wiring has to answer to.
pub(in crate::gui::editor) fn setup_open_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
) {
    let state_open = Rc::clone(state);
    let render_ctx_open = Arc::clone(render_ctx);
    let preview_state_open = Arc::clone(preview_state);
    let solid_last_solved_open = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_open_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("open_native", || {
            if state_open.borrow().is_dirty() {
                state_open.borrow_mut().pending_unsaved_action =
                    Some(PendingUnsavedAction::OpenNative);
                ui.global::<EditorModel>().set_unsaved_dialog_message(
                    "Opening a file will discard the current design's unsaved changes.".into(),
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
    });

    // File > Open Recent (`ui/app.slint`'s `MainWindow.open_recent_native_file`) --
    // a root-component callback rather than an `EditorModel` one, since
    // `recent_native_files`/`open_recent_native_file` are declared directly on
    // `MainWindow` (`ui/app.slint`) rather than on
    // `EditorModel` (`ui/models/editor.slint`, owned elsewhere). Opening an entry
    // re-records it via [`record_recent_native_file`] (inside
    // [`open_design_file`], which this path shares with the ordinary picker),
    // so using a recent file also bumps it back to the front.
    let state_recent = Rc::clone(state);
    let render_ctx_recent = Arc::clone(render_ctx);
    let preview_state_recent = Arc::clone(preview_state);
    let solid_last_solved_recent = Arc::clone(solid_last_solved);
    let ui_weak_recent = ui.as_weak();
    ui.on_open_recent_native_file(move |path| {
        let Some(ui) = ui_weak_recent.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("open_recent_native_file", || {
            open_recent_native_path(
                &ui,
                &state_recent,
                &render_ctx_recent,
                &preview_state_recent,
                &solid_last_solved_recent,
                PathBuf::from(path.as_str()),
            );
        });
    });
}

/// File > Open Recent's own click handler -- loads `native_path` directly (no file
/// picker) via the exact same [`read_picked_path_then`]/[`dispatch_picked`] path
/// [`do_open_native`] uses for a picked file, so it takes a `.isdd` or legacy
/// `.indicatrix` design file, a `.asc` or a `.gem`/`.gcs` alike. Guards on
/// [`EditorState::is_dirty`] like every other destructive replace-the-design entry
/// point in this module, but -- unlike [`setup_open_native_callback`]'s own picker
/// path -- does not yet resume through the Save/Discard/Cancel dialog on a dirty
/// design: `PendingUnsavedAction` has no variant carrying a specific path to resume
/// at (only `OpenNative`, which re-shows the picker from scratch), and that enum is
/// owned by `state/mod.rs`. Refusing with a toast instead is safe (nothing is lost)
/// though less smooth; a future change could close this gap by adding a
/// `PendingUnsavedAction` variant that carries a specific path to resume at.
///
/// `pub(super)` (rather than private) so `gui::editor::mod`'s startup sequence can
/// reuse it for "reopen last design", the leftover-autosave offer and a design given
/// on the command line -- `AppSettings::recent_native_files` already carries the
/// most-recently-used path first; this is the same load path "File > Open Recent"
/// itself uses to open it.
pub(in crate::gui::editor) fn open_recent_native_path(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
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
    let guard = ReplaceGuard::capture(&state.borrow(), false);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    read_picked_path_then(ui, native_path, move |ui, picked| {
        // The read is asynchronous: re-check for edits made since the dirty check
        // above before anything is replaced.
        dispatch_picked(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            guard,
            picked,
        );
    });
}

/// Commits whatever the Open picker (or Open Recent, or the command line) read: re-checks
/// for edits made since the unsaved-changes decision ([`ReplaceGuard`]), then hands
/// the picked design to the replace-state path for its kind. `None` (a cancelled
/// picker or a read that already toasted its failure) does nothing.
fn dispatch_picked(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    guard: ReplaceGuard,
    picked: Option<PickedNative>,
) {
    match picked {
        Some(_) if !guard.allows_replace(ui, state) => {}
        Some(PickedNative::Design {
            native_path,
            loaded,
        }) => open_design_file(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            DesignFileLoad {
                native_path,
                loaded: *loaded,
            },
        ),
        Some(PickedNative::AscOnly { asc_path, asc_text }) => open_plain_asc(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            &asc_path,
            &asc_text,
        ),
        Some(PickedNative::Converted(converted)) => open_converted_design(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            converted,
        ),
        None => {}
    }
}

/// The actual "Open" work -- see [`setup_open_native_callback`]'s own doc
/// comment for why the unsaved-changes guard runs before this is ever called, not
/// inside it. Reads whichever file the cutter picked via [`pick_design_then`],
/// then dispatches (see [`dispatch_picked`]) to [`open_design_file`] (a `.isdd` or
/// legacy `.indicatrix` design file), [`open_plain_asc`] (a bare `.asc`) or
/// [`open_converted_design`] (a `.gem`/`.gcs` file).
pub(in crate::gui::editor) fn do_open_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
) {
    // Captured now -- the dirty check (or the Save/Discard answer that resumed this
    // call) just happened; the picker below is asynchronous and the editor stays
    // interactive while it is up.
    let guard = ReplaceGuard::capture(&state.borrow(), true);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    pick_design_then(ui, move |ui, picked| {
        dispatch_picked(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            guard,
            picked,
        );
    });
}
