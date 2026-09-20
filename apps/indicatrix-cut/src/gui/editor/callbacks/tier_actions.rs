//! The Solve/New/Load-Selected/Undo/Redo/tier/preform/yield-input edit callbacks --
//! one `setup_*` function per Slint callback. See this group's `mod.rs` doc comment
//! for the "`History` is the only thing that mutates `Design`" rule every callback
//! here upholds via `EditorState::apply`.

use super::{
    super::{
        auto_solve, loading,
        material_lookup::nearest_built_in_material,
        native_io::do_open_native,
        state::{
            ANGLE_NUDGE_COALESCE_WINDOW, EditorState, PendingGearRemap, PendingUnsavedAction,
            PushedScratch, angle_nudge_coalesce_key, apply_multi_selection,
            first_unresolved_meet_name, gear_choice_to_teeth, gear_remap_preview,
            parse_design_material_form, push_multi_selected_count, push_tiers, tier_matches_filter,
            tiers_incomplete_under_proposed_symmetry,
        },
        view::{
            SolidLastSolved, push_selected_tier_chips, refresh_all, refresh_editor_panel_stale,
            submit_preview_replan,
        },
    },
    solve_actions::clear_analysis_results,
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
            preview_state::{FacetOverlay, SolidPickState, SolidPreviewState},
        },
    },
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Edit, FreshDesignSpec, History, RemapRounding};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

/// CAD audit item 137: a cylindrical preform's side count, fixed and
/// independent of the design's own index gear. The preform is a piece of
/// rough, not a machine setting -- deriving `PreformSpec::cylinder`'s side
/// count from whichever gear happened to be selected at Apply Preform/New
/// Design time was the category error: switching gear afterward (say 96 ->
/// 8) used to leave a stale 96-sided cylinder with nothing left to revisit
/// it, and a preform applied under an 8-tooth gear stayed visibly octagonal
/// after switching gear back to 96. 96 is high enough that the rough reads as
/// a smooth round in the viewport at every zoom level this app renders --
/// the same figure `indicatrix_cut_core::PreformSpec::cylinder_for_schedule`'s own doc
/// comment uses, for the unrelated "reconstructed once from an already-
/// authored schedule" case in `loading::default_preform_for_schedule`, which
/// this constant deliberately does not touch.
const FIXED_CYLINDER_PREFORM_SIDES: usize = 96;

thread_local! {
    /// The last [`FacetOverlay`] submitted to the solid preview's worker thread --
    /// mirrored here since [`SolidPreviewState::request_facet_overlay`] replaces the
    /// WHOLE overlay on every call (there is no "just update the hover field"
    /// entry point): [`setup_solid_facet_hover_callback`]/[`setup_solid_facet_click_
    /// callback`]/[`setup_toggle_multi_select_callback`]/[`setup_solid_selected_tier_
    /// changed_callback`] each mutate only their own field of this cached copy and
    /// resubmit the merged whole, so hovering a facet never clears the click/multi-
    /// select highlight and vice versa. UI-thread-only, same reasoning as
    /// `auto_solve::RUNTIME`'s own `thread_local!`.
    static FACET_OVERLAY: RefCell<FacetOverlay> = RefCell::new(FacetOverlay::default());
}

/// Mutates the cached [`FacetOverlay`] via `mutate`, then resubmits the merged
/// whole to `preview_state` -- the one place every overlay-touching callback below
/// goes through, so the merge-then-resubmit sequence is never duplicated or done
/// slightly differently at two call sites.
fn resubmit_facet_overlay(
    preview_state: &SolidPreviewState,
    mutate: impl FnOnce(&mut FacetOverlay),
) {
    let overlay = FACET_OVERLAY.with(|cell| {
        let mut overlay = cell.borrow_mut();
        mutate(&mut overlay);
        overlay.clone()
    });
    preview_state.request_facet_overlay(overlay);
}

/// "Solve": the explicit re-solve action -- see this group's `mod.rs` doc comment for
/// why every other edit callback deliberately does NOT do this. The only callback
/// here besides `New`/`Load Selected` that calls `refresh_all` (a real `Design::solve`,
/// potentially multi-second) rather than [`refresh_editor_panel_stale`].
///
/// CAD audit item 216: guards against Deep Solve or Optimize already running, not
/// only a re-entrant Solve click -- Solve, Deep Solve and Optimize used to each
/// guard only their OWN `*_running` flag, so any two of the three could be launched
/// at once (each roughly doubling the others' runtime). `EditorModel.busy_action`
/// (`ui/models/editor.slint`) is the single Slint-side derived source of truth for
/// all three, shown in `EditorCommandBar`/`EditorStatusStrip`; this reads the three
/// flags it is built from directly (equivalent to checking `busy_action != ""`)
/// since that is what every other `on_*` guard in this module already reads, and
/// pulling in a fourth (derived) getter here would buy nothing.
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
        let model = ui.global::<EditorModel>();
        if model.get_solve_running()
            || model.get_deep_solve_running()
            || model.get_optimize_running()
        {
            return;
        }
        let st = state.borrow();
        refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
    });
}

/// "Create" on the New Design dialog -- replaces the editor state with a brand-new
/// design built from the dialog's preform/gear/symmetry/mirror/material fields, via
/// `Design::fresh_from_spec`. Discards the previous design and its undo/redo history
/// entirely -- there is nothing to preserve across a deliberate "start over", so once
/// the fields parse, this checks [`EditorState::is_dirty`] before actually discarding
/// anything: a dirty design stashes [`PendingUnsavedAction::New`] and opens the
/// Save/Discard/Cancel guard instead of proceeding straight to [`do_new_design_create`]
/// -- see [`setup_unsaved_guard_dispatch`] for how "Save"/"Discard" resume it.
/// Validation runs BEFORE that check (matching `native_io::do_open_native`'s own
/// ordering) so a form error still surfaces immediately rather than behind a
/// confirmation dialog for a create that was never going to succeed anyway.
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
              material_index: i32,
              template_index: i32| {
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
                FIXED_CYLINDER_PREFORM_SIDES,
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
                    if state.borrow().is_dirty() {
                        state.borrow_mut().pending_unsaved_action =
                            Some(PendingUnsavedAction::New {
                                spec,
                                template_index,
                            });
                        ui.global::<EditorModel>().set_unsaved_dialog_message(
                            "Starting a new design will discard the current one's unsaved \
                             changes."
                                .into(),
                        );
                        ui.global::<EditorModel>().set_unsaved_dialog_open(true);
                    } else {
                        do_new_design_create(
                            &ui,
                            &state,
                            &render_ctx,
                            &preview_state,
                            &solid_last_solved,
                            spec,
                            template_index,
                        );
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// The actual "New" work, run either directly (a clean design) or as
/// [`PendingUnsavedAction::New`]'s resume once the Save/Discard/Cancel guard clears --
/// see [`setup_new_design_create_callback`]'s own doc comment for why the dirty check
/// runs before this is ever called, not inside it.
fn do_new_design_create(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    spec: FreshDesignSpec,
    // The "Start from" choice: 0 empty, 1 standard round brilliant (CAD audit
    // item 207). Anything else is treated as empty rather than panicking -- the
    // combo is the only producer, but a stale index must not lose a design.
    template_index: i32,
) {
    let mut st = state.borrow_mut();
    // `replace_wholesale`, not a plain `*st = ...`: carries this state's own
    // `generation` `Arc` across the replacement (and bumps it) instead of letting
    // `fresh_from_spec` hand back a brand-new one, so a background Deep
    // Solve/Optimize/auto-solve dispatched against the design being replaced still
    // observes that it changed (see that method's own doc comment).
    st.replace_wholesale(EditorState::fresh_from_spec(spec));
    // Seeded AFTER the replacement, directly on the fresh design, rather than
    // through `History`: this is the design's starting state, not an edit to it,
    // so it must not be undoable back to an empty schedule the cutter never saw.
    // `saved_generation` already matches, so the new design still reads as clean.
    if template_index == 1 {
        st.design.tiers = ConstraintTier::standard_round_brilliant();
    }
    // A Deep Solve/Optimize verdict computed against the design just replaced no
    // longer describes anything on screen -- see `clear_analysis_results`'s own
    // doc comment.
    clear_analysis_results(ui);
    refresh_all(ui, render_ctx, preview_state, solid_last_solved, &st);
    drop(st);
    // See `apply_loaded_design`'s matching reset -- "New" replaces the
    // whole `EditorState` exactly the same way.
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    bump_form_reset_pulse(ui);
    ui.global::<EditorModel>().set_new_dialog_open(false);
    // Explicit, same reasoning as `native_io::commit_loaded_native`'s own matching
    // line: a freshly `fresh_from_spec` design starts clean by construction.
    ui.global::<EditorModel>().set_is_dirty(false);
    // A brand-new design has no file behind it yet, so the window title drops the
    // name entirely rather than keeping whatever was open before.
    ui.set_loaded_design_name(SharedString::new());
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
    /// The catalogue row this design was loaded from -- `Some(full.entry_id)` for a
    /// LOCAL load, `None` for a remote one (see `EditorState::source_entry_id`'s own
    /// doc comment for why a remote entry id must never land here). Threaded through
    /// unconditionally rather than defaulted to `None` here, because another lane's
    /// concurrent edit grew `EditorState` with this field after this struct was
    /// written -- see this task's final report for the two `native_io.rs` construction
    /// sites still needing the equivalent fix (that file is not owned by this lane).
    source_entry_id: Option<i64>,
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
        source_entry_id,
    } = outcome;
    let schedule_ri = loaded.design.meta.refractive_index;
    let mut st = state.borrow_mut();
    // `replace_wholesale`, not a plain `*st = ...` -- see `do_new_design_create`'s
    // matching comment and `EditorState::replace_wholesale`'s own doc comment: this
    // carries the OLD `generation` `Arc` (and bumps it) across the replacement so a
    // background Deep Solve/Optimize/auto-solve dispatched against the design being
    // replaced still observes the change instead of comparing against a counter
    // nobody increments anymore.
    st.replace_wholesale(EditorState {
        // Read before `loaded.design` moves below -- a bare bool/i64 field read
        // never conflicts with a later partial move of a DIFFERENT field of the
        // same `loaded`/`outcome`, but keeping the "used for something else"
        // fields together here (rather than scattered) is easier to audit.
        used_placeholder: loaded.used_placeholder,
        source_entry_id,
        design: loaded.design,
        // CAD audit item 165 -- see `EditorState::fresh`'s matching comment.
        history: History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
        printed_proportions,
        generation: Arc::new(AtomicU64::new(0)),
        design_epoch: Arc::new(AtomicU64::new(0)),
        saved_generation: 0,
        pending_unsaved_action: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        asc_filename: loaded.asc_filename,
        original_asc_text: loaded.original_asc_text,
        pending_gear_remap: None,
        pending_retarget: None,
        multi_selected: BTreeSet::new(),
        // A freshly replaced `EditorState` has never pushed anything yet -- matches
        // `EditorState::fresh_from_spec`'s own construction (`state/mod.rs`), added
        // here because another lane's concurrent edit grew this struct with this
        // field after this literal was written; see this task's final report for the
        // two matching `native_io.rs` construction sites still needing the same fix
        // (that file is not owned by this lane).
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
    });
    // A Deep Solve/Optimize verdict computed against the design just replaced no
    // longer describes anything on screen -- see `clear_analysis_results`'s own
    // doc comment.
    clear_analysis_results(ui);
    refresh_all(ui, render_ctx, preview_state, solid_last_solved, &st);
    // The whole `EditorState` above was just replaced -- any previously selected
    // tier index now names (at best) an unrelated row in the NEWLY loaded design, so
    // both halves of the selection reset unconditionally: the property (for the
    // Solid/Diagram overlay tint) and the pulse (so `EditorView`'s form-reset watcher
    // fires even when the selection happened to already be `-1`; see
    // `EditorModel::form_reset_pulse`'s own doc comment).
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    bump_form_reset_pulse(ui);
    // Explicit, same reasoning as `native_io::commit_loaded_native`'s own matching
    // line: a freshly replaced `EditorState` is clean by construction.
    ui.global::<EditorModel>().set_is_dirty(false);
    // The window title names whichever design is open -- `native_io` sets this on
    // every Save/Open Native, and this is the matching Load Selected path.
    ui.set_loaded_design_name(st.asc_filename.clone().unwrap_or_default().into());
    if loaded.used_placeholder {
        show_toast(
            ui,
            "Loaded a reconstructed schedule -- mast distances are \
             placeholders (no attached .asc file was found); adjust \
             masts before exporting.",
            // Item 179: every mast on this design is a fabricated 0.0. A cutter who
            // misses that can export a file that looks like a real cut
            // instruction and is not, so this must not auto-dismiss the way an
            // "info" toast does.
            "warning",
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
///
/// Checks `entry_id` first (a plain, no-dialog-needed validation -- nothing
/// destructive would happen anyway), THEN [`EditorState::is_dirty`]: a dirty design
/// stashes [`PendingUnsavedAction::LoadSelected`] and opens the Save/Discard/Cancel
/// guard instead of replacing anything, resumed by [`setup_unsaved_guard_dispatch`]
/// (also wired up here -- see that function's own doc comment for why this is its
/// one call site) once "Save"/"Discard" is chosen. Everything past that point is
/// [`do_load_selected`], run either directly or as that resume.
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
    setup_unsaved_guard_dispatch(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
        db,
        source,
    );
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

        if state.borrow().is_dirty() {
            state.borrow_mut().pending_unsaved_action = Some(PendingUnsavedAction::LoadSelected);
            ui.global::<EditorModel>().set_unsaved_dialog_message(
                "Loading another design will discard the current one's unsaved changes.".into(),
            );
            ui.global::<EditorModel>().set_unsaved_dialog_open(true);
            return;
        }
        do_load_selected(
            &ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            &db,
            &source,
        );
    });
}

/// The actual "Load Selected" work -- see [`setup_load_selected_callback`]'s own doc
/// comment for why the dirty check runs before this is ever called, not inside it.
/// `entry_id` is re-read from [`LibraryModel::get_selected_entry_id`] here rather than
/// threaded through as a parameter: the guard dialog blocks every other interaction
/// while it is up, so the selection cannot have changed by the time this runs, and
/// re-reading it is one line versus a parameter every other caller would also need.
fn do_load_selected(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
    if entry_id < 0 {
        show_toast(ui, "No diagram selected to load.", "error");
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
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        fetch_remote_design_source(
            ui.as_weak(),
            worker,
            i64::from(entry_id),
            move |ui, result| match result {
                Ok(remote) => {
                    match loading::design_from_asc_text(&remote.file_name, &remote.asc_text, None) {
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
                                        // A remote entry id and a local row id occupy
                                        // independent id spaces (`EditorState::
                                        // source_entry_id`'s own doc comment) -- never
                                        // stored here.
                                        source_entry_id: None,
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
                    }
                }
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
        show_toast(ui, "Diagram detail not found.", "error");
        return;
    };
    match loading::design_from_full_record(&full) {
        Ok(loaded) => {
            let printed_proportions = loading::external_proportions_from_full_record(&full);
            apply_loaded_design(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                LoadedDesignOutcome {
                    loaded,
                    printed_proportions,
                    label: &full.title,
                    source_entry_id: Some(full.entry_id),
                },
            );
        }
        Err(e) => show_toast(ui, &e, "error"),
    }
}

/// Runs whichever [`PendingUnsavedAction`] `state` is currently holding (taking it,
/// so a second call finds nothing left to resume), or does nothing if there isn't
/// one. Shared by [`setup_unsaved_guard_dispatch`]'s "Save" (once the save actually
/// left the design clean) and "Discard" handlers -- the only two ways to reach past
/// the Save/Discard/Cancel guard.
fn resume_pending_unsaved_action(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    // Bound to a local first, not matched on directly: `match state.borrow_mut()...`
    // would extend that `RefMut` temporary across every arm's own body (Rust's usual
    // scrutinee-temporary-lifetime rule for `match`), and every arm here calls back
    // into a `do_*` function that itself starts with its own `state.borrow_mut()` --
    // a direct match would panic with "already mutably borrowed" the moment any arm
    // ran.
    let pending = state.borrow_mut().pending_unsaved_action.take();
    match pending {
        Some(PendingUnsavedAction::New {
            spec,
            template_index,
        }) => {
            do_new_design_create(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                spec,
                template_index,
            );
        }
        Some(PendingUnsavedAction::LoadSelected) => {
            do_load_selected(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                db,
                source,
            );
        }
        Some(PendingUnsavedAction::OpenNative) => {
            do_open_native(ui, state, render_ctx, preview_state, solid_last_solved);
        }
        None => {}
    }
}

/// The Save/Discard/Cancel unsaved-changes guard's three resolution callbacks --
/// shared by New/Load Selected/Open Native (see [`PendingUnsavedAction`]), since only
/// one of them can ever be pending at a time and Slint only keeps the LAST handler
/// registered for a given callback. Registered once, from
/// [`setup_load_selected_callback`] -- the one owned function with every one of
/// `db`/`source` (needed to resume Load Selected) alongside the render/preview
/// plumbing New and Open Native also need, so it is the natural single home for this
/// rather than splitting it across the files that own each individual action.
///
/// "Save" invokes `EditorModel.save_native` (whatever handler is registered for it --
/// `native_io::setup_save_native_callback`, wired up independently of this function)
/// and only resumes the pending action once that save actually left the design clean;
/// a cancelled or failed save (already toasted by `save_native` itself) aborts the
/// pending action instead of discarding anyway.
fn setup_unsaved_guard_dispatch(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let state_save = Rc::clone(state);
    let render_ctx_save = Arc::clone(render_ctx);
    let preview_state_save = Arc::clone(preview_state);
    let solid_last_solved_save = Arc::clone(solid_last_solved);
    let db_save = Arc::clone(db);
    let source_save = Arc::clone(source);
    let ui_weak_save = ui.as_weak();
    ui.global::<EditorModel>().on_unsaved_dialog_save(move || {
        let Some(ui) = ui_weak_save.upgrade() else {
            return;
        };
        ui.global::<EditorModel>().set_unsaved_dialog_open(false);
        ui.global::<EditorModel>().invoke_save_native();
        if state_save.borrow().is_dirty() {
            state_save.borrow_mut().pending_unsaved_action = None;
            return;
        }
        resume_pending_unsaved_action(
            &ui,
            &state_save,
            &render_ctx_save,
            &preview_state_save,
            &solid_last_solved_save,
            &db_save,
            &source_save,
        );
    });

    let state_discard = Rc::clone(state);
    let render_ctx_discard = Arc::clone(render_ctx);
    let preview_state_discard = Arc::clone(preview_state);
    let solid_last_solved_discard = Arc::clone(solid_last_solved);
    let db_discard = Arc::clone(db);
    let source_discard = Arc::clone(source);
    let ui_weak_discard = ui.as_weak();
    ui.global::<EditorModel>()
        .on_unsaved_dialog_discard(move || {
            let Some(ui) = ui_weak_discard.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_unsaved_dialog_open(false);
            resume_pending_unsaved_action(
                &ui,
                &state_discard,
                &render_ctx_discard,
                &preview_state_discard,
                &solid_last_solved_discard,
                &db_discard,
                &source_discard,
            );
        });

    let state_cancel = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_unsaved_dialog_cancel(move || {
            state_cancel.borrow_mut().pending_unsaved_action = None;
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
///
/// CAD audit item 164: this used to be a literal no-op (`move || {}`), so "No
/// Thanks" visibly did nothing and the suggestion text sat in the Log
/// unchanged -- mirrors the Accept path's own clearing above.
pub(in crate::gui::editor) fn setup_material_suggestion_dismiss_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_material_suggestion_dismiss(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<EditorModel>()
                .set_material_suggestion_name("".into());
            ui.global::<EditorModel>()
                .set_material_suggestion_text("".into());
        });
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
                clamp_selection_to_tier_count(&ui, st.design.tiers.len());
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
                clamp_selection_to_tier_count(&ui, st.design.tiers.len());
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
            match loading::parse_preform_form(
                shape_index,
                &half_width,
                &length_over_width,
                &depth,
                FIXED_CYLINDER_PREFORM_SIDES,
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

/// "Apply Yield Inputs": parses the form and, on success, applies both halves
/// through `EditorState::apply` as ONE [`Edit::Batch`] of [`Edit::
/// SetGirdleDiameterMm`] then [`Edit::SetMaterial`] (CAD audit item 79) --
/// previously two separate, independently-undoable edits, which meant a single
/// Apply Yield Inputs click cost two Ctrl+Z presses to undo and could be
/// undone out of order.
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
                &st.design.material,
            ) {
                Ok((girdle_diameter_mm, material)) => {
                    let _ = st.apply(Edit::Batch(vec![
                        Edit::SetGirdleDiameterMm { girdle_diameter_mm },
                        Edit::SetMaterial { material },
                    ]));
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
/// CAD audit items 129/132/232: every OTHER tier's own name token (`ConstraintTier::
/// names()`, i.e. already split on `/`), with the tier at `excluded_index` left out
/// -- feeds `loading::TierFormFields::other_tier_names` so `parse_tier_form` can
/// reject a name collision (see that field's own doc comment for why an
/// undetected one is worse than merely confusing). `excluded_index < 0` (a
/// brand-new tier being added) excludes nothing, since there is no existing row
/// to exempt from its own check.
fn other_tier_names_excluding(st: &EditorState, excluded_index: i32) -> Vec<String> {
    st.design
        .tiers
        .iter()
        .enumerate()
        .filter(|&(i, _)| excluded_index < 0 || i != excluded_index as usize)
        .flat_map(|(_, tier)| tier.names().into_iter().map(str::to_string))
        .collect()
}

/// [`setup_save_tier_callback`]'s successful-`AddTier` label, looked up while
/// `st` is still borrowed (before the `drop(st)` [`select_and_announce_added_
/// tier`] needs so it can re-borrow `EditorModel` freely).
fn added_tier_label(st: &EditorState, dirty_index: usize) -> Option<String> {
    st.design
        .tiers
        .get(dirty_index)
        .map(|tier| tier_nudge_label(tier, dirty_index))
}

/// CAD audit item 131: selects and reveals the tier `setup_save_tier_callback`'s
/// `AddTier` branch just added, then names it in a toast (CAD audit item 127).
/// Setting `EditorModel.selected_tier_index` alone is enough to reveal the row
/// too: `editor_view.slint`'s `changed tracked_selected_tier_index` re-seeds the
/// inspector form AND calls `tier_table.focus_row`, which is what actually
/// scrolls the new row into view (see that property's own doc comment) -- so the
/// reveal itself is the EXISTING path, not something new here.
fn select_and_announce_added_tier(ui: &MainWindow, dirty_index: usize, label: Option<String>) {
    ui.global::<EditorModel>()
        .set_selected_tier_index(dirty_index as i32);
    if let Some(label) = label {
        show_toast(ui, &format!("Added {label}"), "info");
    }
}

/// [`setup_save_tier_callback`]'s edit + dirty-index pair: a new tier
/// (`index < 0`) inserts right after the currently selected row instead of
/// always appending -- an out-of-range (or no) selection falls back to the
/// previous append-at-end behavior -- while an existing tier (`index >= 0`)
/// simply modifies itself in place.
fn tier_save_edit(
    ui: &MainWindow,
    st: &EditorState,
    index: i32,
    tier: indicatrix_cut_core::ConstraintTier,
) -> (usize, Edit) {
    if index < 0 {
        let append_index = st.design.tiers.len();
        let insert_after_selected =
            usize::try_from(ui.global::<EditorModel>().get_selected_tier_index())
                .ok()
                .filter(|&i| i < append_index)
                .map_or(append_index, |i| i + 1);
        (
            insert_after_selected,
            Edit::AddTier {
                index: insert_after_selected,
                tier,
            },
        )
    } else {
        let index = index as usize;
        (index, Edit::ModifyTier { index, tier })
    }
}

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
            // An existing row keeps its own `imported_meet` and the `.asc` file's
            // original `G` note across this save -- both looked up here, before the
            // mutable borrow below, so editing an imported tier's name/angle/indices
            // never silently drops what the file claimed it meets, nor the note a
            // cutter reads while cutting it.
            let (imported_meet, original_notes) = (index >= 0)
                .then(|| {
                    let st = state.borrow();
                    let tier = st.design.tiers.get(usize::try_from(index).ok()?)?;
                    Some((tier.imported_meet.clone(), tier.original_notes.clone()))
                })
                .flatten()
                .unwrap_or_default();
            let (gear_teeth_abs, other_tier_names) = {
                let st = state.borrow();
                (
                    st.design.meta.gear_teeth_abs(),
                    other_tier_names_excluding(&st, index),
                )
            };
            match loading::parse_tier_form(loading::TierFormFields {
                angle: &angle,
                constraint_kind,
                constraint_text: &constraint_text,
                name: &name,
                indices: &indices,
                gear_teeth_abs,
                imported_meet,
                original_notes,
                other_tier_names: other_tier_names.clone(),
            }) {
                Ok(mut tier) => {
                    // CAD audit item 129: a brand-new tier saved with a blank Name
                    // field would otherwise stay unnamed and un-meetable (
                    // `ConstraintTier::names()` returns nothing for an empty name) --
                    // auto-name it here, matching what Duplicate already does for
                    // its own copies. Only for a fresh `AddTier` (`index < 0`): an
                    // existing tier's name was either already set or the cutter just
                    // deliberately blanked it, neither of which this should override.
                    if index < 0 && tier.name.is_empty() {
                        tier.name = next_free_block_name(tier.angle_deg, &other_tier_names);
                    }
                    // CAD audit item 34: captured before `tier` is moved into
                    // `tier_save_edit` below -- see `non_integral_index_warning`'s
                    // own doc comment for why this warns rather than rejects.
                    let non_integral_warning = non_integral_index_warning(&tier.indices);
                    let mut st = state.borrow_mut();
                    // Preserve the row's own `detached` set across a save --
                    // `parse_tier_form` always returns an empty one (`loading.rs` is
                    // not this lane's file to edit), and without this a rename/
                    // angle/index edit would silently re-link a deliberately
                    // detached tier back into its orbit.
                    if let Some(current) = usize::try_from(index)
                        .ok()
                        .and_then(|i| st.design.tiers.get(i))
                    {
                        tier.detached.clone_from(&current.detached);
                    }
                    // A `MeetNamed` token that resolves to nothing today would
                    // otherwise degrade silently inside the solver (`meet_solver`'s
                    // own doc comment: "an unresolved token is dropped") -- caught
                    // here, before it is ever applied, with the offending name named.
                    if let MeetConstraint::MeetNamed(names) = &tier.constraint
                        && let Some(bad_name) = first_unresolved_meet_name(&st.design, names)
                    {
                        report_tier_form_error(
                            &ui,
                            &format!("No facet named '{bad_name}' -- check the Meets field."),
                        );
                        return;
                    }
                    let (dirty_index, edit) = tier_save_edit(&ui, &st, index, tier);
                    match st.apply(edit) {
                        Ok(()) => {
                            // Clears whatever the LAST save's parse/validation
                            // error left behind (CAD audit item 47): a successful
                            // save means the form is valid again.
                            ui.global::<EditorModel>().set_tier_form_error("".into());
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
                            // CAD audit item 131: select and reveal the row just
                            // added -- an `AddTier`-only branch, since a
                            // `ModifyTier` save is already on the row it edited.
                            // See [`select_and_announce_added_tier`]'s own doc
                            // comment for why setting `selected_tier_index` alone
                            // is enough to reveal it too.
                            if index < 0 {
                                let label = added_tier_label(&st, dirty_index);
                                drop(st);
                                select_and_announce_added_tier(&ui, dirty_index, label);
                            }
                            // CAD audit item 34: shown last (after the "Added
                            // <label>" toast above, when this was a new tier) so
                            // it is the one left on screen -- the single toast
                            // slot keeps only the most recent call, and a
                            // possibly-unintentional fractional index is more
                            // worth a cutter's attention than a bare
                            // confirmation that the save succeeded.
                            if let Some(warning) = non_integral_warning {
                                show_toast(&ui, &warning, "info");
                            }
                        }
                        Err(e) => report_tier_form_error(&ui, &e.to_string()),
                    }
                }
                Err(e) => report_tier_form_error(&ui, &e),
            }
        },
    );
}

/// The tier-list row's own "x" button (and the tier list's Delete/Backspace, both of
/// which call straight through `EditorModel.remove_tier`): applies
/// [`Edit::RemoveTier`] through `EditorState::apply`, then
/// [`adjust_selection_after_remove`] to keep `EditorModel.selected_tier_index`
/// pointing at the right row (or nothing) once the removal has shifted everything
/// after it down by one.
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
            // Captured before the removal for the confirmation toast below --
            // `None` (an already out-of-range index) just skips that toast, the
            // same as today's silent behavior.
            let removed_summary = st.design.tiers.get(index as usize).map(|tier| {
                let name = if tier.name.is_empty() {
                    "(unnamed)".to_string()
                } else {
                    tier.name.clone()
                };
                (name, tier.indices.len())
            });
            match st.apply(Edit::RemoveTier {
                index: index as usize,
            }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    adjust_selection_after_remove(&ui, index);
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
                    drop(st);
                    if let Some((name, facet_count)) = removed_summary {
                        let plural = if facet_count == 1 { "" } else { "s" };
                        show_toast(
                            &ui,
                            &format!("Removed {name} ({facet_count} facet{plural}), Undo"),
                            "info",
                        );
                    }
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
                        // CAD audit item 165: committing back the SAME value is a
                        // real interaction boundary (the cutter opened the cell,
                        // looked, and closed it) -- end any scroll-wheel nudge
                        // coalescing run in progress rather than leaving it open
                        // for a later, unrelated nudge to merge into.
                        st.history.end_coalesce_run();
                        // CAD audit item 127: a committed-but-unchanged edit used
                        // to be silent and indistinguishable from a dropped one.
                        show_toast(&ui, "No change.", "info");
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

/// Clamps a nudged angle to the ORIGINAL tier's crown/pavilion side instead of
/// letting it cross zero -- `meet_solver::blocks::tier_sides`'s side rule
/// (negative is pavilion, non-negative crown, `-0.0` forces pavilion) means a
/// nudge that crosses zero silently reclassifies the tier into the other block
/// with no confirmation and no visible change other than the sign. `-0.0`/`0.0`
/// are used as the two boundary values so the clamped result still carries the
/// correct side under that same unsigned-zero rule, rather than merely being
/// "close to zero" with an arbitrary sign.
const fn clamp_nudge_to_side(current: f64, nudged: f64) -> f64 {
    if current.is_sign_negative() == nudged.is_sign_negative() {
        return nudged;
    }
    if current.is_sign_negative() {
        -0.0
    } else {
        0.0
    }
}

/// A tier's short label for [`clamp_nudge_to_side`]'s explanatory toast --
/// `"tier 5 (Girdle)"` when named, else `"tier 5"` (1-based, matching the tier
/// table's own `#` column).
fn tier_nudge_label(tier: &indicatrix_cut_core::ConstraintTier, index: usize) -> String {
    if tier.name.is_empty() {
        format!("tier {}", index + 1)
    } else {
        format!("tier {} ({})", index + 1, tier.name)
    }
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
            let mut clamped_labels: Vec<String> = Vec::new();
            let changes: Option<Vec<(usize, f64, f64)>> = targets
                .iter()
                .map(|&index| {
                    st.design.tiers.get(index).map(|tier| {
                        let wanted = tier.angle_deg + delta_deg;
                        let nudged = clamp_nudge_to_side(tier.angle_deg, wanted);
                        if nudged != wanted {
                            clamped_labels.push(tier_nudge_label(tier, index));
                        }
                        (index, tier.angle_deg, nudged)
                    })
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
                    // The angle's sign is the only thing that says which block a
                    // tier belongs to (`clamp_nudge_to_side`'s own doc comment), so
                    // a nudge that would cross zero is clamped there instead of
                    // silently reclassifying the tier -- explain the stop instead
                    // of leaving it looking like the nudge simply refused to move
                    // (CAD audit item 46's remainder).
                    if !clamped_labels.is_empty() {
                        show_toast(
                            &ui,
                            &format!(
                                "{} stopped at 0° -- nudging further would move it into the \
                                 other block. Type the angle directly (e.g. \"-0\") to cross \
                                 blocks on purpose.",
                                clamped_labels.join(", ")
                            ),
                            "info",
                        );
                    }
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// A row's "Duplicate" button and the tier list's Ctrl+D: inserts a copy of the
/// named tier (name suffixed `'`, same indices/angle/constraint/detached set)
/// immediately AFTER the source row as a new [`Edit::AddTier`] through
/// `EditorState::apply` -- not appended at the end, since cut order is meaningful
/// (`Edit::AddTier` already supports an arbitrary insertion index; only the call
/// site used to force the end) -- then moves the tier-list selection to the copy.
/// The copy's `imported_meet` is always cleared -- it is a new, user-authored row,
/// not itself something a real `.asc` file's `G` field ever made a claim about,
/// even though the tier it was copied FROM might carry one.
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
            let source_label = if source.name.is_empty() {
                "(unnamed)".to_string()
            } else {
                source.name.clone()
            };
            // CAD audit items 129/132/232: a real generator ("P1 (2)", "P1 (3)",
            // ...) instead of appending an apostrophe -- see
            // `unique_duplicate_name`'s own doc comment for why the old scheme
            // piled up unreadable "P1''''" names AND silently created a duplicate
            // name every meet resolver secretly binds to the FIRST tier holding
            // it.
            let existing_names: Vec<String> =
                st.design.tiers.iter().map(|t| t.name.clone()).collect();
            duplicate.name = unique_duplicate_name(&source.name, &existing_names);
            let duplicate_label = duplicate.name.clone();
            duplicate.imported_meet = None;
            let new_index = index + 1;
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
                    // CAD audit item 127: names the change instead of leaving a
                    // mis-clicked Duplicate indistinguishable from a no-op.
                    show_toast(
                        &ui,
                        &format!("Duplicated {source_label} as {duplicate_label}"),
                        "info",
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Generates a name for [`setup_duplicate_tier_callback`] that is guaranteed not
/// to collide with any name in `existing_names` (CAD audit items 129/132/232) --
/// replaces the old "append an apostrophe" scheme, which produced an unreadable
/// "P1''''" pile on a second or third duplicate of the same tier and, worse,
/// silently created a duplicate name that `MeetNameResolver::name_match`
/// (`indicatrix::geometry::meet_solver::names`) resolves by binding to whichever
/// tier holds it FIRST -- so a duplicate's stale copy of a popular name could
/// silently steal every future `MeetNamed` reference meant for the original.
///
/// [`split_duplicate_suffix`] first removes a trailing `" (N)"` a PREVIOUS call to
/// this same function already appended, so duplicating "P1 (2)" produces
/// "P1 (3)" rather than nesting into "P1 (2) (2)". An empty source name (an
/// unnamed tier) falls back to the base "Tier" rather than producing a bare
/// "(2)" -- giving the duplicate a real name is also what lets it become a
/// `MeetNamed` target, which `ConstraintTier::names` never allows for an empty
/// name (see CAD audit item 129).
fn unique_duplicate_name(source_name: &str, existing_names: &[String]) -> String {
    let (base, source_number) = split_duplicate_suffix(source_name.trim());
    let base = if base.is_empty() { "Tier" } else { base };
    // One past the source's OWN number when it already carries one, so this
    // holds to its documented contract on its own terms rather than relying on
    // the caller's list happening to contain the source tier: duplicating
    // "P1 (2)" gives "P1 (3)" even against an empty list. `checked_add` falls
    // back to 2 for the (unreachable by duplicating, but typeable by hand)
    // number that cannot be counted past.
    let mut n: u32 = source_number
        .and_then(|number| number.checked_add(1))
        .unwrap_or(2);
    loop {
        let candidate = format!("{base} ({n})");
        if !existing_names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        n += 1;
    }
}

/// [`setup_save_tier_callback`]'s auto-name for a brand-new tier saved with a
/// blank Name field (CAD audit item 129, the remainder [`unique_duplicate_name`]
/// above does not cover -- that one only ever runs against an already-named
/// source). An empty name can never become a `MeetNamed` target
/// (`ConstraintTier::names()` returns nothing for it), so leaving a fresh
/// `AddTier` unnamed silently makes it un-meetable until the cutter notices.
///
/// The block letter (`C`rown/`P`avilion) follows the same sign the tier's own
/// angle will be classified by (`meet_solver::blocks`), simplified: this only
/// has to pick a reasonable DEFAULT name the cutter can always retype, so it
/// does not reproduce that module's unsigned-zero "inherits the previous
/// tier's side" rule just to name a single new tier. Case-insensitive
/// collision against `existing_names` counts up past it, matching
/// `unique_duplicate_name`'s own convention.
fn next_free_block_name(angle_deg: f64, existing_names: &[String]) -> String {
    let letter = if angle_deg.is_sign_negative() {
        'P'
    } else {
        'C'
    };
    let mut n: u32 = 1;
    loop {
        let candidate = format!("{letter}{n}");
        if !existing_names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        n += 1;
    }
}

/// Splits `name` into its base portion and the number of a trailing `" (N)"` (a
/// whole, non-negative number in parentheses, preceded by exactly one space) --
/// see [`unique_duplicate_name`]'s own doc comment for why. A name with no such
/// suffix comes back whole with `None`, and so does one whose digits do not fit
/// a `u32`, for the same reason `"P1 (a)"` does: a suffix this cannot read is
/// part of the name the cutter typed, not a counter to continue.
fn split_duplicate_suffix(name: &str) -> (&str, Option<u32>) {
    let Some((base, rest)) = name.rsplit_once(" (") else {
        return (name, None);
    };
    let Some(digits) = rest.strip_suffix(')') else {
        return (name, None);
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return (name, None);
    }
    digits
        .parse()
        .map_or((name, None), |number| (base, Some(number)))
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
    // Piggybacked here (before `state` below is shadowed by its own clone) for
    // the reason `setup_toggle_detach_callback`'s own doc comment gives:
    // `gui::editor::mod::setup_editor_callbacks` has one fixed call site per
    // `setup_*` function name, so a new callback is wired up from an EXISTING
    // call site instead.
    setup_select_tier_range_callback(ui, state);

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
            let mut rows: Vec<EditorTierItem> =
                ui.global::<EditorModel>().get_tiers().iter().collect();
            apply_multi_selection(&mut rows, &multi_selected);
            push_tiers(&ui, rows);
            push_multi_selected_count(&ui, multi_selected.len());
            // Resolves every multi-selected TIER to its member FACET ids so the
            // group is visible in 3D too, not only as a one-pixel row border --
            // `FacetOverlay::multi_selected` is a flat facet id list, built against
            // the last rendered frame's masts the same way the hover/click
            // callbacks already build one.
            if let Some(preview_state) = auto_solve::preview_state() {
                let solved = auto_solve::solid_last_solved()
                    .and_then(|cache| {
                        cache
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    })
                    .unwrap_or_default();
                let facet_map = FacetMap::from_design(&st.design, &solved);
                let facet_ids: Vec<u32> = multi_selected
                    .iter()
                    .flat_map(|&tier_index| facet_map.facets_of_tier(tier_index).iter().copied())
                    .collect();
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.multi_selected = facet_ids;
                });
            }
        });
}

/// The tier list's Shift+click: CAD audit item 39's remainder -- replaces
/// [`EditorState::multi_selected`] wholesale with every tier index between the
/// current selection anchor (`EditorModel.selected_tier_index`) and `index`,
/// inclusive of both ends. Unlike [`setup_toggle_multi_select_callback`]'s
/// Ctrl+click, which only ever flips ONE row in or out, a plain Shift+click
/// always REPLACES the whole set -- the spreadsheet/GCS convention
/// `editor_tier_table.slint`'s row click handler now follows for Shift, checked
/// before that same handler's existing Ctrl+click branch.
///
/// No anchor yet (`selected_tier_index < 0` -- a brand-new design, before any row
/// has ever been selected) falls back to a single-row selection of `index`: there
/// is nothing sensible to range from.
///
/// Reuses the exact row-patch/count-push/facet-overlay-resubmit tail
/// [`setup_toggle_multi_select_callback`] already has, and is piggybacked onto
/// that callback's own registration for the reason [`setup_toggle_detach_callback`]'s
/// own doc comment gives: `gui::editor::mod::setup_editor_callbacks` (not this
/// lane's file to edit) has one fixed call site per `setup_*` function name.
pub(in crate::gui::editor) fn setup_select_tier_range_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_select_tier_range(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let anchor = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index())
                .unwrap_or(index);
            let (lo, hi) = if anchor <= index {
                (anchor, index)
            } else {
                (index, anchor)
            };
            let mut st = state.borrow_mut();
            st.multi_selected = (lo..=hi).collect();
            let multi_selected = st.multi_selected.clone();
            let mut rows: Vec<EditorTierItem> =
                ui.global::<EditorModel>().get_tiers().iter().collect();
            apply_multi_selection(&mut rows, &multi_selected);
            push_tiers(&ui, rows);
            push_multi_selected_count(&ui, multi_selected.len());
            // Same facet-overlay resubmit `setup_toggle_multi_select_callback` uses
            // -- resolves every multi-selected TIER to its member FACET ids so the
            // range is visible in 3D too, not only as a row border.
            if let Some(preview_state) = auto_solve::preview_state() {
                let solved = auto_solve::solid_last_solved()
                    .and_then(|cache| {
                        cache
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    })
                    .unwrap_or_default();
                let facet_map = FacetMap::from_design(&st.design, &solved);
                let facet_ids: Vec<u32> = multi_selected
                    .iter()
                    .flat_map(|&tier_index| facet_map.facets_of_tier(tier_index).iter().copied())
                    .collect();
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.multi_selected = facet_ids;
                });
            }
        });
}

/// The tier-list row's "Detach"/"Reattach" toggle: applies
/// `Design::detach_all_in_tier`/`Design::reattach_all_in_tier` through
/// `EditorState::apply`, flipping [`EditorTierItem::is_detached`](crate::EditorTierItem)
/// so one button serves both directions. An explicit, visible escape hatch: a
/// symmetric tier's occurrences stay linked (an edit moves the whole orbit) until the
/// user clicks this, and detaching never happens as a side effect of any other action.
///
/// Also the wiring point for [`setup_move_tier_callback`]/[`setup_complete_orbit_
/// callback`]/[`setup_clear_multi_select_callback`]/[`setup_remove_multi_selected_
/// callback`]/[`setup_facet_remove_callback`]/[`setup_facet_toggle_detach_callback`]/
/// [`setup_facet_add_callback`]/[`setup_tier_rotate_indices_callback`]/
/// [`setup_tier_mirror_indices_callback`] -- `gui::editor::mod::setup_editor_callbacks`
/// (not this lane's file to edit) has one fixed call site per `setup_*` function
/// name, so a genuinely new callback can only be wired up by piggybacking its own
/// `setup_*` call onto an EXISTING call site that already receives every argument it
/// needs; this is the one existing call already carrying `render_ctx`/
/// `preview_state`/`solid_last_solved` alongside `ui`/`state`.
pub(in crate::gui::editor) fn setup_toggle_detach_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // CAD audit item 117: stashes the shared `RenderContext` handle for
    // `auto_solve::render_ctx()` -- this is one of several `setup_*_callback`s
    // already given the `Arc` directly by `gui::editor::mod::setup_editor_
    // callbacks` (not this lane's file to add a NEW parameter to), and it runs
    // once here, synchronously, before `setup_editor_callbacks` returns and the
    // event loop starts -- so by the time a user can hover or click anything,
    // `setup_solid_facet_hover_callback`/`setup_solid_facet_click_callback`
    // (whose own fixed call site never receives `render_ctx` at all) can already
    // read it back to map a click through the letterboxed pick rectangle.
    auto_solve::stash_render_ctx(render_ctx);

    let state_toggle = Rc::clone(state);
    let render_ctx_toggle = Arc::clone(render_ctx);
    let preview_state_toggle = Arc::clone(preview_state);
    let solid_last_solved_toggle = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_toggle_detach(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if index < 0 {
                return;
            }
            let mut st = state_toggle.borrow_mut();
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
                    refresh_editor_panel_stale(&ui, &render_ctx_toggle, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx_toggle,
                        &preview_state_toggle,
                        &solid_last_solved_toggle,
                        &st,
                        BTreeSet::from([index]),
                        false,
                    );
                    // CAD audit item 127: names the change and which way it went
                    // -- `tier.detached` is now whatever this apply just left it
                    // as, so a non-empty set here means "just detached," empty
                    // means "just reattached."
                    let now_detached = st
                        .design
                        .tiers
                        .get(index)
                        .is_some_and(|tier| !tier.detached.is_empty());
                    let label = st
                        .design
                        .tiers
                        .get(index)
                        .map(|tier| tier_nudge_label(tier, index));
                    drop(st);
                    if let Some(label) = label {
                        let verb = if now_detached {
                            "Detached"
                        } else {
                            "Reattached"
                        };
                        show_toast(&ui, &format!("{verb} {label}"), "info");
                    }
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });

    setup_move_tier_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_complete_orbit_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_clear_multi_select_callback(ui, state);
    setup_remove_multi_selected_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    // #45: per-facet index editing -- piggybacked here for the same reason the four
    // calls above are (see this function's own doc comment's "wiring point" section):
    // this is the one existing `editor::setup_editor_callbacks` call site that already
    // receives `render_ctx`/`preview_state`/`solid_last_solved` alongside `ui`/`state`.
    setup_facet_remove_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_facet_toggle_detach_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_facet_add_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_tier_rotate_indices_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_tier_mirror_indices_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    // CAD audit items 128/130/121 -- piggybacked here for the identical
    // "wiring point" reason given above.
    setup_adopt_all_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_adopt_selected_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_pin_to_mast_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_highlight_tooth_callback(ui, state);
}

/// CAD audit item 128: bulk-adopts every tier that still has an unadopted
/// `imported_meet`, as one undoable step -- built as a single [`Edit::Batch`] of
/// per-tier [`Edit::SetConstraint`]s (mirroring `solve_actions::
/// setup_adopt_meet_callback`'s own one-tier "Adopt") rather than looping single
/// edits, so Undo reverses the whole thing in one step and only ONE `refresh_all`
/// (one real re-solve) runs afterward instead of one per tier. A silent no-op
/// when nothing has an `imported_meet` left to adopt.
///
/// Calls [`refresh_all`], not `refresh_editor_panel_stale`, for the exact reason
/// `setup_adopt_meet_callback`'s own doc comment gives: every value this adopts
/// is already what the design's last real solve produced, so re-solving here
/// pays the same whole-schedule cost that solve already paid, not a new one.
///
/// HANDOFF: needs `callback adopt_all();` declared on `EditorModel` in
/// `ui/models/editor.slint` (not this lane's file) -- `editor_tier_table.slint`'s
/// own "Adopt all" button (added alongside this) already calls it.
pub(in crate::gui::editor) fn setup_adopt_all_callback(
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
    ui.global::<EditorModel>().on_adopt_all(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let edits: Vec<Edit> = st
            .design
            .tiers
            .iter()
            .enumerate()
            .filter_map(|(index, tier)| {
                tier.imported_meet
                    .clone()
                    .map(|constraint| Edit::SetConstraint { index, constraint })
            })
            .collect();
        if edits.is_empty() {
            return;
        }
        let count = edits.len();
        match st.apply(Edit::Batch(edits)) {
            Ok(()) => {
                refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
                drop(st);
                let plural = if count == 1 { "" } else { "s" };
                show_toast(
                    &ui,
                    &format!("Adopted {count} imported meet{plural}"),
                    "info",
                );
            }
            Err(e) => show_toast(&ui, &e.to_string(), "error"),
        }
    });
}

/// CAD audit item 128's second half: like [`setup_adopt_all_callback`] above, but
/// restricted to [`EditorState::multi_selected`] instead of every tier in the
/// design -- freeing one Ctrl-clicked group for Optimize without also disturbing
/// every other still-pinned tier. Applied as a single [`Edit::Batch`] for the
/// identical "one undo step, one re-solve" reason the all-tiers version is. A
/// silent no-op when the selection is empty or none of it has an
/// `imported_meet` left to adopt.
///
/// HANDOFF: needs `callback adopt_selected();` declared on `EditorModel` in
/// `ui/models/editor.slint` (not this lane's file) -- already there as of this
/// pass, so only this handler and `editor_tier_table.slint`'s own "Adopt sel."
/// button (added alongside this) were missing.
pub(in crate::gui::editor) fn setup_adopt_selected_callback(
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
    ui.global::<EditorModel>().on_adopt_selected(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let edits: Vec<Edit> = st
            .design
            .tiers
            .iter()
            .enumerate()
            .filter(|(index, _)| st.multi_selected.contains(index))
            .filter_map(|(index, tier)| {
                tier.imported_meet
                    .clone()
                    .map(|constraint| Edit::SetConstraint { index, constraint })
            })
            .collect();
        if edits.is_empty() {
            return;
        }
        let count = edits.len();
        match st.apply(Edit::Batch(edits)) {
            Ok(()) => {
                refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
                drop(st);
                let plural = if count == 1 { "" } else { "s" };
                show_toast(
                    &ui,
                    &format!("Adopted {count} imported meet{plural} from the selection"),
                    "info",
                );
            }
            Err(e) => show_toast(&ui, &e.to_string(), "error"),
        }
    });
}

/// CAD audit item 130: the inverse of "Adopt" -- freezes a tier's CURRENT solved
/// mast (read back here from the shared `solid_last_solved` cache the tier
/// table's own MAST column is built from) as an exact
/// [`MeetConstraint::ScaleReference`] anchor, through [`Edit::SetConstraint`]
/// exactly like `solve_actions::setup_adopt_meet_callback`'s own "Adopt" --
/// letting a cutter pin a value the solver just derived before optimizing the
/// rest of the design. A silent no-op when there is no current solve for this
/// tier (an out-of-range index, or `solid_last_solved` not populated yet) -- the
/// tier table only shows the "Pin" button while
/// `EditorTierItem::strategy_is_uncertain` is `false`, so this only guards a race
/// with a concurrent edit/solve.
///
/// Calls [`refresh_all`] for the identical reason `setup_adopt_all_callback`
/// above (and `setup_adopt_meet_callback`) do: the value pinned is already what
/// the design's last real solve produced.
///
/// HANDOFF: needs `callback pin_to_mast(int);` declared on `EditorModel` in
/// `ui/models/editor.slint` (not this lane's file) -- the tier table's own
/// per-row "Pin" button (added alongside this) already calls it.
pub(in crate::gui::editor) fn setup_pin_to_mast_callback(
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
        .on_pin_to_mast(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let Some(mast) = solid_last_solved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|solved| solved.get(index))
                .map(|tier| tier.mast)
            else {
                return;
            };
            let mut st = state.borrow_mut();
            match st.apply(Edit::SetConstraint {
                index,
                constraint: MeetConstraint::ScaleReference(mast),
            }) {
                Ok(()) => {
                    refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
                    drop(st);
                    show_toast(
                        &ui,
                        &format!("Pinned tier {} at {mast:.4}", index + 1),
                        "info",
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// CAD audit item 121's remaining half: a clicked index-wheel tooth
/// (`solid_preview::diagram_wiring::setup_diagram_hover_and_click_callbacks`'s
/// own miss branch, which already reports the id through
/// `SolidPreviewModel.diagram_clicked_tooth` -- see that function's own HANDOFF
/// comment naming this callback as the intended consumer) now highlights every
/// facet sharing that tooth, using the reverse of the lookup
/// [`setup_toggle_multi_select_callback`] already does the forward direction of:
/// that one turns a set of TIER indices into their member facet ids via
/// `FacetMap::facets_of_tier`; this one turns one GEAR TOOTH into every facet id
/// whose own `FacetMap::index_on_gear` matches it, by scanning
/// `0..FacetMap::facet_count()` -- the "reverse lookup the facet map already
/// offers" (`cad_todo.md` item 121), since nothing in `facet_map.rs` (not this
/// lane's file) exposes a dedicated tooth-to-facets index. Reuses
/// [`FacetOverlay::multi_selected`] for the tint rather than adding a new overlay
/// field, which would need a `facet_map.rs`/`preview_state.rs` change.
///
/// `tooth < 0` (nothing hit, `SolidPreviewModel.diagram_clicked_tooth`'s own
/// default) clears the highlight instead of leaving a stale one from a previous
/// click.
///
/// HANDOFF: needs `callback highlight_tooth(int);` declared on `EditorModel` in
/// `ui/models/editor.slint` (not this lane's file) -- `editor_tier_table.slint`'s
/// own `tracked_clicked_tooth` mirror (added alongside this; see that property's
/// doc comment for why a mirrored property, not a `changed` handler on the
/// global itself, is what calls it) already invokes it whenever
/// `SolidPreviewModel.diagram_clicked_tooth` changes.
pub(in crate::gui::editor) fn setup_highlight_tooth_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_highlight_tooth(move |tooth: i32| {
            let Some(preview_state) = auto_solve::preview_state() else {
                return;
            };
            if tooth < 0 {
                resubmit_facet_overlay(&preview_state, |overlay| overlay.multi_selected.clear());
                return;
            }
            let st = state.borrow();
            let solved = auto_solve::solid_last_solved()
                .and_then(|cache| {
                    cache
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                })
                .unwrap_or_default();
            let facet_map = FacetMap::from_design(&st.design, &solved);
            let tooth = tooth as u32;
            let facet_ids: Vec<u32> = (0..facet_map.facet_count() as u32)
                .filter(|&facet_id| facet_map.index_on_gear(facet_id as usize) == tooth)
                .collect();
            resubmit_facet_overlay(&preview_state, |overlay| {
                overlay.multi_selected = facet_ids;
            });
        });
}

/// Per-facet index editing (#45): "Remove" -- removes one index-wheel occurrence
/// from a tier via [`Design::remove_orbit_member`]. Removing an occurrence that
/// belongs to a complete, non-detached orbit unit removes every member of that unit
/// with it (see that method's own doc comment) -- deleting "one facet" out of a
/// clean orbit never leaves `symmetry_order` describing a lie about what `indices`
/// actually holds. Wired up from [`setup_toggle_detach_callback`]'s own call site
/// (see that function's doc comment's "wiring point" section for why).
///
/// HANDOFF: needs `callback facet_remove(int, float);` declared on `EditorModel` in
/// `ui/models/editor.slint` (tier index, index-wheel position) -- the per-facet
/// index chips that would call it live in the inspector, owned by another lane.
pub(in crate::gui::editor) fn setup_facet_remove_callback(
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
        .on_facet_remove(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .remove_orbit_member(tier_index, f64::from(position))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Per-facet index editing (#45): "Detach"/"Reattach" toggle for ONE index-wheel
/// occurrence -- the per-facet counterpart to [`setup_toggle_detach_callback`]'s
/// whole-tier button, via [`Design::detach_orbit_member`]/
/// [`Design::reattach_orbit_member`]. `position` already being in the tier's own
/// `detached` list picks the direction, matching [`setup_toggle_detach_callback`]'s
/// own "empty vs. non-empty" convention one level down (a single occurrence rather
/// than the whole tier).
///
/// HANDOFF: needs `callback facet_toggle_detach(int, float);` declared on
/// `EditorModel` in `ui/models/editor.slint` -- same inspector-owned chips as
/// [`setup_facet_remove_callback`]'s own handoff.
pub(in crate::gui::editor) fn setup_facet_toggle_detach_callback(
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
        .on_facet_toggle_detach(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let position = f64::from(position);
            let mut st = state.borrow_mut();
            let Some(tier) = st.design.tiers.get(tier_index) else {
                return;
            };
            let edit_result = if tier.detached.contains(&position) {
                st.design.reattach_orbit_member(tier_index, position)
            } else {
                st.design.detach_orbit_member(tier_index, position)
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
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Per-facet index editing (#45): "Add" -- adds one index-wheel occurrence to a tier
/// via [`Design::add_orbit_member`], expanded to its complete symmetry orbit (see
/// that method's own doc comment): an addition can never leave a half-populated
/// orbit unit behind.
///
/// HANDOFF: needs `callback facet_add(int, float);` declared on `EditorModel` in
/// `ui/models/editor.slint` -- same inspector-owned chips as
/// [`setup_facet_remove_callback`]'s own handoff.
pub(in crate::gui::editor) fn setup_facet_add_callback(
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
        .on_facet_add(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .add_orbit_member(tier_index, f64::from(position))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Tier-wide index editing (#45): "Rotate" -- rotates every index-wheel position in
/// the tier at `tier_index` (both `indices` and `detached`) by `k_teeth` around the
/// gear, via [`Design::rotate_indices`].
///
/// HANDOFF: needs `callback tier_rotate_indices(int, float);` declared on
/// `EditorModel` in `ui/models/editor.slint` (tier index, teeth to rotate by) -- the
/// inspector control that would call it (a "rotate this tier" stepper next to its
/// index chips) is owned by another lane.
pub(in crate::gui::editor) fn setup_tier_rotate_indices_callback(
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
        .on_tier_rotate_indices(move |tier_index: i32, k_teeth: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .rotate_indices(tier_index, f64::from(k_teeth))
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Tier-wide index editing (#45): "Mirror" -- mirrors every index-wheel position in
/// the tier at `tier_index` (both `indices` and `detached`) to the other side of the
/// symmetry axis, via [`Design::mirror_indices`].
///
/// HANDOFF: needs `callback tier_mirror_indices(int);` declared on `EditorModel` in
/// `ui/models/editor.slint` -- same inspector-owned control as
/// [`setup_tier_rotate_indices_callback`]'s own handoff.
pub(in crate::gui::editor) fn setup_tier_mirror_indices_callback(
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
        .on_tier_mirror_indices(move |tier_index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let mut st = state.borrow_mut();
            match st
                .design
                .mirror_indices(tier_index)
                .and_then(|edit| st.apply(edit))
            {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([tier_index]),
                        false,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// Row reorder (`Alt+Up`/`Alt+Down` and the tier list's own move buttons,
/// `editor_tier_table.slint`): moves the tier at `index` to `index + direction` as
/// one [`Edit::MoveTier`] -- a single `History` step (one undo press restores the
/// original order) with its own honest "Move tier P1 up/down" label, rather than
/// the two independently-undoable [`Edit::ModifyTier`] content swaps this used
/// before `Edit::MoveTier` existed. `MoveTier` renumbers every tier strictly
/// between `from` and `to` (see its own doc comment), so this always forces a full
/// re-solve rather than passing a `dirty` set of just the two endpoints.
pub(in crate::gui::editor) fn setup_move_tier_callback(
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
        .on_move_tier(move |index: i32, direction: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let tier_count = st.design.tiers.len();
            let target = if direction < 0 {
                index.checked_sub(1)
            } else {
                index.checked_add(1).filter(|&t| t < tier_count)
            };
            let Some(target) = target else {
                return;
            };
            match st.apply(Edit::MoveTier {
                from: index,
                to: target,
            }) {
                Ok(()) => {
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
                    drop(st);
                    ui.global::<EditorModel>()
                        .set_selected_tier_index(target as i32);
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The tier table's "Complete orbit" action (shown on a row whose
/// [`EditorTierItem::orbit_incomplete`](crate::EditorTierItem) is set): expands
/// EVERY incomplete orbit unit the tier currently decomposes into
/// (`Design::orbit_units`) to its full symmetric membership via one
/// `Design::add_orbit_member` call each, anchored on that unit's own first member
/// -- a tier with several independent incomplete units (rare, but possible) is
/// fully completed in one click, as several separately-undoable `History` steps
/// rather than a new batch primitive.
pub(in crate::gui::editor) fn setup_complete_orbit_callback(
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
        .on_complete_orbit(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let mut st = state.borrow_mut();
            let Ok(units) = st.design.orbit_units(index) else {
                return;
            };
            let anchors: Vec<f64> = units
                .iter()
                .filter(|unit| !unit.is_complete())
                .filter_map(|unit| unit.members.first().copied())
                .collect();
            if anchors.is_empty() {
                return;
            }
            let mut last_err = None;
            for position in anchors {
                let outcome = st
                    .design
                    .add_orbit_member(index, position)
                    .and_then(|edit| st.apply(edit));
                if let Err(e) = outcome {
                    last_err = Some(e);
                }
            }
            match last_err {
                None => {
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
                Some(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The tier table header's "Clear" action on its "N selected" indicator: empties
/// [`EditorState::multi_selected`] without touching `Design`, matching
/// [`setup_solid_selected_tier_changed_callback`]'s own clearing path.
pub(in crate::gui::editor) fn setup_clear_multi_select_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_clear_multi_select(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        if st.multi_selected.is_empty() {
            return;
        }
        st.multi_selected.clear();
        let mut rows: Vec<EditorTierItem> = ui.global::<EditorModel>().get_tiers().iter().collect();
        apply_multi_selection(&mut rows, &st.multi_selected);
        push_tiers(&ui, rows);
        push_multi_selected_count(&ui, 0);
        if let Some(preview_state) = auto_solve::preview_state() {
            resubmit_facet_overlay(&preview_state, |overlay| overlay.multi_selected.clear());
        }
    });
}

/// The tier table header's "Delete" action on its "N selected" indicator: removes
/// every multi-selected tier, highest index first (so removing one never shifts
/// an index still waiting to be removed out from under this loop) -- as several
/// separately-undoable [`Edit::RemoveTier`]s, matching [`setup_complete_orbit_
/// callback`]'s "several `History` steps, no new batch primitive" trade-off.
pub(in crate::gui::editor) fn setup_remove_multi_selected_callback(
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
        .on_remove_multi_selected(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let targets: Vec<usize> = st.multi_selected.iter().rev().copied().collect();
            if targets.is_empty() {
                return;
            }
            let removed_count = targets.len();
            let mut last_err = None;
            for index in targets {
                if let Err(e) = st.apply(Edit::RemoveTier { index }) {
                    last_err = Some(e);
                }
            }
            match last_err {
                None => {
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
                    drop(st);
                    ui.global::<EditorModel>().set_selected_tier_index(-1);
                    bump_form_reset_pulse(&ui);
                    show_toast(&ui, &format!("Removed {removed_count} tier(s)."), "info");
                }
                Some(e) => show_toast(&ui, &e.to_string(), "error"),
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
/// change needs the remap confirmation). Also registers
/// [`EditorModel::on_request_symmetry_preview`] (CAD audit item 135): a live
/// dry-run preview of the SAME proposed change, computed as the Symmetry Order
/// field is edited or Mirror is toggled, so switching (say) 8-fold to 6-fold no
/// longer turns rows amber with no warning and no chance to reconsider before
/// clicking Apply -- mirroring the gear-remap path's own dry-run preview
/// (`gear_remap_preview`/[`setup_gear_apply_callback`]). Registered here rather
/// than as its own `setup_*` function so it can share this function's own
/// `state` clone instead of this module's registration point (`mod.rs`, a
/// different lane's file right now) needing a new call site.
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
    // CAD audit item 135's own preview half -- cloned from `state` BEFORE
    // `on_apply_symmetry` below moves the ORIGINAL `state` binding into its
    // own closure, not after (that closure is `move`, so `state` is gone once
    // it is constructed).
    let state_preview = Rc::clone(&state);
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

    // CAD audit item 135's own preview half -- see this function's own doc
    // comment above.
    let ui_weak_preview = ui.as_weak();
    ui.global::<EditorModel>().on_request_symmetry_preview(
        move |symmetry_order_text: SharedString, mirror: bool| {
            let Some(ui) = ui_weak_preview.upgrade() else {
                return;
            };
            // An unparsable/zero symmetry order can never be applied (see the
            // real `on_apply_symmetry` handler's own validation above) -- no
            // preview to show rather than a stale or misleading one.
            let Ok(symmetry_order) = symmetry_order_text.trim().parse::<u32>() else {
                ui.global::<EditorModel>()
                    .set_symmetry_preview_text(String::new().into());
                return;
            };
            if symmetry_order == 0 {
                ui.global::<EditorModel>()
                    .set_symmetry_preview_text(String::new().into());
                return;
            }
            let st = state_preview.borrow();
            let incomplete =
                tiers_incomplete_under_proposed_symmetry(&st.design, symmetry_order, mirror);
            let text = if incomplete == 0 {
                "No tiers would become incomplete orbits.".to_string()
            } else {
                let plural = if incomplete == 1 { "" } else { "s" };
                format!("{incomplete} tier{plural} would become incomplete orbits.")
            };
            ui.global::<EditorModel>()
                .set_symmetry_preview_text(text.into());
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
/// [`EditorState::pending_gear_remap`] as ONE undoable `History` step, an
/// [`Edit::Batch`] of [`Edit::RemapIndices`] then [`Edit::SetSchedule`] (CAD audit
/// items 79/86) -- previously two separate, independently-undoable steps, so one
/// Undo after a gear change could leave indices remapped for the new gear while
/// the schedule still named the old one. A no-op (closes the panel only) if
/// nothing is pending -- defensive only, since this button only shows while a
/// real remap is pending.
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
        let batch_result = st.apply(Edit::Batch(vec![
            Edit::RemapIndices {
                from_gear: pending.from_gear,
                to_gear: pending.to_gear,
                rounding: pending.rounding,
            },
            Edit::SetSchedule {
                gear_teeth: pending.to_gear,
                symmetry_order: pending.symmetry_order,
                mirror: pending.mirror,
            },
        ]));
        ui.global::<EditorModel>().set_gear_remap_open(false);
        match batch_result {
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

// CAD audit item 86's other half (rounding choice) is NOT implemented here --
// see this file's end-of-task handoff notes. Wiring it requires a new
// `EditorModel` callback (`ui/models/editor.slint`, not owned by this lane)
// and a call to register its handler from `gui/editor/mod.rs` (also not owned
// by this lane); a Rust handler added here ahead of that Slint declaration
// would fail to build (the generated `EditorModel::on_gear_remap_set_rounding`
// would not exist yet) and block this lane's own clippy verification for
// every other fix in this file. See the handoff note for the exact shape.

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

/// CAD audit item 117's input half: maps an incoming Solid-viewport pointer
/// position (`SolidPreviewModel.on_facet_hover`/`on_facet_click`'s own `x`/`y`,
/// LOGICAL pixels) onto the pick buffer's PHYSICAL-pixel coordinate space.
///
/// In Path-traced/Both mode (`SolidPreviewModel.view_mode` `1`/`2`) the solid/
/// edges rasters are now requested at the LETTERBOXED rectangle
/// `render::camera_lighting::contained_request_size` computes -- mirroring
/// `solid_viewport.slint`'s own `image-fit: contain` -- which can be smaller
/// than, and is centred within, the viewport's own raw rectangle (see that
/// function's own doc comment for when and why). Multiplying by
/// `scale_factor` alone (the pre-existing, still-necessary logical-to-physical
/// conversion) is not enough on its own in that case: it would still index the
/// pick buffer as though it covered the FULL, un-letterboxed viewport, landing a
/// click off the traced gem by exactly the letterbox bars' width/height.
///
/// Falls back to the plain scaled position (today's behavior) when
/// `auto_solve::render_ctx` has not been stashed yet, or `RenderContext.width`/
/// `height` is `0` (nothing requested) -- `contained_request_size` itself
/// already returns `viewport_size` unchanged for Solid/Diagram modes, so this
/// is a genuine no-op there, not just an approximation.
fn map_to_pick_coordinates(ui: &MainWindow, x: f32, y: f32) -> (f32, f32) {
    let scale = ui.window().scale_factor();
    let physical = (x * scale, y * scale);
    let Some(render_ctx) = auto_solve::render_ctx() else {
        return physical;
    };
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    let viewport_physical = (
        (ui.global::<SolidPreviewModel>().get_viewport_width() * scale) as u32,
        (ui.global::<SolidPreviewModel>().get_viewport_height() * scale) as u32,
    );
    let render_size = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (ctx.width, ctx.height)
    };
    let contained = crate::gui::render::camera_lighting::contained_request_size(
        view_mode,
        viewport_physical,
        render_size,
    );
    let (margin_x, margin_y) = letterbox_margin(viewport_physical, contained);
    (physical.0 - margin_x, physical.1 - margin_y)
}

/// The physical-pixel margin [`map_to_pick_coordinates`] subtracts before
/// indexing the pick buffer -- pure arithmetic, unit tested directly. `image-fit:
/// contain` centres the smaller `contained_physical` rectangle inside
/// `viewport_physical`, so the margin on each axis is exactly half of
/// whatever's left over; `saturating_sub` guards the (never expected, but never
/// unsafe either) case where `contained_physical` is somehow larger.
fn letterbox_margin(viewport_physical: (u32, u32), contained_physical: (u32, u32)) -> (f32, f32) {
    (
        viewport_physical.0.saturating_sub(contained_physical.0) as f32 / 2.0,
        viewport_physical.1.saturating_sub(contained_physical.1) as f32 / 2.0,
    )
}

/// The Solid viewport's hover callback -- looks up the facet under the cursor against
/// the pick buffer of the LAST rendered frame and sets `editor_solid_hover_text` from
/// `solid_hover_text`, the same frame's own `PreviewFrame::hover_text` table
/// (`SlintSolidSink::apply`, `gui::mod`). A silent no-op off the silhouette or before
/// anything has ever rendered.
///
/// # #22: indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// Every hover used to call `FacetMap::from_design(&st.design, &solved)` -- a full
/// `Design::planes_from_solved`-equivalent rebuild plus a fresh `dedup_planes` pass --
/// on every single mouse-move event. `solid_hover_text` already holds exactly the
/// string this map would have produced for each facet id, computed ONCE per rendered
/// frame by the worker thread (`preview_state::update_diagram_memory_from_design`'s
/// Solid-mode counterpart), so this now degrades to one `Vec::get`. Neither `Design`
/// nor the last-solved mast cache is needed here any more (an editor edit that
/// hasn't re-rendered yet still shows the PREVIOUS frame's hover text, exactly as it
/// showed the previous frame's picked geometry -- no new staleness).
///
/// HANDOFF: needs a `solid_hover_text: &Arc<Mutex<Vec<String>>>` parameter added to
/// this call in `gui::editor::mod::setup_editor_callbacks` (not this lane's file),
/// sourced from the `solid_hover_text` binding `gui::mod::build_main_window` already
/// creates (currently unused past `SlintSolidSink`'s own construction) -- see that
/// file's own "#22" comment for the exact spot.
pub(in crate::gui::editor) fn setup_solid_facet_hover_callback(
    ui: &MainWindow,
    solid_pick_state: &SolidPickState,
) {
    let solid_pick = Arc::clone(&solid_pick_state.pick);
    let solid_hover_text = Arc::clone(&solid_pick_state.hover_text);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_hover(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // The pick buffer is rasterized at the viewport's PHYSICAL size
            // (`view::scaled_viewport_size`) but `x`/`y` are the pointer's LOGICAL
            // position, so they are MULTIPLIED by the same `scale_factor` to reach
            // the physical pixel they name: at 2x, logical x=400 in an 800-wide
            // viewport is physical x=800 in a 1600-wide pick buffer. Dividing would
            // collapse every pick into the top-left quarter of the image.
            // `solid_preview::diagram_wiring` does the same for the diagram's own
            // pick buffer. CAD audit item 117: in Path-traced/Both mode the pick
            // buffer is smaller than the raw viewport (letterboxed), so
            // `map_to_pick_coordinates` also subtracts the centred margin --
            // see that function's own doc comment. `.max(0.0) as u32` saturates a
            // negative position (past the image's edge, or inside a letterbox bar)
            // to `0`; `PickBuffer::facet_at` already bounds-checks against the
            // frame's width/height, so this simply misses (`None`) rather than
            // reading garbage.
            let (px, py) = map_to_pick_coordinates(&ui, x, y);
            let facet_id = solid_pick
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|pick| pick.facet_at(px.max(0.0) as u32, py.max(0.0) as u32));
            let Some(facet_id) = facet_id else {
                ui.global::<SolidPreviewModel>().set_hover_text("".into());
                if let Some(preview_state) = auto_solve::preview_state() {
                    resubmit_facet_overlay(&preview_state, |overlay| overlay.hovered = None);
                }
                return;
            };
            let text = solid_hover_text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(facet_id as usize)
                .cloned()
                .unwrap_or_default();
            ui.global::<SolidPreviewModel>().set_hover_text(text.into());
            if let Some(preview_state) = auto_solve::preview_state() {
                resubmit_facet_overlay(&preview_state, |overlay| overlay.hovered = Some(facet_id));
            }
        });
}

/// The Solid viewport's click callback -- the forward half of the "click selects the
/// tier in the list" link (see [`setup_solid_selected_tier_changed_callback`] for the
/// reverse half).
///
/// # #22: indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// See [`setup_solid_facet_hover_callback`]'s matching doc section: `solid_facet_tier`
/// is the last rendered frame's own `PreviewFrame::facet_tier` table, so resolving a
/// clicked facet to its owning tier is now one `Vec::get` instead of a fresh
/// `FacetMap::from_design` rebuild. Neither `Design` nor the last-solved mast cache is
/// needed here any more.
///
/// HANDOFF: needs a `solid_facet_tier: &Arc<Mutex<Vec<Option<usize>>>>` parameter
/// added to this call in `gui::editor::mod::setup_editor_callbacks` (not this lane's
/// file), sourced from the `solid_facet_tier` binding `gui::mod::build_main_window`
/// already creates -- same handoff spot as `setup_solid_facet_hover_callback`'s own.
pub(in crate::gui::editor) fn setup_solid_facet_click_callback(
    ui: &MainWindow,
    solid_pick_state: &SolidPickState,
) {
    let solid_pick = Arc::clone(&solid_pick_state.pick);
    let solid_facet_tier = Arc::clone(&solid_pick_state.facet_tier);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_click(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // See `setup_solid_facet_hover_callback`'s matching comment for why the
            // incoming (logical) coordinates go through `map_to_pick_coordinates`
            // (scaled AND, in Path-traced/Both mode, letterbox-corrected) here.
            let (px, py) = map_to_pick_coordinates(&ui, x, y);
            let Some(facet_id) = solid_pick
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|pick| pick.facet_at(px.max(0.0) as u32, py.max(0.0) as u32))
            else {
                // CAD audit item 229: a click that misses the silhouette clears
                // the tier selection instead of leaving it alone -- matching what
                // `solid_preview::diagram_wiring`'s own Diagram-mode click miss
                // branch already does (see that function's own #229 comment).
                // Setting `selected_tier_index` alone is enough: `changed
                // selected_tier_index` in `models/editor.slint` fires
                // `selected_tier_changed`, which re-seeds/clears the inspector
                // form on the Rust side (`setup_solid_selected_tier_changed_
                // callback`).
                ui.global::<EditorModel>().set_selected_tier_index(-1);
                return;
            };
            let tier_index = solid_facet_tier
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(facet_id as usize)
                .copied()
                .flatten();
            if let Some(tier_index) = tier_index {
                ui.global::<EditorModel>()
                    .set_selected_tier_index(tier_index as i32);
            }
            // The clicked facet stays lit regardless of whether it resolved to a
            // tier above -- a facet under the cursor is always a real pick.
            if let Some(preview_state) = auto_solve::preview_state() {
                resubmit_facet_overlay(&preview_state, |overlay| {
                    overlay.selected_facet = Some(facet_id);
                });
            }
        });
}

/// The reverse link: whenever the tier list's selection changes (a row click, or
/// [`setup_solid_facet_click_callback`] setting `editor_selected_tier_index` from a
/// viewport click), re-submits a redraw with the new `selected_tier` so the overlay
/// tint follows it. Cheap: the mesh is unchanged (`dirty` empty), so the worker's
/// `MeshCache` hits and only the style/render redo.
///
/// Also narrows [`EditorState::multi_selected`] back down to nothing here: EVERY
/// path that fires `selected_tier_changed` is, by construction, a plain (non-Ctrl)
/// selection -- `setup_toggle_multi_select_callback`'s own Ctrl+click path never
/// touches `editor_selected_tier_index` at all, so it never reaches this callback --
/// so this is the one place that needs to clear a forgotten multi-select group
/// before it silently widens the next angle nudge (see `setup_nudge_angle_callback`'s
/// own doc comment for that batch behaviour). Patches the already-pushed
/// `editor_tiers` model in place, the same as `setup_toggle_multi_select_callback`
/// does, rather than a full [`refresh_editor_panel_stale`] -- narrowing the
/// multi-select highlight is not itself a `Design` edit and must not re-label the
/// validation banner "Not solved".
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
            // Slint runs this handler SYNCHRONOUSLY from inside
            // `set_selected_tier_index`, and two Rust paths write that property while
            // still holding `state.borrow_mut()`: `apply_loaded_design` (Load
            // Selected, whose guard stays live for the window title below it) and the
            // Remove Tier callback via `adjust_selection_after_remove`. Both used to
            // panic here with "RefCell already mutably borrowed".
            //
            // Deferred rather than skipped: unlike `recompute_dirty`, the work below
            // is not reproduced by those callers -- it clears a stale multi-select
            // group, and `setup_solid_facet_click_callback` documents the property
            // write as the only thing needed to re-seed the inspector. One
            // event-loop turn is enough, because a `RefCell` guard can never outlive
            // the call that took it.
            let busy = state.try_borrow_mut().is_err();
            if !busy {
                if let Some(ui) = ui_weak.upgrade() {
                    apply_selected_tier_change(
                        &ui,
                        &state,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                    );
                }
                return;
            }
            let ui_weak = ui_weak.clone();
            let state = Rc::clone(&state);
            let render_ctx = Arc::clone(&render_ctx);
            let preview_state = Arc::clone(&preview_state);
            let solid_last_solved = Arc::clone(&solid_last_solved);
            slint::Timer::single_shot(Duration::ZERO, move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                apply_selected_tier_change(
                    &ui,
                    &state,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                );
            });
        });
}

/// The body of [`setup_solid_selected_tier_changed_callback`]'s handler, factored out
/// so it can run either immediately or one event-loop turn later -- see that
/// function's own comment for which paths need the deferral and why.
fn apply_selected_tier_change(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let mut st = state.borrow_mut();
    if !st.multi_selected.is_empty() {
        st.multi_selected.clear();
        let mut rows: Vec<EditorTierItem> = ui.global::<EditorModel>().get_tiers().iter().collect();
        apply_multi_selection(&mut rows, &st.multi_selected);
        push_tiers(ui, rows);
        push_multi_selected_count(ui, 0);
        resubmit_facet_overlay(preview_state, |overlay| overlay.multi_selected.clear());
    }
    // The inspector's index chips describe whichever tier is now loaded, and
    // a plain row click (no edit) reaches Rust nowhere else.
    push_selected_tier_chips(ui, &st);
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        &st,
        BTreeSet::new(),
        false,
    );
}

/// Shows one tier-form failure in both places a cutter looks: inline under the
/// field that caused it (`EditorModel.tier_form_error`, which the inspector renders
/// in red) and as a toast. Factored out because all three failure branches of
/// [`setup_save_tier_callback`] must agree on the wording -- an inline message that
/// disagrees with the toast is worse than either alone.
fn report_tier_form_error(ui: &MainWindow, message: &str) {
    ui.global::<EditorModel>()
        .set_tier_form_error(message.into());
    show_toast(ui, message, "error");
}

/// CAD audit item 34: `loading::parse_tier_form` deliberately ACCEPTS a
/// non-integral index-wheel position (real `.asc` files carry a small but
/// real fraction of these -- see that function's own doc comment) rather
/// than rejecting it, since a hand-typed fraction is sometimes exactly what
/// was meant. But it used to give a cutter no signal at all when it was NOT
/// meant -- a stray extra digit ("12.5" for "12") only ever surfaced later as
/// an obscure solver oddity. Called from [`setup_save_tier_callback`] after a
/// successful parse, alongside the toast, rather than inside `parse_tier_form`
/// itself, so a save is never blocked by this -- only flagged. `1e-3`
/// matches `indicatrix_cut_core`'s own `orbit::model::INDEX_TOLERANCE` order of
/// magnitude for "close enough to call it a whole tooth".
fn non_integral_index_warning(indices: &[f64]) -> Option<String> {
    let mut fractional: Vec<String> = indices
        .iter()
        .filter(|v| (**v - v.round()).abs() > 1e-3)
        .map(|v| format!("{v:.3}"))
        .collect();
    fractional.dedup();
    if fractional.is_empty() {
        return None;
    }
    Some(format!(
        "Note: non-integral index position(s) {} -- check this was intentional.",
        fractional.join(", ")
    ))
}

/// The tier list's filter box asks Rust whether one row matches, because Slint's
/// `string` type has no substring test. A `pure` callback, so the table can call it
/// straight from a per-row binding.
pub(in crate::gui::editor) fn setup_tier_filter_callback(ui: &MainWindow) {
    ui.global::<EditorModel>().on_tier_matches_filter(
        |haystack: SharedString, filter: SharedString| tier_matches_filter(&haystack, &filter),
    );
}

/// Bumps `EditorModel.form_reset_pulse` -- see that property's own doc comment
/// (`ui/models/editor.slint`) for why a plain increment, not a flag, is what makes
/// `EditorView`'s watcher fire even when `selected_tier_index` is already `-1` (so
/// setting it to `-1` again would raise no `changed` on its own).
fn bump_form_reset_pulse(ui: &MainWindow) {
    let pulse = ui.global::<EditorModel>().get_form_reset_pulse();
    ui.global::<EditorModel>()
        .set_form_reset_pulse(pulse.wrapping_add(1));
}

/// Adjusts `EditorModel.selected_tier_index` after [`Edit::RemoveTier`] removes the
/// tier at `removed_index`, shifting every later tier down by one (see
/// `indicatrix_cut_core`'s own `edit::apply` -- read-only here): equal to the removed
/// index, the selected tier no longer exists, so the selection is cleared and
/// [`bump_form_reset_pulse`] guarantees `EditorView`'s form still blanks even if the
/// selection was already `-1`; greater than it, the same tier survived one position
/// lower, so the index is shifted down to keep pointing at it (this fires
/// `EditorView`'s `changed tracked_selected_tier_index` for real, re-seeding the
/// form from the right row). Anything else (less than, or already `-1`) is left
/// untouched.
fn adjust_selection_after_remove(ui: &MainWindow, removed_index: i32) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    if selected == removed_index {
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        bump_form_reset_pulse(ui);
    } else if selected > removed_index {
        ui.global::<EditorModel>()
            .set_selected_tier_index(selected - 1);
    }
}

/// Clamps `EditorModel.selected_tier_index` after an undo/redo to `tier_count`
/// (`design.tiers.len()` post-replay). Unlike [`adjust_selection_after_remove`],
/// undo/redo can change the tier count by any amount in either direction (an
/// `AddTier`/`RemoveTier` reversed, or several tiers' worth of a coalesced
/// `RetargetAngles`), so there is no single shifted-by-one relationship to
/// preserve -- an index that no longer names a real tier is simply cleared, with
/// the same [`bump_form_reset_pulse`] guarantee.
fn clamp_selection_to_tier_count(ui: &MainWindow, tier_count: usize) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let out_of_range = usize::try_from(selected).is_ok_and(|index| index >= tier_count);
    if out_of_range {
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        bump_form_reset_pulse(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        letterbox_margin, next_free_block_name, split_duplicate_suffix, unique_duplicate_name,
    };

    // --- next_free_block_name (CAD audit item 129) ---

    #[test]
    fn next_free_block_name_starts_at_1_for_a_crown_angle() {
        assert_eq!(next_free_block_name(45.0, &[]), "C1");
    }

    #[test]
    fn next_free_block_name_uses_p_for_a_negative_angle() {
        assert_eq!(next_free_block_name(-40.0, &[]), "P1");
    }

    #[test]
    fn next_free_block_name_treats_non_negative_zero_as_crown() {
        assert_eq!(next_free_block_name(0.0, &[]), "C1");
    }

    #[test]
    fn next_free_block_name_skips_names_already_in_use() {
        let existing = vec!["C1".to_string(), "C2".to_string()];
        assert_eq!(next_free_block_name(45.0, &existing), "C3");
    }

    #[test]
    fn next_free_block_name_collision_check_is_case_insensitive() {
        let existing = vec!["c1".to_string()];
        assert_eq!(next_free_block_name(45.0, &existing), "C2");
    }

    // --- unique_duplicate_name / split_duplicate_suffix (CAD audit items 129/132/232) ---

    #[test]
    fn unique_duplicate_name_starts_at_2_when_nothing_collides() {
        assert_eq!(unique_duplicate_name("P1", &[]), "P1 (2)");
    }

    #[test]
    fn unique_duplicate_name_skips_names_already_in_use() {
        let existing = vec!["P1".to_string(), "P1 (2)".to_string(), "P1 (3)".to_string()];
        assert_eq!(unique_duplicate_name("P1", &existing), "P1 (4)");
    }

    #[test]
    fn unique_duplicate_name_collision_check_is_case_insensitive() {
        let existing = vec!["p1 (2)".to_string()];
        assert_eq!(unique_duplicate_name("P1", &existing), "P1 (3)");
    }

    #[test]
    fn unique_duplicate_name_counts_up_instead_of_nesting() {
        // Duplicating a tier already named "P1 (2)" must produce "P1 (3)", not
        // "P1 (2) (2)" -- see `split_duplicate_suffix`'s own doc comment. The
        // empty `existing_names` is the point: this has to hold on the
        // function's own terms, not only because the real caller passes a list
        // that contains the source tier (which would make "P1 (2)" collide).
        assert_eq!(unique_duplicate_name("P1 (2)", &[]), "P1 (3)");
    }

    #[test]
    fn unique_duplicate_name_continues_from_a_high_source_number() {
        assert_eq!(unique_duplicate_name("P1 (9)", &[]), "P1 (10)");
    }

    #[test]
    fn unique_duplicate_name_treats_an_unreadable_suffix_as_part_of_the_name() {
        // Not reachable by duplicating, but a cutter can type any name they
        // like -- this must still produce something, never loop or panic.
        let huge = format!("P1 ({})", u128::from(u32::MAX) + 1);
        assert_eq!(unique_duplicate_name(&huge, &[]), format!("{huge} (2)"));
    }

    #[test]
    fn unique_duplicate_name_falls_back_to_tier_for_an_unnamed_source() {
        assert_eq!(unique_duplicate_name("", &[]), "Tier (2)");
    }

    #[test]
    fn split_duplicate_suffix_reads_a_trailing_parenthesized_number() {
        assert_eq!(split_duplicate_suffix("P1 (2)"), ("P1", Some(2)));
        assert_eq!(split_duplicate_suffix("P1 (12)"), ("P1", Some(12)));
    }

    #[test]
    fn split_duplicate_suffix_leaves_unrelated_text_alone() {
        assert_eq!(split_duplicate_suffix("P1"), ("P1", None));
        assert_eq!(split_duplicate_suffix("P1 (a)"), ("P1 (a)", None));
        assert_eq!(split_duplicate_suffix("P1 (2"), ("P1 (2", None));
        assert_eq!(split_duplicate_suffix("P1/P2 (2)"), ("P1/P2", Some(2)));
    }

    // --- letterbox_margin (CAD audit item 117) ---

    #[test]
    fn letterbox_margin_is_zero_when_the_pick_buffer_fills_the_viewport() {
        assert_eq!(letterbox_margin((800, 600), (800, 600)), (0.0, 0.0));
    }

    #[test]
    fn letterbox_margin_is_half_the_leftover_on_each_side() {
        // A 1920x1080 (16:9) render letterboxed into an 800x600 (4:3) viewport --
        // `contained_request_size` would fit the width and shrink the height.
        assert_eq!(letterbox_margin((800, 600), (800, 450)), (0.0, 75.0));
    }

    #[test]
    fn letterbox_margin_never_goes_negative_if_the_pick_buffer_is_larger() {
        assert_eq!(letterbox_margin((800, 600), (900, 700)), (0.0, 0.0));
    }
}
