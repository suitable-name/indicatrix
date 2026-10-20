//! Commits a loaded design file / `.asc` design into `EditorState`, wholesale, and pushes
//! the result into the panel/viewport -- the shared tail every "Open" path
//! (a `.isdd` design file, a bare `.asc`, or a converted `.gem`/`.gcs`) funnels
//! through.

mod restore_material;

use super::{
    autosave::record_recent_native_file,
    design_paths::{is_autosave_design_path, schedule_name_for_design_path},
    open_picker::ConvertedPick,
    remember_design_location,
};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            callbacks::clear_analysis_results,
            loading::LoadedDesign,
            state::{
                DesignFileExtras, EditorState, MaterialComboCache, PendingUnsavedAction,
                PushedScratch,
            },
            view::{push_has_design, refresh_all},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::{History, native::LoadedDesign as DesignFileLoaded};
use indicatrix_editor::EditorSession;
use restore_material::restore_custom_material;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

/// What an open path knew about the design it is about to replace when the cutter last
/// decided about that design's unsaved changes (the dirty check, or the Save/Discard
/// answer that resumed the open).
///
/// The open itself is asynchronous -- a file picker, a file read -- and the editor
/// stays interactive meanwhile, so edits can land after the decision. Replacing the
/// design at completion without asking again would discard them silently;
/// [`Self::allows_replace`] is the commit-time re-check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReplaceGuard {
    /// `EditorState::current_generation` when the decision was made.
    generation: u64,
    /// Whether the unsaved-changes dialog can resume this open. `true` for the picker
    /// path (`PendingUnsavedAction::OpenNative` re-shows the picker); `false` for Open
    /// Recent, which has no resumable action carrying a path and refuses with a toast.
    resumable: bool,
}

impl ReplaceGuard {
    /// Captures the design's generation as of now.
    pub(super) fn capture(state: &EditorState, resumable: bool) -> Self {
        Self {
            generation: state.current_generation(),
            resumable,
        }
    }

    /// Whether replacing the design now would throw away edits made since the guard
    /// was captured: the generation moved AND the design is unsaved. A generation that
    /// did not move means the cutter already answered for exactly this design (a
    /// "Discard" that resumed the open leaves it dirty by choice).
    #[must_use]
    pub(super) const fn discards_new_edits(self, current_generation: u64, is_dirty: bool) -> bool {
        current_generation != self.generation && is_dirty
    }

    /// `true` when the open may replace the design. Otherwise nothing has been
    /// replaced and the cutter has been told: the unsaved-changes dialog is shown
    /// again for a resumable open, a toast explains a non-resumable one.
    pub(super) fn allows_replace(self, ui: &MainWindow, state: &Rc<RefCell<EditorState>>) -> bool {
        let (current_generation, is_dirty) = {
            let st = state.borrow();
            (st.current_generation(), st.is_dirty())
        };
        if !self.discards_new_edits(current_generation, is_dirty) {
            return true;
        }
        if self.resumable {
            state.borrow_mut().pending_unsaved_action = Some(PendingUnsavedAction::OpenNative);
            let model = ui.global::<EditorModel>();
            model.set_unsaved_dialog_message(
                "The design was edited while the file was being chosen. Opening it will \
                 discard those unsaved changes."
                    .into(),
            );
            model.set_unsaved_dialog_open(true);
        } else {
            show_toast(
                ui,
                "The design was edited while the file was loading. Save or discard those \
                 changes, then open the recent file again.",
                "error",
            );
        }
        false
    }
}

/// [`open_design_file`]'s own load result -- bundled (rather than two more
/// parameters) purely to keep that function under clippy's argument-count lint.
pub(super) struct DesignFileLoad {
    pub(super) native_path: PathBuf,
    pub(super) loaded: DesignFileLoaded,
}

/// A `.isdd` design file (or a legacy `.indicatrix` one): the file's own tiers WERE the
/// design, so there is no paired `.asc`, fingerprint or tier-overlay question to ask.
/// Restores a custom material snapshot and records the file as this design's save
/// location -- unless it is a recovery snapshot, which is never a location to save back
/// to (the next Save asks for a name, suggesting the design's own). A legacy
/// `.indicatrix` file is a location to open from, not to save back to: Save proposes the
/// `.isdd` name beside it and leaves the old file alone.
///
/// The design is recorded under `<stem>.asc` ([`schedule_name_for_design_path`]): the
/// name the editor keys its window title, catalogue write-back and "Export .asc"
/// default on, though no `.asc` exists.
pub(super) fn open_design_file(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    load: DesignFileLoad,
) {
    let DesignFileLoad {
        native_path,
        loaded,
    } = load;
    let native_path_display = native_path.display().to_string();
    let recovered = is_autosave_design_path(&native_path);
    if recovered {
        remember_design_location(None, None);
    } else {
        record_recent_native_file(ui, &native_path_display);
        remember_design_location(Some(native_path.clone()), None);
    }
    let (material_note, material_unresolved) = restore_custom_material(
        ui,
        render_ctx,
        loaded.restorable_custom_material.as_ref(),
        loaded.design.material.name.as_deref(),
        loaded.material_resolution,
    );
    let draft_note = loaded
        .draft
        .then_some(" It was saved as an unsolved draft; add a scale-reference tier to finish it.");
    let printed_proportions = loaded.printed_proportions;
    state.borrow_mut().replace_wholesale(EditorState {
        deep_solve_result_generation: None,
        session: EditorSession::with_history(loaded.design, History::new()),
        printed_proportions,
        design_epoch: Arc::new(AtomicU64::new(0)),
        pending_unsaved_action: None,
        after_save: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        asc_filename: Some(schedule_name_for_design_path(&native_path)),
        original_asc_text: None,
        pending_gear_remap: None,
        pending_retarget: None,
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        applied_library_material: None,
        source_entry_id: None,
        used_placeholder: false,
        // The design file's own `[meta]` and attachments, kept for the next Save. A file
        // without an id gets the UUID of its location now (the same one every time it is
        // opened), which that Save writes. A recovered autosave snapshot is not a place
        // the design stays at, so it gets a fresh one.
        file_extras: if recovered {
            DesignFileExtras::new(loaded.metadata, loaded.attachments)
                .with_design_uuid_assigned(None)
        } else {
            DesignFileExtras::new(loaded.metadata, loaded.attachments)
                .with_design_uuid_assigned_for_file(&native_path)
        },
        has_design: true,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    if let Some(name) = native_path.file_name() {
        ui.set_loaded_design_name(name.to_string_lossy().into_owned().into());
    }
    let status_lead = if recovered {
        format!(
            "Recovered '{native_path_display}' from an autosave snapshot. Save writes it to a \
             new .isdd file."
        )
    } else {
        format!("Loaded '{native_path_display}'.")
    };
    show_toast(
        ui,
        &format!(
            "{status_lead}{}{}",
            material_note.unwrap_or_default(),
            draft_note.unwrap_or_default()
        ),
        if material_unresolved || loaded.draft {
            "warning"
        } else {
            "success"
        },
    );
}

/// The bare-`.asc` path -- builds the design exactly the
/// way `gui::editor::loading::design_from_asc_text` already does for a catalogue
/// attachment (no meet-intent overlay, no fingerprint, no draft flag: there is no
/// design file at all), then replaces `state` wholesale via [`finish_state_replace`],
/// the same tail [`open_design_file`] runs for a design file.
pub(super) fn open_plain_asc(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    asc_path: &Path,
    asc_text: &str,
) {
    let file_name = asc_path.file_name().map_or_else(
        || "design.asc".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match crate::gui::editor::loading::design_from_asc_text(&file_name, asc_text, None) {
        Ok(loaded) => {
            commit_plain_design(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                asc_path,
                loaded,
            );
            show_toast(
                ui,
                &format!(
                    "Loaded '{}' (plain .asc -- authored meet constraints and detached \
                     facets are not available; open the .isdd design file for those).",
                    asc_path.display()
                ),
                "success",
            );
        }
        Err(e) => show_toast(ui, &format!("Cannot open: {e}"), "error"),
    }
}

/// A `.gem`/`.gcs` file converted to `.asc` cutting instructions
/// ([`ConvertedPick`]): built into a design by the SAME
/// `gui::editor::loading::design_from_asc_text` a bare `.asc` uses and committed
/// through [`commit_plain_design`], so it behaves exactly like an opened `.asc`.
///
/// The design is recorded under `<stem>.asc` (never the source file's own name),
/// so Save offers a new `.isdd` design file and can never overwrite the `.gem`/
/// `.gcs`; the window title still names the file actually opened. Reader and
/// converter warnings go into the toast, which then stays up as a warning.
pub(super) fn open_converted_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    picked: ConvertedPick,
) {
    let ConvertedPick {
        source_path,
        asc_file_name,
        asc_text,
        warnings,
        frosting,
    } = picked;
    match crate::gui::editor::loading::design_from_asc_text(&asc_file_name, &asc_text, None) {
        Ok(mut loaded) => {
            loaded.design.apply_imported_frosting(&frosting);
            commit_plain_design(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                &source_path,
                loaded,
            );
            if let Some(name) = source_path.file_name() {
                ui.set_loaded_design_name(name.to_string_lossy().into_owned().into());
            }
            let notes = if warnings.is_empty() {
                String::new()
            } else {
                format!(" Notes: {}.", warnings.join("; "))
            };
            show_toast(
                ui,
                &format!(
                    "Loaded '{}' as .asc cutting instructions. Save writes a new \
                     .isdd design file named after '{asc_file_name}'; the original file is \
                     never changed.{notes}",
                    source_path.display()
                ),
                if warnings.is_empty() {
                    "success"
                } else {
                    "warning"
                },
            );
        }
        Err(e) => show_toast(ui, &format!("Cannot open: {e}"), "error"),
    }
}

/// Replaces `state` wholesale with a design loaded from a bare `.asc` (or a
/// `.gem`/`.gcs` converted to one) and runs [`finish_state_replace`] -- the shared
/// body of [`open_plain_asc`] and [`open_converted_design`]. There is no native
/// design file, so the remembered save location is cleared. `source_path` is the file
/// the cutter actually picked (the `.asc`, or the `.gem`/`.gcs` it was converted from):
/// it names the design for the library database until a Save gives it an id.
fn commit_plain_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    source_path: &Path,
    loaded: LoadedDesign,
) {
    // A bare `.asc` has no design file at all -- see `CURRENT_NATIVE_PATH`'s own doc
    // comment. Cleared rather than left at whatever the PREVIOUS design's own
    // save/open set it to, so a later Save here is never mistaken for "re-saving
    // that unrelated design's own file."
    remember_design_location(None, None);
    // `replace_wholesale`, not a plain `*state.borrow_mut() = ...`: carries
    // this state's own `generation` `Arc` across the replacement (and bumps
    // it) instead of handing back a brand-new one, so a background Deep
    // Solve/Optimize/auto-solve dispatched against the design being replaced
    // still observes the change -- see that method's own doc comment
    // (`state/mod.rs`) and `tier_actions::do_new_design_create`'s matching
    // comment for the same reasoning applied to New/Load Selected.
    state.borrow_mut().replace_wholesale(EditorState {
        session: EditorSession::with_history(loaded.design, History::new()),
        printed_proportions: None,
        design_epoch: Arc::new(AtomicU64::new(0)),
        pending_unsaved_action: None,
        after_save: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        deep_solve_result_generation: None,
        asc_filename: loaded.asc_filename,
        original_asc_text: loaded.original_asc_text,
        pending_gear_remap: None,
        pending_retarget: None,
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        applied_library_material: None,
        // Open (this path and the design-file one above) has no catalogue row of its
        // own -- it loaded from a file the cutter picked directly, not from a library
        // selection -- so there is nothing here for a later Save to write back
        // to. `gui::editor::callbacks::tier_actions::setup_load_selected_callback`'s
        // local branch is the one place this is ever `Some`.
        source_entry_id: None,
        // A design-file/plain-`.asc` open carries a real recorded
        // schedule, never the angle-table reconstruction fallback.
        used_placeholder: false,
        file_extras: DesignFileExtras::default().with_design_uuid_assigned_for_file(source_path),
        has_design: true,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
}

/// The tail every "replace `EditorState` wholesale" open path shares, once the new
/// state is already stored: refresh the viewport/panel from it, then reset selection
/// unconditionally (a previously selected tier index now names, at best, an
/// unrelated row in whatever design just replaced it) and mark the fresh state clean.
/// Shared by [`open_design_file`] (a design file) and [`open_plain_asc`] (a
/// bare `.asc`) so the "reset selection, mark clean" sequence is written exactly once
/// for both -- the same reset `gui::editor::callbacks::tier_actions::apply_loaded_design`/
/// `setup_new_design_create_callback` run for Load Selected/New.
fn finish_state_replace(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    state: &Rc<RefCell<EditorState>>,
) {
    // A Deep Solve or Optimize verdict describes the design that was just replaced,
    // so it must not outlive it -- see `clear_analysis_results`' own doc comment.
    clear_analysis_results(ui);
    // a material-suggestion banner (`tier_actions::apply_loaded_design`'s
    // own Load Selected path sets/clears this from the newly loaded design's
    // own RI) also describes whatever design was open before this replace --
    // Open/Open Recent/the startup restore had no reset of their own at
    // all, leaving a stale accept/dismiss banner from a previous design.
    ui.global::<EditorModel>()
        .set_material_suggestion_name("".into());
    ui.global::<EditorModel>()
        .set_material_suggestion_text("".into());
    let st = state.borrow();
    refresh_all(ui, render_ctx, preview_state, solid_last_solved, &st, true);
    // Names the window title (`MainWindow.loaded_design_name`)
    // after whatever design this replace just loaded -- covers both `open_design_file`
    // and `open_plain_asc`, the two callers of this shared tail. Falls back to empty
    // (bare "Indicatrix Cut") only in the defensive case where `asc_filename` was
    // somehow never set, which neither caller actually does.
    ui.set_loaded_design_name(st.asc_filename.clone().unwrap_or_default().into());
    // Every open path (Open, Open Recent, the startup restore) installs a
    // real design -- the empty-state card grid must give way to it.
    push_has_design(ui, &st);
    // The UUID the library database files this design's variants, cutting progress and
    // lighting choice under -- logged so a "my variants are gone" report can be matched
    // against what the file carries.
    tracing::debug!("Opened design with UUID {}", st.design_uuid());
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
