//! The Solve/New/Load-Selected/Undo/Redo/tier/preform/yield-input edit callbacks --
//! one `setup_*` function per Slint callback. See this group's `mod.rs` doc comment
//! for the "`History` is the only thing that mutates `Design`" rule every callback
//! here upholds via `EditorState::apply`.

use super::{
    super::{
        auto_solve, edit_intent, loading,
        material_lookup::nearest_built_in_material,
        native_io::do_open_native,
        stall_guard::stall_guard,
        state::{
            ANGLE_NUDGE_COALESCE_WINDOW, EditorState, MaterialComboCache, PendingGearRemap,
            PendingUnsavedAction, PushedScratch, angle_nudge_coalesce_key, apply_multi_selection,
            first_unresolved_meet_name, gear_choice_to_teeth, gear_remap_preview,
            parse_design_material_form, push_multi_selected_count, push_tiers,
            representative_crown_and_pavilion_angles_deg, tier_matches_filter,
            tiers_incomplete_under_proposed_symmetry,
        },
        view::{
            SolidLastSolved, push_selected_tier_chips, refresh_all_now, refresh_editor_panel_stale,
            submit_preview_replan, sync_viewport_material_link,
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
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, FreshDesignSpec, History, RemapRounding, TierTarget,
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::{Cell, RefCell, RefMut},
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// A cylindrical preform's side count, fixed and independent of the design's
/// index gear. The preform is a piece of rough, not a machine setting. Switching
/// gear after Apply Preform leaves the original rough's side count behind if it
/// derived from the old gear; a preform applied under one gear stays shaped by it
/// if switched. 96 is high enough that the rough reads as smooth in the viewport
/// at every zoom level -- the same figure
/// `indicatrix_cut_core::PreformSpec::cylinder_for_schedule`'s own doc uses for
/// the "reconstructed once from an already-authored schedule" case in
/// `loading::default_preform_for_schedule`, which this constant deliberately does
/// not touch.
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
    static FACET_OVERLAY: RefCell<FacetOverlay> = const {
        RefCell::new(FacetOverlay {
            hovered: None,
            selected_facet: None,
            multi_selected: Vec::new(),
        })
    };

    /// The last clicked facet's own identifying text -- the SAME "tier name, index
    /// N of M" string [`SolidPreviewModel::hover_text`] shows transiently on hover.
    /// Kept around so a click's selection keeps reading on screen after the
    /// pointer leaves. Reused rather than a new `EditorModel`/`SolidPreviewModel`
    /// property: no `.slint` file declares one, and this gives a
    /// persistent per-facet readout with the existing tooltip mechanism alone.
    /// See [`setup_solid_facet_hover_callback`]'s miss branch and
    /// [`setup_solid_facet_click_callback`], which writes it. UI-thread-only,
    /// same reasoning as `FACET_OVERLAY` above.
    static SELECTED_FACET_LABEL: RefCell<String> = const { RefCell::new(String::new()) };

    /// The design generation (`EditorState::generation`) at the moment
    /// [`setup_gear_apply_callback`] built the pending remap's preview --
    /// compared against the LIVE generation in [`setup_gear_remap_confirm_callback`]
    /// to refuse a stale Confirm. Uses the same guard shape `retarget_actions::
    /// apply_pending_retarget`/`solve_actions`'s optimize-apply path already use
    /// (`started_generation` compared via `AtomicU64::load`). Reimplemented here
    /// rather than adding a field to `PendingGearRemap` or changing
    /// `EditorState::pending_gear_remap`'s type. Cleared whenever nothing is
    /// pending, so a stale leftover value can never be compared against by mistake.
    static PENDING_GEAR_REMAP_GENERATION: Cell<Option<u64>> = const { Cell::new(None) };
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

/// Builds a [`FacetMap`] from `design` and the last-solved mast cache, but only
/// when that cache is aligned with `design`'s CURRENT tier count. An unfiltered
/// `solid_last_solved().unwrap_or_default()` here fed
/// `FacetMap::from_design` a stale solve (wrong length) whenever a tier had just
/// been added or removed and the background solve had not caught up yet;
/// `facet_map.rs`'s own fallback for a missing tier is mast 0, so every
/// highlight/hover/pick landed on the wrong facet until the next frame. Falls
/// back to `FacetMap::default()` (no facets) rather than a stale solve, same
/// reasoning [`retarget_actions::setup_snapshot_callbacks`]'s length filter uses
/// for its own `solid_last_solved` read.
fn facet_map_from_aligned_solve(design: &Design) -> FacetMap {
    let solved = auto_solve::solid_last_solved()
        .and_then(|cache| {
            cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .filter(|solved| solved.len() == design.tiers.len())
        .unwrap_or_default();
    FacetMap::from_design(design, &solved)
}

/// "Solve": the explicit re-solve action -- see this group's `mod.rs` doc comment for
/// why every other edit callback deliberately does NOT do this. The only callback
/// here besides `New`/`Load Selected` that calls `refresh_all` (a real `Design::solve`,
/// potentially multi-second) rather than [`refresh_editor_panel_stale`].
///
/// Guards against Deep Solve or Optimize already running, not only a
/// re-entrant Solve click. Solve, Deep Solve and Optimize each
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
        stall_guard("on_solve", || {
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
            refresh_all_now(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &state,
                false,
            );
        });
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
            stall_guard("on_new_design_create", || {
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
            });
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
    // The "Start from" choice: 0 is "Empty"; 1..=N indexes
    // `indicatrix_cut_core::templates::TEMPLATES` at `template_index - 1`
    // (index 1 is "Standard Round Brilliant", `TEMPLATES[0]`, matching this
    // gallery's own display order -- see `gui/editor/templates.rs::
    // setup_template_gallery`). Any index the table has no entry for (0, a
    // negative value, or one past the end) is treated as empty rather than
    // panicking -- the combo/gallery is the only producer, but a stale index
    // must not lose a design.
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
    // `template_index - 1`: index 1 is the gallery's first template
    // ("Standard Round Brilliant", `TEMPLATES[0]`) -- see this parameter's own
    // doc comment above. `usize::try_from` refuses a negative index (0 =
    // "Empty", or a stale/corrupt value) instead of panicking on the cast.
    if let Ok(table_index) = usize::try_from(template_index - 1)
        && let Some(spec) = indicatrix_cut_core::templates::TEMPLATES.get(table_index)
    {
        st.design.tiers = spec.tiers();
    }
    // A Deep Solve/Optimize verdict computed against the design just replaced no
    // longer describes anything on screen -- see `clear_analysis_results`'s own
    // doc comment.
    clear_analysis_results(ui);
    drop(st);
    refresh_all_now(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        true,
    );
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
    /// unconditionally rather than defaulted to `None` here: every
    /// `LoadedDesignOutcome` construction site, including `native_io.rs`'s own, must
    /// supply the correct value for this field rather than rely on a default.
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
        // See `EditorState::fresh`'s matching comment.
        history: History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
        printed_proportions,
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
        multi_selected: BTreeSet::new(),
        // A freshly replaced `EditorState` has never pushed anything yet -- matches
        // `EditorState::fresh_from_spec`'s own construction (`state/mod.rs`); every
        // other `EditorState` construction site, including `native_io.rs`'s own,
        // must set this field the same way.
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        // Same reasoning as `last_pushed_scratch` immediately above -- a freshly
        // replaced `EditorState` has no cached material-combo options yet either.
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
    });
    // A Deep Solve/Optimize verdict computed against the design just replaced no
    // longer describes anything on screen -- see `clear_analysis_results`'s own
    // doc comment.
    clear_analysis_results(ui);
    // Captured before dropping the borrow below -- `refresh_all_now` needs to
    // borrow `state` itself, so `st` cannot still be held across that call.
    let loaded_asc_filename = st.asc_filename.clone();
    drop(st);
    refresh_all_now(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        true,
    );
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
    ui.set_loaded_design_name(loaded_asc_filename.unwrap_or_default().into());
    if loaded.used_placeholder {
        show_toast(
            ui,
            "Loaded a reconstructed schedule -- mast distances are \
             placeholders (no attached .asc file was found); adjust \
             masts before exporting.",
            // Every mast on this design is a fabricated 0.0. A cutter who
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
    // Suggests the built-in material whose n_D is nearest the schedule RI (within
    // 0.01) -- never applied automatically, only offered as a banner the user can
    // dismiss or accept.
    if let Some((name, ri)) = nearest_built_in_material(schedule_ri, 0.01) {
        // Unrelated pre-existing build break fixed in passing (not part of this
        // session's GPU/display-thread work): the suggestion text is built BEFORE
        // `name` moves into `set_material_suggestion_name` below, not after.
        let suggestion_text = format!("Set material to {name} (RI {ri:.4})?");
        ui.global::<EditorModel>()
            .set_material_suggestion_name(name.into());
        ui.global::<EditorModel>()
            .set_material_suggestion_text(suggestion_text.into());
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
        stall_guard("on_load_selected", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                show_toast(&ui, "No diagram selected to load.", "error");
                return;
            }

            if state.borrow().is_dirty() {
                state.borrow_mut().pending_unsaved_action =
                    Some(PendingUnsavedAction::LoadSelected);
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
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
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
/// Just clears the banner; mirrors the Accept path's own clearing above.
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
                // Undo can move any tier (or a whole structural AddTier/RemoveTier
                // change), so an edit whose blast radius isn't tracked precisely forces
                // a full solve rather than guessing a `dirty` set -- and, for the same
                // reason, no cached mast is trusted for.
                refresh_editor_panel_stale(
                    &ui,
                    &render_ctx,
                    &st,
                    &(0..st.design.tiers.len()).collect(),
                );
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
                // Same "blast radius unknown, trust nothing cached" reasoning as
                // `setup_undo_callback`'s matching arm.
                refresh_editor_panel_stale(
                    &ui,
                    &render_ctx,
                    &st,
                    &(0..st.design.tiers.len()).collect(),
                );
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
                    // The preform reshapes the bounding planes but never moves a
                    // tier's own mast -- no tier is dirty.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
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
/// SetGirdleDiameterMm`] then [`Edit::SetMaterial`] -- two separate,
/// independently-undoable edits would cost a single Apply Yield Inputs click two
/// Ctrl+Z presses to undo, and could be undone out of order.
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
                    // Neither `SetGirdleDiameterMm` nor `SetMaterial` names a tier
                    // index (both design-wide), so -- like `Edit::SetPreform`/
                    // `Edit::SetMeta` elsewhere in this module -- `EditorState::apply`
                    // cannot fail on this `Batch` in practice; kept `let _ =`.
                    let _ = st.apply(Edit::Batch(vec![
                        Edit::SetGirdleDiameterMm { girdle_diameter_mm },
                        Edit::SetMaterial { material },
                    ]));
                    // Girdle diameter and material alone never move a tier's own
                    // mast -- no tier is dirty.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
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

/// "Apply Y-Offset". `Design::preform_y_offset`/
/// `Edit::SetPreformYOffset` are real, applied, mast-preserving edits, but nothing in
/// this app ever set them away from `0.0` until this callback -- see `EditorModel.
/// preform_y_offset_mm`'s own doc comment (`ui/models/editor.slint`) for why the field
/// is typed in millimetres (the cutter's own rough measurement) rather than model
/// units. Converts through the design's own mm-per-unit factor
/// (`Design::yield_report(&solved).mm_per_unit`, the same anchor
/// [`super::super::state::preform_mm_texts`] already reads) before building the
/// `Edit`; toasts instead of applying anything when the field does not parse, or
/// when no girdle diameter/solve has anchored that factor yet.
pub(in crate::gui::editor) fn setup_apply_preform_y_offset_callback(
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
        .on_apply_preform_y_offset(move |mm_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            stall_guard("on_apply_preform_y_offset", || {
                let trimmed = mm_text.trim();
                let Ok(mm) = trimmed.parse::<f64>() else {
                    show_toast(
                        &ui,
                        &format!("Y-offset '{trimmed}' is not a number."),
                        "error",
                    );
                    return;
                };
                if !mm.is_finite() {
                    show_toast(&ui, "Y-offset must be a finite number.", "error");
                    return;
                }
                let mut st = state.borrow_mut();
                // The UI thread never solves -- this reuses the last
                // background/synchronous solve's cached masts (same cache
                // `deep_solve`'s own setup reads) instead of
                // a fresh `Design::solve()` while `state.borrow_mut()` is held. When
                // no cached solve matches this design's current tier count (a design
                // that has never solved yet, or an edit landed since the cache was
                // last populated), this asks for one explicitly rather than solving
                // inline.
                let Some(solved) = auto_solve::solid_last_solved()
                    .and_then(|cache| {
                        cache
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    })
                    .filter(|solved| solved.len() == st.design.tiers.len())
                else {
                    show_toast(&ui, "Solve first, then set a Y-offset.", "error");
                    return;
                };
                let Some(mm_per_unit) = st.design.yield_report(&solved).mm_per_unit else {
                    show_toast(
                        &ui,
                        "Set a girdle diameter in the Yield tab before setting a Y-offset in \
                     millimetres.",
                        "error",
                    );
                    return;
                };
                let y_offset = mm / mm_per_unit;
                // Mast-preserving (see `Edit::SetPreformYOffset`'s own doc comment) --
                // no tier is dirty, same reasoning `setup_apply_preform_callback` uses.
                //
                // A failed apply left the field showing a value that was never
                // actually recorded, with nothing telling the cutter why.
                // `Edit::SetPreformYOffset` is mast-preserving and takes no tier index,
                // so `EditorState::apply` can only fail here on an internal invariant
                // violation, not a cutter mistake -- still surfaced rather than assumed
                // impossible.
                if let Err(e) = st.apply(Edit::SetPreformYOffset { y_offset }) {
                    show_toast(&ui, &e.to_string(), "error");
                    return;
                }
                refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
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
        });
}

/// "Apply Cheater Offset". Sets (or, for a blank field,
/// clears) one tier's own cheater/azimuth offset via [`Edit::SetCheaterOffset`], a
/// cutting-sheet annotation with no geometric effect (see that variant's own doc
/// comment) -- so unlike every other tier edit in this module, this never calls
/// [`submit_preview_replan`]: there is nothing for the solid preview to redraw.
pub(in crate::gui::editor) fn setup_apply_cheater_offset_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_apply_cheater_offset(move |index: i32, text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let trimmed = text.trim();
            let offset_deg = if trimmed.is_empty() {
                None
            } else {
                match trimmed.parse::<f64>() {
                    Ok(value) if value.is_finite() => Some(value),
                    _ => {
                        show_toast(
                            &ui,
                            &format!("Cheater offset '{trimmed}' is not a number."),
                            "error",
                        );
                        return;
                    }
                }
            };
            let mut st = state.borrow_mut();
            // `index` names a real row when the field was focused, but the tier list
            // can change while a cutter is still typing in this field -- surfaced
            // rather than silently dropping a stale-index edit.
            if let Err(e) = st.apply(Edit::SetCheaterOffset { index, offset_deg }) {
                show_toast(&ui, &e.to_string(), "error");
                return;
            }
            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
        });
}

/// "Apply Tier Note". Sets (or, for a
/// blank field, clears) one tier's own cutter-authored free-text note via
/// [`Edit::SetTierNote`], a cutting-sheet annotation with no geometric effect
/// (see that variant's own doc comment) -- so exactly like
/// [`setup_apply_cheater_offset_callback`], this never calls
/// [`submit_preview_replan`]: there is nothing for the solid preview to redraw.
///
/// Unlike the cheater offset (a number that fails to parse), any text is a
/// valid note, so there is no error-toast branch here -- blank just clears it.
pub(in crate::gui::editor) fn setup_apply_tier_note_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_apply_tier_note(move |index: i32, text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let trimmed = text.trim();
            let note = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            let mut st = state.borrow_mut();
            // Same stale-index reasoning as `setup_apply_cheater_offset_callback` just above.
            if let Err(e) = st.apply(Edit::SetTierNote { index, note }) {
                show_toast(&ui, &e.to_string(), "error");
                return;
            }
            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
        });
}

/// Replaces the design's title (the
/// first `H` header), any further header lines, footnotes, and the index wheel's
/// zero-tooth reference angle as ONE undoable [`Edit::SetMeta`], mirroring
/// [`setup_apply_yield_inputs_callback`]'s own "one form, one undo step" shape.
/// `title`/`extra_headers`/`footnotes` are each split on `';'` into individual
/// header/footnote lines (`extra_headers` following the title as further `H`
/// lines) -- see `EditorModel.design_title`'s own doc comment (`ui/models/
/// editor.slint`) for the exact field layout this mirrors. An unparseable
/// `gear_ref` toasts instead of applying anything.
pub(in crate::gui::editor) fn setup_apply_design_meta_callback(
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
    ui.global::<EditorModel>().on_apply_design_meta(
        move |title: SharedString,
              extra_headers: SharedString,
              footnotes: SharedString,
              gear_ref: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let split_lines = |text: &str| -> Vec<String> {
                text.split(';')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            };
            let trimmed_title = title.trim();
            let mut headers = Vec::new();
            if !trimmed_title.is_empty() {
                headers.push(trimmed_title.to_string());
            }
            headers.extend(split_lines(&extra_headers));
            let footnotes = split_lines(&footnotes);
            let gear_ref_trimmed = gear_ref.trim();
            let Ok(gear_reference_angle) = gear_ref_trimmed.parse::<f64>() else {
                show_toast(
                    &ui,
                    &format!("Gear reference angle '{gear_ref_trimmed}' is not a number."),
                    "error",
                );
                return;
            };
            if !gear_reference_angle.is_finite() {
                show_toast(
                    &ui,
                    "Gear reference angle must be a finite number.",
                    "error",
                );
                return;
            }
            let mut st = state.borrow_mut();
            // Mast-preserving (see `Edit::SetMeta`'s own doc comment) -- no tier is
            // dirty, but the gear reference angle can rotate the rendered index
            // wheel, so the solid preview still needs a fresh (non-blocking) replan.
            // `SetMeta` names no tier index (design-wide headers/footnotes/gear
            // angle only), so -- like `Edit::SetPreform` above -- `EditorState::apply`
            // cannot fail on it in practice; kept `let _ =`, not surfaced, for the
            // same reason.
            let _ = st.apply(Edit::SetMeta {
                headers,
                footnotes,
                gear_reference_angle,
            });
            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
            submit_preview_replan(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &st,
                BTreeSet::new(),
                false,
            );
        },
    );
}

/// "Add Tier" / "Save Tier": parses the form and applies it through
/// `EditorState::apply` as `AddTier` (index `< 0`, "new tier" mode -- appended at the
/// end) or `ModifyTier` (an existing row's index).
/// Every OTHER tier's own name token (`ConstraintTier::names()`, split on `/`),
/// with the tier at `excluded_index` left out. Feeds `loading::TierFormFields::
/// other_tier_names` so `parse_tier_form` can reject name collisions. `excluded_index
/// < 0` (a brand-new tier) excludes nothing, since there is no existing row to exempt.
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

/// Selects and reveals the tier `setup_save_tier_callback`'s `AddTier` branch
/// just added, then names it in a toast. Setting `EditorModel.selected_tier_index`
/// is enough to reveal: `editor_view.slint`'s `changed tracked_selected_tier_index`
/// re-seeds the inspector form AND calls `tier_table.focus_row`, which scrolls the
/// new row into view.
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

/// [`setup_save_tier_callback`]'s target-parsing step: parses
/// `constraint_kind`/`constraint_text` (the SAME two fields
/// [`loading::parse_tier_form`] already validated for its own 0/1/2 kinds)
/// via [`loading::parse_tier_target`] into the [`TierTarget`] the form's
/// Meets combo authored, if any -- kinds `3`/`4`/`5` ("cut to depth"/"girdle
/// thickness"/"table width"); `Ok(None)` for every other kind, since
/// `parse_tier_form` already turned those into a plain `ConstraintTier` with
/// no target at all.
///
/// Reports any parse error exactly the way a `parse_tier_form` failure would
/// (`report_tier_form_error`, classified via `tier_form_error_field`) and
/// returns `Err(())` -- the caller reads that as "already reported, apply
/// nothing" and bails out. `Result<Option<TierTarget>, ()>` rather than the
/// more obvious `Option<Option<TierTarget>>` purely to dodge clippy's
/// `option_option` lint; the `()` carries no information beyond "stop."
/// Split out of `setup_save_tier_callback` purely to keep that function
/// under clippy's `too_many_lines` lint.
fn parse_tier_target_reporting(
    ui: &MainWindow,
    constraint_kind: i32,
    constraint_text: &str,
) -> Result<Option<TierTarget>, ()> {
    loading::parse_tier_target(constraint_kind, constraint_text).map_err(|e| {
        report_tier_form_error(ui, &e, tier_form_error_field(&e));
    })
}

/// [`tier_save_edit`], extended for depth/girdle-thickness/table-width targets:
/// wraps its edit in an
/// [`Edit::Batch`] with [`Edit::SetTierTarget`] whenever `target` is `Some`,
/// or whenever the tier CURRENTLY at `index` already carries one -- which
/// must then be explicitly cleared (`Edit::SetTierTarget { target: None }`)
/// the moment the cutter saves with a plain Meets kind (0/1/2), or it would
/// silently keep resolving against a target the form no longer shows. A
/// brand-new tier (`index < 0`) never has one to clear. Split out of
/// [`setup_save_tier_callback`] purely to keep that function under clippy's
/// `too_many_lines` lint.
fn tier_save_edit_with_target(
    ui: &MainWindow,
    st: &EditorState,
    index: i32,
    tier: indicatrix_cut_core::ConstraintTier,
    target: Option<TierTarget>,
) -> (usize, Edit) {
    let had_target = usize::try_from(index)
        .ok()
        .and_then(|i| st.design.tier_target(i))
        .is_some();
    let (dirty_index, edit) = tier_save_edit(ui, st, index, tier);
    let edit = if target.is_some() || had_target {
        Edit::Batch(vec![
            edit,
            Edit::SetTierTarget {
                index: dirty_index,
                target,
            },
        ])
    } else {
        edit
    };
    (dirty_index, edit)
}

/// [`setup_save_tier_callback`]'s preamble: the current row's carried-through
/// `imported_meet`/`original_notes` (for an existing tier), the gear tooth
/// count, and every OTHER tier's own name tokens -- everything [`loading::
/// parse_tier_form`] needs beyond the form's own five text/enum fields. Split
/// out purely to keep that function under clippy's function-length lint.
struct SaveTierFormContext {
    imported_meet: Option<MeetConstraint>,
    original_notes: Option<String>,
    gear_teeth_abs: u32,
    other_tier_names: Vec<String>,
}

/// Builds [`SaveTierFormContext`] from a short-lived immutable borrow of
/// `state` -- dropped before this returns, so the caller's later
/// `state.borrow_mut()` never races it.
fn save_tier_form_context(state: &Rc<RefCell<EditorState>>, index: i32) -> SaveTierFormContext {
    let st = state.borrow();
    // An existing row keeps its own `imported_meet` and the `.asc` file's
    // original `G` note across this save -- looked up here, before the
    // caller's mutable borrow, so editing an imported tier's name/angle/indices
    // never silently drops what the file claimed it meets, nor the note a
    // cutter reads while cutting it.
    let (imported_meet, original_notes) = (index >= 0)
        .then(|| {
            let tier = st.design.tiers.get(usize::try_from(index).ok()?)?;
            Some((tier.imported_meet.clone(), tier.original_notes.clone()))
        })
        .flatten()
        .unwrap_or_default();
    SaveTierFormContext {
        imported_meet,
        original_notes,
        gear_teeth_abs: st.design.meta.gear_teeth_abs(),
        other_tier_names: other_tier_names_excluding(&st, index),
    }
}

/// [`apply_tier_save_success`]'s outcome fields, bundled purely to keep that
/// function under clippy's too-many-arguments lint.
struct TierSaveOutcome {
    /// The form's own `index` argument: negative for a fresh `AddTier`, the
    /// tier's own index for a `ModifyTier`.
    index: i32,
    /// The saved tier's actual index in `st.design.tiers` after the edit applied.
    dirty_index: usize,
    /// [`non_integral_index_warning`]'s verdict for the saved indices, if any.
    non_integral_warning: Option<String>,
}

/// [`setup_save_tier_callback`]'s success arm: clears the form's stale error state,
/// replans the preview for `outcome.dirty_index`, reveals a freshly added row, and
/// surfaces the non-integral-index warning (if any). Split out purely to keep that
/// function under clippy's function-length lint.
///
/// Takes `st` by value (not `&mut EditorState`) so the `AddTier` branch can `drop`
/// it before calling back into Slint through `select_and_announce_added_tier` --
/// exactly the same borrow-scope this body had inlined, just made explicit at the
/// call boundary.
fn apply_tier_save_success(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    st: RefMut<'_, EditorState>,
    outcome: TierSaveOutcome,
) {
    let TierSaveOutcome {
        index,
        dirty_index,
        non_integral_warning,
    } = outcome;
    // A successful save means the form is valid again, so this clears whatever the
    // last save's parse/validation error left behind.
    let model = ui.global::<EditorModel>();
    model.set_tier_form_error("".into());
    model.set_tier_form_error_field("".into());
    // `AddTier` changes the tier count, so the alignment
    // check falls back to a full solve regardless of
    // `dirty`; for `ModifyTier` this one index is exactly
    // what changed.
    refresh_editor_panel_stale(ui, render_ctx, &st, &BTreeSet::from([dirty_index]));
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        &st,
        BTreeSet::from([dirty_index]),
        false,
    );
    // Selects and reveals the row just added -- an `AddTier`-only branch, since a
    // `ModifyTier` save is already on the row it edited. See
    // [`select_and_announce_added_tier`]'s own doc comment for why setting
    // `selected_tier_index` alone is enough to reveal it too.
    if index < 0 {
        let label = added_tier_label(&st, dirty_index);
        drop(st);
        select_and_announce_added_tier(ui, dirty_index, label);
    }
    // The non-integral-index warning is shown last (after the "Added <label>" toast
    // above, when this was a new tier) so it is the one left on screen -- the single
    // toast slot keeps only the most recent call, and a possibly-unintentional
    // fractional index is more worth a cutter's attention than a bare confirmation
    // that the save succeeded.
    if let Some(warning) = non_integral_warning {
        show_toast(ui, &warning, "info");
    }
}

/// The Tier form's Save action: parses the form via [`loading::parse_tier_form`],
/// applies the resulting [`Edit`] through [`EditorState::apply`], and refreshes the
/// preview and panel on success.
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
            let SaveTierFormContext {
                imported_meet,
                original_notes,
                gear_teeth_abs,
                other_tier_names,
            } = save_tier_form_context(&state, index);
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
                    // The form's Meets combo also carries three target kinds that
                    // `loading::parse_tier_form` above only turns into a `ScaleReference(0.0)`
                    // placeholder -- see `parse_tier_target_reporting`'s own doc comment.
                    let Ok(target) =
                        parse_tier_target_reporting(&ui, constraint_kind, &constraint_text)
                    else {
                        return;
                    };
                    // A brand-new tier saved with a blank Name
                    // field would otherwise stay unnamed and un-meetable (
                    // `ConstraintTier::names()` returns nothing for an empty name) --
                    // auto-name it here, matching what Duplicate already does for
                    // its own copies. Only for a fresh `AddTier` (`index < 0`): an
                    // existing tier's name was either already set or the cutter just
                    // deliberately blanked it, neither of which this should override.
                    if index < 0 && tier.name.is_empty() {
                        tier.name = next_free_block_name(tier.angle_deg, &other_tier_names);
                    }
                    // Captured before `tier` is moved into
                    // `tier_save_edit` below -- see `non_integral_index_warning`'s
                    // own doc comment for why this warns rather than rejects.
                    let non_integral_warning = non_integral_index_warning(&tier.indices);
                    let mut st = state.borrow_mut();
                    // Preserve the row's own `detached` set across a save --
                    // `parse_tier_form` always returns an empty one, and without this
                    // a rename/angle/index edit would silently re-link a deliberately
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
                            "constraint",
                        );
                        return;
                    }
                    let (dirty_index, edit) =
                        tier_save_edit_with_target(&ui, &st, index, tier, target);
                    match st.apply(edit) {
                        Ok(()) => {
                            apply_tier_save_success(
                                &ui,
                                &render_ctx,
                                &preview_state,
                                &solid_last_solved,
                                st,
                                TierSaveOutcome {
                                    index,
                                    dirty_index,
                                    non_integral_warning,
                                },
                            );
                        }
                        Err(e) => {
                            let message = e.to_string();
                            let field = tier_form_error_field(&message);
                            report_tier_form_error(&ui, &message, field);
                        }
                    }
                }
                Err(e) => report_tier_form_error(&ui, &e, tier_form_error_field(&e)),
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
                    // Tier count changed -- the alignment check falls back to a full solve.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    adjust_selection_after_remove(&ui, index);
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
                        // Committing back the SAME value is a
                        // real interaction boundary (the cutter opened the cell,
                        // looked, and closed it) -- end any scroll-wheel nudge
                        // coalescing run in progress rather than leaving it open
                        // for a later, unrelated nudge to merge into.
                        st.history.end_coalesce_run();
                        // Without this toast, a committed-but-unchanged edit would
                        // be silent and indistinguishable from a dropped one.
                        show_toast(&ui, "No change.", "info");
                        return;
                    }
                    let mut tier = current.clone();
                    tier.angle_deg = angle_deg;
                    match st.apply(Edit::ModifyTier { index, tier }) {
                        Ok(()) => {
                            refresh_editor_panel_stale(
                                &ui,
                                &render_ctx,
                                &st,
                                &BTreeSet::from([index]),
                            );
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
///
/// `History::apply_coalescing`
/// already merged the UNDO step for a fast nudge burst, but every tick still ran
/// its own apply/refresh/replan cycle on the UI thread (each of those clones the
/// whole `Design` twice -- `view::submit_preview_replan_for`'s own doc comment).
/// This now posts an [`edit_intent::EditIntent::NudgeAngle`] into a queue this
/// function builds once, and [`apply_nudge_intent`] (the actual apply/clamp/
/// refresh/replan/toast logic, moved out of this closure unchanged) runs at most
/// once per 16ms drain tick, against the SUMMED `delta_deg` of everything posted
/// since the last tick -- see [`edit_intent::EditIntentQueue`]'s own doc comment.
pub(in crate::gui::editor) fn setup_nudge_angle_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let intent_queue = {
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        edit_intent::EditIntentQueue::new(move |intent| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let edit_intent::EditIntent::NudgeAngle { targets, delta_deg } = intent else {
                return;
            };
            apply_nudge_intent(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &targets,
                delta_deg,
            );
        })
    };
    let state = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_nudge_angle(move |anchor_index: i32, delta_deg: f32| {
            stall_guard("on_nudge_angle", || {
                let Ok(anchor_index) = usize::try_from(anchor_index) else {
                    return;
                };
                let st = state.borrow();
                let is_multi_target =
                    st.multi_selected.len() > 1 && st.multi_selected.contains(&anchor_index);
                let targets: Vec<usize> = if is_multi_target {
                    st.multi_selected.iter().copied().collect()
                } else {
                    vec![anchor_index]
                };
                drop(st);
                intent_queue.post(edit_intent::EditIntent::NudgeAngle {
                    targets,
                    delta_deg: f64::from(delta_deg),
                });
            });
        });
}

/// [`setup_nudge_angle_callback`]'s actual apply/clamp/refresh/replan/toast work,
/// run once per drained [`edit_intent::EditIntent::NudgeAngle`] against the
/// SUMMED `delta_deg` of the whole coalesced burst -- see that function's own doc
/// comment. `targets`/`delta_deg` are read fresh against the design's CURRENT
/// angle at drain time (not whatever it was when the first tick of the burst
/// posted), so [`clamp_nudge_to_side`]'s zero-crossing clamp always judges the
/// real, final position, exactly as if the summed delta had been applied in one
/// step -- which, after this change, it is.
fn apply_nudge_intent(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    targets: &[usize],
    delta_deg: f64,
) {
    let mut st = state.borrow_mut();
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
    let key = angle_nudge_coalesce_key(targets);
    match st.apply_coalescing(Edit::RetargetAngles { changes }, key) {
        Ok(()) => {
            let dirty: BTreeSet<usize> = targets.iter().copied().collect();
            refresh_editor_panel_stale(ui, render_ctx, &st, &dirty);
            submit_preview_replan(
                ui,
                render_ctx,
                preview_state,
                solid_last_solved,
                &st,
                dirty,
                false,
            );
            // The angle's sign is the only thing that says which block a
            // tier belongs to (`clamp_nudge_to_side`'s own doc comment), so
            // a nudge that would cross zero is clamped there instead of
            // silently reclassifying the tier -- explain the stop instead
            // of leaving it looking like the nudge simply refused to move.
            if !clamped_labels.is_empty() {
                show_toast(
                    ui,
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
        Err(e) => show_toast(ui, &e.to_string(), "error"),
    }
}

/// A row's "Duplicate" button and the tier list's Ctrl+D: inserts a copy of the
/// named tier (name suffixed `'`, same indices/angle/constraint/detached set)
/// immediately AFTER the source row as a new [`Edit::AddTier`] through
/// `EditorState::apply` -- not appended at the end, since cut order is meaningful
/// (`Edit::AddTier` already supports an arbitrary insertion index, so this passes
/// the source row's own position plus one rather than appending at the end) --
/// then moves the tier-list selection to the copy.
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
            // Uses a counted `" (N)"` suffix instead of appending an apostrophe --
            // see `unique_duplicate_name`'s own doc comment for why an apostrophe
            // scheme piles up unreadable "P1''''" names AND silently creates a
            // duplicate name that a meet resolver secretly binds to the FIRST tier
            // holding it.
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
                    // `AddTier` changes the tier count -- same full-solve fallback
                    // `setup_save_tier_callback`'s own `AddTier` path uses.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::from([new_index]));
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
                    // Names the change instead of leaving a
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

/// [`setup_generate_step_series_callback`]'s form-parsing half -- everything the
/// "Generate steps" form needs turned into real values BEFORE
/// [`ConstraintTier::step_series`] can be called, except the indices list, which
/// the caller parses separately via [`loading::parse_index_list`] (that parse
/// also needs `state`'s own `gear_teeth_abs`, not available here). An empty
/// anchor field means [`MeetConstraint::MeetExisting`] (the same "blank means use
/// whatever's already anchored" convention -- the caller is relying on an anchor
/// elsewhere in the design); a
/// non-empty one is parsed and pinned via [`MeetConstraint::ScaleReference`].
///
/// # Errors
///
/// A message naming the offending field, ready to show the cutter.
fn parse_step_series_form(
    start_angle_text: &str,
    angle_step_text: &str,
    count: i32,
    anchor_text: &str,
) -> Result<(f64, f64, usize, MeetConstraint), String> {
    let start_angle: f64 = start_angle_text
        .trim()
        .parse()
        .map_err(|_| format!("Start angle '{start_angle_text}' is not a number."))?;
    let angle_step: f64 = angle_step_text
        .trim()
        .parse()
        .map_err(|_| format!("Angle step '{angle_step_text}' is not a number."))?;
    if !start_angle.is_finite() || !angle_step.is_finite() {
        return Err("Start angle and angle step must be finite numbers.".to_string());
    }
    let count = usize::try_from(count)
        .ok()
        .filter(|&count| count >= 1)
        .ok_or_else(|| "Tier count must be a positive whole number.".to_string())?;
    let anchor_text = anchor_text.trim();
    let first_constraint = if anchor_text.is_empty() {
        MeetConstraint::MeetExisting
    } else {
        let anchor: f64 = anchor_text
            .parse()
            .map_err(|_| format!("Anchor '{anchor_text}' is not a number."))?;
        if !anchor.is_finite() {
            return Err("Anchor must be a finite number.".to_string());
        }
        MeetConstraint::ScaleReference(anchor)
    };
    Ok((start_angle, angle_step, count, first_constraint))
}

/// "Generate steps": builds `count`
/// tiers via [`ConstraintTier::step_series`] and applies them as one
/// [`Edit::Batch`] of [`Edit::AddTier`]s, appended after the design's current
/// last tier. Mirrors `editor_tier_table.slint`'s own call site's argument
/// order: name prefix, start-angle text, angle-step text, tier count, a
/// comma-separated index list shared by every generated tier (parsed the same
/// way [`setup_save_tier_callback`]'s own indices field is, via
/// [`loading::parse_index_list`]), and an optional anchor -- see
/// [`parse_step_series_form`]'s own doc comment for what an empty one means.
/// Previously this callback did not exist at all (the Slint side called
/// straight into nothing); see [`setup_toggle_detach_callback`]'s own doc
/// comment for why it is registered from there rather than from
/// `gui::editor::mod::setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_generate_step_series_callback(
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
    ui.global::<EditorModel>().on_generate_step_series(
        move |name_prefix: SharedString,
              start_angle_text: SharedString,
              angle_step_text: SharedString,
              count: i32,
              indices_text: SharedString,
              anchor_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (start_angle, angle_step, count, first_constraint) = match parse_step_series_form(
                &start_angle_text,
                &angle_step_text,
                count,
                &anchor_text,
            ) {
                Ok(parsed) => parsed,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let mut st = state.borrow_mut();
            let gear_teeth_abs = st.design.meta.gear_teeth_abs();
            let indices = match loading::parse_index_list(&indices_text, gear_teeth_abs) {
                Ok(indices) => indices,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let tiers = ConstraintTier::step_series(
                &name_prefix,
                start_angle,
                angle_step,
                count,
                &indices,
                &first_constraint,
            );
            let start_index = st.design.tiers.len();
            let edits: Vec<Edit> = tiers
                .into_iter()
                .enumerate()
                .map(|(offset, tier)| Edit::AddTier {
                    index: start_index + offset,
                    tier,
                })
                .collect();
            let added_count = edits.len();
            match st.apply(Edit::Batch(edits)) {
                Ok(()) => {
                    let dirty: BTreeSet<usize> = (start_index..start_index + added_count).collect();
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        dirty,
                        false,
                    );
                    drop(st);
                    show_toast(&ui, &format!("Generated {added_count} tier(s)."), "info");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );
}

/// "Mirror tier to other block" :
/// duplicates the tier at `tier_index` to the opposite block via
/// [`ConstraintTier::mirrored_to_other_block`] (angle negated, same
/// indices/constraint/detached set, name suffixed by `name_suffix`) and applies
/// it as one [`Edit::AddTier`], appended after the design's current last tier.
/// A silent no-op for an out-of-range `tier_index`, the same guard
/// [`setup_duplicate_tier_callback`] uses for its own source lookup.
pub(in crate::gui::editor) fn setup_mirror_tier_to_other_block_callback(
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
    ui.global::<EditorModel>().on_mirror_tier_to_other_block(
        move |tier_index: i32, name_suffix: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let Some(source) = st.design.tiers.get(tier_index) else {
                return;
            };
            let mirrored = source.mirrored_to_other_block(&name_suffix);
            let mirrored_label = mirrored.name.clone();
            let new_index = st.design.tiers.len();
            match st.apply(Edit::AddTier {
                index: new_index,
                tier: mirrored,
            }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::from([new_index]));
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
                    show_toast(&ui, &format!("Mirrored to {mirrored_label}"), "info");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );
}

/// Generates a name for [`setup_duplicate_tier_callback`] that is guaranteed not
/// to collide with any name in `existing_names`, by counting up a `" (N)"` suffix
/// (`"P1 (2)"`, `"P1 (3)"`, ...) rather than appending an apostrophe: the
/// apostrophe scheme produced an unreadable "P1''''" pile on a second or third
/// duplicate of the same tier and, worse, silently created a duplicate name that
/// `MeetNameResolver::name_match` (`indicatrix::geometry::meet_solver::names`)
/// resolves by binding to whichever tier holds it FIRST -- so a duplicate's stale
/// copy of a popular name could silently steal every future `MeetNamed` reference
/// meant for the original.
///
/// [`split_duplicate_suffix`] first removes a trailing `" (N)"` a PREVIOUS call to
/// this same function already appended, so duplicating "P1 (2)" produces
/// "P1 (3)" rather than nesting into "P1 (2) (2)". An empty source name (an
/// unnamed tier) falls back to the base "Tier" rather than producing a bare
/// "(2)" -- giving the duplicate a real name is also what lets it become a
/// `MeetNamed` target, since `ConstraintTier::names` never resolves a name from an
/// empty string.
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
/// blank Name field ([`unique_duplicate_name`] above does not cover this case --
/// that one only ever runs against an already-named
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
                let facet_map = facet_map_from_aligned_solve(&st.design);
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

/// The tier list's Shift+click: replaces
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
                let facet_map = facet_map_from_aligned_solve(&st.design);
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
/// [`setup_tier_mirror_indices_callback`]/[`setup_generate_step_series_callback`]/
/// [`setup_mirror_tier_to_other_block_callback`] (the last two share this one
/// entry point) -- `gui::editor::mod::setup_editor_callbacks` has one fixed call
/// site per `setup_*` function name, so a genuinely new callback can only be wired
/// up by piggybacking its own `setup_*` call onto an EXISTING call site that
/// already receives every argument it needs; this is the one existing call already
/// carrying `render_ctx`/`preview_state`/`solid_last_solved` alongside `ui`/`state`.
pub(in crate::gui::editor) fn setup_toggle_detach_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // Stashes the shared `RenderContext` handle for
    // `auto_solve::render_ctx()` -- this is one of several `setup_*_callback`s
    // already given the `Arc` directly by `gui::editor::mod::setup_editor_
    // callbacks`, and it runs once here, synchronously, before
    // `setup_editor_callbacks` returns and the
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
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx_toggle,
                        &st,
                        &BTreeSet::from([index]),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx_toggle,
                        &preview_state_toggle,
                        &solid_last_solved_toggle,
                        &st,
                        BTreeSet::from([index]),
                        false,
                    );
                    // Names the change and which way it went
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
    //    // "wiring point" reason given above.
    setup_adopt_all_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_adopt_selected_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_pin_to_mast_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_highlight_tooth_callback(ui, state);
    //    // reason given above.
    setup_generate_step_series_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    setup_mirror_tier_to_other_block_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
}

/// Bulk-adopts every tier that still has an unadopted
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
/// Wired to `EditorModel.adopt_all()` (declared in `ui/models/editor.slint`) --
/// `editor_tier_table.slint`'s own "Adopt all" button calls it.
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
        stall_guard("on_adopt_all", || {
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
                    drop(st);
                    refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
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
    });
}

/// Like [`setup_adopt_all_callback`] above, but
/// restricted to [`EditorState::multi_selected`] instead of every tier in the
/// design -- freeing one Ctrl-clicked group for Optimize without also disturbing
/// every other still-pinned tier. Applied as a single [`Edit::Batch`] for the
/// identical "one undo step, one re-solve" reason the all-tiers version is. A
/// silent no-op when the selection is empty or none of it has an
/// `imported_meet` left to adopt.
///
/// Wired to `EditorModel.adopt_selected()` (declared in `ui/models/editor.slint`)
/// -- `editor_tier_table.slint`'s own "Adopt sel." button calls it.
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
        stall_guard("on_adopt_selected", || {
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
                    drop(st);
                    refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
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
    });
}

/// The inverse of "Adopt" -- freezes a tier's CURRENT solved
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
/// Wired to `EditorModel.pin_to_mast(int)` (declared in `ui/models/editor.slint`)
/// -- the tier table's own per-row "Pin" button calls it.
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
            stall_guard("on_pin_to_mast", || {
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
                        // Pin freezes a tier at exactly the mast the last real solve
                        // already
                        // produced for it (this function's own doc comment), so a full
                        // `refresh_all` re-solve is redundant work for the SAME reason
                        // `setup_adopt_meet_callback`'s own doc comment gives -- and,
                        // unlike a wholesale replace, this touches only one tier.
                        // Pushes the immediate UI update from the CACHED solve
                        // (`refresh_editor_panel_stale` with `dirty = {index}`, which
                        // reads every OTHER row from the last-solved cache -- see
                        // `push_stale_content`'s own doc comment) and lets the
                        // background dirty-set replan confirm it via `resolve_dirty`
                        // instead of re-solving the whole design synchronously here.
                        let dirty: BTreeSet<usize> = std::iter::once(index).collect();
                        refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                        submit_preview_replan(
                            &ui,
                            &render_ctx,
                            &preview_state,
                            &solid_last_solved,
                            &st,
                            dirty,
                            false,
                        );
                        show_toast(
                            &ui,
                            &format!("Pinned tier {} at {mast:.4}", index + 1),
                            "info",
                        );
                    }
                    Err(e) => show_toast(&ui, &e.to_string(), "error"),
                }
            });
        });
}

/// A clicked index-wheel tooth
/// (`solid_preview::diagram_wiring::setup_diagram_hover_and_click_callbacks`'s
/// own miss branch, which already reports the id through
/// `SolidPreviewModel.diagram_clicked_tooth` -- see that function's own doc
/// comment naming this callback as the intended consumer) now highlights every
/// facet sharing that tooth, using the reverse of the lookup
/// [`setup_toggle_multi_select_callback`] already does the forward direction of:
/// that one turns a set of TIER indices into their member facet ids via
/// `FacetMap::facets_of_tier`; this one turns one GEAR TOOTH into every facet id
/// whose own `FacetMap::index_on_gear` matches it, by scanning
/// `0..FacetMap::facet_count()`, since `facet_map.rs` exposes no dedicated
/// tooth-to-facets index. Reuses
/// [`FacetOverlay::multi_selected`] for the tint rather than adding a new overlay
/// field, which would need a `facet_map.rs`/`preview_state.rs` change.
///
/// `tooth < 0` (nothing hit, `SolidPreviewModel.diagram_clicked_tooth`'s own
/// default) clears the highlight instead of leaving a stale one from a previous
/// click.
///
/// Wired to `EditorModel.highlight_tooth(int)` (declared in
/// `ui/models/editor.slint`) -- `editor_tier_table.slint`'s own
/// `tracked_clicked_tooth` mirror (see that property's doc comment for why a
/// mirrored property, not a `changed` handler on the global itself, is what calls
/// it) invokes it whenever `SolidPreviewModel.diagram_clicked_tooth` changes.
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
            let facet_map = facet_map_from_aligned_solve(&st.design);
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
/// Wired to `EditorModel.facet_remove(int, float)` (declared in
/// `ui/models/editor.slint`; tier index, index-wheel position) -- the per-facet
/// index chips that call it live in `editor_inspector.slint`.
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
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
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
/// Wired to `EditorModel.facet_toggle_detach(int, float)` (declared in
/// `ui/models/editor.slint`) -- called from the same per-facet chips in
/// `editor_inspector.slint` as [`setup_facet_remove_callback`].
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
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
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
/// Wired to `EditorModel.facet_add(int, float)` (declared in
/// `ui/models/editor.slint`) -- called from the same per-facet chips in
/// `editor_inspector.slint` as [`setup_facet_remove_callback`].
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
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
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
/// Wired to `EditorModel.tier_rotate_indices(int, float)` (declared in
/// `ui/models/editor.slint`; tier index, teeth to rotate by) -- the inspector's
/// "rotate this tier" stepper in `editor_inspector.slint` calls it.
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
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
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
/// Wired to `EditorModel.tier_mirror_indices(int)` (declared in
/// `ui/models/editor.slint`) -- called from the same inspector control in
/// `editor_inspector.slint` as [`setup_tier_rotate_indices_callback`].
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
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &BTreeSet::from([tier_index]),
                    );
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
                    // A move can shift index-wheel alignment for every tier between
                    // the old and new position, not tracked precisely here -- same
                    // "blast radius unknown" treatment as Undo/Redo.
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &(0..st.design.tiers.len()).collect(),
                    );
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
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::from([index]));
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
                    // Tier count changed -- the length check falls back to a full
                    // solve regardless of `dirty`.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
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

/// Whether a plain material
/// pick (`picked_name`, with no typed RI override of its own) should pin
/// [`MaterialSelection::refractive_index_override`] to the design's legacy
/// exported RI (`legacy_ri`, i.e. `meta.refractive_index` -- the schedule's own
/// `I` line) so the pick does not silently rewrite it. Returns `None` (pick
/// stays unpinned, the newly resolved material's own `n_D` wins) whenever
/// `previous_material_name` is `Some`: once a design already has a NAMED
/// material, that material's own `n_D` (or an explicit typed override) is the
/// design's RI story from then on, and pinning the OUTGOING material's RI onto
/// the incoming one would be exactly the bug this guard exists to prevent --
/// see [`loading::ri_override_to_preserve`] for the tolerance check itself.
fn ri_override_for_material_pick(
    picked_name: Option<&str>,
    previous_material_name: Option<&str>,
    legacy_ri: f64,
) -> Option<f64> {
    if previous_material_name.is_some() {
        return None;
    }
    loading::ri_override_to_preserve(picked_name?, legacy_ri)
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
                Ok(mut material) => {
                    // A plain material
                    // pick (no typed RI override -- that path is left alone, it
                    // is an explicit choice) must not silently change what the
                    // exported I line reads, but must also not pin the OUTGOING
                    // material's RI onto the incoming one. See
                    // `ri_override_for_material_pick`'s own doc comment.
                    if material.refractive_index_override.is_none() {
                        material.refractive_index_override = ri_override_for_material_pick(
                            material.name.as_deref(),
                            st.design.material.name.as_deref(),
                            st.design.meta.refractive_index,
                        );
                    }
                    match st.apply(Edit::SetMaterial { material }) {
                        Ok(()) => {
                            // A material change never moves a tier's own mast.
                            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
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
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// "Set material" on an inferred-material guess (the "Inferred material shown as
/// a guess, never as a fact" principle) -- writes `name` into the design's
/// [`indicatrix_cut_core::MaterialSelection`]
/// as an ordinary, undoable `Edit::SetMaterial`, keeping the design's own
/// current specific-gravity/RI-override fields untouched (only the NAME
/// changes -- this is "confirm the guess", not "reconfigure the material").
/// Once a name is set, `view::refresh_design_settings` stops computing a
/// guess at all (`design.material.name.is_some()`), so the guess label
/// disappears and the next native save carries the confirmed name through
/// `design.material`.
pub(in crate::gui::editor) fn setup_material_guess_set_callback(
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
        .on_set_material_from_guess(move |name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let mut material = st.design.material.clone();
            material.name = Some(name.to_string());
            match st.apply(Edit::SetMaterial { material }) {
                Ok(()) => {
                    // A material name change never moves a tier's own mast.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                    show_toast(&ui, &format!("Material set to {name}."), "success");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The design settings panel's Symmetry/Mirror "Apply" -- wholesale
/// [`Edit::SetSchedule`], keeping the design's CURRENT gear (this control never
/// changes gear -- that's [`setup_gear_apply_callback`]'s job, since only a gear
/// change needs the remap confirmation). Also registers
/// [`EditorModel::on_request_symmetry_preview`]: a live
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
    //    // `on_apply_symmetry` below moves the ORIGINAL `state` binding into its
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
                    // Symmetry/mirror can move every tier's index-wheel position, not
                    // tracked precisely here, so force a full (non-blocking) solve --
                    // and trust no cached mast either.
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &(0..st.design.tiers.len()).collect(),
                    );
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

    //    // comment above.
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
    let state_apply = Rc::clone(state);
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
            let mut st = state_apply.borrow_mut();
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
            // Records which generation this preview was built
            // against, so `setup_gear_remap_confirm_callback` can refuse to apply a
            // preview the design has since moved past -- see
            // `PENDING_GEAR_REMAP_GENERATION`'s own doc comment.
            PENDING_GEAR_REMAP_GENERATION
                .with(|cell| cell.set(Some(st.generation.load(Ordering::Relaxed))));
            drop(st);
            ui.global::<EditorModel>()
                .set_gear_remap_rows(ModelRc::new(VecModel::from(rows)));
            ui.global::<EditorModel>().set_gear_remap_open(true);
        },
    );

    // `EditorModel.gear_remap_set_rounding` (`ui/models/editor.slint`) is
    // registered here, alongside `on_gear_apply` above, rather than as its own
    // `setup_*` function: a new registration needs no new call site in
    // `gui::editor::mod`'s hub, while a new function would. This is what lets the
    // UI's own rounding choice reach `PendingGearRemap`'s existing `rounding`
    // field and `gear_remap_preview`'s existing parameter for it. Re-runs the
    // SAME dry-run preview `on_gear_apply` above computes, just with the newly
    // chosen rounding, so the red/black preview rows stay honest about what
    // Confirm will actually do.
    let state_rounding = Rc::clone(state);
    let ui_weak_rounding = ui.as_weak();
    ui.global::<EditorModel>()
        .on_gear_remap_set_rounding(move |rounding_index: i32| {
            let Some(ui) = ui_weak_rounding.upgrade() else {
                return;
            };
            let rounding = match rounding_index {
                1 => RemapRounding::Floor,
                2 => RemapRounding::Ceil,
                _ => RemapRounding::Nearest,
            };
            let mut st = state_rounding.borrow_mut();
            let Some(pending) = st.pending_gear_remap.as_mut() else {
                // Defensive only: the rounding selector only shows while a remap
                // is pending, same as Confirm's own no-op guard.
                return;
            };
            pending.rounding = rounding;
            let from_gear = pending.from_gear;
            let to_gear = pending.to_gear;
            let rows: Vec<GearRemapRow> =
                gear_remap_preview(&st.design, from_gear, to_gear, rounding);
            drop(st);
            ui.global::<EditorModel>()
                .set_gear_remap_rows(ModelRc::new(VecModel::from(rows)));
        });
}

/// The gear-remap confirmation panel's "Apply" -- commits
/// [`EditorState::pending_gear_remap`] as ONE undoable `History` step, an
/// [`Edit::Batch`] of [`Edit::RemapIndices`] then [`Edit::SetSchedule`] -- as two
/// separate, independently-undoable steps, one Undo after a gear change could
/// leave indices remapped for the new gear while the schedule still named the old
/// one. A no-op (closes the panel only) if
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
        // Refuses a Confirm whose preview no longer describes
        // the live design -- an edit landed (a tier add/edit, an Undo, another
        // Apply) while the panel sat open, so `pending`'s from/to-gear rows may no
        // longer match what `Edit::RemapIndices`/`Edit::SetSchedule` are about to do.
        // Matches `retarget_actions::apply_pending_retarget`'s own stale-generation
        // refusal shape.
        let started_generation = PENDING_GEAR_REMAP_GENERATION.with(Cell::take);
        if started_generation.is_some_and(|g| g != st.generation.load(Ordering::Relaxed)) {
            ui.global::<EditorModel>().set_gear_remap_open(false);
            show_toast(
                &ui,
                "The design changed while Apply Gear was open, so this preview no \
                 longer matches it. Re-open Apply Gear to remap against the current \
                 design.",
                "warning",
            );
            return;
        }
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
                // A gear remap rewrites every tier's index-wheel position -- force a
                // full solve rather than guessing a `dirty` set, and trust no cached
                // mast either.
                refresh_editor_panel_stale(
                    &ui,
                    &render_ctx,
                    &st,
                    &(0..st.design.tiers.len()).collect(),
                );
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
        // Matches the `take()` in `setup_gear_remap_confirm_callback` -- no pending
        // remap should ever leave a stale recorded generation behind it.
        PENDING_GEAR_REMAP_GENERATION.with(|cell| cell.set(None));
        ui.global::<EditorModel>().set_gear_remap_open(false);
    });
}

/// The viewport's "Linked to design" checkbox -- when switched ON, syncs the shared
/// viewport's render material to the design's own material IMMEDIATELY, so turning it
/// on feels responsive. `refresh_design_settings` keeps it in sync from then on.
///
/// Calls [`view::sync_viewport_material_link`] directly (`pub(super)`) rather than
/// the heavier `refresh_editor_panel_stale`, which would wipe the solved-state
/// banner, MAST/SOLVE figures, warnings and yield report and schedule a full
/// background re-solve for what is a display-only toggle that never touches
/// `Design`. Calling the real function directly, rather than a narrower copy,
/// also keeps the Render Material dropdown's own displayed index and
/// `stone_width_mm` in sync alongside `material_name` -- all three must move
/// together, or turning "Linked" on after picking a different material in the
/// dropdown would leave the dropdown and the absorption-path scaling both stale.
pub(in crate::gui::editor) fn setup_viewport_material_linked_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state_linked = Rc::clone(state);
    let render_ctx_linked = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_viewport_material_linked_changed(move |linked: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if linked {
                let st = state_linked.borrow();
                let selected_material_index = {
                    let mut ctx = render_ctx_linked
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    sync_viewport_material_link(&ui, &mut ctx, &st.design)
                };
                drop(st);
                // Set only after the `render_ctx` guard above is
                // dropped -- see `sync_viewport_material_link`'s own doc comment.
                if let Some(idx) = selected_material_index {
                    ui.global::<ViewportModel>()
                        .set_selected_material_index(idx);
                }
            }
        });

    // Registered here, alongside the viewport-link callback
    // above, rather than as its own `setup_*` function -- both need exactly
    // `(ui, state, render_ctx)`, and a new registration needs no new call site in
    // `gui::editor::mod`'s hub the way a new function would. Wired to
    // `EditorModel.apply_printed_proportions` (`ui/models/editor.slint`), called
    // from `editor_design_settings.slint`.
    let state_props = Rc::clone(state);
    let render_ctx_props = Arc::clone(render_ctx);
    let ui_weak_props = ui.as_weak();
    ui.global::<EditorModel>().on_apply_printed_proportions(
        move |vol_w3: SharedString,
              lw: SharedString,
              cw: SharedString,
              pw: SharedString,
              hw: SharedString| {
            let Some(ui) = ui_weak_props.upgrade() else {
                return;
            };
            let props = match parse_printed_proportions_form(&vol_w3, &lw, &cw, &pw, &hw) {
                Ok(props) => props,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let has_any = props.vol_w3.is_some()
                || props.lw.is_some()
                || props.cw.is_some()
                || props.pw.is_some()
                || props.hw.is_some();
            let mut st = state_props.borrow_mut();
            st.printed_proportions = has_any.then_some(props);
            // A printed-proportions edit never moves a tier's own mast.
            refresh_editor_panel_stale(&ui, &render_ctx_props, &st, &BTreeSet::new());
            drop(st);
            show_toast(
                &ui,
                if has_any {
                    "Printed proportions saved -- Deep Solve can now verify against them."
                } else {
                    "Printed proportions cleared."
                },
                "success",
            );
        },
    );
}

/// One printed-proportions field's text, parsed as `None` for blank text or
/// `Some(value)` for a finite positive number -- shared by
/// [`parse_printed_proportions_form`] across all five fields.
///
/// # Errors
///
/// A ready-to-toast message naming `label` when `text` is non-blank but does not
/// parse as a finite positive number.
fn parse_printed_proportions_field(label: &str, text: &str) -> Result<Option<f64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: f64 = trimmed
        .parse()
        .map_err(|_| format!("{label} '{trimmed}' is not a number."))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{label} must be a positive number."));
    }
    Ok(Some(value))
}

/// Parses the printed-proportions panel's five text fields
/// into an [`ExternalProportions`], the exact shape
/// [`super::super::loading::external_proportions_from_full_record`] already
/// builds from a catalogue row's own measured columns -- see that function's own
/// doc comment for the target this feeds ([`EditorState::printed_proportions`],
/// Deep Solve's external verification).
///
/// # Errors
///
/// The first field (in `vol_w3, lw, cw, pw, hw` order) that fails to parse, via
/// [`parse_printed_proportions_field`].
fn parse_printed_proportions_form(
    vol_w3: &str,
    lw: &str,
    cw: &str,
    pw: &str,
    hw: &str,
) -> Result<indicatrix::geometry::stone_metrics::ExternalProportions, String> {
    Ok(indicatrix::geometry::stone_metrics::ExternalProportions {
        vol_w3: parse_printed_proportions_field("Vol/W\u{b3}", vol_w3)?,
        lw: parse_printed_proportions_field("L/W", lw)?,
        cw: parse_printed_proportions_field("C/W", cw)?,
        pw: parse_printed_proportions_field("P/W", pw)?,
        hw: parse_printed_proportions_field("H/W", hw)?,
    })
}

/// Maps an incoming Solid-viewport pointer
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
/// # Indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// Rebuilding `FacetMap::from_design(&st.design, &solved)` -- a full
/// `Design::planes_from_solved`-equivalent rebuild plus a fresh `dedup_planes` pass --
/// on every single mouse-move event would be wasteful. `solid_hover_text` already
/// holds exactly the string that map would produce for each facet id, computed ONCE
/// per rendered frame by the worker thread
/// (`preview_state::update_diagram_memory_from_design`'s Solid-mode counterpart), so
/// this degrades to one `Vec::get`. Neither `Design` nor the last-solved mast cache
/// is needed here at all (an editor edit that hasn't re-rendered yet still shows the
/// PREVIOUS frame's hover text, exactly as it shows the previous frame's picked
/// geometry -- no new staleness).
///
/// `solid_hover_text` reaches this callback through `solid_pick_state` (see
/// [`SolidPickState`]'s own doc comment): `gui::mod::build_main_window` owns the
/// `Arc<Mutex<Vec<String>>>` `SlintSolidSink` writes every frame, and bundles it into
/// the `SolidPickState` this function's caller passes through.
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
            // pick buffer. In Path-traced/Both mode the pick
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
                // Falls back to the last CLICKED facet's own
                // label (if any) instead of blanking the tooltip outright, so the
                // selection stays readable once the pointer leaves it -- see
                // `SELECTED_FACET_LABEL`'s own doc comment.
                let selected_label = SELECTED_FACET_LABEL.with(|cell| cell.borrow().clone());
                ui.global::<SolidPreviewModel>()
                    .set_hover_text(selected_label.into());
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
/// # Indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// See [`setup_solid_facet_hover_callback`]'s matching doc section: `solid_facet_tier`
/// is the last rendered frame's own `PreviewFrame::facet_tier` table, so resolving a
/// clicked facet to its owning tier is one `Vec::get` instead of a fresh
/// `FacetMap::from_design` rebuild. Neither `Design` nor the last-solved mast cache is
/// needed here at all. `solid_facet_tier` reaches this callback the same way
/// `solid_hover_text` reaches the hover callback: through `solid_pick_state`, bundled
/// there by `gui::mod::build_main_window`.
pub(in crate::gui::editor) fn setup_solid_facet_click_callback(
    ui: &MainWindow,
    solid_pick_state: &SolidPickState,
) {
    let solid_pick = Arc::clone(&solid_pick_state.pick);
    let solid_facet_tier = Arc::clone(&solid_pick_state.facet_tier);
    let solid_hover_text = Arc::clone(&solid_pick_state.hover_text);
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
                // A click that misses the silhouette clears
                // the tier selection instead of leaving it alone -- matching what
                // `solid_preview::diagram_wiring`'s own Diagram-mode click miss
                // branch already does.
                // Setting `selected_tier_index` alone is enough: `changed
                // selected_tier_index` in `models/editor.slint` fires
                // `selected_tier_changed`, which re-seeds/clears the inspector
                // form on the Rust side (`setup_solid_selected_tier_changed_
                // callback`).
                ui.global::<EditorModel>().set_selected_tier_index(-1);
                // A miss also clears whatever facet was
                // previously identified -- nothing is selected any more, so
                // nothing should keep reading in the tooltip.
                SELECTED_FACET_LABEL.with(|cell| cell.borrow_mut().clear());
                ui.global::<SolidPreviewModel>().set_hover_text("".into());
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
            // Identifies the clicked facet itself, not just its
            // owning tier -- GemCad/GCS-style "which index is this facet, and
            // which member of the orbit did I click" -- reusing the SAME per-facet
            // label `setup_solid_facet_hover_callback` shows transiently on hover
            // (`solid_hover_text`), but kept in `SELECTED_FACET_LABEL` so it
            // survives the pointer leaving the facet.
            let label = solid_hover_text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(facet_id as usize)
                .cloned()
                .unwrap_or_default();
            SELECTED_FACET_LABEL.with(|cell| cell.borrow_mut().clone_from(&label));
            ui.global::<SolidPreviewModel>()
                .set_hover_text(label.into());
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
    // Ending a coalesce run needs a real
    // interaction boundary, and the ideal one (pointer release/focus loss on the
    // `TierAngleCell` doing the nudging) lives in `editor_tier_table.slint`, not
    // here (see `History::end_coalesce_run`'s own doc comment). The
    // selection changing IS something this function can observe: it fires only on
    // a genuine `changed selected_tier_index` (a different row/facet clicked, or the
    // selection cleared), never on the nudge control's own repeated ticks, so a
    // wheel-nudge run in progress on the tier the cutter just navigated away from
    // must not sit open for an unrelated later nudge on that same tier (after
    // selecting elsewhere and back within the coalescing window) to merge into.
    st.history.end_coalesce_run();
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
///
/// `field` is one of `"angle"`/`"name"`/
/// `"indices"`/`"constraint"`, or `""` for a general/unclassified error -- see
/// `EditorModel.tier_form_error_field`'s own doc comment (`ui/models/editor.slint`)
/// for the exact contract `editor_inspector.slint`
/// reads this against to put a red border on the SPECIFIC offending control,
/// not only the shared message under the whole form.
fn report_tier_form_error(ui: &MainWindow, message: &str, field: &str) {
    let model = ui.global::<EditorModel>();
    model.set_tier_form_error(message.into());
    model.set_tier_form_error_field(field.into());
    show_toast(ui, message, "error");
}

/// Classifies a [`loading::parse_tier_form`] error message into which tier-form
/// field it concerns -- see [`report_tier_form_error`]'s own doc comment for what
/// each returned string means. Matched on the exact wording `loading.rs`'s own
/// error branches build (reads the rendered text rather than a structured
/// variant, since `loading::parse_tier_form` returns a plain `String`); a message this
/// does not recognize classifies as `""`, the same "general/unclassified" bucket
/// an apply-time (post-parse) failure falls into.
fn tier_form_error_field(message: &str) -> &'static str {
    if message.starts_with("Angle") {
        "angle"
    } else if message.starts_with("Another tier is already named") {
        "name"
    } else if message.starts_with("Index '") {
        "indices"
    } else if message.starts_with("\"Meet named\"")
        || message.starts_with("Scale reference")
        || message.starts_with("No facet named")
        || message.starts_with("Unknown constraint kind")
        // `loading::parse_tier_target`'s own three target labels --
        // same "constraint" bucket as `Scale reference`'s wording, since these
        // are all failures of the same Meets-combo numeric field.
        || message.starts_with("Depth")
        || message.starts_with("Girdle thickness")
        || message.starts_with("Table width")
    {
        "constraint"
    } else {
        ""
    }
}

/// `loading::parse_tier_form` deliberately ACCEPTS a
/// non-integral index-wheel position (real `.asc` files carry a small but
/// real fraction of these -- see that function's own doc comment) rather
/// than rejecting it, since a hand-typed fraction is sometimes exactly what
/// was meant. Without this warning a cutter gets no signal at all when it was
/// NOT meant -- a stray extra digit ("12.5" for "12") would only ever surface
/// later as an obscure solver oddity. Called from [`setup_save_tier_callback`] after a
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

/// Live critical-angle guidance in the Tier form's Angle field (see
/// `EditorModel::angle_live_preview`'s own doc comment on `ui/models/editor.slint`).
/// Parses `text` (the field's own
/// live content) as a signed degree value: a pavilion angle (`< 0`) reads the
/// plain table-only critical-angle margin
/// ([`super::super::state::tier_margin_and_risk`]'s own pavilion branch, via
/// [`indicatrix_cut_core::tier_margin_deg`]/[`indicatrix_cut_core::windowing_risk`]);
/// a crown angle (`> 0`) reads the crown-window ESTIMATE
/// ([`indicatrix_cut_core::crown_window_margin_deg`]/
/// [`indicatrix_cut_core::crown_windowing_risk`]) against the design's own
/// representative pavilion angle
/// ([`super::super::state::representative_crown_and_pavilion_angles_deg`]) --
/// the exact same functions a saved row's MARGIN cell uses, so a value typed
/// but not yet saved reads the identical bar. Uses the design's plain
/// [`indicatrix_cut_core::Design::effective_refractive_index`] (built-in
/// materials only, no custom-catalogue lookup) rather than threading
/// `RenderContext` through for this preview-only path -- the authoritative
/// "Eff. RI"/critical-angle readouts elsewhere already use the full
/// custom-material-aware value; this is a live estimate while typing, not the
/// figure of record. An unparseable/blank/zero angle, or a crown angle with no
/// pavilion tier in the design to estimate against, clears the bar
/// (`level = -1`).
pub(in crate::gui::editor) fn setup_angle_live_preview_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_angle_live_preview(move |text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let model = ui.global::<EditorModel>();
            let clear = || {
                model.set_angle_live_margin_text(SharedString::new());
                model.set_angle_live_margin_level(-1);
                model.set_angle_live_margin_is_estimate(false);
            };
            let Ok(angle_deg) = text.trim().parse::<f64>() else {
                clear();
                return;
            };
            let st = state.borrow();
            let n_d = st.design.effective_refractive_index();
            if angle_deg < 0.0 {
                let margin = indicatrix_cut_core::tier_margin_deg(angle_deg, n_d);
                let level = match indicatrix_cut_core::windowing_risk(angle_deg, n_d) {
                    indicatrix_cut_core::Risk::Safe => 0,
                    indicatrix_cut_core::Risk::Marginal => 1,
                    indicatrix_cut_core::Risk::Windows => 2,
                };
                model.set_angle_live_margin_text(format!("{margin:+.1}\u{b0}").into());
                model.set_angle_live_margin_level(level);
                model.set_angle_live_margin_is_estimate(false);
            } else if angle_deg > 0.0 {
                let (_, pavilion_deg) = representative_crown_and_pavilion_angles_deg(&st.design);
                let Some(pavilion_deg) = pavilion_deg else {
                    clear();
                    return;
                };
                let margin =
                    indicatrix_cut_core::crown_window_margin_deg(pavilion_deg, angle_deg, n_d);
                let level =
                    match indicatrix_cut_core::crown_windowing_risk(pavilion_deg, angle_deg, n_d) {
                        indicatrix_cut_core::Risk::Safe => 0,
                        indicatrix_cut_core::Risk::Marginal => 1,
                        indicatrix_cut_core::Risk::Windows => 2,
                    };
                model.set_angle_live_margin_text(format!("{margin:+.1}\u{b0}").into());
                model.set_angle_live_margin_level(level);
                model.set_angle_live_margin_is_estimate(true);
            } else {
                clear();
            }
        });
}

/// The anchor explainer card's "Got it" / "Don't show again" dismiss buttons:
/// "Don't show again" additionally persists the suppression (`state::anchor_explainer_suppress_permanently`,
/// `AppSettings::suppressed_confirmations` key `"anchor_explainer"`) so
/// `state::should_open_anchor_explainer` never reopens it again, on top of the
/// session-scoped "already shown once" guard that function already applies.
pub(in crate::gui::editor) fn setup_anchor_explainer_dismiss_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_anchor_explainer_dismiss(move |dont_show_again: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_anchor_explainer_open(false);
            if dont_show_again {
                super::super::state::anchor_explainer_suppress_permanently();
            }
        });
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
        letterbox_margin, next_free_block_name, parse_printed_proportions_field,
        parse_printed_proportions_form, ri_override_for_material_pick, split_duplicate_suffix,
        tier_form_error_field, unique_duplicate_name,
    };

    // --- ri_override_for_material_pick ---

    #[test]
    fn ri_override_for_material_pick_does_not_pin_when_the_design_already_had_a_material() {
        // Diamond -> Quartz on a design whose CURRENT material is already named
        // "Diamond": pinning the outgoing material's (Diamond's) RI onto the
        // incoming pick (Quartz) is exactly the bug this guard prevents.
        assert_eq!(
            ri_override_for_material_pick(Some("Quartz"), Some("Diamond"), 2.417),
            None
        );
    }

    #[test]
    fn ri_override_for_material_pick_pins_the_legacy_ri_on_a_first_pick_that_drifts() {
        // No material named yet (fresh design, legacy schedule RI 1.54): picking
        // Diamond (n_D ~1.5442) should pin the legacy figure so the exported I
        // line does not silently move.
        assert_eq!(
            ri_override_for_material_pick(Some("Diamond"), None, 1.54),
            Some(1.54)
        );
    }

    #[test]
    fn ri_override_for_material_pick_does_nothing_for_a_non_built_in_name() {
        assert_eq!(
            ri_override_for_material_pick(Some("Not A Real Material"), None, 1.54),
            None
        );
    }

    // --- next_free_block_name  ---

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

    // --- unique_duplicate_name / split_duplicate_suffix ---
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

    // --- parse_printed_proportions_form/_field  ---

    #[test]
    fn parse_printed_proportions_field_treats_blank_text_as_none() {
        assert_eq!(parse_printed_proportions_field("L/W", ""), Ok(None));
        assert_eq!(parse_printed_proportions_field("L/W", "   "), Ok(None));
    }

    #[test]
    fn parse_printed_proportions_field_parses_a_positive_number() {
        assert_eq!(
            parse_printed_proportions_field("L/W", "1.05"),
            Ok(Some(1.05))
        );
    }

    #[test]
    fn parse_printed_proportions_field_rejects_unparsable_text() {
        assert!(parse_printed_proportions_field("L/W", "abc").is_err());
    }

    #[test]
    fn parse_printed_proportions_field_rejects_zero_and_negative() {
        assert!(parse_printed_proportions_field("L/W", "0").is_err());
        assert!(parse_printed_proportions_field("L/W", "-1.0").is_err());
    }

    #[test]
    fn parse_printed_proportions_form_builds_every_field() {
        let props = parse_printed_proportions_form("1.30", "1.05", "0.61", "0.43", "0.60").unwrap();
        assert_eq!(props.vol_w3, Some(1.30));
        assert_eq!(props.lw, Some(1.05));
        assert_eq!(props.cw, Some(0.61));
        assert_eq!(props.pw, Some(0.43));
        assert_eq!(props.hw, Some(0.60));
    }

    #[test]
    fn parse_printed_proportions_form_allows_every_field_blank() {
        let props = parse_printed_proportions_form("", "", "", "", "").unwrap();
        assert_eq!(props.vol_w3, None);
        assert_eq!(props.lw, None);
        assert_eq!(props.cw, None);
        assert_eq!(props.pw, None);
        assert_eq!(props.hw, None);
    }

    #[test]
    fn parse_printed_proportions_form_names_the_failing_field() {
        let err = parse_printed_proportions_form("1.30", "not a number", "0.61", "0.43", "0.60")
            .unwrap_err();
        assert!(err.contains("L/W"), "error should name the field: {err}");
    }

    // --- letterbox_margin  ---

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

    // --- tier_form_error_field  ---

    #[test]
    fn tier_form_error_field_classifies_every_documented_message_shape() {
        assert_eq!(
            tier_form_error_field("Angle '41x' is not a number."),
            "angle"
        );
        assert_eq!(
            tier_form_error_field(
                "Angle 91.00\u{b0} exceeds 90\u{b0} -- angles are measured from the girdle \
                 plane, so no crown or pavilion facet can be steeper than that."
            ),
            "angle"
        );
        assert_eq!(
            tier_form_error_field(
                "Another tier is already named 'P1' -- facet names must be unique so meet \
                 constraints resolve to the right tier."
            ),
            "name"
        );
        assert_eq!(
            tier_form_error_field("Index '99' is outside this design's 96-tooth gear."),
            "indices"
        );
        assert_eq!(
            tier_form_error_field("\"Meet named\" needs at least one facet name."),
            "constraint"
        );
        assert_eq!(
            tier_form_error_field("Scale reference 'x' is not a number."),
            "constraint"
        );
        assert_eq!(
            tier_form_error_field("No facet named 'C1' -- check the Meets field."),
            "constraint"
        );
        assert_eq!(
            tier_form_error_field("Unknown constraint kind 7."),
            "constraint"
        );
    }

    #[test]
    fn tier_form_error_field_is_empty_for_an_unrecognized_message() {
        assert_eq!(
            tier_form_error_field("Failed to remap this design's indices to the new gear."),
            ""
        );
    }
}
