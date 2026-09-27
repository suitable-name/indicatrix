//! Commits a loaded native/`.asc` design into `EditorState`, wholesale, and pushes
//! the result into the panel/viewport -- the shared tail every "Open Native" path
//! (a real pair, a self-contained native file, or a bare `.asc`) funnels through.

use super::{CURRENT_NATIVE_PATH, autosave::record_recent_native_file};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            callbacks::clear_analysis_results,
            state::{EditorState, MaterialComboCache, PushedScratch},
            view::{push_has_design, refresh_all},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::{
    FingerprintCheck, History, LoadPairedResult, TierOverlay,
    native::{LoadNativeOnlyResult, gem_material_from_custom_snapshot},
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

/// [`open_native_self_contained`]'s own load result -- bundled (rather than
/// three more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning [`LoadedNativeOutcome`] uses.
pub(super) struct SelfContainedLoad<'a> {
    pub(super) native_path: &'a Path,
    pub(super) loaded: LoadNativeOnlyResult,
    pub(super) asc_filename: &'a str,
}

/// The self-contained-native-file
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
    // Same reasoning as `commit_loaded_native` -- this design's own
    // sidecar just landed here, so a later Save Native re-writing this exact path
    // needs no overwrite confirmation.
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(native_path.to_path_buf()));

    let (material_note, material_still_unresolved) = if let (Some(snapshot), Some(name)) = (
        loaded.restorable_custom_material.as_ref(),
        loaded.design.material.name.as_deref(),
    ) {
        let gem = gem_material_from_custom_snapshot(name, snapshot);
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let materials = Arc::make_mut(&mut ctx.custom_materials);
        if let Some(pos) = materials
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
        {
            materials[pos] = gem;
        } else {
            materials.push(gem);
        }
        drop(ctx);
        (
            Some(format!(
                " '{name}' was restored from this file's own saved material data."
            )),
            false,
        )
    } else {
        let unresolved = matches!(
            loaded.material_resolution,
            indicatrix_cut_core::native::MaterialResolution::Unresolved
        );
        (
            unresolved.then(|| format!(" {}", loaded.material_resolution)),
            unresolved,
        )
    };
    let newer_version_note = loaded.written_by_newer_version.then(|| {
        " This file was written by a newer version of Indicatrix Cut; some settings \
          may not have been understood and could be lost on your next save."
            .to_string()
    });
    let is_mismatch = material_still_unresolved || loaded.written_by_newer_version;

    let printed_proportions = loaded.printed_proportions;
    state.borrow_mut().replace_wholesale(EditorState {
        deep_solve_result_generation: None,
        design: loaded.design,
        history: History::new(),
        printed_proportions,
        generation: Arc::new(AtomicU64::new(0)),
        design_epoch: Arc::new(AtomicU64::new(0)),
        saved_generation: 0,
        pending_unsaved_action: None,
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
        multi_selected: std::collections::BTreeSet::new(),
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        source_entry_id: None,
        used_placeholder: false,
        has_design: true,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    show_toast(
        ui,
        &format!(
            "Recovered '{native_path_display}' -- no paired .asc file was found, so this design \
             was rebuilt directly from the native file's own saved data.{}{}",
            material_note.unwrap_or_default(),
            newer_version_note.unwrap_or_default()
        ),
        if is_mismatch { "warning" } else { "success" },
    );
}

/// The bare-`.asc`-with-no-sidecar path -- builds the design exactly the
/// way `gui::editor::loading::design_from_asc_text` already does for a catalogue
/// attachment (no meet-intent overlay, no fingerprint, no draft flag: there is no
/// native file at all), then replaces `state` wholesale via [`finish_state_replace`],
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
            // A bare `.asc` has no native sidecar at all -- see `CURRENT_NATIVE_PATH`'s
            // own doc comment. Cleared rather than left at whatever the PREVIOUS
            // design's own save/open set it to, so a later Save Native here is never
            // mistaken for "re-saving that unrelated design's own file."
            CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = None);
            // `replace_wholesale`, not a plain `*state.borrow_mut() = ...`: carries
            // this state's own `generation` `Arc` across the replacement (and bumps
            // it) instead of handing back a brand-new one, so a background Deep
            // Solve/Optimize/auto-solve dispatched against the design being replaced
            // still observes the change -- see that method's own doc comment
            // (`state/mod.rs`) and `tier_actions::do_new_design_create`'s matching
            // comment for the same reasoning applied to New/Load Selected.
            state.borrow_mut().replace_wholesale(EditorState {
                design: loaded.design,
                history: History::new(),
                printed_proportions: None,
                generation: Arc::new(AtomicU64::new(0)),
                design_epoch: Arc::new(AtomicU64::new(0)),
                saved_generation: 0,
                pending_unsaved_action: None,
                deep_solve: None,
                optimize: None,
                pending_optimize: Arc::new(Mutex::new(None)),
                deep_solve_result_generation: None,
                asc_filename: loaded.asc_filename,
                original_asc_text: loaded.original_asc_text,
                pending_gear_remap: None,
                pending_retarget: None,
                multi_selected: std::collections::BTreeSet::new(),
                last_pushed_scratch: RefCell::new(PushedScratch::default()),
                material_combo_cache: RefCell::new(MaterialComboCache::default()),
                // Open Native (this path and the paired-load
                // one below) has no catalogue row of its own -- it loaded from a
                // file the cutter picked directly, not from a library selection --
                // so there is nothing here for a later Save Native to write back
                // to. `gui::editor::callbacks::tier_actions::setup_load_selected_callback`'s
                // local branch is the one place this is ever `Some`.
                source_entry_id: None,
                // A native/plain-`.asc` open carries a real recorded
                // schedule, never the angle-table reconstruction fallback.
                used_placeholder: false,
                has_design: true,
            });
            finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
            show_toast(
                ui,
                &format!(
                    "Loaded '{}' (plain .asc, no native sidecar found -- authored meet \
                     constraints and detached facets are not available).",
                    asc_path.display()
                ),
                "success",
            );
        }
        Err(e) => show_toast(ui, &format!("Cannot open: {e}"), "error"),
    }
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
    let st = state.borrow();
    refresh_all(ui, render_ctx, preview_state, solid_last_solved, &st, true);
    // Names the window title (`MainWindow.loaded_design_name`)
    // after whatever design this replace just loaded -- covers both `open_native_pair`
    // and `open_plain_asc`, the two callers of this shared tail. Falls back to empty
    // (bare "Indicatrix Cut") only in the defensive case where `asc_filename` was
    // somehow never set, which neither caller actually does.
    ui.set_loaded_design_name(st.asc_filename.clone().unwrap_or_default().into());
    // Every open path (Open Native, Open Recent, the startup restore) installs a
    // real design -- the empty-state card grid must give way to it.
    push_has_design(ui, &st);
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
    /// What the toast calls the file -- the picked native file's own display path.
    pub(super) native_path_display: String,
    pub(super) asc_filename: String,
    pub(super) asc_text: String,
    /// Drives the toast's class: `"warning"` (which `gui::show_toast` never
    /// auto-dismisses, same as `"error"`, but is coloured and captioned as a note
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
    // This design's OWN sidecar just landed here -- see
    // `CURRENT_NATIVE_PATH`'s own doc comment for why a later Save Native re-writing
    // this exact path needs no overwrite confirmation.
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(PathBuf::from(&native_path_display)));
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
    let (material_note, material_still_unresolved) = if let (Some(snapshot), Some(name)) = (
        loaded.restorable_custom_material.as_ref(),
        loaded.design.material.name.as_deref(),
    ) {
        let gem = gem_material_from_custom_snapshot(name, snapshot);
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let materials = Arc::make_mut(&mut ctx.custom_materials);
        if let Some(pos) = materials
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
        {
            materials[pos] = gem;
        } else {
            materials.push(gem);
        }
        drop(ctx);
        (
            Some(format!(
                " '{name}' was restored from this file's own saved material data."
            )),
            false,
        )
    } else {
        // A material name this build can't resolve AND has no snapshot to
        // restore from (a sidecar with no saved material snapshot, or a
        // material that was never actually custom) still silently becomes
        // Diamond once `MaterialSelection::resolve` runs -- see
        // `MaterialResolution`'s own doc comment. Surfaced here rather than
        // swallowed, so at least the open toast says so.
        let unresolved = matches!(
            loaded.material_resolution,
            indicatrix_cut_core::native::MaterialResolution::Unresolved
        );
        (
            unresolved.then(|| format!(" {}", loaded.material_resolution)),
            unresolved,
        )
    };
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
    // earlier Save Native -- see `EditorState::printed_proportions`'s own doc
    // comment) rather than hard-coded `None`, so Deep Solve still has printed figures
    // to verify against after a Save Native/Open Native round trip, not only on this
    // design's very first "Load Selected" from the catalogue. Still `None` for a
    // sidecar saved before printed proportions were recorded there, or one for a
    // design never loaded from a catalogue row at all.
    let printed_proportions = loaded.printed_proportions;
    state.borrow_mut().replace_wholesale(EditorState {
        deep_solve_result_generation: None,
        design: loaded.design,
        history: History::new(),
        printed_proportions,
        generation: Arc::new(AtomicU64::new(0)),
        design_epoch: Arc::new(AtomicU64::new(0)),
        saved_generation: 0,
        pending_unsaved_action: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        asc_filename: Some(asc_filename),
        original_asc_text: Some(asc_text),
        pending_gear_remap: None,
        pending_retarget: None,
        multi_selected: std::collections::BTreeSet::new(),
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        // Same reasoning as the plain-`.asc` Open Native path
        // above -- the native sidecar's own `[source]` table (`printed_proportions`,
        // just above) carries a catalogue row's PRINTED proportions, but not which
        // row it was loaded from, so there is nothing here to write back to either.
        source_entry_id: None,
        // A native/plain-`.asc` open carries a real recorded
        // schedule, never the angle-table reconstruction fallback.
        used_placeholder: false,
        has_design: true,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    show_toast(
        ui,
        &format!(
            "Loaded '{native_path_display}'. {outcome_note}{}{}",
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
