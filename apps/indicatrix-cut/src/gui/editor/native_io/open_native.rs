//! "Open Native": loads a `.indicatrix.toml` (or legacy `.gemcut.toml`) sidecar
//! together with its paired `.asc`, replacing the whole editor state the same way
//! "New"/"Load Selected" do. [`setup_open_native_callback`]/[`open_recent_native_path`]
//! are this group's entry points; [`do_open_native`] dispatches a picked file to
//! [`super::open_commit`]'s replace-state paths; [`setup_mismatch_dialog_callbacks`]
//! resolves a fingerprint mismatch the cutter must choose how to handle.

use super::{
    open_commit::{
        LoadedNativeOutcome, SelfContainedLoad, commit_loaded_native, open_native_self_contained,
        open_plain_asc,
    },
    open_picker::{
        NativePairOrSelfContained, PickedNative, PickedPair, pick_native_or_asc_then,
        read_native_pair_then,
    },
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
use indicatrix_cut_core::{FingerprintCheck, TierOverlay, load_paired};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

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
pub(in crate::gui::editor) fn setup_open_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
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
        crate::gui::editor::stall_guard::stall_guard("open_native", || {
            if state_open.borrow().is_dirty() {
                state_open.borrow_mut().pending_unsaved_action =
                    Some(PendingUnsavedAction::OpenNative);
                ui.global::<EditorModel>().set_unsaved_dialog_message(
                    "Opening a native file will discard the current design's unsaved changes."
                        .into(),
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
/// picker) via the exact same [`read_native_pair`]/[`open_native_pair`] path
/// [`do_open_native`] uses for a picked `.toml`. Guards on
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
/// reuse it for "reopen last design" -- `AppSettings::recent_native_files`
/// already carries the most-recently-used path first; this is the same load path
/// "File > Open Recent" itself uses to open it.
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
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    // `native_path` itself is still borrowed for this very call while `on_done` is
    // being constructed below (it moves its own copy in).
    let native_path_for_read = native_path.clone();
    read_native_pair_then(ui, &native_path_for_read, move |ui, result| {
        match result {
            Some(NativePairOrSelfContained::Pair {
                parsed_native,
                asc_text,
                native_text,
            }) => open_native_pair(
                ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                PickedPair {
                    native_path,
                    parsed_native,
                    asc_text,
                    native_text,
                },
            ),
            // The same autosave-restore fallback `do_open_native` gets, reached
            // here too since a leftover autosave file is reopened through this
            // same function
            // (`gui::editor::mod`'s startup sequence calls this with
            // `find_leftover_autosave`'s own path).
            Some(NativePairOrSelfContained::SelfContained {
                loaded,
                asc_filename,
            }) => open_native_self_contained(
                ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                SelfContainedLoad {
                    native_path: &native_path,
                    loaded: *loaded,
                    asc_filename: &asc_filename,
                },
            ),
            None => {}
        }
    });
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

/// The actual "Open Native" work -- see [`setup_open_native_callback`]'s own doc
/// comment for why the unsaved-changes guard runs before this is ever called, not
/// inside it. Reads whichever file the cutter picked via [`pick_native_or_asc_then`],
/// then dispatches to [`open_native_pair`] (a real pair) or [`open_plain_asc`] (a
/// bare `.asc`, no sidecar).
pub(in crate::gui::editor) fn do_open_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    pick_native_or_asc_then(ui, move |ui, picked| match picked {
        Some(PickedNative::Pair(picked)) => {
            open_native_pair(
                ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                *picked,
            );
        }
        Some(PickedNative::AscOnly { asc_path, asc_text }) => open_plain_asc(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            &asc_path,
            &asc_text,
        ),
        Some(PickedNative::SelfContained {
            native_path,
            loaded,
            asc_filename,
        }) => open_native_self_contained(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            SelfContainedLoad {
                native_path: &native_path,
                loaded: *loaded,
                asc_filename: &asc_filename,
            },
        ),
        None => {}
    });
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
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
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

/// The fingerprint-mismatch dialog's three resolution callbacks -- see
/// [`PendingMismatch`]/[`PENDING_MISMATCH`]. Registered once, from
/// [`setup_open_native_callback`], since only Open Native can ever populate
/// [`PENDING_MISMATCH`] in the first place.
fn setup_mismatch_dialog_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
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
