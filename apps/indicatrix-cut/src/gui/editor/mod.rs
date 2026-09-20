//! Wires `indicatrix-cut-core`'s editor core into the "Edit" sub-tab -- tier list,
//! preform controls, undo/redo, live-solid validation status, loading the currently
//! selected catalogue design (or starting fresh), feeding the edited geometry into the
//! existing Live Render viewport, and exporting the edited schedule as `.asc`.
//!
//! Only compiled with the `editor` feature, which is also what makes the
//! `indicatrix-cut-core` dependency itself optional.
//!
//! # `History` is the only thing that mutates `Design`
//!
//! `indicatrix_cut_core::History::undo`/`redo` each `.expect(...)` that the recorded
//! inverse they're about to replay still applies -- a failure there can only mean
//! something else mutated `Design` behind `History`'s back, and silently swallowing
//! that would hide real corruption. So nothing in this group may call
//! `Design::apply_edit` directly: [`state::EditorState::apply`]/
//! [`apply_coalescing`](state::EditorState::apply_coalescing)/
//! [`undo`](state::EditorState::undo)/[`redo`](state::EditorState::redo)/
//! [`apply_optimize_outcome`](state::EditorState::apply_optimize_outcome) are the
//! only five functions that touch `design` and `history` together, all routing
//! through `History`, and every callback in [`callbacks`]/[`native_io`] calls one of
//! those five. (This doc comment predates `apply_coalescing`, the angle-nudge
//! coalescing path added alongside `setup_nudge_angle_callback`; see
//! `state::EditorState`'s own doc comment, which already accounts for it.)
//!
//! # Feeding the viewport
//!
//! There is no separate 3D scene for the Edit tab -- it shares the exact
//! `RenderContext::active_planes` the "Live Render" sub-tab already draws.
//! [`state::design_to_gpu_planes`] converts `Design::planes()`'s `(normal, offset)`
//! half-spaces (the `n . x <= m` convention) into `GpuFacetPlane`s (`n . x + d = 0`)
//! via the sign flip `GpuFacetPlane::to_halfspace_f64` documents (`m = -d`, so
//! `d = -m`) -- this group only ever goes in that one direction, never the reverse.
//!
//! [`view::refresh_editor_panel`] runs once at startup so the tab isn't blank, but
//! `refresh_viewport` does not, so wiring this group up cannot stomp whatever the
//! currently selected catalogue design already put in the shared viewport.
//! [`view::refresh_all`] (panel + viewport together) is what `New`/`Load Selected`/the
//! explicit "Solve" action call -- see "Solve on explicit action" below for why every
//! other edit callback calls [`view::refresh_editor_panel_stale`] instead.
//!
//! # Solve on explicit action, not on every edit
//!
//! Re-solving on every callback was measured unsafe to generalize: real `.asc`
//! fixtures take hundreds of milliseconds to multiple seconds to solve (a real
//! 103-tier/210-plane design: 5.9s), because `meet_solver`'s refinement sweep is cubic
//! in plane count. Solving after every keystroke would freeze the UI on designs
//! nowhere near the solver's own plane cap.
//!
//! So this group solves only on the explicit "Solve" button
//! ([`callbacks::setup_solve_callback`]) -- OR, for a design cheap enough to measure
//! as such, automatically after a debounce (see "Never block the UI thread with a
//! solve" below for both). Every other edit callback calls
//! [`view::refresh_editor_panel_stale`]: it updates everything that does not require a
//! solve and marks the mast/strategy columns and validation banner visibly stale
//! ("Not solved -- click Solve...", or, once auto-solve is scheduled, "Solving...")
//! rather than re-solving inline or silently showing a previous solve's masts as if
//! they still applied. `New`/`Load Selected` still solve immediately for a small
//! design (see below), since loading is the one point where showing the real solved
//! state right away is worth a possible wait.
//!
//! A later, subgraph-only resolve path was built and measured, and does not change
//! this: `meet_solver`'s refinement sweep rebuilds its candidate-vertex arrangement
//! from every tier's plane regardless of which are pinned, so a subgraph resolve costs
//! the same as a full solve on any design with a non-trivial meet-derived remainder.
//!
//! # Never block the UI thread with a solve
//!
//! Every one of this group's `Design::solve` call sites (directly, or via
//! [`state::tier_items`]/[`state::status_text_and_is_problem`]/
//! [`state::manufacturability_warning_lines`]/[`state::yield_report_texts`]/
//! [`state::design_to_gpu_planes`], which each solve internally) now runs OFF the UI
//! thread, following `deep_solve.rs`'s own `thread::spawn` +
//! `Weak::upgrade_in_event_loop` convention -- see [`auto_solve`]'s module doc
//! comment for the full epoch/sequence-number mechanism a completed background solve
//! is checked against before it is allowed to touch the display.
//!
//! The one exception: [`view::refresh_all`] (New/Load Selected/Adopt/the explicit
//! "Solve" action) still solves synchronously for a design at or under
//! [`auto_solve::should_solve_synchronously`], both because that is fast enough in
//! practice not to matter and because "New" specifically promises an immediately
//! solved, unstale design. Above that tier count, [`view::refresh_all`] pushes the
//! same stale content [`view::refresh_editor_panel_stale`] does after any other edit,
//! then immediately dispatches a background solve -- see [`view::refresh_all`]'s own
//! doc comment.
//!
//! On top of that, [`view::refresh_editor_panel_stale`] -- called by every OTHER edit
//! callback already -- ends with [`auto_solve::on_edit`]: when this design's last
//! measured solve is cheap enough (under a user-adjustable, persisted budget,
//! `editor_auto_solve_budget_ms`; `0` disables), a debounced background solve is
//! scheduled automatically, so small/medium designs get a fresh path-traced view and
//! masts without ever pressing Solve. Above the budget, today's plain stale-marker
//! behaviour is unchanged, with a banner note explaining why auto-solve is off for
//! this particular design.
//!
//! # Module split
//!
//! Split into [`state`] (`EditorState` + pure tier/status view-model helpers),
//! [`loading`] (resolving a `Design` from a catalogue record, parsing edit forms),
//! [`view`] (pushing state into `EditorView`/the viewport, Deep Solve/Optimize
//! formatting), [`callbacks`] (Slint callback wiring), [`native_io`] (`.asc`
//! export, native save/open), and [`auto_solve`] (background/auto-solve machinery --
//! see "Never block the UI thread with a solve" above), with
//! [`setup_editor_callbacks`] as this group's main public entry point.
//!
//! [`apply_matching_preview_frame`] is the one other public item: a narrow bridge
//! `gui::SlintSolidSink::apply` (a solid-preview WORKER-thread callback, hopped
//! onto the UI thread via `slint::Weak::upgrade_in_event_loop`, `Send`-bound) calls
//! to reach this group's UI-thread-confined `auto_solve::Runtime` -- it cannot
//! reach `EditorState`'s `Rc<RefCell<..>>` at all (see that `impl`'s own doc
//! comment). See [`auto_solve::take_matching_design`] for the full `cad_todo.md`
//! #73 mechanism this exists for.

mod auto_solve;
mod callbacks;
mod cut_sheet;
mod deep_solve;
mod loading;
mod material_lookup;
mod native_io;
mod optimize_solve;
pub mod retarget;
mod state;
mod view;

use crate::{
    MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::solid_preview::preview_state::{SolidLastSolved, SolidPickState, SolidPreviewState},
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle as _, Model as _};
use state::EditorState;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Wires every "Edit" sub-tab callback (declared on `EditorModel`, `ui/models/editor.slint`'s
/// global -- see `ui/README.md` for how a `.slint` component reaches it directly, with
/// no forwarding through `MainWindow`) to a shared [`EditorState`], and populates the
/// tab's own display once up front (see [`view::refresh_editor_panel`]) so it isn't
/// blank when first opened.
///
/// Split into one `setup_*` function per callback (in [`callbacks`]/[`native_io`]),
/// the same shape every other `gui::*` module in this crate uses.
/// CAD audit item 112: "Abandon Solve".
///
/// `auto_solve::cancel_in_flight_solve` invalidates the in-flight result and drops
/// any queued dispatch; this function is what gives the cutter their editor back on
/// the same click, rather than leaving the Solve button disabled until an abandoned
/// worker finally lands. The design itself is untouched, so the honest state
/// afterwards is "stale" -- it still needs a solve, just not that one.
///
/// Note the worker does finish in the background: `Design::solve` has no mid-run
/// checkpoint, unlike Optimize's real per-evaluation one. This is UI-level
/// abandonment, and the button says "Abandon" rather than "Cancel" for that reason.
fn setup_solve_cancel_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<crate::EditorModel>().on_solve_cancel(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        auto_solve::cancel_in_flight_solve();
        let model = ui.global::<crate::EditorModel>();
        model.set_solve_running(false);
        model.set_solve_state("stale".into());
        model.set_status_text("Solve abandoned -- click Solve when you are ready.".into());
        model.set_status_is_problem(true);
    });
}

/// CAD audit items 95 and 78: offers to reopen at startup instead of always
/// opening on a blank design.
///
/// Deliberately an OFFER, matching the item's own wording. Silently reopening
/// yesterday's design would be a surprise on a tool people also use to start new
/// work -- and worse, it would hide a crash-recovery file inside an ordinary-looking
/// session, so a cutter could overwrite unsaved work without ever being told it
/// existed.
///
/// A leftover autosave outranks the recent-files list: that file is on disk only
/// because a previous run did not shut down cleanly, and it holds work that was
/// never saved at all. An ordinary recent file can always be reopened later from
/// File > Open Recent; the autosave is deleted by the next successful save.
fn setup_startup_restore(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let model = ui.global::<crate::EditorModel>();
    let autosave = native_io::find_leftover_autosave();
    let offer = autosave.clone().or_else(|| {
        ui.get_recent_native_files()
            .iter()
            .next()
            .map(|path| std::path::PathBuf::from(path.as_str()))
    });
    let Some(offer) = offer else {
        return;
    };
    model.set_startup_restore_is_autosave(autosave.is_some());
    model.set_startup_restore_path(offer.display().to_string().into());

    let state_accept = Rc::clone(state);
    let render_ctx_accept = Arc::clone(render_ctx);
    let preview_accept = Arc::clone(preview_state);
    let solved_accept = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_accept(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // Cleared FIRST: the open below can itself toast or open the
            // fingerprint-mismatch dialog, and this prompt must be gone by then
            // rather than stacked underneath it.
            let path = ui.global::<crate::EditorModel>().get_startup_restore_path();
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
            native_io::open_recent_native_path(
                &ui,
                &state_accept,
                &render_ctx_accept,
                &preview_accept,
                &solved_accept,
                std::path::PathBuf::from(path.as_str()),
            );
        });

    let ui_weak_dismiss = ui.as_weak();
    ui.global::<crate::EditorModel>()
        .on_startup_restore_dismiss(move || {
            let Some(ui) = ui_weak_dismiss.upgrade() else {
                return;
            };
            // The file is left exactly where it is -- declining the offer is not a
            // decision to throw work away. A leftover autosave is removed only by
            // the next successful save (see `finish_save_native_success`).
            ui.global::<crate::EditorModel>()
                .set_startup_restore_path(String::new().into());
        });
}

/// CAD audit item 211: redraws the preview when the tier-cutoff slider moves.
///
/// `submit_preview_replan` already reads `SolidPreviewModel.tier_cutoff` and hands it
/// to `SolidPreviewState::set_tier_cutoff`, but nothing asked for a replan when the
/// slider itself moved -- so the whole path from slider to `Design::planes_through_tier`
/// was correct and simply never ran until an unrelated edit triggered one.
///
/// An empty `dirty` set with `force_full_solve: false`: changing how much of the
/// schedule is DRAWN does not change the design, so the solve is reusable and only
/// the plane arrangement needs rebuilding.
fn setup_tier_cutoff_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<crate::SolidPreviewModel>()
        .on_tier_cutoff_changed(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            view::submit_preview_replan(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &st,
                std::collections::BTreeSet::new(),
                false,
            );
        });
}

pub fn setup_editor_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    // The Solid viewport's shared pick-buffer/hover-text/facet-tier state -- read
    // by its hover/click callbacks instead of rebuilding a `FacetMap` per mouse
    // event. Written by `gui::SlintSolidSink::apply` as each frame lands -- see
    // `callbacks::setup_solid_facet_hover_callback` and [`SolidPickState`]'s own
    // doc comment.
    solid_pick_state: &SolidPickState,
) {
    // Before any callback (and therefore any possible background-solve dispatch) is
    // wired up -- see `auto_solve::init`'s own doc comment for why this module needs
    // these handles up front rather than reading them off `EditorState`.
    auto_solve::init(preview_state, solid_last_solved);
    let state = Rc::new(RefCell::new(EditorState::fresh()));
    view::refresh_editor_panel(ui, render_ctx, &state.borrow());
    setup_startup_restore(ui, &state, render_ctx, preview_state, solid_last_solved);

    callbacks::setup_new_design_create_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_load_selected_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
        db,
        source,
    );
    callbacks::setup_solve_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_undo_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_redo_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_apply_preform_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_apply_yield_inputs_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_save_tier_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_remove_tier_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_toggle_detach_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    // Inline tier-list editing/angle-nudge/duplicate/multi-select -- see
    // `callbacks::tier_actions`'s own doc comments on each.
    callbacks::setup_inline_set_angle_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_nudge_angle_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_duplicate_tier_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_toggle_multi_select_callback(ui, &state);
    native_io::setup_export_asc_callback(ui, &state);
    native_io::setup_export_cutting_sheet_callback(ui, &state);
    native_io::setup_export_diagram_callback(ui, &state);
    native_io::setup_save_native_callback(ui, &state, db, source);
    native_io::setup_open_native_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_adopt_meet_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_deep_solve_callback(ui, &state);
    setup_solve_cancel_callback(ui);
    setup_tier_cutoff_callback(ui, &state, render_ctx, preview_state, solid_last_solved);
    callbacks::setup_deep_solve_cancel_callback(ui, &state);
    callbacks::setup_optimize_callback(ui, &state, render_ctx);
    callbacks::setup_optimize_cancel_callback(ui, &state);
    callbacks::setup_optimize_apply_callback(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    setup_editor_secondary_callbacks(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
        solid_pick_state,
    );
}

/// The rest of [`setup_editor_callbacks`]'s registrations (design settings, the
/// material-suggestion banner, Retarget, and the Solid-viewport hover/click/selection
/// wiring) -- split out purely to keep the public entry point under clippy's
/// function-length lint.
fn setup_editor_secondary_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    // See [`setup_editor_callbacks`]'s own matching parameter doc comment.
    solid_pick_state: &SolidPickState,
) {
    // The design settings panel: material/RI, gear-remap confirmation,
    // symmetry/mirror, and the viewport's "linked to design" material sync.
    callbacks::setup_apply_design_material_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_gear_apply_callback(ui, state);
    callbacks::setup_gear_remap_confirm_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_gear_remap_cancel_callback(ui, state);
    callbacks::setup_apply_symmetry_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_viewport_material_linked_changed_callback(ui, state, render_ctx);
    // Catalogue-load material suggestion banner.
    callbacks::setup_material_suggestion_accept_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_material_suggestion_dismiss_callback(ui);
    // "Retarget for material".
    callbacks::setup_retarget_open_callback(ui, state, render_ctx);
    callbacks::setup_retarget_proposal_changed_callback(ui, state, render_ctx);
    callbacks::setup_retarget_apply_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_retarget_close_callback(ui, state);
    callbacks::setup_tier_filter_callback(ui);
    // The Solid viewport's hover/click picking, and the tier-list <-> preview
    // selection reverse link.
    callbacks::setup_solid_facet_hover_callback(ui, solid_pick_state);
    callbacks::setup_solid_facet_click_callback(ui, solid_pick_state);
    callbacks::setup_solid_selected_tier_changed_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
}

/// `cad_todo.md` #73: called from `gui::SlintSolidSink::apply` once a solid-preview
/// frame lands, naming `generation` (that frame's own
/// `solid_preview::preview_state::PreviewFrame::generation`) and `solved` (its
/// masts). A no-op when `generation` no longer names the live design -- see
/// [`auto_solve::take_matching_design`]'s own doc comment for the exact
/// staleness check -- otherwise pushes the tier table's rows, the validation
/// banner, the manufacturability warnings and the yield figures straight from
/// `solved`, via [`view::push_solved_preview`], instead of leaving that to a
/// second, separately dispatched `Design::solve()`.
///
/// See this module's own doc comment ("Module split") for why this, alongside
/// [`setup_editor_callbacks`], is the only other function this group exposes
/// beyond its own module boundary.
pub fn apply_matching_preview_frame(ui: &MainWindow, generation: u64, solved: &[SolvedTier]) {
    if let Some((design, multi_selected)) = auto_solve::take_matching_design(generation) {
        view::push_solved_preview(ui, &design, solved, &multi_selected);
    }
}
