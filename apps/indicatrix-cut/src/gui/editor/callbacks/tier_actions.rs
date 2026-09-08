//! The Solve/New/Load-Selected/Undo/Redo/tier/preform/yield-input edit callbacks --
//! one `setup_*` function per Slint callback. See this group's `mod.rs` doc comment
//! for the "`History` is the only thing that mutates `Design`" rule every callback
//! here upholds via `EditorState::apply`.

use super::super::{
    loading,
    material_lookup::nearest_built_in_material,
    state::{
        EditorState, PendingGearRemap, angle_nudge_coalesce_key, apply_multi_selection,
        gear_choice_to_teeth, gear_remap_preview, parse_design_material_form,
    },
    view::{SolidLastSolved, refresh_all, refresh_editor_panel_stale, submit_preview_replan},
};
use crate::{
    EditorModel, EditorTierItem, GearRemapRow, LibraryModel, MainWindow, SolidPreviewModel,
    ViewportModel,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        library::remote::fetch_remote_design_source,
        show_toast,
        solid_preview::{
            facet_map::FacetMap,
            preview_state::{PickBuffer, SolidPreviewState},
        },
    },
};
use indicatrix_cut_core::{Edit, History, RemapRounding};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

/// "Solve": the explicit re-solve action -- see this group's `mod.rs` doc comment for
/// why every other edit callback deliberately does NOT do this. The only callback
/// here besides `New`/`Load Selected` that calls `refresh_all` (a real `Design::solve`,
/// potentially multi-second) rather than [`refresh_editor_panel_stale`].
pub(in crate::gui::editor) fn setup_solve_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_solve(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let st = state.borrow();
        refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
    });
}

/// "Create" on the New Design dialog -- replaces the editor state with a brand-new
/// design built from the dialog's preform/gear/symmetry/mirror/material fields, via
/// `Design::fresh_from_spec`. Discards the previous design and its undo/redo history
/// entirely -- there is nothing to preserve across a deliberate "start over".
/// Opening/closing the dialog is pure Slint state; this only fires on "Create".
pub(in crate::gui::editor) fn setup_new_design_create_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_new_design_create(
        move |shape_index: i32,
              half_width: SharedString,
              length_over_width: SharedString,
              depth: SharedString,
              gear_preset_index: i32,
              gear_custom_text: SharedString,
              symmetry_order_text: SharedString,
              mirror: bool,
              material_index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let gear_teeth = match gear_choice_to_teeth(gear_preset_index, &gear_custom_text) {
                Ok(t) => t,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let preform = match loading::parse_preform_form(
                shape_index,
                &half_width,
                &length_over_width,
                &depth,
                gear_teeth.unsigned_abs() as usize,
            ) {
                Ok(p) => p,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            match loading::parse_new_design_form(
                gear_teeth,
                preform,
                &symmetry_order_text,
                mirror,
                material_index,
            ) {
                Ok(spec) => {
                    let mut st = state.borrow_mut();
                    *st = EditorState::fresh_from_spec(spec);
                    refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
                    ui.global::<EditorModel>().set_new_dialog_open(false);
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

thread_local! {
    /// The live `EditorState`, stashed here purely so
    /// [`setup_load_selected_callback`]'s remote branch can still reach it once
    /// control is back on the UI thread. `gui::library::remote::fetch_remote_design_source`
    /// (itself wrapping `bridge::library::source::spawn_library_request`) requires its
    /// completion callback to be `Send + 'static`, because that callback's value is
    /// physically moved through a `std::thread::spawn` closure before being handed to
    /// `Weak::upgrade_in_event_loop` -- and an `Rc<RefCell<EditorState>>` capture can
    /// never satisfy `Send` (Rc's ref-count is non-atomic), no matter that the
    /// callback only ever actually RUNS back on the UI thread. Mirrors
    /// `auto_solve::RUNTIME`'s own `thread_local!` for the identical reason (see that
    /// module's doc comment). Set once, the first time [`setup_load_selected_callback`]
    /// runs -- there is only ever one `EditorState` for the app's lifetime -- and read
    /// back (never cleared) inside the remote branch's completion closure, which never
    /// captures the `Rc` itself.
    static REMOTE_LOAD_TARGET: RefCell<Option<Rc<RefCell<EditorState>>>> = const { RefCell::new(None) };
}

/// Bundles [`apply_loaded_design`]'s per-call payload -- kept as one struct (rather
/// than three more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning `auto_solve::BackgroundSolveResult` uses.
struct LoadedDesignOutcome<'a> {
    loaded: loading::LoadedDesign,
    printed_proportions: Option<indicatrix::geometry::stone_metrics::ExternalProportions>,
    /// What the success toast calls the design -- see [`apply_loaded_design`]'s own
    /// doc comment.
    label: &'a str,
}

/// The shared tail of [`setup_load_selected_callback`]'s local and remote branches:
/// replaces `state` wholesale with `loaded`'s design (fresh `History`, every other
/// per-design transient field cleared -- the same wholesale replacement
/// `setup_new_design_create_callback` does for "New"), pushes a real solve into the
/// panel/viewport via `refresh_all`, shows the "loaded"/"placeholder" toast, and
/// offers the catalogue material-suggestion banner built from `loaded.design`'s own
/// schedule RI.
///
/// `loaded_label` names what the success toast calls the design: the local branch's
/// own catalogue title, or the remote branch's bare `.asc` file name -- a remote
/// fetch (`gui::library::remote::RemoteDesignSource`) never carries the catalogue
/// title alongside its `.asc` text, only the attachment's own name.
fn apply_loaded_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    outcome: LoadedDesignOutcome<'_>,
) {
    let LoadedDesignOutcome {
        loaded,
        printed_proportions,
        label: loaded_label,
    } = outcome;
    let schedule_ri = loaded.design.meta.refractive_index;
    let mut st = state.borrow_mut();
    *st = EditorState {
        design: loaded.design,
        history: History::new(),
        printed_proportions,
        generation: Arc::new(AtomicU64::new(0)),
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        asc_filename: loaded.asc_filename,
        original_asc_text: loaded.original_asc_text,
        pending_gear_remap: None,
        pending_retarget: None,
        multi_selected: BTreeSet::new(),
    };
    refresh_all(ui, render_ctx, preview_state, solid_last_solved, &st);
    if loaded.used_placeholder {
        show_toast(
            ui,
            "Loaded a reconstructed schedule -- mast distances are \
             placeholders (no attached .asc file was found); adjust \
             masts before exporting.",
            "info",
        );
    } else {
        show_toast(
            ui,
            &format!("Loaded '{loaded_label}' into the editor."),
            "success",
        );
    }
    drop(st);
    // Suggests the built-in material whose n_D is nearest the schedule RI (within
    // 0.01) -- never applied automatically, only offered as a banner the user can
    // dismiss or accept.
    if let Some((name, ri)) = nearest_built_in_material(schedule_ri, 0.01) {
        ui.global::<EditorModel>()
            .set_material_suggestion_name(name.into());
        ui.global::<EditorModel>()
            .set_material_suggestion_text(format!("Set material to {name} (RI {ri:.4})?").into());
    } else {
        ui.global::<EditorModel>()
            .set_material_suggestion_name("".into());
        ui.global::<EditorModel>()
            .set_material_suggestion_text("".into());
    }
}

/// "Load Selected": replaces the editor state with the currently selected catalogue
/// design. Local library: reads the full record straight from `db` and prefers a
/// real attached `.asc` (see `loading::design_from_full_record`). Remote library:
/// fetches that entry's own `.asc` text off the UI thread
/// (`gui::library::remote::fetch_remote_design_source`) and builds the identical
/// `Design` from it via `loading::design_from_asc_text` -- there is no printed-
/// proportions/placeholder-reconstruction data available over that wire, so a
/// remote load never offers Deep Solve's external verification and never falls back
/// to the angle-table placeholder (a remote entry with no attached `.asc` reports
/// `DesignSourceNotAvailable`, surfaced as a toast instead).
pub(in crate::gui::editor) fn setup_load_selected_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let state = Rc::clone(state);
    REMOTE_LOAD_TARGET.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&state)));
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let db = Arc::clone(db);
    let source = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_load_selected(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
        if entry_id < 0 {
            show_toast(&ui, "No diagram selected to load.", "error");
            return;
        }

        let current_source = source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(worker) = current_source.worker().cloned() {
            // Remote: fetch this entry's own `.asc` text off the UI thread. The
            // completion closure below must be `Send + 'static` (see
            // `REMOTE_LOAD_TARGET`'s own doc comment for why it deliberately never
            // captures `state` directly) -- everything it DOES capture
            // (`render_ctx`/`preview_state`/`solid_last_solved`) is already `Arc`-
            // wrapped Send-safe data this crate moves across thread boundaries
            // elsewhere (e.g. `auto_solve::dispatch_background_solve`).
            let render_ctx = Arc::clone(&render_ctx);
            let preview_state = Arc::clone(&preview_state);
            let solid_last_solved = Arc::clone(&solid_last_solved);
            fetch_remote_design_source(
                ui.as_weak(),
                worker,
                i64::from(entry_id),
                move |ui, result| match result {
                    Ok(remote) => match loading::design_from_asc_text(
                        &remote.file_name,
                        &remote.asc_text,
                        None,
                    ) {
                        Ok(loaded) => REMOTE_LOAD_TARGET.with(|cell| {
                            let target = cell.borrow().clone();
                            if let Some(state) = target {
                                apply_loaded_design(
                                    ui,
                                    &state,
                                    &render_ctx,
                                    &preview_state,
                                    &solid_last_solved,
                                    LoadedDesignOutcome {
                                        loaded,
                                        printed_proportions: None,
                                        label: &remote.file_name,
                                    },
                                );
                            }
                        }),
                        Err(e) => show_toast(
                            ui,
                            &format!(
                                "'{}' failed to parse as a .asc cutting schedule: {e}",
                                remote.file_name
                            ),
                            "error",
                        ),
                    },
                    Err(e) => show_toast(ui, &e, "error"),
                },
            );
            return;
        }

        // Local: read the full record straight from the DB.
        let full = {
            let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            db.get_diagram_full(i64::from(entry_id))
        };
        let Ok(Some(full)) = full else {
            show_toast(&ui, "Diagram detail not found.", "error");
            return;
        };
        match loading::design_from_full_record(&full) {
            Ok(loaded) => {
                let printed_proportions = loading::external_proportions_from_full_record(&full);
                apply_loaded_design(
                    &ui,
                    &state,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    LoadedDesignOutcome {
                        loaded,
                        printed_proportions,
                        label: &full.title,
                    },
                );
            }
            Err(e) => show_toast(&ui, &e, "error"),
        }
    });
}

/// The material-suggestion banner's "Set Material" action -- see
/// `setup_load_selected_callback` for when this banner is populated. Applies
/// [`loading::material_selection_for_accepted_suggestion`]'s
/// [`indicatrix_cut_core::MaterialSelection`] (pinning `refractive_index_override` to
/// the schedule's own recorded RI whenever the suggested built-in's `n_D` would
/// otherwise silently move it by more than 0.01) as one undoable
/// [`Edit::SetMaterial`], then clears the banner.
pub(in crate::gui::editor) fn setup_material_suggestion_accept_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_material_suggestion_accept(move |name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let schedule_ri = st.design.meta.refractive_index;
            let material = loading::material_selection_for_accepted_suggestion(
                &name,
                schedule_ri,
                &st.design.material,
            );
            match st.apply(Edit::SetMaterial { material }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    // Material-only: geometry is unchanged, only the critical-angle
                    // overlay (which depends on n_D) needs a fresh render.
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
            drop(st);
            ui.global::<EditorModel>()
                .set_material_suggestion_name("".into());
            ui.global::<EditorModel>()
                .set_material_suggestion_text("".into());
        });
}

/// The material-suggestion banner's dismiss ("No thanks") action -- just clears the
/// banner; the schedule's material/RI stay exactly as loaded.
pub(in crate::gui::editor) fn setup_material_suggestion_dismiss_callback(ui: &MainWindow) {
    ui.global::<EditorModel>()
        .on_material_suggestion_dismiss(move || {});
}

/// "Undo": a no-op (no toast, no viewport refresh) when there is nothing to undo --
/// `EditorView`'s own button is already disabled via `can_undo` in that case, so this
/// only guards against a stale click racing a state change, not the common path.
pub(in crate::gui::editor) fn setup_undo_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_undo(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        match st.undo() {
            Ok(true) => {
                refresh_editor_panel_stale(&ui, &render_ctx, &st);
                // Undo can move any tier (or a whole structural AddTier/RemoveTier
                // change), so an edit whose blast radius isn't tracked precisely forces
                // a full solve rather than guessing a `dirty` set.
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    true,
                );
            }
            Ok(false) => {}
            // The recorded inverse edit failed to replay -- the design/history stack
            // are left as `EditorState::undo` found them (the failed edit stays on the
            // undo stack, see `History::undo`'s doc comment), so this is safe to just
            // report rather than panic.
            Err(e) => show_toast(&ui, &format!("Undo failed: {e}"), "error"),
        }
    });
}

/// "Redo": same no-op-when-nothing-to-do treatment as [`setup_undo_callback`].
pub(in crate::gui::editor) fn setup_redo_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_redo(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        match st.redo() {
            Ok(true) => {
                refresh_editor_panel_stale(&ui, &render_ctx, &st);
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    true,
                );
            }
            Ok(false) => {}
            // See `setup_undo_callback`'s matching arm -- same recovery guarantee,
            // symmetrically for the redo stack.
            Err(e) => show_toast(&ui, &format!("Redo failed: {e}"), "error"),
        }
    });
}

/// "Apply Preform": parses the form (see `loading::parse_preform_form`) and, on
/// success, applies it through `EditorState::apply` as a [`Edit::SetPreform`] -- the
/// only `Edit` variant this callback ever constructs.
pub(in crate::gui::editor) fn setup_apply_preform_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_preform(
        move |shape_index: i32,
              half_width: SharedString,
              length_over_width: SharedString,
              depth: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let cylinder_sides = st.design.meta.gear_teeth_abs() as usize;
            match loading::parse_preform_form(
                shape_index,
                &half_width,
                &length_over_width,
                &depth,
                cylinder_sides,
            ) {
                // `SetPreform` never fails (it names no tier index), so the only
                // `Err` path here is this function's own parse failure, already reported.
                Ok(preform) => {
                    let _ = st.apply(Edit::SetPreform { preform });
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    // The preform reshapes the bounding planes but never moves a
                    // tier's own mast -- no tier is dirty.
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// "Apply Yield Inputs": parses the form and, on success, applies both halves through
/// `EditorState::apply` as [`Edit::SetGirdleDiameterMm`] then [`Edit::SetMaterial`] --
/// two separate, independently-undoable edits, not a single combined one, since
/// `indicatrix-cut-core` has no "apply several edits atomically" primitive and
/// neither edit can fail once parsing has succeeded.
pub(in crate::gui::editor) fn setup_apply_yield_inputs_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_yield_inputs(
        move |girdle_diameter_mm: SharedString,
              material_index: i32,
              specific_gravity_override: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            match super::super::state::parse_yield_form(
                &girdle_diameter_mm,
                material_index,
                &specific_gravity_override,
            ) {
                Ok((girdle_diameter_mm, material)) => {
                    let _ = st.apply(Edit::SetGirdleDiameterMm { girdle_diameter_mm });
                    let _ = st.apply(Edit::SetMaterial { material });
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    // Girdle diameter and material alone never move a tier's own
                    // mast -- no tier is dirty.
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// "Add Tier" / "Save Tier": parses the form and applies it through
/// `EditorState::apply` as `AddTier` (index `< 0`, "new tier" mode -- appended at the
/// end) or `ModifyTier` (an existing row's index).
pub(in crate::gui::editor) fn setup_save_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_save_tier(
        move |index: i32,
              angle: SharedString,
              constraint_kind: i32,
              constraint_text: SharedString,
              name: SharedString,
              indices: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // An existing row keeps its own `imported_meet` across this save --
            // looked up here, before the mutable borrow below, so editing an
            // imported tier's name/angle/indices never silently drops what the file
            // claimed it meets.
            let imported_meet = (index >= 0)
                .then(|| {
                    state
                        .borrow()
                        .design
                        .tiers
                        .get(index as usize)?
                        .imported_meet
                        .clone()
                })
                .flatten();
            match loading::parse_tier_form(
                &angle,
                constraint_kind,
                &constraint_text,
                &name,
                &indices,
                imported_meet,
            ) {
                Ok(tier) => {
                    let mut st = state.borrow_mut();
                    let dirty_index = if index < 0 {
                        st.design.tiers.len()
                    } else {
                        index as usize
                    };
                    let edit = if index < 0 {
                        Edit::AddTier {
                            index: st.design.tiers.len(),
                            tier,
                        }
                    } else {
                        Edit::ModifyTier {
                            index: index as usize,
                            tier,
                        }
                    };
                    match st.apply(edit) {
                        Ok(()) => {
                            refresh_editor_panel_stale(&ui, &render_ctx, &st);
                            // `AddTier` changes the tier count, so the alignment
                            // check falls back to a full solve regardless of
                            // `dirty`; for `ModifyTier` this one index is exactly
                            // what changed.
                            submit_preview_replan(
                                &ui,
                                &render_ctx,
                                &preview_state,
                                &solid_last_solved,
                                &st,
                                BTreeSet::from([dirty_index]),
                                false,
                            );
                        }
                        Err(e) => show_toast(&ui, &e.to_string(), "error"),
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// The tier-list row's own "x" button: applies [`Edit::RemoveTier`] through
/// `EditorState::apply`.
pub(in crate::gui::editor) fn setup_remove_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_remove_tier(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if index < 0 {
                return;
            }
            let mut st = state.borrow_mut();
            match st.apply(Edit::RemoveTier {
                index: index as usize,
            }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    // Tier count changed -- the alignment check falls back to a full solve.
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The tier list's inline angle cell (`editor_view.slint`'s `TierAngleCell`): commits
/// on Enter or focus loss as one [`Edit::ModifyTier`] (angle only, everything else on
/// the tier untouched) through `EditorState::apply`. Invalid text is reported via
/// toast and left uncommitted -- since [`refresh_editor_panel_stale`] is skipped on
/// that path, `editor_tiers` (and so the cell's own display) is untouched too, which
/// only reverts the cell's local scratch text because `TierAngleCell` recreates its
/// `LineEdit` (and re-seeds it from the row's real `angle_deg`) on every fresh
/// `editor_tiers` push -- see that component's own doc comment. A parsed value
/// identical to the tier's current angle is a silent no-op (no `Edit`, no refresh):
/// committing an unchanged value should not spend an undo slot.
pub(in crate::gui::editor) fn setup_inline_set_angle_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_inline_set_angle(move |index: i32, text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(current) = st.design.tiers.get(index) else {
                return;
            };
            match loading::parse_angle_only(&text) {
                Ok(angle_deg) => {
                    // Bit-exact, not `==` (clippy::float_cmp): the parsed text
                    // round-tripping to a genuinely unchanged value is the only case this
                    // needs to catch -- committing a real no-op should not spend an undo
                    // slot -- and comparing bit patterns rather than magnitudes sidesteps
                    // that lint without needing an epsilon whose size would be arbitrary
                    // here.
                    if angle_deg.to_bits() == current.angle_deg.to_bits() {
                        return;
                    }
                    let mut tier = current.clone();
                    tier.angle_deg = angle_deg;
                    match st.apply(Edit::ModifyTier { index, tier }) {
                        Ok(()) => {
                            refresh_editor_panel_stale(&ui, &render_ctx, &st);
                            submit_preview_replan(
                                &ui,
                                &render_ctx,
                                &preview_state,
                                &solid_last_solved,
                                &st,
                                BTreeSet::from([index]),
                                false,
                            );
                        }
                        Err(e) => show_toast(&ui, &e.to_string(), "error"),
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        });
}

/// The tier list's angle-nudge path -- the inline cell's Up/Down/wheel and the tier
/// form's Angle field's Up/Down (see `editor_view.slint`'s `TierAngleCell::step`,
/// `TierAngleCell::nudge`, and the form's own `LineEdit.key-pressed`) all forward
/// here as `(anchor_index, delta_deg)`.
///
/// When `anchor_index` is part of a multi-select group of two or more
/// (`EditorState::multi_selected`), every selected tier is nudged together as ONE
/// undoable [`Edit::RetargetAngles`] -- reusing that existing "several tiers, one
/// undo step, exact per-tier inverse" primitive rather than a new `Edit::Batch`
/// variant, since `RetargetAngles` already is exactly that (see its own doc comment
/// in `indicatrix_cut_core::Edit`). A lone tier still goes through the same
/// `RetargetAngles` path with a single-element `changes` vec, so there is only one
/// code path here rather than a single/multi split.
///
/// Applied through [`EditorState::apply_coalescing`] (not [`EditorState::apply`]) so
/// several nudges typed/scrolled in quick succession collapse into one undo step --
/// see [`angle_nudge_coalesce_key`] for how the coalescing key is derived from the
/// nudge's actual target set, distinguishing a lone tier's nudge from a
/// multi-selected group containing that same tier.
pub(in crate::gui::editor) fn setup_nudge_angle_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_nudge_angle(move |anchor_index: i32, delta_deg: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(anchor_index) = usize::try_from(anchor_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let is_multi_target =
                st.multi_selected.len() > 1 && st.multi_selected.contains(&anchor_index);
            let targets: Vec<usize> = if is_multi_target {
                st.multi_selected.iter().copied().collect()
            } else {
                vec![anchor_index]
            };
            let delta_deg = f64::from(delta_deg);
            let changes: Option<Vec<(usize, f64, f64)>> = targets
                .iter()
                .map(|&index| {
                    st.design
                        .tiers
                        .get(index)
                        .map(|tier| (index, tier.angle_deg, tier.angle_deg + delta_deg))
                })
                .collect();
            let Some(changes) = changes else {
                return;
            };
            if changes.is_empty() {
                return;
            }
            let key = angle_nudge_coalesce_key(&targets);
            match st.apply_coalescing(Edit::RetargetAngles { changes }, key) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        targets.into_iter().collect(),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// A row's "Duplicate" button and the tier list's Ctrl+D: appends a copy of the
/// named tier (name suffixed `'`, same indices/angle/constraint/detached set) as a
/// new [`Edit::AddTier`] through `EditorState::apply`, then moves the tier-list
/// selection to the copy. The copy's `imported_meet` is always cleared -- it is a
/// new, user-authored row, not itself something a real `.asc` file's `G` field ever
/// made a claim about, even though the tier it was copied FROM might carry one.
pub(in crate::gui::editor) fn setup_duplicate_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_duplicate_tier(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let Some(source) = st.design.tiers.get(index) else {
                return;
            };
            let mut duplicate = source.clone();
            duplicate.name = format!("{}'", duplicate.name);
            duplicate.imported_meet = None;
            let new_index = st.design.tiers.len();
            match st.apply(Edit::AddTier {
                index: new_index,
                tier: duplicate,
            }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    // `AddTier` changes the tier count -- same full-solve fallback
                    // `setup_save_tier_callback`'s own `AddTier` path uses.
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([new_index]),
                        false,
                    );
                    drop(st);
                    ui.global::<EditorModel>()
                        .set_selected_tier_index(new_index as i32);
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The tier list's Ctrl+click: toggles one row into/out of
/// [`EditorState::multi_selected`], then patches `EditorTierItem::multi_selected`
/// onto the ALREADY-PUSHED `editor_tiers` model in place, rather than calling
/// [`refresh_editor_panel_stale`] -- toggling a multi-select highlight changes
/// nothing about `Design`, so it must never re-label the validation banner "Not
/// solved" the way every real edit's stale-refresh does. See
/// [`super::super::state::tier_items_stale`]'s own doc comment for why this function
/// exists instead of threading the selection through that builder.
pub(in crate::gui::editor) fn setup_toggle_multi_select_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_toggle_multi_select(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            if !st.multi_selected.remove(&index) {
                st.multi_selected.insert(index);
            }
            let multi_selected = st.multi_selected.clone();
            drop(st);
            let mut rows: Vec<EditorTierItem> =
                ui.global::<EditorModel>().get_tiers().iter().collect();
            apply_multi_selection(&mut rows, &multi_selected);
            ui.global::<EditorModel>()
                .set_tiers(ModelRc::new(VecModel::from(rows)));
        });
}

/// The tier-list row's "Detach"/"Reattach" toggle: applies
/// `Design::detach_all_in_tier`/`Design::reattach_all_in_tier` through
/// `EditorState::apply`, flipping [`EditorTierItem::is_detached`](crate::EditorTierItem)
/// so one button serves both directions. An explicit, visible escape hatch: a
/// symmetric tier's occurrences stay linked (an edit moves the whole orbit) until the
/// user clicks this, and detaching never happens as a side effect of any other action.
pub(in crate::gui::editor) fn setup_toggle_detach_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_toggle_detach(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if index < 0 {
                return;
            }
            let mut st = state.borrow_mut();
            let index = index as usize;
            let Some(tier) = st.design.tiers.get(index) else {
                return;
            };
            let edit_result = if tier.detached.is_empty() {
                st.design.detach_all_in_tier(index)
            } else {
                st.design.reattach_all_in_tier(index)
            };
            match edit_result.and_then(|edit| st.apply(edit)) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The design settings panel's material combo + RI override field -- applies
/// [`Edit::SetMaterial`] via [`parse_design_material_form`], reading the combo's
/// current option list from `editor_material_combo_options` (pushed fresh every
/// refresh, so this always parses against the SAME list the user actually saw).
pub(in crate::gui::editor) fn setup_apply_design_material_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_design_material(
        move |combo_index: i32, ri_override_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let options: Vec<String> = ui
                .global::<EditorModel>()
                .get_material_combo_options()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let mut st = state.borrow_mut();
            match parse_design_material_form(
                combo_index,
                &ri_override_text,
                &options,
                &st.design.material,
            ) {
                Ok(material) => match st.apply(Edit::SetMaterial { material }) {
                    Ok(()) => {
                        refresh_editor_panel_stale(&ui, &render_ctx, &st);
                        submit_preview_replan(
                            &ui,
                            &render_ctx,
                            &preview_state,
                            &solid_last_solved,
                            &st,
                            BTreeSet::new(),
                            false,
                        );
                    }
                    Err(e) => show_toast(&ui, &e.to_string(), "error"),
                },
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// The design settings panel's Symmetry/Mirror "Apply" -- wholesale
/// [`Edit::SetSchedule`], keeping the design's CURRENT gear (this control never
/// changes gear -- that's [`setup_gear_apply_callback`]'s job, since only a gear
/// change needs the remap confirmation).
pub(in crate::gui::editor) fn setup_apply_symmetry_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_symmetry(
        move |symmetry_order_text: SharedString, mirror: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let symmetry_order: u32 = match symmetry_order_text.trim().parse() {
                Ok(v) if v >= 1 => v,
                _ => {
                    show_toast(
                        &ui,
                        "Symmetry order must be a positive whole number.",
                        "error",
                    );
                    return;
                }
            };
            let mut st = state.borrow_mut();
            let gear_teeth = st.design.meta.gear_teeth;
            match st.apply(Edit::SetSchedule {
                gear_teeth,
                symmetry_order,
                mirror,
            }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    // Symmetry/mirror can move every tier's index-wheel position, not
                    // tracked precisely here, so force a full (non-blocking) solve.
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        true,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );
}

/// The design settings panel's gear combo "Apply" -- computes a real dry-run preview
/// ([`gear_remap_preview`]) and opens the confirmation panel rather than applying
/// anything directly; see [`setup_gear_remap_confirm_callback`]/
/// [`setup_gear_remap_cancel_callback`] for how that panel closes. A no-op (just a
/// toast) when the chosen gear is already the design's current one.
pub(in crate::gui::editor) fn setup_gear_apply_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_gear_apply(
        move |gear_preset_index: i32, gear_custom_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let to_gear = match gear_choice_to_teeth(gear_preset_index, &gear_custom_text) {
                Ok(t) => t,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let mut st = state.borrow_mut();
            let from_gear = st.design.meta.gear_teeth;
            if to_gear == from_gear {
                show_toast(&ui, "Already using this gear.", "info");
                return;
            }
            let rounding = RemapRounding::Nearest;
            let rows: Vec<GearRemapRow> =
                gear_remap_preview(&st.design, from_gear, to_gear, rounding);
            st.pending_gear_remap = Some(PendingGearRemap {
                from_gear,
                to_gear,
                symmetry_order: st.design.meta.symmetry_order,
                mirror: st.design.meta.mirror,
                rounding,
            });
            drop(st);
            ui.global::<EditorModel>()
                .set_gear_remap_rows(ModelRc::new(VecModel::from(rows)));
            ui.global::<EditorModel>().set_gear_remap_open(true);
        },
    );
}

/// The gear-remap confirmation panel's "Apply" -- commits
/// [`EditorState::pending_gear_remap`] as two separate, independently-undoable
/// `History` steps ([`Edit::RemapIndices`] then [`Edit::SetSchedule`]), then closes
/// the panel. A no-op (closes the panel only) if nothing is pending -- defensive
/// only, since this button only shows while a real remap is pending.
pub(in crate::gui::editor) fn setup_gear_remap_confirm_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_gear_remap_confirm(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let Some(pending) = st.pending_gear_remap.take() else {
            ui.global::<EditorModel>().set_gear_remap_open(false);
            return;
        };
        let remap_result = st.apply(Edit::RemapIndices {
            from_gear: pending.from_gear,
            to_gear: pending.to_gear,
            rounding: pending.rounding,
        });
        let schedule_result = remap_result.and_then(|()| {
            st.apply(Edit::SetSchedule {
                gear_teeth: pending.to_gear,
                symmetry_order: pending.symmetry_order,
                mirror: pending.mirror,
            })
        });
        ui.global::<EditorModel>().set_gear_remap_open(false);
        match schedule_result {
            Ok(()) => {
                refresh_editor_panel_stale(&ui, &render_ctx, &st);
                // A gear remap rewrites every tier's index-wheel position -- force a
                // full solve rather than guessing a `dirty` set.
                submit_preview_replan(
                    &ui,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    true,
                );
            }
            Err(e) => show_toast(&ui, &e.to_string(), "error"),
        }
    });
}

/// The gear-remap confirmation panel's "Cancel" -- discards the pending remap
/// without touching `Design`; the gear combo's display is restored to the design's
/// real current gear on the next refresh.
pub(in crate::gui::editor) fn setup_gear_remap_cancel_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_gear_remap_cancel(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        state.borrow_mut().pending_gear_remap = None;
        ui.global::<EditorModel>().set_gear_remap_open(false);
    });
}

/// The viewport's "Linked to design" checkbox -- when switched ON, syncs the shared
/// viewport's render material to the design's own material IMMEDIATELY, so turning it
/// on feels responsive. `refresh_design_settings` keeps it in sync from then on.
pub(in crate::gui::editor) fn setup_viewport_material_linked_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_viewport_material_linked_changed(move |linked: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if linked {
                let st = state.borrow();
                refresh_editor_panel_stale(&ui, &render_ctx, &st);
            }
        });
}

/// The Solid viewport's hover callback -- looks up the facet under the cursor against
/// the pick buffer of the LAST rendered frame and sets `editor_solid_hover_text` to
/// `FacetMap::hover_text`'s result. A silent no-op off the silhouette or before
/// anything has ever rendered.
pub(in crate::gui::editor) fn setup_solid_facet_hover_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    solid_pick: &Arc<Mutex<Option<PickBuffer>>>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let solid_pick = Arc::clone(solid_pick);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_hover(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            // `as u32` saturates a negative `f32` to `0`; `PickBuffer::facet_at` already
            // bounds-checks against the frame's width/height, so a hover just past the
            // image's edge simply misses (`None`) rather than reading garbage.
            let Some(facet_id) = solid_pick
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|pick| pick.facet_at(x.max(0.0) as u32, y.max(0.0) as u32))
            else {
                ui.global::<SolidPreviewModel>().set_hover_text("".into());
                return;
            };
            let solved = solid_last_solved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_default();
            let facet_map = FacetMap::from_design(&st.design, &solved);
            let n_d = st.design.effective_refractive_index();
            ui.global::<SolidPreviewModel>()
                .set_hover_text(facet_map.hover_text(facet_id as usize, n_d).into());
        });
}

/// The Solid viewport's click callback -- the forward half of the "click selects the
/// tier in the list" link (see [`setup_solid_selected_tier_changed_callback`] for the
/// reverse half).
pub(in crate::gui::editor) fn setup_solid_facet_click_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    solid_pick: &Arc<Mutex<Option<PickBuffer>>>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let solid_pick = Arc::clone(solid_pick);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_click(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let Some(facet_id) = solid_pick
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|pick| pick.facet_at(x.max(0.0) as u32, y.max(0.0) as u32))
            else {
                return;
            };
            let solved = solid_last_solved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_default();
            let facet_map = FacetMap::from_design(&st.design, &solved);
            if let Some(tier_index) = facet_map.tier_of(facet_id as usize) {
                ui.global::<EditorModel>()
                    .set_selected_tier_index(tier_index as i32);
            }
        });
}

/// The reverse link: whenever the tier list's selection changes (a row click, or
/// [`setup_solid_facet_click_callback`] setting `editor_selected_tier_index` from a
/// viewport click), re-submits a redraw with the new `selected_tier` so the overlay
/// tint follows it. Cheap: the mesh is unchanged (`dirty` empty), so the worker's
/// `MeshCache` hits and only the style/render redo.
pub(in crate::gui::editor) fn setup_solid_selected_tier_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_selected_tier_changed(move |_index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            submit_preview_replan(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &st,
                BTreeSet::new(),
                false,
            );
        });
}
