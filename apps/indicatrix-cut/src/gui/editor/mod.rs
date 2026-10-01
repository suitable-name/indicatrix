//! Wires `indicatrix-cut-core`'s editor core into the "Edit" sub-tab -- tier list,
//! preform controls, undo/redo, live-solid validation status, loading the currently
//! selected catalogue design (or starting fresh), feeding the edited geometry into the
//! existing Live Render viewport, and exporting the edited schedule as `.asc`.
//!
//! # `History` is the only thing that mutates `Design`
//!
//! `indicatrix_cut_core::History::undo`/`redo` replay a recorded inverse against the
//! `Design` and return `Err(EditError)` -- leaving the undo/redo entry in place -- when
//! it no longer applies. That can only mean something else mutated `Design` behind
//! `History`'s back, and silently swallowing it would hide real corruption, so the
//! callers in [`callbacks`] surface the error (a toast) instead of unwrapping it. For
//! the same reason nothing in this group may call `Design::apply_edit` directly: every
//! edit of `design` goes through the shared `indicatrix_editor::EditorSession`,
//! reached via [`state::EditorState`]'s entry points (`apply`, `undo`, `redo`,
//! `nudge_angles`, `set_tier_angle`, `pin_tier_mast`, `rotate_tier_indices`, and
//! `apply_optimize_outcome` through `Deref`), all of which route through `History`.
//! `EditorState::apply_coalescing` exists only for tests; the production nudge path
//! is `nudge_angles`.
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
//! [`state::yield_report_texts`]/[`state::design_to_gpu_planes`], which each
//! solve internally) must run OFF the UI thread, following `deep_solve.rs`'s own
//! `thread::spawn` + `Weak::upgrade_in_event_loop` convention -- see [`auto_solve`]'s
//! module doc comment for the full epoch/sequence-number mechanism a completed
//! background solve is checked against before it is allowed to touch the display. A
//! call site that solves on the UI thread is a defect (a solve costs from a fraction
//! of a millisecond to several seconds); the exception below is the only sanctioned one.
//!
//! The one exception: [`view::refresh_all`] (New/Load Selected/Adopt/the explicit
//! "Solve" action) still solves synchronously for a design at or under
//! `indicatrix_editor::solve_policy::should_solve_synchronously_for` (few planes, few
//! meet-derived tiers, no tier targets, and a last measured solve that was fast), both
//! because that is fast enough in practice not to matter and because "New" specifically
//! promises an immediately solved, unstale design. Above that cost, [`view::refresh_all`] pushes the
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
//! export, native save/open), [`setup`] (the startup-restore offer, the solve-cancel and
//! tier-cutoff wirings) and [`auto_solve`] (background/auto-solve machinery --
//! see "Never block the UI thread with a solve" above), with
//! [`setup_editor_callbacks`] as this group's main public entry point.
//!
//! [`apply_matching_preview_frame`] is the one other public item: a narrow bridge
//! `gui::SlintSolidSink::apply` (a solid-preview WORKER-thread callback, hopped
//! onto the UI thread via `slint::Weak::upgrade_in_event_loop`, `Send`-bound) calls
//! to reach this group's UI-thread-confined `auto_solve::Runtime` -- it cannot
//! reach `EditorState`'s `Rc<RefCell<..>>` at all (see that `impl`'s own doc
//! comment). See [`auto_solve::take_matching_design`] for the full mechanism this
//! exists for.

mod activity;
mod auto_solve;
mod callbacks;
// The visual before/after compare window (Retarget, Optimize, Snapshot) -- see
// that module's own doc comment.
mod compare;
mod cut_sheet;
mod deep_solve;
// The coalesce-at-the-source UI intent queue: `setup_tier_cutoff_callback`
// (below) and `callbacks::tier_actions::setup_nudge_angle_callback`/
// `callbacks::retarget_actions::setup_retarget_proposal_changed_callback` each
// build their own instance -- see that module's own doc comment.
pub(in crate::gui::editor) mod edit_intent;
// The worked-example walkthrough: step content, automatic advance, and the
// completion checks every refresh ends in -- see this module's own doc comment.
mod guide;
// Byte-identity pins for everything shared with `crates/indicatrix-editor` -- see
// that module's own doc comment.
#[cfg(test)]
mod identity_pins;
mod loading;
pub(in crate::gui) mod material_lookup;
// Mouse-driven angle/depth/index drag handles on the Solid viewport's selected facet --
// see that module's own doc comment.
mod manipulate;
// `pub(in crate::gui)`, not private: `gui::library::local::import` reuses
// `native_io::ask_write_confirm`'s in-window confirm dialog/continuation for its
// own "replace existing design(s)?" prompt -- see that function's own doc
// comment.
pub(in crate::gui) mod native_io;
mod optimize_solve;
// "Retarget for material" moved to `indicatrix-editor` unchanged; re-exported at
// its old path.
pub use indicatrix_editor::retarget;
mod setup;
// The shortcut table shared by the in-app overlay and the generated section of
// appendix B -- see this module's own doc comment.
mod shortcuts;
mod solve_service;
// The shared generation-stamp registry for analysis results with no `EditorState`
// field of their own to track staleness on -- see this module's own doc comment.
mod stale;
mod stall_guard;
mod state;
// The New Design template gallery's Rust-side glue -- see this module's own doc
// comment.
mod templates;
mod view;

// Called wherever the main window hides, so the compare window never outlives it.
pub(in crate::gui) use compare::close_compare_window;

use crate::{
    MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::solid_preview::preview_state::{SolidLastSolved, SolidPickState, SolidPreviewState},
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_vault::db::sqlite::Database;
use setup::{setup_solve_cancel_callback, setup_startup_restore, setup_tier_cutoff_callback};
use state::EditorState;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Wires up every editor callback (Tier form, Deep Solve/Optimize, Retarget, undo/
/// redo, native I/O, and the rest of [`callbacks`]) against `ui` and a freshly
/// constructed [`EditorState`], and starts [`auto_solve::init`]/the
/// [`activity::ActivityRegistry`] before any of them can fire. This group's main
/// public entry point -- see this module's own doc comment ("Module split").
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
    // Constructed once, before any callback (and therefore any possible
    // background-solve/Deep Solve/Optimize dispatch) is wired up -- see
    // `activity::ActivityRegistry`'s own doc comment.
    let activity = activity::ActivityRegistry::new(ui);
    // Before any callback (and therefore any possible background-solve dispatch) is
    // wired up -- see `auto_solve::init`'s own doc comment for why this module needs
    // these handles up front rather than reading them off `EditorState`.
    auto_solve::init(preview_state, solid_last_solved, &activity);
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
    native_io::setup_export_asc_callback(ui, &state, render_ctx);
    native_io::setup_export_cutting_sheet_callback(ui, &state, render_ctx);
    native_io::setup_export_diagram_callback(ui, &state);
    native_io::setup_save_native_callback(ui, &state, db, source, render_ctx);
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
    // The "Compute Tilt Curves" analysis action.
    callbacks::setup_batch_tilt_for_open_design_callback(ui, &state, render_ctx, db);
    // The guide pushes its static steps once and re-checks a step's goal against
    // `state` on entry; the shortcuts overlay is static data, and the template
    // gallery's own "Create" path goes through `setup_new_design_create_callback`
    // above (see `templates.rs`'s own doc comment).
    guide::setup_guide(ui, &state);
    shortcuts::setup_shortcuts_overlay(ui);
    templates::setup_template_gallery(ui);
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
    // Apply preform Y offset.
    callbacks::setup_apply_preform_y_offset_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    // Apply cheater offset.
    callbacks::setup_apply_cheater_offset_callback(ui, state, render_ctx);
    // Apply tier note.
    callbacks::setup_apply_tier_note_callback(ui, state, render_ctx);
    // Apply design metadata.
    callbacks::setup_apply_design_meta_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
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
    callbacks::setup_retarget_proposal_changed_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_retarget_apply_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    callbacks::setup_retarget_close_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    setup_editor_tertiary_callbacks(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
        solid_pick_state,
    );
}

/// The remainder of [`setup_editor_secondary_callbacks`]'s registrations
/// (Snapshot/Deep-Solve-pin/Optimize-preview/tier-cutoff-replan/tier-filter,
/// and the Solid-viewport hover/click/selection wiring) -- split out purely to
/// keep that function under clippy's function-length lint.
fn setup_editor_tertiary_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    // See [`setup_editor_callbacks`]'s own matching parameter doc comment.
    solid_pick_state: &SolidPickState,
) {
    // Snapshot Design / Compare to Snapshot -- see
    // `callbacks::retarget_actions::setup_snapshot_callbacks`'s own doc comment; the
    // command bar's Snapshot and Compare buttons (`editor_command_bar.slint`) call
    // `EditorModel.snapshot_design()`/`compare_to_snapshot()`.
    callbacks::setup_snapshot_callbacks(ui, state, solid_last_solved);
    // The visual compare window's entry points (Retarget/Optimize "Compare…",
    // the snapshot table's "Compare visually…").
    compare::setup_compare_callbacks(ui, state, render_ctx, preview_state, solid_last_solved);
    // Pin to verified mast -- see `callbacks::setup_deep_solve_pin_callback`'s
    // own doc comment; the per-tier pin control in `editor_status_strip.slint`'s
    // Deep Solve table calls `EditorModel.pin_verified_mast`.
    callbacks::setup_deep_solve_pin_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    // Optimize Preview toggle -- see `callbacks::setup_optimize_preview_callback`'s
    // own doc comment; the Preview checkbox in `editor_inspector/optimize_tab.slint`
    // calls `EditorModel.optimize_preview_toggled`.
    callbacks::setup_optimize_preview_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    // `EditorModel.tier_cutoff_replan` and its callback were deleted here.
    // The "Cut: N/M" slider's `changed` handler (`setup_tier_cutoff_callback`,
    // registered earlier in this function) already replans on every genuine drag
    // (the Slider uses an interaction callback instead of a property-change
    // handler); a second callback would only duplicate that work.
    callbacks::setup_tier_filter_callback(ui);
    // Live critical-angle guidance in the Tier form, the one-time anchor
    // explainer card, and "Set material" on an inferred-material guess.
    callbacks::setup_angle_live_preview_callback(ui, state);
    callbacks::setup_anchor_explainer_dismiss_callback(ui);
    callbacks::setup_material_guess_set_callback(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
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
    // The angle/depth/index drag handles on the selected facet, and the Snap pill.
    manipulate::setup_manipulate_callbacks(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
        solid_pick_state,
    );
}

/// Called from `gui::SlintSolidSink::apply` once a solid-preview
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
///
/// `render_ctx` is read here purely to hand [`view::push_solved_preview`] the
/// SAME custom-catalogue material list every other effective-RI readout uses, so
/// a design named after a custom material never has this specific path silently
/// score it against a built-in fallback. The same locked read also hands over
/// `RenderContext::custom_material_specific_gravity`, for the identical reason.
pub fn apply_matching_preview_frame(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    solved: &[SolvedTier],
) {
    if let Some((design, multi_selected)) = auto_solve::take_matching_design(generation) {
        let (custom_materials, custom_sg) = {
            let ctx = render_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                ctx.custom_materials.as_ref().clone(),
                Arc::clone(&ctx.custom_material_specific_gravity),
            )
        };
        view::push_solved_preview(
            ui,
            &design,
            solved,
            &multi_selected,
            &custom_materials,
            &custom_sg,
        );
        // This frame may be a partial (subgraph-resolved) replan, still showing
        // "one edit behind" -- a no-op when it isn't.
        auto_solve::schedule_idle_replan_if_stale(
            ui,
            render_ctx,
            generation,
            &design,
            &multi_selected,
        );
    }
}

/// Runs on the UI thread after every solid-preview frame has been stored (its pick
/// buffer, [`SolidPickState::geometry`] and the pushed images); the manipulation
/// handles refresh from here (see `manipulate::on_frame_landed`: a few `Mutex` reads
/// and one cached `FacetMap`, never a rebuild per orbit frame).
///
/// `generation` is the landed frame's own generation
/// (`PreviewFrame::generation`): a frame of the COMMITTED design landing while a
/// provisional slice is on screen has replaced the provisional picture, and the Slice
/// tool re-renders it (or, when the design moved on, discards the slice).
///
/// `pub` because its caller, `gui::solid_sink::SlintSolidSink::apply`, lives in `gui`,
/// outside this module.
pub fn on_solid_frame_landed(ui: &MainWindow, generation: u64) {
    manipulate::on_frame_landed(ui, generation);
}

/// Whether a solid-preview frame stamped `generation` describes the COMMITTED design
/// and so may update `solid_last_solved`, the tier table and the path tracer's planes.
/// `false` only for the Slice tool's reserved provisional generation
/// (`manipulate::PROVISIONAL_GENERATION`): such a frame shows a design that is not
/// (yet) the committed one.
///
/// `pub` because `gui::solid_sink::SlintSolidSink::apply` lives outside this module.
#[must_use]
pub const fn frame_updates_mast_cache(generation: u64) -> bool {
    manipulate::frame_updates_mast_cache(generation)
}

/// Hands the Slice tool the solved masts of a provisional-generation frame -- the sink
/// keeps them out of `solid_last_solved`, but the provisional tier's outline and handles
/// need them. A no-op without a provisional slice.
///
/// `pub` because `gui::solid_sink::SlintSolidSink::apply` lives outside this module.
pub fn note_provisional_frame_masts(masts: Vec<SolvedTier>) {
    manipulate::note_provisional_masts(masts);
}

/// Hands the Slice tool the plane arrangement a provisional-generation frame was drawn
/// from, so later camera / view-mode / background-solve redraws (which reproject the
/// committed planes) keep drawing the provisional facet. A no-op without a provisional
/// slice, and for planes that do not match the provisional design.
///
/// `pub` because `gui::solid_sink::SlintSolidSink::apply` lives outside this module.
pub fn note_provisional_frame_planes(planes: &[(glam::Vec3, f32)]) {
    manipulate::note_provisional_planes(planes);
}

// The one resolution of a catalogue record's facet planes (design file first, angle
// table as the fallback) -- the detail view's 3D preview and both batch engines
// call it. See `loading::catalogue_planes`' own module doc comment.
pub use loading::{CataloguePlanesSource, resolve_catalogue_planes};
