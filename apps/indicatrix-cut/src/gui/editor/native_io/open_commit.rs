//! Commits a loaded native/`.asc` design into `EditorState`, wholesale, and pushes
//! the result into the panel/viewport -- the shared tail every "Open" path
//! (a `.indicatrix` design file, an older pair, an older self-contained sidecar, or a
//! bare `.asc`) funnels through.

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
use indicatrix_cut_core::{
    FingerprintCheck, History, LoadPairedResult, TierOverlay,
    native::{LoadNativeOnlyResult, LoadedDesign as DesignFileLoaded},
};
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

/// [`open_native_self_contained`]'s own load result -- bundled (rather than
/// three more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning [`LoadedNativeOutcome`] uses.
pub(super) struct SelfContainedLoad<'a> {
    pub(super) native_path: &'a Path,
    pub(super) loaded: LoadNativeOnlyResult,
    pub(super) asc_filename: &'a str,
}

/// [`open_design_file`]'s own load result -- bundled (rather than two more
/// parameters) purely to keep that function under clippy's argument-count lint.
pub(super) struct DesignFileLoad {
    pub(super) native_path: PathBuf,
    pub(super) loaded: DesignFileLoaded,
}

/// A `.indicatrix` design file: the file's own tiers WERE the design, so there is no
/// paired `.asc`, fingerprint or tier-overlay question to ask. Restores a custom
/// material snapshot like [`commit_loaded_native`] does, and records the file as this
/// design's save location -- unless it is a recovery snapshot, which is never a
/// location to save back to (the next Save asks for a name, suggesting the design's
/// own).
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
    let lead = if recovered {
        format!(
            "Recovered '{native_path_display}' from an autosave snapshot. Save writes it to a \
             new .indicatrix file."
        )
    } else {
        format!("Loaded '{native_path_display}'.")
    };
    show_toast(
        ui,
        &format!(
            "{lead}{}{}",
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

/// The older self-contained `.indicatrix.toml` (recovery snapshot)
/// path -- [`read_native_pair_then`] only ever hands this a [`LoadNativeOnlyResult`]
/// once it has already confirmed no paired `.asc` was findable AND
/// [`load_native_only`] actually accepted the file, so there is no fingerprint/
/// tier-overlay question to ask here at all (unlike [`open_native_pair`]): the
/// file's own `tiers` array WAS the design, full stop. Restores a custom material
/// snapshot exactly like [`commit_loaded_native`] does, on the same
/// [`LoadNativeOnlyResult::restorable_custom_material`]/`material_resolution`
/// fields [`LoadPairedResult`] also carries.
pub(super) fn open_native_self_contained(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
    load: SelfContainedLoad<'_>,
) {
    let SelfContainedLoad {
        native_path,
        loaded,
        asc_filename,
    } = load;
    let native_path_display = native_path.display().to_string();
    record_recent_native_file(ui, &native_path_display);
    // An older recovery snapshot is never a location to save back to: the next Save
    // asks for a `.indicatrix` file name.
    remember_design_location(None, None);

    let (material_note, material_still_unresolved) = restore_custom_material(
        ui,
        render_ctx,
        loaded.restorable_custom_material.as_ref(),
        loaded.design.material.name.as_deref(),
        loaded.material_resolution,
    );
    let newer_version_note = loaded.written_by_newer_version.then(|| {
        " This file was written by a newer version of Indicatrix Cut; some settings \
          may not have been understood and could be lost on your next save."
            .to_string()
    });
    let is_mismatch = material_still_unresolved || loaded.written_by_newer_version;

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
        // No paired `.asc` text exists at all -- see this function's own doc
        // comment; a later Save/Export builds a fresh one from scratch, exactly
        // like a brand-new design.
        asc_filename: Some(asc_filename.to_string()),
        original_asc_text: None,
        pending_gear_remap: None,
        pending_retarget: None,
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        source_entry_id: None,
        used_placeholder: false,
        // The older sidecar has no id: its location names the design (see
        // `state::design_identity`).
        file_extras: DesignFileExtras::default().with_design_uuid_assigned_for_file(native_path),
        has_design: true,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    show_toast(
        ui,
        &format!(
            "Recovered '{native_path_display}' -- no paired .asc file was found, so this design \
             was rebuilt directly from the file's own saved data.{}{}",
            material_note.unwrap_or_default(),
            newer_version_note.unwrap_or_default()
        ),
        if is_mismatch { "warning" } else { "success" },
    );
}

/// The bare-`.asc`-with-no-sidecar path -- builds the design exactly the
/// way `gui::editor::loading::design_from_asc_text` already does for a catalogue
/// attachment (no meet-intent overlay, no fingerprint, no draft flag: there is no
/// design file at all), then replaces `state` wholesale via [`finish_state_replace`],
/// the same tail [`commit_loaded_native`] runs for the paired case.
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
                    "Loaded '{}' (plain .asc, no .indicatrix design file found -- authored meet \
                     constraints and detached facets are not available).",
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
/// through [`commit_plain_design`], so it behaves exactly like an opened `.asc`
/// with no sidecar.
///
/// The design is recorded under `<stem>.asc` (never the source file's own name),
/// so Save offers a new `.indicatrix` design file and can never overwrite the `.gem`/
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
    } = picked;
    match crate::gui::editor::loading::design_from_asc_text(&asc_file_name, &asc_text, None) {
        Ok(loaded) => {
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
                     .indicatrix design file named after '{asc_file_name}'; the original file is \
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
        // Open (this path and the paired-load
        // one below) has no catalogue row of its own -- it loaded from a
        // file the cutter picked directly, not from a library selection --
        // so there is nothing here for a later Save to write back
        // to. `gui::editor::callbacks::tier_actions::setup_load_selected_callback`'s
        // local branch is the one place this is ever `Some`.
        source_entry_id: None,
        // A native/plain-`.asc` open carries a real recorded
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
/// Shared by [`commit_loaded_native`] (a native+`.asc` pair) and [`open_plain_asc`] (a
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
    // after whatever design this replace just loaded -- covers both `open_native_pair`
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

/// Bundles [`commit_loaded_native`]'s per-call payload -- kept as one struct (rather
/// than five more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning `tier_actions::LoadedDesignOutcome` uses.
pub(super) struct LoadedNativeOutcome {
    pub(super) loaded: LoadPairedResult,
    /// What the toast calls the file -- the picked file's own display path.
    pub(super) native_path_display: String,
    pub(super) asc_filename: String,
    pub(super) asc_text: String,
    /// Drives the toast's class: `"warning"` (which `gui::show_toast` never
    /// auto-dismisses, same as `"error"`, but is colored and captioned as a note
    /// rather than a failure) rather than `"info"`'s 3.5-second flash,
    /// since a mismatch means something about this design's authored intent may not
    /// have made the round trip -- worth a permanent, plainly-worded note, not a
    /// flash a cutter can miss mid-click, and not styled as an error when nothing
    /// actually failed.
    pub(super) is_mismatch: bool,
}

/// Replaces `state` wholesale with `outcome.loaded`'s design and pushes the result
/// into the panel/viewport -- the shared tail [`do_open_native`]'s clean path and both
/// of [`setup_mismatch_dialog_callbacks`]'s committing branches (Apply Anyway/Use .asc
/// Only) all funnel through, so the "replace state, reset selection, report the
/// outcome" sequence is written exactly once.
pub(super) fn commit_loaded_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &crate::gui::editor::view::SolidLastSolved,
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
    // An older pair has no design file yet: the next Save asks for a `.indicatrix`
    // name (suggesting the design's own) in the folder this pair lives in, and leaves
    // the older files untouched.
    remember_design_location(
        None,
        Path::new(&native_path_display)
            .parent()
            .map(Path::to_path_buf),
    );
    // A plain sentence, not
    // `FingerprintCheck`/`TierOverlay`'s own technical `Display` text (e.g.
    // "per-tier meet-intent overlay skipped (fingerprint mismatch)") -- see
    // `plain_load_outcome_text`'s own doc comment.
    let outcome_note = plain_load_outcome_text(&loaded.fingerprint, &loaded.tier_overlay);
    // A material name this build can't resolve AND whose sidecar carried a
    // `[material.custom]` snapshot is
    // restored into this session's own custom-material registry right now --
    // `RenderContext::custom_materials`, the same list the material editor's own
    // "Save Custom Material" pushes into (`gui::optics::custom_materials`) --
    // instead of silently rendering as Diamond. See
    // `LoadPairedResult::restorable_custom_material`'s own doc comment for why
    // this is the caller's job, not `load_paired`'s.
    // `material_still_unresolved` is `false` for a successful restoration -- that
    // is good news, not a mismatch, so it must not push this toast into the
    // persistent "warning" class below the way a genuinely unresolved material
    // does.
    let (material_note, material_still_unresolved) = restore_custom_material(
        ui,
        render_ctx,
        loaded.restorable_custom_material.as_ref(),
        loaded.design.material.name.as_deref(),
        loaded.material_resolution,
    );
    // `format_version` was written but never checked -- a sidecar from a
    // newer build could carry fields this one silently drops into `unknown` and
    // re-serializes on the next save (quietly degrading it further each round trip).
    // Warned rather than refused: every named field here already tolerates being
    // absent, so the design itself still loaded fine.
    let newer_version_note = loaded.written_by_newer_version.then(|| {
        " This file was written by a newer version of Indicatrix Cut; some settings \
          may not have been understood and could be lost on your next save."
            .to_string()
    });
    let is_mismatch = is_mismatch || material_still_unresolved || loaded.written_by_newer_version;

    // `replace_wholesale`, not a plain `*state.borrow_mut() = ...` -- see
    // `open_plain_asc`'s matching comment and `EditorState::replace_wholesale`'s own
    // doc comment (`state/mod.rs`) for why: this carries the OLD `generation` `Arc`
    // (and bumps it) across the replacement so a background Deep Solve/Optimize/
    // auto-solve dispatched against the design being replaced still observes the
    // change instead of comparing against a counter nobody increments anymore.
    // Restored from the sidecar's own `[source]` table (written by an
    // earlier Save -- see `EditorState::printed_proportions`'s own doc
    // comment) rather than hard-coded `None`, so Deep Solve still has printed figures
    // to verify against after a Save/Open round trip, not only on this
    // design's very first "Load Selected" from the catalogue. Still `None` for a
    // sidecar saved before printed proportions were recorded there, or one for a
    // design never loaded from a catalogue row at all.
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
        asc_filename: Some(asc_filename),
        original_asc_text: Some(asc_text),
        pending_gear_remap: None,
        pending_retarget: None,
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        // Same reasoning as the plain-`.asc` Open path
        // above -- the native sidecar's own `[source]` table (`printed_proportions`,
        // just above) carries a catalogue row's PRINTED proportions, but not which
        // row it was loaded from, so there is nothing here to write back to either.
        source_entry_id: None,
        // A native/plain-`.asc` open carries a real recorded
        // schedule, never the angle-table reconstruction fallback.
        used_placeholder: false,
        // The older pair has no id: the sidecar's location names the design (see
        // `state::design_identity`).
        file_extras: DesignFileExtras::default()
            .with_design_uuid_assigned_for_file(Path::new(&native_path_display)),
        has_design: true,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    show_toast(
        ui,
        &format!(
            "Loaded '{native_path_display}'. {outcome_note}{}{} Saving writes a new \
             .indicatrix file and leaves these older files untouched.",
            material_note.unwrap_or_default(),
            newer_version_note.unwrap_or_default()
        ),
        if is_mismatch { "warning" } else { "success" },
    );
}

/// Plain-English replacement for [`FingerprintCheck`]/[`TierOverlay`]'s own
/// `Display` text in the load-outcome toast. The crate's internal diagnostic prose
/// ("per-tier meet-intent overlay skipped (fingerprint mismatch)") says nothing to
/// a cutter about what actually happened to their file, so this rewrites it in
/// plain language a cutter can read without knowing what a fingerprint or a tier
/// overlay is. Persistence and severity (a "warning" toast that stays until
/// dismissed, rather than a 3.5-second "info" flash) are handled separately by
/// the caller.
pub(super) fn plain_load_outcome_text(
    fingerprint: &FingerprintCheck,
    tier_overlay: &TierOverlay,
) -> String {
    if matches!(fingerprint, FingerprintCheck::Match) {
        return match tier_overlay {
            TierOverlay::Applied | TierOverlay::AppliedDespiteMismatch => {
                "Your saved meet constraints were restored.".to_string()
            }
            TierOverlay::SkippedTierCountMismatch {
                native_tiers,
                asc_tiers,
            } => format!(
                "This file's saved tier count ({native_tiers}) does not match the .asc file's \
                 ({asc_tiers}), so your saved meet constraints could not be restored."
            ),
            TierOverlay::SkippedFingerprintMismatch => {
                // Not reached in practice once `fingerprint` is `Match` -- kept as its
                // own honest arm rather than assumed unreachable, since the two checks
                // are independent types with no shared invariant enforcing this.
                "Your saved meet constraints were restored.".to_string()
            }
            TierOverlay::AppliedFromDraft => "Loaded from an unsolved draft: the masts shown are \
                 placeholders, not a real solve."
                .to_string(),
        };
    }
    match tier_overlay {
        TierOverlay::AppliedDespiteMismatch => "The .asc file changed since this sidecar was \
             saved, so its geometry was used as-is; your saved meet constraints were re-applied \
             anyway, at your request, and may no longer line up with the changed tiers."
            .to_string(),
        _ => "The .asc file changed since this sidecar was saved, so its geometry was used as-is \
             and your saved meet constraints were not restored."
            .to_string(),
    }
}
