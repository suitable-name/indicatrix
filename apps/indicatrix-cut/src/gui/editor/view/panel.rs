//! The "real solve" panel refresh: [`refresh_editor_panel`]/
//! [`refresh_editor_panel_from_solve`] and the manufacturability/preform/yield/
//! proportion-verdict pushes that ride along with it. See this group's `mod.rs`
//! doc comment for the "Solve on explicit action, not on every edit" reasoning
//! this half of the split exists to honour -- [`super::panel_stale`] is the
//! no-solve counterpart.

use super::{
    inspector::refresh_design_settings,
    panel_stale::refresh_deep_solve_availability,
    state::{
        EditorState, ScratchDelta, apply_multi_selection, apply_proposed_angles,
        cutting_instructions_rows, design_label_text, girdle_and_ratio_texts,
        manufacturability_warnings_tagged, preform_mm_texts, preform_y_offset_mm_text,
        proportion_verdicts, proportions_texts, push_multi_selected_count, push_rows, push_tiers,
        status_text_and_is_problem, status_text_and_is_problem_from_solved, tier_items,
        tier_items_from_solved, yield_report_texts, yield_report_texts_from_solved,
    },
};
// The `_from_solved` proportion mirrors moved to `indicatrix_editor` with the rest of
// the yield view model; re-exported here (and through `view`) at their old paths.
use crate::{
    AngleItem, EditorModel, EditorTierItem, MainWindow, UndoRedoLabels,
    bridge::render_thread::RenderContext,
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, DesignSolveError, PreformShape};
pub(in crate::gui::editor) use indicatrix_editor::view_model::yield_report::{
    girdle_and_ratio_texts_from_solved, preform_mm_texts_from_solved, proportions_texts_from_solved,
};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering};

/// Refreshes everything `EditorView` itself renders (tier list, undo/redo
/// availability, validation banner, preform fields) -- but NOT the shared viewport,
/// see [`super::viewport::refresh_all`] for why those are kept separate.
///
/// Calls [`Design::solve`] exactly ONCE and derives every other panel field from
/// that SAME `solved_result` via its `_from_solved` counterpart or a small local
/// mirror of it (`proportions_texts_from_solved`/`preform_mm_texts_from_solved`
/// below -- `state` is a shared module this crate does not add functions to,
/// same reasoning `auto_solve::design_to_gpu_planes_from_solved` already documents
/// on itself). Deriving every field from that one solve avoids the six or more
/// independent solves that separately calling `tier_items`,
/// `status_text_and_is_problem` (itself two solves, via `status()`/`measure()`),
/// `manufacturability_warning_lines`, `yield_report_texts`, `proportions_texts`,
/// `preform_mm_texts`, and a final explicit `state.design.solve()` for
/// `cutting_rows` would cost for one "Solve" click.
///
/// Only used by [`super::viewport::refresh_all`] (New/Load/the explicit "Solve"
/// action). Every other edit callback uses
/// [`super::panel_stale::refresh_editor_panel_stale`] instead, which updates the
/// same fields except the ones that require a solve.
///
/// Returns the solve result so [`super::viewport::refresh_all`] can hand it to
/// [`super::viewport::refresh_viewport`] without that function paying for a
/// SECOND solve of its own. A thin wrapper around
/// [`refresh_editor_panel_from_solve`] that solves `state.design` itself. See
/// that function's own doc comment for the caller (`refresh_all`'s synchronous
/// branch) that instead already has a solve result on hand and would otherwise
/// pay for a second, redundant one: `refresh_all` solves once to measure
/// `Design::solve`'s wall time, then passes that same result into this function
/// instead of letting it solve again, keeping the click to one real solve.
pub(in crate::gui::editor) fn refresh_editor_panel(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) -> Option<Vec<SolvedTier>> {
    let solved_result = state.design.solve();
    refresh_editor_panel_from_solve(ui, render_ctx, state, solved_result)
}

/// [`refresh_editor_panel`]'s own body, minus the `Design::solve()` call itself --
/// takes an already-computed `solved_result` instead, so a caller that solved
/// `state.design` for its own reasons (timing it, most commonly) can hand that
/// SAME result in rather than triggering a second, thrown-away solve.
/// [`super::viewport::refresh_all`] is exactly that caller: it needs to measure
/// `Design::solve`'s own wall time (comparable to
/// `auto_solve::dispatch_background_solve`'s own measurement, see that call
/// site's doc comment) without also timing this function's UI-model pushes, so
/// it solves once, records the duration, and passes the result here instead of
/// letting this function solve again.
pub(in crate::gui::editor) fn refresh_editor_panel_from_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    solved_result: Result<Vec<SolvedTier>, DesignSolveError>,
) -> Option<Vec<SolvedTier>> {
    // Computed exactly once per refresh -- see `ScratchDelta`'s own doc
    // comment for why each group below is gated on its OWN flag rather than
    // "anything changed".
    let delta = state.record_scratch_push();
    let n_d = refresh_design_settings(ui, render_ctx, state, &delta);
    // Derived from the same solve everything else on this refresh uses, never a
    // second one. Zero when the design does not currently solve, which reads as
    // "nothing to count yet" rather than as a stale figure.
    let facet_count = solved_result.as_ref().ok().map_or(0, |solved| {
        i32::try_from(super::solve_results::facet_count_from_solved(
            &state.design,
            solved,
        ))
        .unwrap_or(i32::MAX)
    });
    ui.global::<EditorModel>().set_facet_count(facet_count);
    let solved = solved_result.as_ref().ok();

    let tiers = solved.map_or_else(
        || tier_items(&state.design, n_d),
        |solved| tier_items_from_solved(&state.design, solved, n_d),
    );
    push_tier_list_and_undo_redo(ui, state, tiers);
    // The inspector form follows the design the list now shows: an adopt, retarget
    // apply, optimize apply or deep-solve pin changes the selected tier's values.
    super::panel_stale::sync_tier_form_with_design(ui, state);

    let (status_text, is_problem) = solved.map_or_else(
        || status_text_and_is_problem(&state.design),
        |solved| status_text_and_is_problem_from_solved(&state.design, solved),
    );
    ui.global::<EditorModel>()
        .set_status_text(status_text.into());
    ui.global::<EditorModel>().set_status_is_problem(is_problem);
    ui.global::<EditorModel>()
        .set_solve_state(if is_problem { "failed" } else { "solved" }.into());

    // Opens the one-time anchor explainer card the first time (this session) a
    // design lacks an anchor -- see `state::should_open_anchor_explainer`'s own
    // doc comment.
    if super::state::should_open_anchor_explainer(solved_result.is_err()) {
        ui.global::<EditorModel>().set_anchor_explainer_open(true);
    }

    push_solve_dependent_panel_fields(ui, render_ctx, state, &delta, solved, n_d);

    ui.global::<EditorModel>()
        .set_design_label(design_label_text(state.asc_filename.as_deref()).into());
    let rows: Vec<AngleItem> = solved.map_or_else(Vec::new, |solved| {
        cutting_instructions_rows(&state.design, solved)
    });
    push_rows(
        &ui.global::<EditorModel>().get_cutting_rows(),
        rows,
        |model| {
            ui.global::<EditorModel>().set_cutting_rows(model);
        },
    );

    refresh_deep_solve_availability(ui, state);
    super::panel_stale::refresh_optimize_availability(ui, state);
    solved_result.ok()
}

/// The manufacturability-warnings/preform-mm/yield/proportions push
/// [`refresh_editor_panel_from_solve`] makes for this same "Solve" click's
/// state -- split out purely to keep that function under clippy's
/// function-length lint. `solved` is the SAME solve that function's own
/// caller already has; `None` only for a design that does not currently solve
/// at all, in which case every one of these falls back to its own internally-
/// solving form exactly like [`refresh_editor_panel_from_solve`] itself does
/// for the tier list/status text just above this call.
fn push_solve_dependent_panel_fields(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
    solved: Option<&Vec<SolvedTier>>,
    n_d: f64,
) {
    push_manufacturability_and_preform_scratch(ui, state, delta, solved);
    push_yield_and_proportions(ui, render_ctx, state, delta, solved, n_d);
}

/// The manufacturability-warnings/preform-mm push half of
/// [`push_solve_dependent_panel_fields`] -- split out purely to keep that
/// function (and this one) under clippy's function-length lint.
fn push_manufacturability_and_preform_scratch(
    ui: &MainWindow,
    state: &EditorState,
    delta: &ScratchDelta,
    solved: Option<&Vec<SolvedTier>>,
) {
    // Kept as tagged pairs (not the flattened `manufacturability_warning_lines`/
    // `_from_solved` text-only list) so the tier index survives to
    // `manufacturability_warning_tiers`.
    let tagged_warnings =
        manufacturability_warnings_tagged(&state.design, solved.map(Vec::as_slice));
    let warning_tiers: Vec<i32> = tagged_warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    let warnings: Vec<SharedString> = tagged_warnings
        .into_iter()
        .map(|(_, text)| SharedString::from(text))
        .collect();
    push_rows(
        &ui.global::<EditorModel>().get_manufacturability_warnings(),
        warnings,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warnings(model);
        },
    );
    push_rows(
        &ui.global::<EditorModel>()
            .get_manufacturability_warning_tiers(),
        warning_tiers,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warning_tiers(model);
        },
    );

    if delta.preform {
        let preform = &state.design.preform;
        ui.global::<EditorModel>()
            .set_preform_shape_index(match preform.shape {
                PreformShape::Block => 0,
                PreformShape::Cylinder { .. } => 1,
            });
        ui.global::<EditorModel>()
            .set_preform_half_width(format!("{:.4}", preform.half_width).into());
        ui.global::<EditorModel>()
            .set_preform_length_over_width(format!("{:.4}", preform.length_over_width).into());
        ui.global::<EditorModel>()
            .set_preform_depth(format!("{:.4}", preform.depth).into());
        // The mm equivalent shown ALONGSIDE the model-unit fields above        // `preform_mm_texts`'s own doc comment.
        let (preform_half_width_mm, preform_depth_mm) = solved.map_or_else(
            || preform_mm_texts(&state.design),
            |solved| preform_mm_texts_from_solved(&state.design, solved),
        );
        ui.global::<EditorModel>()
            .set_preform_half_width_mm_text(preform_half_width_mm.into());
        ui.global::<EditorModel>()
            .set_preform_depth_mm_text(preform_depth_mm.into());
        // `mm_per_unit` comes from this SAME solve, matching
        // `preform_half_width_mm`/`preform_depth_mm` above.
        let mm_per_unit = solved.and_then(|solved| state.design.yield_report(solved).mm_per_unit);
        ui.global::<EditorModel>().set_preform_y_offset_mm(
            preform_y_offset_mm_text(state.design.preform_y_offset, mm_per_unit).into(),
        );
    }
}

/// The yield/proportions/girdle-ratio push half of
/// [`push_solve_dependent_panel_fields`] -- split out purely to keep that
/// function (and this one) under clippy's function-length lint.
///
/// `render_ctx` is read here purely for
/// [`RenderContext::custom_material_specific_gravity`]: a
/// cheap `Arc` clone under the lock, handed to [`yield_report_texts`]/
/// [`yield_report_texts_from_solved`] so a custom catalogue material's own
/// recorded specific gravity reaches the carat-weight estimate -- see those
/// functions' own doc comments for why `gui::editor::state` takes this as a
/// plain parameter rather than reaching for `RenderContext` itself.
fn push_yield_and_proportions(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
    solved: Option<&Vec<SolvedTier>>,
    n_d: f64,
) {
    // Seed the Yield form's scratch buffers from the design's current state
    // (only when it actually changed -- see `delta`'s own doc comment), then
    // push the read-only figures this same "Solve" click's state produces
    // (always, since those are never user-editable).
    push_yield_material_scratch(ui, state, delta);
    let custom_sg = Arc::clone(
        &render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .custom_material_specific_gravity,
    );
    let (vol_yield_text, carat_text, sg_used_text, fit_text) = solved.map_or_else(
        || yield_report_texts(&state.design, &custom_sg),
        |solved| yield_report_texts_from_solved(&state.design, solved, &custom_sg),
    );
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());

    let (table_pct, crown_height, pavilion_depth, total_depth, length_to_width) = solved
        .map_or_else(
            || proportions_texts(&state.design),
            |solved| proportions_texts_from_solved(&state.design, solved),
        );
    ui.global::<EditorModel>()
        .set_proportions_table_pct(table_pct.into());
    ui.global::<EditorModel>()
        .set_proportions_crown_height(crown_height.into());
    ui.global::<EditorModel>()
        .set_proportions_pavilion_depth(pavilion_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_total_depth(total_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_length_to_width(length_to_width.into());

    // Girdle thickness and the printed C/W%/P/W%/G/W% ratios `proportions_texts`
    // above still does not expose -- see `girdle_and_ratio_texts`'s own doc
    // comment.
    let (girdle_thickness, crown_to_width, pavilion_to_width, girdle_to_width) = solved
        .map_or_else(
            || girdle_and_ratio_texts(&state.design),
            |solved| girdle_and_ratio_texts_from_solved(&state.design, solved),
        );
    ui.global::<EditorModel>()
        .set_girdle_thickness_text(girdle_thickness.into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text(crown_to_width.into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text(pavilion_to_width.into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text(girdle_to_width.into());

    push_proportion_verdicts_from_solved(ui, &state.design, solved.map(Vec::as_slice), n_d);
}

/// the ONE place that computes AND pushes the Preform tab's five
/// "within"/"near"/"outside" proportion-verdict chips against
/// `indicatrix_cut_core::proportions_windows`'s reference table (see
/// [`super::state::proportion_verdicts`]'s own doc comment) -- shared by the
/// synchronous "Solve" push ([`push_yield_and_proportions`], above),
/// `auto_solve::apply::push_solve_dependent_background_fields` (the debounced
/// background-solve completion), and `view::viewport::push_solved_preview`
/// (a completed solid-preview replan). Every path that publishes a solve's
/// numbers pushes the chips with them, so a design solved in the background
/// (any design the solve policy does not run inline:
/// `indicatrix_editor::solve_policy::should_solve_synchronously_for` runs inline
/// only at most 32 planes, at most 8 meet-derived tiers, no tier targets and no
/// slow last measurement) never keeps chips from an older solve next to fresh
/// numbers.
///
/// Re-measures the SAME `solved` mast list `proportions_texts_from_solved`/
/// `girdle_and_ratio_texts_from_solved` already read from (a cheap geometry
/// measurement, not a re-solve). Pushes every chip's "nothing to judge yet"
/// level (`-1`) and an empty reason when `solved` is `None` (design not
/// currently solved/closed) OR `design.tiers.is_empty()` (the SAME
/// tierless guard the Preform tab's own proportions readouts already apply --
/// `state::yield_report::proportions_texts`'s own doc comment -- a bare,
/// uncut preform is not a stone whose crown/pavilion angles or table percent
/// mean anything to judge).
///
/// Deliberately never spells out `state::yield_report::ProportionVerdicts`
/// (this function's own local `verdicts` binding, or a return type, or a
/// parameter -- nowhere): that type lives in a `state` submodule private to
/// `state` itself (`mod yield_report;`, no re-export of the type, only of the
/// [`proportion_verdicts`] function that builds one), so no file outside
/// `state/` can name it at all. Computing it in a type-inferred local and
/// consuming it fully within this one function (rather than splitting
/// "compute" from "push" across two functions, which WOULD need to name it)
/// is what lets `view`/`auto_solve` share this logic without a
/// `state/mod.rs` re-export outside this module.
pub(in crate::gui::editor) fn push_proportion_verdicts_from_solved(
    ui: &MainWindow,
    design: &Design,
    solved: Option<&[SolvedTier]>,
    n_d: f64,
) {
    // The overall Good / Check / Problem verdict follows the same solves these chips do
    // (see `gui::editor::verdict`): one call here covers the synchronous Solve, a background
    // solve's completion and a finished solid-preview replan.
    crate::gui::editor::verdict::on_solved(ui, design, solved, n_d);
    let verdicts = (!design.tiers.is_empty())
        .then(|| solved.and_then(|solved| design.stone_proportions(solved)))
        .flatten()
        .map(|proportions| proportion_verdicts(design, &proportions, n_d));
    let push = |level: i32,
                reason: &str,
                set_level: fn(&MainWindow, i32),
                set_reason: fn(&MainWindow, SharedString)| {
        set_level(ui, level);
        set_reason(ui, reason.into());
    };
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.table_pct.level, v.table_pct.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_table_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_table_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.crown_angle.level, v.crown_angle.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_crown_angle_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_crown_angle_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.pavilion_angle.level, v.pavilion_angle.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_pavilion_angle_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_pavilion_angle_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.total_depth_pct.level, v.total_depth_pct.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_total_depth_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_total_depth_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.girdle_pct.level, v.girdle_pct.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_girdle_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_girdle_reason(v);
        },
    );
}

/// Patches `state.pending_optimize`'s
/// `AngleChange`s onto `tiers` via [`apply_proposed_angles`] -- but only when
/// that pending result still applies to `state`'s CURRENT generation, the same
/// check [`super::panel_stale::refresh_optimize_availability`] already makes
/// for `EditorModel.optimize_can_apply`. A superseded result (an edit landed
/// since Optimize last ran) must not paint a ghost angle for a design it no
/// longer describes.
fn apply_pending_optimize_ghost(tiers: &mut [EditorTierItem], state: &EditorState) {
    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    let pending = state
        .pending_optimize
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some((outcome, started_generation)) = pending.as_ref()
        && *started_generation == current_generation
    {
        apply_proposed_angles(tiers, &outcome.changes);
    }
}

/// Applies the multi-select highlight, pushes `tiers` into `EditorModel.tiers`
/// (plus the multi-select count and the selected tier's facet-chip row), and
/// refreshes the undo/redo buttons' enabled state and labels -- the common tail
/// [`refresh_editor_panel`] and [`super::panel_stale::push_stale_content`] share,
/// since they differ only in HOW `tiers` itself gets built (a real solve vs. the
/// no-solve placeholder). Split out so neither caller grows past this crate's
/// hundred-line function guideline.
///
/// `pub(super)`, not private: [`super::panel_stale::push_stale_content`] (a
/// sibling file) shares this exact tail.
pub(super) fn push_tier_list_and_undo_redo(
    ui: &MainWindow,
    state: &EditorState,
    mut tiers: Vec<EditorTierItem>,
) {
    apply_multi_selection(&mut tiers, &state.multi_selected);
    apply_pending_optimize_ghost(&mut tiers, state);
    push_tiers(ui, tiers);
    // Pushed from the `&EditorState` already in hand, NOT through the
    // `recompute_dirty` callback that `changed tiers` fires. That callback runs
    // synchronously from inside `push_tiers` above, and almost every caller here is
    // holding `state.borrow_mut()` at the time, so reading the `RefCell` from
    // inside it panicked with "already mutably borrowed" on New and Load Selected.
    // Computing it here is also strictly more correct: it is the same state that
    // produced the rows, so the marker can never describe a different one.
    ui.global::<EditorModel>().set_is_dirty(state.is_dirty());
    push_multi_selected_count(ui, state.multi_selected.len());
    super::inspector::push_selected_tier_chips(ui, state);
    ui.global::<EditorModel>()
        .set_can_undo(state.history.can_undo());
    ui.global::<EditorModel>()
        .set_can_redo(state.history.can_redo());
    // A step with words of its own ("Open variant ...") keeps them in the hints.
    ui.global::<UndoRedoLabels>()
        .set_undo_label(state.undo_hint().into());
    ui.global::<UndoRedoLabels>()
        .set_redo_label(state.redo_hint().into());
}

/// Pushes the Yield form's girdle-diameter/material scratch fields from the
/// design's current state, each gated on its own [`ScratchDelta`] flag exactly
/// like every other group here (see that type's own doc comment) -- shared by
/// [`refresh_editor_panel`] and [`super::panel_stale::push_stale_content`] since
/// both seed the same two fields from the same design state, whether or not a
/// solve has just run.
///
/// `pub(super)`, not private: [`super::panel_stale::push_stale_preform_and_yield`]
/// (a sibling file) shares this exact seed.
pub(super) fn push_yield_material_scratch(
    ui: &MainWindow,
    state: &EditorState,
    delta: &ScratchDelta,
) {
    if delta.girdle {
        ui.global::<EditorModel>().set_girdle_diameter_mm(
            state
                .design
                .girdle_diameter_mm
                .map_or_else(String::new, |mm| format!("{mm:.4}"))
                .into(),
        );
    }
    if delta.material {
        // no longer pushes `EditorModel.material_index` -- the Preform
        // tab's "Yield Material" combo that property fed was removed
        // (`ui/components/editor_inspector/preform_tab.slint`): its selected
        // index was never actually read back (`state::parse_yield_form`'s own
        // `_material_index` parameter stayed unused), so keeping a dead
        // control's display in sync with the design's real material served
        // no purpose. The Design Settings panel's own material combo
        // (`EditorModel.material_combo_index`, `view::inspector::
        // refresh_design_settings`) is the one that actually names/changes it.
        ui.global::<EditorModel>().set_specific_gravity_override(
            state
                .design
                .material
                .specific_gravity_override
                .map_or_else(String::new, |sg| format!("{sg:.4}"))
                .into(),
        );
    }
}
