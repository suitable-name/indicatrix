//! Pushing [`EditorState`](super::state::EditorState) into `EditorView`/the shared
//! viewport ([`refresh_editor_panel`]/[`refresh_viewport`]/[`refresh_all`]/
//! [`refresh_editor_panel_stale`]), and the Deep Solve/Optimize hint and result-
//! formatting helpers. See this group's own `mod.rs` doc comment for the "Solve on
//! explicit action, not on every edit" reasoning [`refresh_editor_panel_stale`] exists
//! to honour.

use super::{
    auto_solve,
    deep_solve::TierMastDelta,
    material_lookup::{
        EditorMaterialLookup, MATERIAL_MATCH_TOLERANCE, material_for_refractive_index,
        material_guess_candidates, nearest_built_in_material, traced_gem_material,
    },
    stale,
    state::{
        EditorState, ScratchDelta, apply_multi_selection, apply_proposed_angles,
        builtin_preset_names, cutting_schedule_rows, design_label_text,
        design_material_index_from_name, design_to_gpu_planes, gear_index_from_teeth,
        girdle_and_ratio_texts, index_chip_items, manufacturability_warnings_tagged,
        material_index_from_name, preform_mm_texts, preform_y_offset_mm_text, proportion_verdicts,
        proportions_texts, push_multi_selected_count, push_rows, push_tiers, result_is_stale,
        ri_source_text, should_open_anchor_explainer, status_text_and_is_problem,
        status_text_and_is_problem_from_solved, tier_items, tier_items_from_solved,
        tier_items_stale_with_last_solved, yield_report_texts, yield_report_texts_from_solved,
    },
};
use crate::{
    AngleItem, DeepSolveTierRow, EditorModel, EditorTierItem, IndexChipItem, MainWindow,
    OptimizeChangeRow, OptimizeResultRow, SolidPreviewModel, TiltModel, UndoRedoLabels,
    ViewportModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        render::camera_lighting::contained_request_size,
        solid_preview::preview_state::{CameraPose, ReplanRequest, SolidPreviewState},
    },
};
// Re-exported so sibling `callbacks::*` modules can spell this `view::SolidLastSolved`
// (matching how they already reach every other `view::*` helper) rather than reaching
// past this module into `solid_preview::preview_state` directly.
pub(in crate::gui::editor) use crate::gui::solid_preview::preview_state::SolidLastSolved;
use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier, VerifiedSolveReport};
use indicatrix_cut_core::{
    Design, DesignSolveError, MaterialLookup, ObjectiveWeights, OptimizeOutcome, PreformShape,
    critical_angle_deg, free_tier_indices,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
    time::{Duration, Instant},
};

/// Refreshes everything `EditorView` itself renders (tier list, undo/redo
/// availability, validation banner, preform fields) -- but NOT the shared viewport,
/// see [`refresh_all`] for why those are kept separate.
///
/// Calls [`Design::solve`] exactly ONCE and derives every other panel field from
/// that SAME `solved_result` via its `_from_solved` counterpart or a small local
/// mirror of it (`proportions_texts_from_solved`/`preform_mm_texts_from_solved`
/// below -- `state/mod.rs` is a shared file this module does not add functions to,
/// same reasoning `auto_solve::design_to_gpu_planes_from_solved` already documents
/// on itself). Deriving every field from that one solve avoids the six or more
/// independent solves that separately calling `tier_items`,
/// `status_text_and_is_problem` (itself two solves, via `status()`/`measure()`),
/// `manufacturability_warning_lines`, `yield_report_texts`, `proportions_texts`,
/// `preform_mm_texts`, and a final explicit `state.design.solve()` for
/// `cutting_rows` would cost for one "Solve" click.
///
/// Only used by [`refresh_all`] (New/Load/the explicit "Solve" action). Every
/// other edit callback uses [`refresh_editor_panel_stale`] instead, which updates
/// the same fields except the ones that require a solve.
///
/// Returns the solve result so [`refresh_all`] can hand it to [`refresh_viewport`]
/// without that function paying for a SECOND solve of its own. A thin wrapper
/// around [`refresh_editor_panel_from_solve`] that solves `state.design` itself --
/// see that function's own doc comment for the caller (`refresh_all`'s
/// synchronous branch) that instead already has a solve result on hand and would
/// otherwise pay for a second, redundant one: `refresh_all` solves once to measure
/// `Design::solve`'s wall time, then passes that same result into this function
/// instead of letting it solve again, keeping the click to one real solve.
pub(super) fn refresh_editor_panel(
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
/// SAME result in rather than triggering a second, thrown-away solve. `refresh_all`
/// is exactly that caller: it needs to measure `Design::solve`'s own wall time
/// (comparable to `auto_solve::dispatch_background_solve`'s own measurement, see
/// that call site's doc comment) without also timing this function's UI-model
/// pushes, so it solves once, records the duration, and passes the result here
/// instead of letting this function solve again.
pub(super) fn refresh_editor_panel_from_solve(
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
        i32::try_from(facet_count_from_solved(&state.design, solved)).unwrap_or(i32::MAX)
    });
    ui.global::<EditorModel>().set_facet_count(facet_count);
    let solved = solved_result.as_ref().ok();

    let tiers = solved.map_or_else(
        || tier_items(&state.design, n_d),
        |solved| tier_items_from_solved(&state.design, solved, n_d),
    );
    push_tier_list_and_undo_redo(ui, state, tiers);

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
    if should_open_anchor_explainer(solved_result.is_err()) {
        ui.global::<EditorModel>().set_anchor_explainer_open(true);
    }

    push_solve_dependent_panel_fields(ui, render_ctx, state, &delta, solved, n_d);

    ui.global::<EditorModel>()
        .set_design_label(design_label_text(state.asc_filename.as_deref()).into());
    let rows: Vec<AngleItem> = solved.map_or_else(Vec::new, |solved| {
        cutting_schedule_rows(&state.design, solved)
    });
    push_rows(
        &ui.global::<EditorModel>().get_cutting_rows(),
        rows,
        |model| {
            ui.global::<EditorModel>().set_cutting_rows(model);
        },
    );

    refresh_deep_solve_availability(ui, state);
    refresh_optimize_availability(ui, state);
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
        // The mm equivalent shown ALONGSIDE the model-unit fields above -- see
        // `preform_mm_texts`'s own doc comment.
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

    push_proportion_verdicts(ui, &state.design, solved, n_d);
}

/// The verdict-chip push half of [`push_yield_and_proportions`] -- split out
/// purely to keep that function under clippy's function-length lint. Pushes
/// "within" / "near" / "outside" chips next to the raw proportion numbers
/// [`push_yield_and_proportions`]
/// already pushes, judged against `indicatrix_cut_core::proportions_windows`'s
/// reference table -- see [`super::state::proportion_verdicts`]'s own doc
/// comment. Re-measures the SAME `solved` mast list
/// `proportions_texts_from_solved`/`girdle_and_ratio_texts_from_solved`
/// already read from (a cheap geometry measurement, not a re-solve); `None`
/// (design not solved/closed) pushes every chip's "nothing to judge yet"
/// level (`-1`) and an empty reason.
fn push_proportion_verdicts(
    ui: &MainWindow,
    design: &Design,
    solved: Option<&Vec<SolvedTier>>,
    n_d: f64,
) {
    let verdicts = solved
        .and_then(|solved| design.stone_proportions(solved))
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

/// [`proportions_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`refresh_editor_panel`]'s own doc
/// comment for why this small mirror lives here rather than as a new function on
/// `state/mod.rs` (a shared file this module does not add functions to). Mirrors
/// that function's body exactly, minus the internal `design.solve()` it exists to
/// avoid repeating -- INCLUDING its `tiers.is_empty()` guard: a tierless design
/// still solves (an empty mast list is a valid, closed, zero-plane solve), so a
/// caller here can perfectly well be holding exactly that solved-empty list, and
/// without this guard `stone_proportions` would measure the bare preform block as
/// if it were the stone. `pub(super)` (not just private) so `auto_solve::
/// apply_background_solve_result` can call this directly to close the same
/// background-solve panel-field gap.
#[must_use]
pub(super) fn proportions_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash(), dash());
    }
    let Some(proportions) = design.stone_proportions(solved) else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let table_percent_text = proportions
        .table_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let crown_height_text = proportions
        .crown_height
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let pavilion_depth_text = proportions
        .pavilion_depth
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let total_depth_text = format!("{:.3}{unit}", proportions.total_depth);
    let length_to_width_text = proportions
        .length_to_width
        .map_or_else(dash, |v| format!("{v:.3}"));
    (
        table_percent_text,
        crown_height_text,
        pavilion_depth_text,
        total_depth_text,
        length_to_width_text,
    )
}

/// [`girdle_and_ratio_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`proportions_texts_from_solved`]'s
/// own doc comment for why this small mirror lives here rather than as a new
/// function on `state/mod.rs`. Mirrors that function's body exactly, minus the
/// internal `design.solve()` it exists to avoid repeating.
///
/// A tierless design does NOT skip `Design::solve` -- an empty mast list is a
/// valid, closed, zero-plane solve -- so a caller here can perfectly well be
/// holding exactly that solved-empty list. This mirror carries the SAME
/// `tiers.is_empty()` guard [`girdle_and_ratio_texts`] documents, not a weaker
/// one, since without it `stone_proportions` would measure the bare preform
/// block (girdle 50% of a cube, crown/pavilion 0%) as if it were the stone --
/// the hazard sits on `refresh_editor_panel_from_solve`'s hot path
/// (`push_yield_and_proportions` below, which always prefers this `_from_solved`
/// mirror over the guarded plain function once a design solves). `pub(super)`
/// (not just private) so `auto_solve::apply_background_solve_result` can call
/// this directly.
#[must_use]
pub(super) fn girdle_and_ratio_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash());
    }
    let Some(proportions) = design.stone_proportions(solved) else {
        return (dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let girdle_thickness_text = proportions
        .girdle_thickness
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let crown_to_width_percent_text = proportions
        .crown_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let pavilion_to_width_percent_text = proportions
        .pavilion_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let girdle_to_width_percent_text = proportions
        .girdle_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    (
        girdle_thickness_text,
        crown_to_width_percent_text,
        pavilion_to_width_percent_text,
        girdle_to_width_percent_text,
    )
}

/// [`preform_mm_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`proportions_texts_from_solved`]'s
/// own doc comment for why this lives here. `pub(super)` (not just private) so
/// `auto_solve::apply_background_solve_result` can call this directly.
#[must_use]
pub(super) fn preform_mm_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String) {
    let Some(mm_per_unit) = design.yield_report(solved).mm_per_unit else {
        return (String::new(), String::new());
    };
    let preform = &design.preform;
    (
        format!("\u{2248} {:.3} mm", preform.half_width * mm_per_unit),
        format!("\u{2248} {:.3} mm", preform.depth * mm_per_unit),
    )
}

/// Patches `state.pending_optimize`'s
/// `AngleChange`s onto `tiers` via [`apply_proposed_angles`] -- but only when
/// that pending result still applies to `state`'s CURRENT generation, the same
/// check [`refresh_optimize_availability`] already makes for `EditorModel.
/// optimize_can_apply`. A superseded result (an edit landed since Optimize
/// last ran) must not paint a ghost angle for a design it no longer describes.
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
/// [`refresh_editor_panel`] and [`push_stale_content`] share, since they differ
/// only in HOW `tiers` itself gets built (a real solve vs. the no-solve
/// placeholder). Split out so neither caller grows past this crate's
/// hundred-line function guideline.
fn push_tier_list_and_undo_redo(
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
    push_selected_tier_chips(ui, state);
    ui.global::<EditorModel>()
        .set_can_undo(state.history.can_undo());
    ui.global::<EditorModel>()
        .set_can_redo(state.history.can_redo());
    ui.global::<UndoRedoLabels>().set_undo_label(
        state
            .history
            .peek_undo()
            .map_or_else(String::new, |e| e.describe(&state.design))
            .into(),
    );
    ui.global::<UndoRedoLabels>().set_redo_label(
        state
            .history
            .peek_redo()
            .map_or_else(String::new, |e| e.describe(&state.design))
            .into(),
    );
}

/// Pushes the Yield form's girdle-diameter/material scratch fields from the
/// design's current state, each gated on its own [`ScratchDelta`] flag exactly
/// like every other group here (see that type's own doc comment) -- shared by
/// [`refresh_editor_panel`] and [`push_stale_content`] since both seed the same
/// two fields from the same design state, whether or not a solve has just run.
fn push_yield_material_scratch(ui: &MainWindow, state: &EditorState, delta: &ScratchDelta) {
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
        ui.global::<EditorModel>()
            .set_material_index(material_index_from_name(
                state.design.material.name.as_deref(),
            ));
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

/// Pushes the design settings panel's state (material combo options and index,
/// RI-override/effective-RI/critical-angle readouts, gear/symmetry/mirror) and,
/// while "linked to design" is on, syncs the shared viewport's render material
/// (and its displayed selection/index) to match -- a display override made while
/// unlinked must never be silently overridden. Shared by [`refresh_editor_panel`]/
/// [`refresh_editor_panel_stale`].
///
/// `viewport_material_linked` alone is the real gate a display override needs.
/// Also requiring `render_view_tab == 1` (the Edit tab itself being shown)
/// would tie the render's material to tab history rather than to the design:
/// switching to Live Render and back would never resync anything until the
/// next edit happened to run this function again.
///
/// Returns this design's effective refractive index so [`tier_items`]/
/// [`tier_items_stale`] can reuse the identical value for their per-tier
/// margin/risk column rather than re-deriving it.
///
/// `delta` (from [`EditorState::record_scratch_push`], computed once by the
/// caller) gates the material/gear/symmetry scratch pushes independently, so
/// an edit that only changed one of the three never re-seeds -- and so
/// silently discards any in-progress typing/selection in -- the other two's
/// fields.
/// The material name the tracer should use for `design`, and -- when there is no
/// honest answer -- the sentence saying why it will not trace at all.
///
/// A design built from an `.asc`, or a brand-new one, carries
/// `MaterialSelection::none()`: the schedule records a refractive index but never a
/// species. Substituting `"Diamond"` in that case would let a quartz design get
/// traced, tilt-swept and HUD-scored at n=2.417 while MARGIN and the critical angle
/// beside them use its real n=1.5442 -- two numbers on screen contradicting each
/// other with no hint why, and angles that window badly in quartz looking fine in
/// the render.
///
/// The rule, preferred over silently substituting anything: use the
/// design's own named material when it has one; otherwise the nearest built-in
/// within [`MATERIAL_MATCH_TOLERANCE`] of its actual refractive index; and when
/// nothing is that close, refuse. A `Some(reason)` suspends both tracing and metrics
/// (see `bridge::render_thread::frame_helpers::SuspensionFlags`), and the reason is
/// shown in place of a simulation nobody should trust.
fn traced_material_for(
    design: &Design,
    custom: &[indicatrix::optics::materials::GemMaterial],
) -> (String, Option<String>) {
    if let Some(name) = &design.material.name {
        // Checked against the SAME catalogue [`EditorMaterialLookup`] resolves
        // through, so this can never disagree with what actually gets traced.
        // Returning `(name.clone(), None)` unconditionally for ANY named selection
        // would let a design naming a custom material since deleted (or an old
        // `.asc`'s hand-typed/typo'd name) trace as "resolved" with no override
        // set -- `sync_viewport_material_link` would then fall through to
        // `resolve_material`'s own by-name lookup, which silently substitutes
        // `materials[0]` (Diamond) for an unrecognized name.
        return if EditorMaterialLookup::new(custom).lookup(name).is_some() {
            (name.clone(), None)
        } else {
            (
                String::new(),
                Some(format!(
                    "This design's material '{name}' is not a built-in preset or a \
                     saved custom material, so there is nothing to trace -- pick a \
                     material in Design Settings, or re-save the missing custom \
                     material, rather than rendering it as something else."
                )),
            )
        };
    }
    let n_d = design.effective_refractive_index();
    if let Some((name, _)) = nearest_built_in_material(n_d, MATERIAL_MATCH_TOLERANCE) {
        return (name, None);
    }
    // The name is left empty deliberately: nothing should trace, so there is no
    // material to name, and a plausible-looking placeholder here is exactly the bug.
    (
        String::new(),
        Some(format!(
            "This design names no material, and its refractive index ({n_d:.4}) matches \
             no built-in preset within {MATERIAL_MATCH_TOLERANCE:.2}. Pick a material in \
             Design Settings -- rendering it as something else would give you the optics \
             of a different stone."
        )),
    )
}

/// The material-guess badge's push half of [`refresh_design_settings`] --
/// split out purely to keep that function under clippy's function-length
/// lint. Inferred material is shown as a guess, never as a fact: when
/// `design.material.name` is `None`
/// (every untouched `.asc` import carries only an `I`-line refractive index,
/// no species), this looks up the nearest built-in preset within tolerance
/// ([`material_for_refractive_index`]) and pushes a guess label plus the
/// OTHER close candidates ([`material_guess_candidates`]) for the tooltip.
/// Once a name IS set, every one of these three properties goes back to
/// `""` -- there is nothing left to guess, and [`super::state::
/// proportion_verdicts`]-style "guess vs fact" confusion is exactly what this
/// exists to prevent.
fn push_material_guess(ui: &MainWindow, design: &Design, n_d: f64) {
    let clear = || {
        ui.global::<EditorModel>()
            .set_material_guess_text(SharedString::new());
        ui.global::<EditorModel>()
            .set_material_guess_name(SharedString::new());
        ui.global::<EditorModel>()
            .set_material_guess_other_candidates_text(SharedString::new());
    };
    if design.material.name.is_some() {
        clear();
        return;
    }
    let Some((name, _)) = material_for_refractive_index(n_d) else {
        clear();
        return;
    };
    ui.global::<EditorModel>()
        .set_material_guess_text(format!("{name}? (from RI {n_d:.2})").into());
    ui.global::<EditorModel>()
        .set_material_guess_name(name.clone().into());
    let others: Vec<String> = material_guess_candidates(n_d, MATERIAL_MATCH_TOLERANCE)
        .into_iter()
        .filter(|(candidate, _)| candidate != &name)
        .map(|(candidate, ri)| format!("{candidate} ({ri:.3})"))
        .collect();
    ui.global::<EditorModel>()
        .set_material_guess_other_candidates_text(if others.is_empty() {
            SharedString::new()
        } else {
            format!("Also within tolerance: {}", others.join(", ")).into()
        });
}

fn refresh_design_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
) -> f64 {
    let design = &state.design;

    // Scoped so the `RenderContext` lock (a shared resource the render thread
    // also wants every frame) is held only for the work that actually needs
    // it, and is a real `Drop` guard released at the end of this block rather
    // than sitting locked across the `delta`-gated pushes below, which touch
    // only `design`/`ui` -- clippy::nursery's `significant_drop_tightening`.
    let (n_d, options) = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Custom-catalogue-aware: unlike `effective_refractive_index`,
        // this also resolves a custom material by name before falling back to a built-in
        // or the design's `I` line -- see `ri_source_text` below for the matching
        // "where did this number come from" explanation shown in the inspector.
        let n_d = design.effective_refractive_index_with(&ctx.custom_materials);
        ui.global::<EditorModel>()
            .set_ri_source_text(ri_source_text(&design.material, &ctx.custom_materials).into());
        // Custom materials can change any time, independently of `design` -- this
        // option LIST is always refreshed; only the in-out `*_index`/`*_text`
        // selections below are gated on `delta`.
        let options = state.material_combo_options(&ctx.custom_materials);
        drop(ctx);
        (n_d, options)
    };
    // A second, separate lock acquisition (rather than reusing the guard
    // above): its own last use sits right at this block's own end, so the
    // lock is never held across work -- the `delta`-gated pushes below -- that
    // does not need it either.
    let selected_material_index = {
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sync_viewport_material_link(ui, &mut ctx, design)
    };
    // Set only after the `render_ctx` guard above is dropped -- see
    // `sync_viewport_material_link`'s own doc comment.
    if let Some(idx) = selected_material_index {
        ui.global::<ViewportModel>()
            .set_selected_material_index(idx);
    }
    // `options` is already cached at the `Vec<String>` level
    // (`EditorState::material_combo_options`'s own `MaterialComboCache`, rebuilt
    // only when the catalogue's custom-material names change). Comparing against
    // what is already pushed skips rebuilding a brand-new `ModelRc<VecModel<_>>`
    // from that cached `Vec` (and the Slint-side model reset it triggers) unless
    // the catalogue itself actually changed -- rebuilding unconditionally on every
    // refresh would force the combo box to fully reset its rows on every angle
    // nudge just as much as on an actual material save.
    let current_options = ui.global::<EditorModel>().get_material_combo_options();
    let unchanged = current_options.row_count() == options.len()
        && current_options
            .iter()
            .zip(options.iter())
            .all(|(current, new)| current == new.as_str());
    if !unchanged {
        ui.global::<EditorModel>()
            .set_material_combo_options(ModelRc::new(VecModel::from(
                options
                    .iter()
                    .cloned()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));
    }
    // Pushed the same way, so `new_design_dialog.slint`'s New
    // Design material `ComboBox` binds its `model` to `EditorModel.new_material_options`
    // instead of hand-maintaining a literal list in the SAME order as
    // `builtin_preset_names` -- see that function's own doc comment.
    // `builtin_preset_names` never changes at runtime, so this is redundant work on
    // every refresh, not a correctness issue -- matching `material_combo_options`
    // just above rather than adding a one-shot-at-startup special case for one more
    // static list.
    ui.global::<EditorModel>()
        .set_new_material_options(ModelRc::new(VecModel::from(
            builtin_preset_names()
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    if delta.material {
        ui.global::<EditorModel>()
            .set_material_combo_index(design_material_index_from_name(
                design.material.name.as_deref(),
                &options,
            ));
        ui.global::<EditorModel>().set_ri_override_text(
            design
                .material
                .refractive_index_override
                .map_or_else(String::new, |v| format!("{v:.4}"))
                .into(),
        );
    }
    ui.global::<EditorModel>()
        .set_effective_ri_text(format!("{n_d:.4}").into());
    ui.global::<EditorModel>()
        .set_critical_angle_text(format!("{:.2}\u{b0}", critical_angle_deg(n_d)).into());
    push_material_guess(ui, design, n_d);
    if delta.gear {
        ui.global::<EditorModel>()
            .set_gear_index(gear_index_from_teeth(design.meta.gear_teeth));
        ui.global::<EditorModel>()
            .set_gear_custom_text(design.meta.gear_teeth.to_string().into());
    }
    if delta.symmetry {
        ui.global::<EditorModel>()
            .set_symmetry_order_text(design.meta.symmetry_order.to_string().into());
        ui.global::<EditorModel>().set_mirror(design.meta.mirror);
        // Whatever the last typed-but-not-yet-applied preview
        // said is no longer meaningful once the fields are freshly reseeded from
        // the real (just-applied, or freshly loaded) design -- a value that
        // matches the live design has nothing left to preview.
        ui.global::<EditorModel>()
            .set_symmetry_preview_text("".into());
    }
    if delta.meta {
        // `design.meta.headers`' first entry is the design's
        // title (the catalogue round-trip convention -- see `EditorModel.
        // design_title`'s own doc comment, `ui/models/editor.slint`), every
        // further entry an "extra" header line, `';'`-joined back for the
        // form's single-line field -- `apply_design_meta`'s own inverse split.
        let (title, extra_headers) = design
            .meta
            .headers
            .split_first()
            .map_or((String::new(), String::new()), |(title, rest)| {
                (title.clone(), rest.join(";"))
            });
        ui.global::<EditorModel>().set_design_title(title.into());
        ui.global::<EditorModel>()
            .set_design_extra_headers(extra_headers.into());
        ui.global::<EditorModel>()
            .set_design_footnotes(design.meta.footnotes.join(";").into());
        ui.global::<EditorModel>().set_design_gear_reference_angle(
            format!("{}", design.meta.gear_reference_angle).into(),
        );
    }

    n_d
}

/// The design-settings panel's "linked to design" viewport sync -- see
/// [`refresh_design_settings`]'s doc comment for why there is no separate
/// `render_view_tab` gate. Split out purely to keep that function under clippy's
/// function-length lint.
///
/// Returns the Render Material dropdown's
/// resolved index instead of setting `ViewportModel.selected_material_index`
/// itself. That setter's `changed` handler runs a DB query
/// (`custom_materials.rs`), and every caller holds `ctx` -- the `render_ctx`
/// mutex the render thread also locks every frame -- across this function's
/// call; setting the property from inside that guard stalls the render
/// thread on every refresh. Callers must set the returned index themselves,
/// after the guard on `ctx` has been dropped.
#[must_use]
pub(super) fn sync_viewport_material_link(
    ui: &MainWindow,
    ctx: &mut RenderContext,
    design: &Design,
) -> Option<i32> {
    if !ui.global::<ViewportModel>().get_viewport_material_linked() {
        return None;
    }
    let (name, unresolved) = traced_material_for(design, &ctx.custom_materials);
    if ctx.material_unresolved != unresolved {
        ctx.material_unresolved.clone_from(&unresolved);
        ctx.dirty = true;
    }
    // The resolved material itself, not merely its name:
    // `context::resolve_material` can only look a name up in the built-in table,
    // so a catalogue custom material or a typed RI override never reached the
    // trace at all. `material_override` is what the tracer, the HUD metrics, the
    // tilt sweep and the hover preview all prefer over that name lookup.
    //
    // Keyed on `name` -- the material this design is traced AS, decided by
    // `traced_material_for` just above -- and NOT on `design.material`. Passing the
    // raw selection here sent it through `MaterialSelection::resolve`, whose
    // no-name fallback is `GemMaterial::diamond()`: every `.asc` import and every
    // new design names no material, so the override was diamond, the override beats
    // the name, and the stone rendered with diamond's (empty) absorption bands --
    // colourless whatever the schedule said. `traced_gem_material` returns `None`
    // instead of substituting, and an absent override falls through to
    // `context::resolve_material`'s own by-name lookup, so the name and the override
    // can no longer describe two different stones.
    let resolved = unresolved
        .is_none()
        .then(|| {
            traced_gem_material(
                &name,
                &design.material,
                &EditorMaterialLookup::new(&ctx.custom_materials),
            )
        })
        .flatten();
    if ctx.material_override != resolved {
        ctx.material_override = resolved;
        ctx.dirty = true;
    }
    if ctx.material_name != name {
        // Cloned, not moved -- `name` is looked up again just below to sync
        // the dropdown's own selected index. `clone_from` reuses the existing
        // allocation instead of dropping it for a fresh one.
        ctx.material_name.clone_from(&name);
        ctx.dirty = true;
    }
    // Keeps the Render Material dropdown's own displayed selection in sync with
    // the material actually being traced on every refresh. Writing this only
    // once, at startup (`startup_settings::apply_saved_settings`), would let the
    // dropdown go on showing e.g. "Diamond" long after `ctx.material_name` (and
    // so the trace, and the tilt dialog's staleness check, both of which read
    // `ViewportModel.selected_material_index`/`material_options`) had moved to
    // something else entirely.
    let options = ui.global::<ViewportModel>().get_material_options();
    let selected_material_index = crate::gui::startup_settings::find_option_index(&options, &name);
    // The design's own real girdle diameter, under the same link gate as the
    // material above -- without this write, `context::apply_material_overrides`
    // would always skip absorption-path scaling (treating every design as if it
    // had no physical size), and the Yield panel's millimetre carat estimate
    // would have no counterpart in the render's own colour depth.
    let stone_width_mm = design.girdle_diameter_mm.unwrap_or(0.0) as f32;
    if (ctx.stone_width_mm - stone_width_mm).abs() > f32::EPSILON {
        ctx.stone_width_mm = stone_width_mm;
        ctx.dirty = true;
    }
    selected_material_index
}

/// Builds the chip row for whichever tier the inspector's Tier tab currently has
/// loaded (`EditorModel.selected_tier_index`; `None` for "Add Tier" or an
/// out-of-range value) -- the per-facet editing surface. Computed
/// fresh from `design` on the UI thread every refresh, for only the one tier
/// being edited, rather than for every row: see [`IndexChipItem`]'s own doc
/// comment for why this can never be a field on the (background-thread-built,
/// `Send`) [`crate::EditorTierItem`] instead.
#[must_use]
fn selected_tier_chips(design: &Design, selected_tier_index: i32) -> Vec<IndexChipItem> {
    usize::try_from(selected_tier_index)
        .ok()
        .and_then(|idx| design.tiers.get(idx))
        .map_or_else(Vec::new, |tier| {
            index_chip_items(&tier.indices, &tier.detached)
        })
}

/// Pushes [`selected_tier_chips`]'s result into `EditorModel.selected_tier_chips`,
/// plus the currently selected tier's own cheater-offset text
/// into `EditorModel.selected_tier_cheater_offset_text` -- shared by
/// [`refresh_editor_panel`]/[`push_stale_content`]/`callbacks::tier_actions::
/// apply_selected_tier_change` (a plain row-click/facet-click selection change,
/// with no edit of its own) so both stay current after every one of the three
/// ways the selected tier can change, exactly like `tiers` itself.
pub(super) fn push_selected_tier_chips(ui: &MainWindow, state: &EditorState) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let cheater_offset_text = usize::try_from(selected)
        .ok()
        .and_then(|index| state.design.cheater_offset_deg(index))
        .map_or_else(String::new, |deg| format!("{deg:.2}"));
    ui.global::<EditorModel>()
        .set_selected_tier_cheater_offset_text(cheater_offset_text.into());
    let note_text = usize::try_from(selected)
        .ok()
        .and_then(|index| state.design.tier_note(index))
        .map_or_else(String::new, str::to_string);
    ui.global::<EditorModel>()
        .set_selected_tier_note_text(note_text.into());
    let chips = selected_tier_chips(&state.design, selected);
    push_rows(
        &ui.global::<EditorModel>().get_selected_tier_chips(),
        chips,
        |model| ui.global::<EditorModel>().set_selected_tier_chips(model),
    );
}

/// Pushes Deep Solve's `available`/`hint` properties (see [`deep_solve_hint`]) --
/// shared by [`refresh_editor_panel`] and [`refresh_editor_panel_stale`] since
/// whether there is anything to repair depends on the tier list, which both paths
/// refresh.
fn refresh_deep_solve_availability(ui: &MainWindow, state: &EditorState) {
    let (available, hint) = deep_solve_hint(state);
    ui.global::<EditorModel>()
        .set_deep_solve_available(available);
    ui.global::<EditorModel>().set_deep_solve_hint(hint.into());
}

/// Pushes Optimize's `available`/`hint` properties (see [`optimize_hint`]) -- shared
/// by [`refresh_editor_panel`] and [`refresh_editor_panel_stale`] for the identical
/// reason [`refresh_deep_solve_availability`] is.
///
/// Also re-derives `editor_optimize_can_apply` from `EditorState::pending_optimize`
/// against the CURRENT `generation` -- so any edit that reaches this function
/// immediately greys the Apply button out the moment it would apply to a design the
/// search no longer describes, rather than waiting for a click to be refused.
/// `setup_optimize_callback`'s completion handler sets this property too, for the
/// one moment (a run finishing) that bypasses both refresh functions.
/// The coordinate-stage evaluation budget a fresh Optimize run
/// would actually use right now, read from `EditorModel.optimize_budget_text` (an
/// in-out text field the Optimize tab exposes). Falls back to
/// [`indicatrix_cut_core::OptimizeConfig::default`]'s own `200` for an empty,
/// unparseable, or non-positive value, so a field nobody has touched yet (or a
/// build with no such control at all) behaves as if this budget field did not
/// exist.
///
/// `callbacks::solve_actions::setup_optimize_callback` reads the SAME property the
/// same way when it actually launches a run, so the number quoted here can never
/// disagree with the number a click would use.
#[must_use]
pub(super) fn configured_optimize_max_evaluations(ui: &MainWindow) -> usize {
    ui.global::<EditorModel>()
        .get_optimize_budget_text()
        .parse::<usize>()
        .ok()
        .filter(|&v| v > 0)
        .unwrap_or(200)
}

fn refresh_optimize_availability(ui: &MainWindow, state: &EditorState) {
    let (available, hint) = optimize_hint(state, configured_optimize_max_evaluations(ui));
    ui.global::<EditorModel>().set_optimize_available(available);
    ui.global::<EditorModel>().set_optimize_hint(hint.into());

    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    let can_apply = state
        .pending_optimize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|(_, started_generation)| *started_generation == current_generation);
    ui.global::<EditorModel>().set_optimize_can_apply(can_apply);
}

/// The no-solve counterpart to [`refresh_editor_panel`] -- called after every edit
/// action that is not the explicit "Solve" button. Updates the tier list via
/// [`tier_items_stale`] (no mast/strategy data) and undo/redo/preform fields exactly
/// like [`refresh_editor_panel`] does, but overwrites the validation banner with a
/// fixed, unmissable "not solved" message instead of calling
/// [`status_text_and_is_problem`] -- this function must never touch
/// `Design::solve`/`status`/`measure` even indirectly.
///
/// Ends by calling [`auto_solve::on_edit`]: every edit callback in this group calls
/// this function already, so that one call is this crate's single hook point for
/// "maybe schedule a debounced background solve" -- see that function's own doc
/// comment. [`push_stale_content`] is split out separately so [`refresh_all`]'s
/// large-design path can push the identical stale content WITHOUT also scheduling a
/// redundant debounced auto-solve on top of the immediate background solve it
/// dispatches itself.
/// `dirty` names the tier(s) the triggering edit is known to have touched -- see
/// [`super::state::tier_items_stale_with_last_solved`]'s own doc comment for
/// exactly how those rows (and every other row) are treated. Pass every tier
/// index (`0..state.design.tiers.len()`) for an edit whose blast radius isn't
/// tracked precisely, the same cases [`submit_preview_replan`]'s own
/// `force_full_solve: true` already names (Undo/Redo, a gear remap, a
/// symmetry/mirror change).
pub(super) fn refresh_editor_panel_stale(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    dirty: &BTreeSet<usize>,
) {
    push_stale_content(ui, render_ctx, state, dirty);
    auto_solve::on_edit(ui, render_ctx, state);
}

/// [`push_stale_content`]'s own
/// "which analysis results now describe an older generation" segment, split out
/// purely to keep that function under clippy's function-length lint. Recomputed
/// on EVERY edit (not only when a Deep Solve/Optimize run itself completes) so
/// the "Stale: design changed" badge appears the instant a FURTHER edit lands on
/// top of an already-displayed result -- see `EditorModel.deep_solve_stale`'s own
/// doc comment.
fn push_stale_generation_badges(ui: &MainWindow, state: &EditorState, current_generation: u64) {
    ui.global::<EditorModel>()
        .set_deep_solve_stale(result_is_stale(
            state.deep_solve_result_generation,
            current_generation,
        ));
    let optimize_result_generation = state
        .pending_optimize
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(_, generation)| *generation);
    ui.global::<EditorModel>()
        .set_optimize_stale(result_is_stale(
            optimize_result_generation,
            current_generation,
        ));
    // `stale::refresh_badges` covers the Retarget proposal, the one analysis
    // result with no `EditorState` field of its own to stamp a generation on --
    // see that module's own doc comment for why Deep Solve/Optimize stay on the
    // two direct pushes just above instead.
    stale::refresh_badges(ui, current_generation);
}

/// [`push_stale_content`]'s manufacturability-warnings segment, split out purely
/// to keep that function under clippy's function-length lint.
///
/// A stale solve's MESH-based manufacturability findings would no longer
/// describe the current (edited, unsolved) design, so those are not shown -- but
/// the two mast-free checks (gear quantization, cut order) need no mast at all
/// and stay real and actionable even here, tagged "(pre-solve)" so they are
/// never mistaken for a completed pass.
fn push_stale_warnings(ui: &MainWindow, state: &EditorState) {
    let tagged_warnings = manufacturability_warnings_tagged(&state.design, None);
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
}

/// [`push_stale_content`]'s preform/yield-figure segment, split out purely to
/// keep that function under clippy's function-length lint. The preform SCRATCH
/// fields (shape/half-width/length-over-width/depth) only reseed when the
/// design's own preform actually changed (`delta.preform`); the mm readouts and
/// every yield figure below need a real solve this "no-solve" path deliberately
/// never does, so they are unconditionally cleared on every stale push
/// rather than left showing a superseded result.
fn push_stale_preform_and_yield(ui: &MainWindow, state: &EditorState, delta: &ScratchDelta) {
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
    }
    ui.global::<EditorModel>()
        .set_preform_half_width_mm_text("".into());
    ui.global::<EditorModel>()
        .set_preform_depth_mm_text("".into());
    // `mm_per_unit: None` always resolves to `""`.
    ui.global::<EditorModel>().set_preform_y_offset_mm(
        preform_y_offset_mm_text(state.design.preform_y_offset, None).into(),
    );

    // The form fields still get seeded from the design's current state (an edit may
    // have just changed the girdle diameter/material), but only when that value
    // actually changed (`delta`), and the read-only figures are cleared, not left
    // showing a superseded result: `yield_report` needs a solve this function
    // deliberately never does.
    push_yield_material_scratch(ui, state, delta);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text("".into());
    ui.global::<EditorModel>().set_carat_weight_text("".into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text("".into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning("".into());
}

/// [`push_stale_content`]'s proportions/cutting-schedule/facet-count reset,
/// split out purely to keep that function under clippy's function-length lint.
/// All three need a real solve -- cleared here and repopulated once
/// [`refresh_editor_panel`] next runs (the explicit "Solve" action, or New/Load).
fn push_stale_proportions_reset(ui: &MainWindow) {
    ui.global::<EditorModel>()
        .set_proportions_table_pct("-".into());
    ui.global::<EditorModel>()
        .set_proportions_crown_height("-".into());
    ui.global::<EditorModel>()
        .set_proportions_pavilion_depth("-".into());
    ui.global::<EditorModel>()
        .set_proportions_total_depth("-".into());
    ui.global::<EditorModel>()
        .set_proportions_length_to_width("-".into());
    ui.global::<EditorModel>()
        .set_girdle_thickness_text("-".into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));
    // Facets are a solve-dependent count exactly
    // like the cutting schedule just above -- reset to 0 ("nothing solved yet")
    // rather than left showing a superseded design's count, matching
    // `refresh_editor_panel_from_solve`'s own "0 when the design does not
    // currently solve" reasoning for this same property.
    ui.global::<EditorModel>().set_facet_count(0);
}

/// The actual "no-solve" content push -- see [`refresh_editor_panel_stale`]'s doc
/// comment for why this is split out, and for `dirty`'s own meaning. Split
/// further into [`push_stale_generation_badges`]/[`push_stale_warnings`]/
/// [`push_stale_preform_and_yield`]/[`push_stale_proportions_reset`] purely to
/// keep this function itself under clippy's function-length lint -- each of
/// those pushes one self-contained group of `EditorModel` properties and touches
/// nothing the others do.
fn push_stale_content(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    dirty: &BTreeSet<usize>,
) {
    push_trace_staleness(ui, render_ctx, state);
    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    push_stale_generation_badges(ui, state, current_generation);

    let delta = state.record_scratch_push();
    let n_d = refresh_design_settings(ui, render_ctx, state, &delta);
    // A cached solve from BEFORE this edit is still real
    // evidence for every row `dirty` does not name -- see `auto_solve::
    // solid_last_solved`'s own doc comment for why this is read from the shared
    // thread-local handle rather than threaded through as a parameter (this
    // function has no `SolidLastSolved` of its own to read).
    let last_solved = auto_solve::solid_last_solved().and_then(|cache| {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    });
    let tiers =
        tier_items_stale_with_last_solved(&state.design, n_d, last_solved.as_deref(), dirty);
    push_tier_list_and_undo_redo(ui, state, tiers);

    ui.global::<EditorModel>().set_status_text(
        "Not solved -- click Solve to compute masts and validate this design.".into(),
    );
    ui.global::<EditorModel>().set_status_is_problem(true);
    ui.global::<EditorModel>().set_solve_state("stale".into());

    push_stale_warnings(ui, state);
    push_stale_preform_and_yield(ui, state, &delta);
    push_stale_proportions_reset(ui);

    // The design label is cheap and never solve-dependent -- always kept current.
    ui.global::<EditorModel>()
        .set_design_label(design_label_text(state.asc_filename.as_deref()).into());

    refresh_deep_solve_availability(ui, state);
    refresh_optimize_availability(ui, state);
}

/// The Solid/Diagram viewport's own size in PHYSICAL pixels, for a raster/pick
/// request. `SolidPreviewModel.viewport_width`/`viewport_height` are LOGICAL (DIP)
/// sizes pushed straight from `solid_viewport.slint`'s own `width`/`height`, so a
/// request built from them one-to-one rasterizes at only one device pixel per
/// logical pixel -- soft on any `HiDPI` display. Multiplying by
/// `ui.window().scale_factor()` here is one half of the `HiDPI` fix; the other half
/// is `callbacks::tier_actions`'s hover/click conversion, which MULTIPLIES the
/// incoming (logical) pointer position by the same factor to reach the physical
/// pixel it names in the resulting (now physically sized) pick buffer.
/// `solid_preview::diagram_wiring` does the same for the diagram's pick buffer.
pub(super) fn scaled_viewport_size(ui: &MainWindow) -> (u32, u32) {
    let scale = ui.window().scale_factor();
    (
        (ui.global::<SolidPreviewModel>().get_viewport_width() * scale) as u32,
        (ui.global::<SolidPreviewModel>().get_viewport_height() * scale) as u32,
    )
}

/// Writes `design`'s current plane arrangement into the shared (GPU path-traced)
/// viewport and marks it dirty, then re-issues those same planes to the solid
/// preview -- see this group's `mod.rs` doc comment ("Feeding the viewport") for why
/// this is split out from [`refresh_editor_panel`].
///
/// `SolidPreviewState::request_redraw`'s `planes` sign convention (`n . x <= m`) is
/// the OPPOSITE of `design_to_gpu_planes`'s `GpuFacetPlane` (`n . x + d = 0`) --
/// `GpuFacetPlane::to_halfspace_f64`'s `m = -d` flip, reapplied here by hand since
/// `active_planes` is already in `GpuFacetPlane` form.
/// Tells the viewport whether the path-traced image still describes the design on
/// the bench.
///
/// Compares the generation stamped on the planes the render thread actually holds
/// against `state`'s live one. Pushed from both the stale path (every ordinary edit,
/// where it becomes true) and the refresh path (a real solve, where the planes have
/// just been re-pushed and it becomes false again), so the marker never outlives the
/// condition it reports.
fn push_trace_staleness(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    let generation = state.generation.load(std::sync::atomic::Ordering::Relaxed);
    let stale = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ctx.traced_planes_are_stale(generation)
    };
    ui.global::<ViewportModel>().set_trace_stale(stale);
}

/// `solved` is the SAME solve [`refresh_editor_panel`] already computed for this
/// same "Solve" click -- `None` only for a design that does
/// not currently solve at all (a `MissingAnchor`, most commonly), in which case
/// this falls back to the plain, internally-solving forms.
/// Passing it through here avoids a SECOND independent
/// `Design::solve()` on top of `refresh_editor_panel`'s own.
fn refresh_viewport(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    solved: Option<&[SolvedTier]>,
) {
    let mut ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The editor always wins a claim, so this cannot fail --
    // but it goes through the same single write path as every other writer so the
    // ownership stamp stays truthful, which is what stops a later catalogue click
    // from silently replacing the design under the cutter's hands.
    let planes_for_claim = solved.map_or_else(
        || design_to_gpu_planes(&state.design),
        |solved| auto_solve::design_to_gpu_planes_from_solved(&state.design, solved),
    );
    ctx.claim_active_planes(
        std::sync::Arc::new(planes_for_claim),
        Some((
            state.design.meta.gear_teeth_abs(),
            state.design.meta.gear_reference_angle as f32,
        )),
        PlanesOwner::Editor {
            generation: state.generation.load(std::sync::atomic::Ordering::Relaxed),
        },
    );
    ctx.dirty = true;
    let design_gear = ctx.design_gear;
    let planes: Vec<(glam::Vec3, f32)> = ctx
        .active_planes
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    // Letterboxes the request to the traced image's own
    // rectangle in Path-traced/Both, exactly like `camera_lighting::
    // resubmit_at_current_pose` already does for a camera drag/zoom/view-mode
    // switch -- see `contained_request_size`'s own doc comment for why this
    // call site needs the identical treatment.
    let size = contained_request_size(view_mode, scaled_viewport_size(ui), (ctx.width, ctx.height));
    preview_state.request_redraw_with_gear(
        planes,
        CameraPose {
            yaw: ctx.yaw,
            pitch: ctx.pitch,
            distance: ctx.distance,
        },
        size,
        view_mode,
        design_gear,
    );
    drop(ctx);

    // The editor just claimed the shared plane slot for
    // `state.design` -- whatever material name a PREVIOUS occupant (a catalogue
    // preview load, the only writer of this field) left in `cached_curve_material`
    // no longer describes anything, so it must stop being compared against
    // `ctx.material_name` for tilt-dialog staleness. Clearing it here (rather than
    // leaving it to whatever the next catalogue load happens to overwrite it with)
    // means "no cached artefact for this design" is representable instead of an
    // unrelated design's material silently standing in for it.
    ui.global::<TiltModel>()
        .set_cached_curve_material("".into());
    // Geometry just changed under the tilt dialog -- if it's
    // open, its four curves and summary badges are about to describe stale
    // geometry as settled results unless a fresh sweep is requested.
    // `AxesCacheKey` (tilt_profile.rs) already hashes the planes, so this is a
    // no-op resweep whenever nothing about the planes actually moved.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }

    // This is the SAME real design solve (`New`/`Load Selected`/the explicit
    // "Solve" action) `refresh_editor_panel` already computed. Stashing it here,
    // rather than independently re-solving a second time just to populate this
    // cache, is what lets the NEXT small edit's `submit_preview_replan` call use
    // a real `resolve_dirty` subgraph solve rather than falling back to another
    // full solve.
    if let Some(solved) = solved {
        *solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(solved.to_vec());
    }
    // After the guard above is dropped: the planes the tracer holds were just
    // re-stamped with this generation, so the trace-staleness marker clears here.
    push_trace_staleness(ui, render_ctx, state);
}

/// Submits a replan-on-edit request to the Solid preview's worker thread -- the
/// actual `resolve_dirty`/`Design::solve` call never runs here, on the UI thread.
///
/// `dirty` should name exactly the tier(s) the edit touched when known precisely (a
/// single-tier Save/Remove/detach toggle); leave it empty for an edit that never
/// moves any tier's mast (material/preform/girdle-diameter). Pass
/// `force_full_solve: true` for an edit whose blast radius isn't tracked precisely
/// (Undo/Redo, a gear remap, a symmetry/mirror change): forces a full `solve()`
/// rather than a possibly-wrong subgraph `resolve_dirty` -- always safe, only
/// potentially slower.
///
/// Also stamps the request with `state.generation`'s current value and stashes a
/// matching `Design`/`multi_selected` snapshot via `auto_solve::
/// stash_current_design` -- see [`push_solved_preview`]/
/// `auto_solve::take_matching_design` for the consuming half once the worker's
/// frame lands back in `gui::SlintSolidSink::apply`.
pub(super) fn submit_preview_replan(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    dirty: BTreeSet<usize>,
    force_full_solve: bool,
) {
    submit_preview_replan_for(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        ReplanSource {
            design: &state.design,
            generation: state.generation.load(AtomicOrdering::Relaxed),
            multi_selected: &state.multi_selected,
        },
        dirty,
        force_full_solve,
    );
}

/// [`submit_preview_replan`]'s `design`/`generation`/`multi_selected` inputs,
/// bundled (rather than three more parameters on [`submit_preview_replan_for`])
/// purely to keep that function under clippy's argument-count lint -- the same
/// reasoning `callbacks::tier_actions::LoadedDesignOutcome`/`auto_solve::
/// PanelInputs` already use for themselves.
#[derive(Clone, Copy)]
pub(super) struct ReplanSource<'a> {
    pub(super) design: &'a Design,
    pub(super) generation: u64,
    pub(super) multi_selected: &'a BTreeSet<usize>,
}

/// [`submit_preview_replan`]'s own body, taking a [`ReplanSource`] SNAPSHOT
/// rather than a live `&EditorState` -- split out so
/// `auto_solve::schedule_idle_replan_if_stale` can resubmit a follow-up replan
/// once the solid-preview worker goes idle after a partial (subgraph) resolve,
/// without needing the live `Rc<RefCell<EditorState>>` that module deliberately
/// never holds (see its own doc comment, "Why a `thread_local!`, not a new
/// `EditorState` field"). [`submit_preview_replan`] itself is the thin,
/// `EditorState`-shaped wrapper every other caller keeps using.
pub(super) fn submit_preview_replan_for(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    source: ReplanSource<'_>,
    dirty: BTreeSet<usize>,
    force_full_solve: bool,
) {
    let ReplanSource {
        design,
        generation,
        multi_selected,
    } = source;
    let last_solved = if force_full_solve {
        None
    } else {
        solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    };
    let (camera, render_size, n_d) = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            CameraPose {
                yaw: ctx.yaw,
                pitch: ctx.pitch,
                distance: ctx.distance,
            },
            (ctx.width, ctx.height),
            // `_with` resolves a CUSTOM
            // catalogue material by name too, not only a built-in preset -- the
            // bare accessor silently falls back to 1.5442 (fused quartz) for any
            // design named after a custom material, which the tilt overlay this
            // `n_d` feeds (`tier_items`'s critical-angle margin column) would then
            // measure against the wrong critical angle with nothing on screen
            // saying so.
            design.effective_refractive_index_with(&ctx.custom_materials),
        )
    };
    let selected_tier = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok();
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    // See `refresh_viewport`'s identical call, just above,
    // for why -- this is the post-edit path `contained_request_size`'s own doc
    // comment names as still needing the same treatment.
    let size = contained_request_size(view_mode, scaled_viewport_size(ui), render_size);
    let show_preform = ui.global::<SolidPreviewModel>().get_show_preform_planes();
    let enlarged_panel = ui
        .global::<SolidPreviewModel>()
        .get_diagram_enlarged_panel();
    // ONE `Arc<Design>` snapshot per drained edit-intent frame, shared (via
    // cheap `Arc::clone`, never a second deep clone) between this
    // stash and the replan request below. `auto_solve::take_matching_design` can
    // hand this SAME snapshot back to `editor::apply_matching_preview_frame` once
    // a solid-preview frame lands claiming this exact generation -- see that
    // function's own doc comment for why this cannot instead be read straight off
    // `state` from there (`gui::SlintSolidSink::apply` runs off the UI thread it
    // hops back onto, `Send`-bound, and can never reach this
    // `Rc<RefCell<EditorState>>`).
    //
    // `ReplanRequest::design` is `Arc<Design>` (`solid_preview::
    // preview_state`/`live_update`): `Arc::clone` below shares this exact
    // allocation with `design_snapshot` rather than cloning `design` a second
    // time (`(*design_snapshot).clone()`) to build a plain, non-`Arc` field.
    // Sharing the allocation all the way through `PlanJob`/`PlannedFrame` to the
    // render worker means a burst of coalesced replan requests (an angle-nudge
    // drag, most commonly) clones the design at most once per generation, not
    // once per request.
    let design_snapshot = Arc::new(design.clone());
    auto_solve::stash_current_design(
        generation,
        Arc::clone(&design_snapshot),
        multi_selected.clone(),
    );
    // Read here rather than cached on `SolidPreviewState` alone, so the
    // slider and the redraw can never disagree about how much of the schedule is
    // being shown. `-1` (the default) means the whole design.
    let cutoff = ui.global::<SolidPreviewModel>().get_tier_cutoff();
    preview_state.set_tier_cutoff(usize::try_from(cutoff).ok());
    preview_state.request_replan(ReplanRequest {
        design: Arc::clone(&design_snapshot),
        dirty,
        last_solved,
        camera,
        size,
        selected_tier,
        n_d,
        view_mode,
        generation,
        show_preform,
        enlarged_panel,
    });
}

/// Pushes the tier table's rows, the validation banner, the
/// manufacturability warnings and the yield figures straight from a solid-preview
/// frame's own already-solved `solved` masts, once `auto_solve::
/// take_matching_design` has confirmed the frame's own generation still names the
/// live design -- see that function's own doc comment for the staleness check.
/// `design`/`multi_selected` are the plain clones that same call handed back.
///
/// Deliberately narrower than [`refresh_editor_panel`]/[`push_stale_content`]:
/// proportions, the cutting schedule, preform/material scratch fields and Deep
/// Solve/Optimize availability are all either solve-independent (already current,
/// pushed synchronously by [`refresh_editor_panel_stale`] at edit time) or not
/// pushed by this mechanism -- only the four fields that
/// [`auto_solve::dispatch_background_solve`]'s OWN completion would otherwise have
/// been the sole source of.
///
/// Called from `gui::SlintSolidSink::apply` -- a solid-preview WORKER-thread
/// callback hopped onto the UI thread via `slint::Weak::upgrade_in_event_loop` --
/// through `editor::apply_matching_preview_frame`'s thin forwarding wrapper, the
/// one bridge this group exposes beyond [`super::setup_editor_callbacks`] itself
/// (see that module's own doc comment, "Module split").
///
/// `custom_materials`: resolves `design`'s
/// effective refractive index the SAME custom-catalogue-aware way
/// [`refresh_design_settings`]/`auto_solve::panel_inputs` already do -- the bare
/// accessor would silently fall back to 1.5442 for a design named
/// after a custom catalogue material, leaving the tier table's critical-angle
/// margin column disagreeing with the Design Settings panel's own effective-RI
/// readout for exactly that design.
///
/// `custom_sg`: the catalogue's custom-material specific-
/// gravity table, handed to [`yield_report_texts_from_solved`] for the same
/// reason `auto_solve::panel_inputs`/[`push_yield_and_proportions`] do -- see that
/// function's own doc comment.
pub(super) fn push_solved_preview(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[indicatrix::optics::materials::GemMaterial],
    custom_sg: &[(String, f64)],
) {
    let n_d = design.effective_refractive_index_with(custom_materials);
    let mut tiers = tier_items_from_solved(design, solved, n_d);
    apply_multi_selection(&mut tiers, multi_selected);
    push_tiers(ui, tiers);
    push_multi_selected_count(ui, multi_selected.len());

    let (status_text, is_problem) = status_text_and_is_problem_from_solved(design, solved);
    ui.global::<EditorModel>()
        .set_status_text(status_text.into());
    ui.global::<EditorModel>().set_status_is_problem(is_problem);
    ui.global::<EditorModel>()
        .set_solve_state(if is_problem { "failed" } else { "solved" }.into());

    // The tagged pairs, not `manufacturability_warning_lines_
    // from_solved`'s flattened text-only list, so the tier index survives to
    // `manufacturability_warning_tiers` -- `editor_tier_table.slint`'s own row
    // markers read this to flag the specific row a warning is about.
    let tagged_warnings = manufacturability_warnings_tagged(design, Some(solved));
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

    let (vol_yield_text, carat_text, sg_used_text, fit_text) =
        yield_report_texts_from_solved(design, solved, custom_sg);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());
}

/// [`refresh_editor_panel`] + [`refresh_viewport`] together -- pushes a real solve's
/// result into both the panel and the shared viewport. Only called by `New`, `Load
/// Selected`, "Adopt", and the explicit "Solve" action; every other edit callback
/// calls [`refresh_editor_panel_stale`] instead and leaves the viewport untouched.
///
/// # Synchronous only for a cheap-enough design
///
/// `Design::solve`'s refinement sweep is cubic in plane count (a real 103-tier/210-
/// plane design: 5.9s -- see `mod.rs`'s "Never block the UI thread with a solve"
/// section), so this only solves inline on the UI thread for a design at or under
/// [`auto_solve::should_solve_synchronously`] -- comfortably fast in practice, and
/// keeps the "New" dialog's promise of an immediately solved, unstale design without
/// a visible "Solving..." flash. A larger design instead gets the SAME stale content
/// [`refresh_editor_panel_stale`] pushes after any other edit, plus an immediately
/// (not debounced) dispatched background solve -- see
/// [`auto_solve::dispatch_background_solve`].
///
/// This treats the explicit Solve button/F5 identically to New/Load rather than
/// special-casing it back to always-background regardless of size: a design small
/// enough for this function's synchronous path solves fast enough that forcing it
/// through a worker thread and a `Weak::upgrade_in_event_loop` round trip would only
/// add latency, not remove any UI freeze worth avoiding.
///
/// `wholesale`: `true` only for a caller that just
/// replaced `EditorState` wholesale (New/Load Selected/Open Native) -- see
/// [`auto_solve::reset_for_new_design`]'s own doc comment for why exactly those
/// three (and only those three) need the reset it performs. Every OTHER caller
/// (the explicit "Solve" button, Adopt/Adopt All/Adopt Selected/Pin to
/// Mast/Optimize Apply/Retarget Apply) passes `false`: it is still solving the
/// SAME design the auto-solve budget has already been measuring, so wiping that
/// measurement on every one of those actions would make
/// [`auto_solve::should_solve_synchronously`]'s "prefer a real measurement" rule
/// unreachable on precisely the path (an explicit re-Solve) it exists for --
/// `auto_solve::last_solve()` is read instead, exactly like every other caller of
/// that function already reads a real measurement when one exists.
/// The [`Rc<RefCell<EditorState>>`]-based entry point every OWNED caller
/// (`callbacks::tier_actions`/`callbacks::solve_actions`/`callbacks::
/// retarget_actions`) uses instead of calling [`refresh_all`] directly, so the
/// sync solve branch paints "Solving" before it runs.
///
/// # Why this wraps [`refresh_all`] rather than [`refresh_all`] itself changing
///
/// `native_io::finish_state_replace` also calls
/// [`refresh_all`] directly, passing a plain `&EditorState` obtained from its own
/// `state.borrow()` -- changing [`refresh_all`]'s OWN signature to take
/// `Rc<RefCell<EditorState>>` would break that call site. So [`refresh_all`]
/// keeps its original signature and behaviour completely unchanged (`native_io.rs`
/// is unaffected), and this function is the additional, paint-first entry point
/// every owned call site uses instead.
///
/// # What "paint-first" means here
///
/// [`refresh_all`]'s own sync branch (at or under
/// [`auto_solve::should_solve_synchronously`]) sets `solve_running`/
/// `solve_state` to "solving" and then immediately runs the blocking
/// `Design::solve()` in the same call, with no yield back to the event loop in
/// between -- the toolkit only ever paints the FINAL state once the whole
/// callback returns, so "Solving..." is never actually visible for a fast-sync
/// design (see [`refresh_all`]'s own doc comment). This
/// function peeks the SAME plane-estimate/last-solve decision [`refresh_all`]
/// makes internally; when it would take the sync branch, it sets
/// `solve_running`/`solve_state` HERE, then defers the actual (still fully
/// synchronous) [`refresh_all`] call behind a `Timer::single_shot(Duration::ZERO,
/// ..)` so the event loop gets to paint this frame first. The background branch
/// is unaffected -- it already returns without blocking, so it runs immediately.
pub(super) fn refresh_all_now(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &Rc<RefCell<EditorState>>,
    wholesale: bool,
) {
    let (plane_estimate, last_solve) = {
        let st = state.borrow();
        let plane_estimate: usize = st.design.tiers.iter().map(|t| t.indices.len()).sum();
        // Mirrors `refresh_all`'s own `wholesale` branch exactly (see that
        // function's doc comment) EXCEPT for the `auto_solve::reset_for_new_design`
        // side effect itself, which must run exactly once -- left for the real
        // `refresh_all` call below to perform, not duplicated here.
        let last_solve = if wholesale {
            None
        } else {
            auto_solve::last_solve()
        };
        (plane_estimate, last_solve)
    };
    if !auto_solve::should_solve_synchronously(plane_estimate, last_solve) {
        let st = state.borrow();
        refresh_all(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            &st,
            wholesale,
        );
        return;
    }
    // Paints "Solving..." now; the real (still synchronous) solve runs once the
    // event loop has had a chance to render this frame -- see this function's own
    // doc comment.
    ui.global::<EditorModel>().set_solve_running(true);
    ui.global::<EditorModel>().set_solve_state("solving".into());
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let state = Rc::clone(state);
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let st = state.borrow();
        refresh_all(
            &ui,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            &st,
            wholesale,
        );
    });
}

pub(super) fn refresh_all(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    wholesale: bool,
) {
    // Plane count, not tier count. Tier count does not drive
    // solve cost -- a wide-orbit tier emits many planes at once, so a small schedule
    // can still be an expensive, UI-blocking solve (the corpus's worst case is 103
    // tiers but 210 planes). The index count per tier is the cheap estimate of that,
    // available without solving.
    let plane_estimate: usize = state.design.tiers.iter().map(|t| t.indices.len()).sum();
    let last_solve = if wholesale {
        // A fresh call replacing `EditorState` wholesale means a real solve is
        // about to happen (either synchronously below, or via the background
        // dispatch) against a DIFFERENT design than whatever `Runtime::last_solve`
        // currently holds -- that previous design's measured solve time has
        // nothing to say about this one's cost, so it is cleared unconditionally.
        // The synchronous branch below overwrites it again immediately with a
        // real measurement; the background branch leaves it `None` until that
        // dispatch completes, which `auto_solve::should_schedule_auto_solve`'s
        // doc comment already treats as "try auto-solve," a reasonable default
        // right after a solve this function itself just triggered. `None` for
        // THIS call's own measurement deliberately: the reset immediately above
        // has just cleared it, and reaching back for the value it cleared would
        // judge a freshly loaded design by the previous one's solve time --
        // exactly the case where the two have nothing to do with each other, and
        // the one where guessing wrong blocks the UI thread.
        auto_solve::reset_for_new_design();
        None
    } else {
        // Same design as before (an explicit Solve/Adopt/Optimize Apply/etc. on
        // whatever is already loaded) -- its own last REAL measurement, if any,
        // is exactly the signal `should_solve_synchronously` wants. Clearing
        // it here unconditionally would make that rule
        // unreachable for a re-Solve on a design just over the plane-count
        // estimate but well under its own measured time.
        auto_solve::last_solve()
    };
    if auto_solve::should_solve_synchronously(plane_estimate, last_solve) {
        // Brackets the synchronous solve with
        // the SAME `solve_running`/`solve_state` signal `dispatch_background_solve`
        // already gives its own (background) solve, so the command bar/status
        // strip never have a code path that solves without SOME visible marker of
        // it -- even though, being fully synchronous, nothing repaints between
        // this `true` and the `false` a few lines down (no yield back to the
        // event loop happens mid-call); the toolkit only ever paints the FINAL
        // state once this function returns. Real, uninterrupted busy feedback for
        // this fast path would need the solve itself deferred behind a zero-delay
        // timer tick so "Solving..." can paint first -- a larger change to
        // `refresh_all`'s currently-synchronous contract that every one of its
        // several call sites would need auditing
        // against, left undone here. What this DOES fix: `solve_state` is
        // "solving" for the whole duration of this call rather than whatever it
        // was left at by the PREVIOUS refresh, so a reentrant read of it (were one
        // ever added) could not mistake this design for already-idle mid-solve.
        ui.global::<EditorModel>().set_solve_running(true);
        ui.global::<EditorModel>().set_solve_state("solving".into());
        // Timed around the real `Design::solve()` call alone,
        // not `refresh_editor_panel`'s UI-model pushes -- inflating the measured
        // duration with that work would make it incomparable with
        // `auto_solve::dispatch_background_solve`'s own timer (the async path's
        // baseline `should_schedule_auto_solve` compares this measurement
        // against), so a design near the budget threshold could flip auto-solve on
        // or off depending only on which path solved it, not on how expensive
        // solving actually is.
        //
        // Timed once here rather than a second,
        // thrown-away `state.design.solve()` purely to measure elapsed time,
        // immediately followed by `refresh_editor_panel` solving the SAME design
        // again internally to build the panel fields -- doing that would cost two
        // real solves for one "Solve" click. `refresh_editor_panel_from_solve`
        // below takes this exact result instead of re-solving, so this is the
        // only solve on this path.
        let start = Instant::now();
        let solved_result = state.design.solve();
        let elapsed = start.elapsed();
        auto_solve::record_solve_duration(elapsed);
        // Same measurement `record_solve_duration` already takes, also shown here
        // -- captured once rather than re-read, so the figure
        // on screen is exactly the one the auto-solve budget decides on.
        ui.global::<EditorModel>()
            .set_last_solve_duration_ms(i32::try_from(elapsed.as_millis()).unwrap_or(i32::MAX));
        // `solved` is this SAME solve's own result, handed to `refresh_viewport`
        // below so it never pays for a second one of its own either.
        let solved = refresh_editor_panel_from_solve(ui, render_ctx, state, solved_result);
        refresh_viewport(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            state,
            solved.as_deref(),
        );
        // `refresh_editor_panel` has already set `solve_state` to "solved"/"failed"
        // above -- only the running flag still needs clearing.
        ui.global::<EditorModel>().set_solve_running(false);
    } else {
        // Too expensive for the synchronous path above -- reached either by a
        // fresh New/Load/Open (`wholesale`, where any cached `solid_last_solved`
        // on hand describes the design that was just replaced, not this one) or
        // by an explicit re-Solve of a design that is itself large/slow-measured
        // (`wholesale: false` does not make this branch assume the cache is a
        // stranger's, but a stale cache is
        // just as possible here: an earlier edit on this same design could have
        // left `solid_last_solved` behind a subgraph resolve that never reached
        // every tier). Either way, treat every tier as dirty rather than risk a
        // coincidental length match showing a stale/unrelated design's masts as
        // this one's own.
        let all_dirty: BTreeSet<usize> = (0..state.design.tiers.len()).collect();
        push_stale_content(ui, render_ctx, state, &all_dirty);
        auto_solve::dispatch_background_solve(
            ui,
            render_ctx,
            state.design.clone(),
            &state.generation,
            state.multi_selected.clone(),
        );
    }
}

/// Whether Deep Solve is available for `state`'s current design, and the
/// explanatory hint text `EditorView` shows next to its button either way:
///
/// - No printed proportions at all: disabled -- `solve_meet_points_verified` has no
///   external signal to score against.
/// - Printed proportions exist, but every tier is pinned to its recorded mast:
///   **disabled** (a run against an all-pinned design cannot
///   change the verdict, so offering it invites minutes of work for nothing),
///   hinted that there's nothing to repair yet -- the user must first convert a
///   tier to a meet constraint (the tier list's "Adopt" action) before Deep Solve
///   has anything to search over.
/// - Printed proportions exist and at least one tier is meet-derived: enabled,
///   hinted with the cost/cancellability caveat instead.
fn deep_solve_hint(state: &EditorState) -> (bool, String) {
    if state.printed_proportions.is_none() {
        return (
            false,
            "Deep Solve needs this design's printed proportions (Vol/W^3, L/W, C/W, P/W, \
             H/W) from the catalogue to verify against -- unavailable for a new or \
             placeholder-reconstructed design."
                .to_string(),
        );
    }
    let has_repairable = state
        .design
        .tiers
        .iter()
        .any(|t| !matches!(t.constraint, MeetConstraint::ScaleReference(_)));
    if has_repairable {
        (
            true,
            "Slow (a mean of ~68 solves per design on the corpus -- minutes on a large \
             design); runs off the UI thread and can be cancelled."
                .to_string(),
        )
    } else {
        // An all-`ScaleReference` design cannot be repaired --
        // every tier is already pinned to its recorded mast, so a run here would
        // spend minutes unable to change the verdict either way. Disabled, not
        // merely enabled-with-a-caveat like the branch above -- the command bar's
        // Deep Solve button already gates its `enabled` on this same flag
        // (`editor_command_bar.slint`), so no markup change is needed here.
        (
            false,
            "Every tier is currently pinned to its recorded mast, exactly as imported -- \
             Deep Solve has nothing to repair until you convert a tier to a meet \
             constraint (the tier list's Adopt action)."
                .to_string(),
        )
    }
}

/// Renders a completed Deep Solve's [`VerifiedSolveReport`] as the status banner
/// text -- honestly: always shows the actual score movement and run count, never
/// just a pass/fail badge, and never claims `accepted` proves the geometry is right.
pub(super) fn format_deep_solve_report(report: &VerifiedSolveReport, stale: bool) -> String {
    let verdict = if report.accepted {
        "ACCEPTED -- reproduces the printed figures to verification accuracy (not a \
         correctness proof, only a strong external signal)"
    } else {
        "not accepted -- still deviates from the printed figures"
    };
    let scores = if report.initial_score.is_finite() {
        format!(
            "combined deviation {:.4} -> {:.4} ({} vertex-level repair(s), {} anchor \
             calibration move(s), {} pipeline run(s))",
            report.initial_score,
            report.final_score,
            report.overrides_applied,
            report.anchor_moves_applied,
            report.pipeline_runs
        )
    } else {
        "none of this design's printed figures overlapped what could be measured -- \
         unverifiable"
            .to_string()
    };
    let stale_note = if stale {
        " NOTE: the design changed while this ran -- re-run Deep Solve for a result that \
         reflects the current schedule."
    } else {
        ""
    };
    format!("Deep Solve: {verdict}. {scores}.{stale_note}")
}

/// The tier name to show alongside a tier-index reference in a result table --
/// `""` (never a placeholder like "(unnamed)") for an out-of-range index, since a
/// design edited between when a background search started and when its result
/// landed can shrink the tier list out from under a stale row (the caller already
/// flags that case `stale` independently; see [`TierMastDelta`]'s own doc comment).
fn tier_name_for_row(design: &Design, tier_index: usize) -> String {
    design
        .tiers
        .get(tier_index)
        .map(|tier| tier.name.clone())
        .unwrap_or_default()
}

/// Builds one [`DeepSolveTierRow`] per [`TierMastDelta`] -- the per-tier table
/// behind Deep Solve's aggregate verdict,
/// additional to (never replacing) [`format_deep_solve_report`]'s status-line
/// summary and [`super::deep_solve::format_tier_mast_deltas`]'s own one-line
/// suffix. `deltas` is expected to already be [`super::deep_solve::
/// tier_mast_deltas`]'s output -- already filtered to only the tiers whose mast
/// actually moved.
///
/// # Call site
/// `callbacks::solve_actions::setup_deep_solve_callback` calls this at its own
/// `DeepSolveOutcome::Completed` arm (see that function's doc comment) and pushes
/// the result to `EditorModel.deep_solve_tier_rows`, rendered by the Log popup's
/// per-tier table.
#[must_use]
pub(super) fn deep_solve_tier_rows(
    deltas: &[TierMastDelta],
    design: &Design,
) -> Vec<DeepSolveTierRow> {
    deltas
        .iter()
        .map(|d| DeepSolveTierRow {
            tier_number: format!("#{}", d.tier_index + 1).into(),
            name: tier_name_for_row(design, d.tier_index).into(),
            before_mast: format!("{:.4}", d.before_mast).into(),
            after_mast: format!("{:.4}", d.after_mast).into(),
            delta: format!("{:+.4}", d.delta()).into(),
        })
        .collect()
}

/// Builds one [`OptimizeChangeRow`] per [`indicatrix_cut_core::AngleChange`] a
/// pending Optimize result would apply -- the per-tier table behind
/// [`optimize_result_rows`]'s four aggregate component rows,
/// so a cutter can see WHICH tiers move, and by how much, before
/// clicking Apply.
///
/// # Call site
/// `callbacks::solve_actions::handle_optimize_outcome` calls this alongside
/// [`optimize_result_rows`] and pushes the result to
/// `EditorModel.optimize_change_rows`, rendered as the "Tiers this would change"
/// table in the Optimize tab.
#[must_use]
pub(super) fn optimize_change_rows(
    outcome: &OptimizeOutcome,
    design: &Design,
) -> Vec<OptimizeChangeRow> {
    outcome
        .changes
        .iter()
        .map(|change| OptimizeChangeRow {
            tier_number: format!("#{}", change.index + 1).into(),
            name: tier_name_for_row(design, change.index).into(),
            from_angle: format!("{:.2}\u{b0}", change.from_deg).into(),
            to_angle: format!("{:.2}\u{b0}", change.to_deg).into(),
            delta: format!("{:+.2}\u{b0}", change.to_deg - change.from_deg).into(),
        })
        .collect()
}

/// The design's total facet count after gear/symmetry expansion -- the count
/// [`EditorStatusStrip`](crate::gui::editor)'s persistent solver-state segment
/// still lacks (the state dot, tier
/// count and last-solve duration are already live there). One entry in
/// [`Design::planes_from_solved`]'s own output IS one facet, so this is just its
/// length -- no new geometry computation, only a name for a count that already
/// exists.
///
/// # Call site
/// [`refresh_editor_panel_from_solve`] (this same file, line ~113) calls this and
/// pushes the result to `EditorModel.facet_count`.
#[must_use]
pub(super) fn facet_count_from_solved(design: &Design, solved: &[SolvedTier]) -> usize {
    design.planes_from_solved(solved).len()
}

/// Whether Optimize is available for `state`'s current design, and the explanatory
/// hint `EditorView` shows next to its button either way.
///
/// Unlike [`deep_solve_hint`], this button is genuinely **disabled** (not merely
/// enabled-with-a-caveat) when there's nothing free to move: `free_tier_indices`
/// empty means `optimize_design` would return its input unchanged at zero
/// evaluations.
///
/// The one case worth stating loudly: a design fresh off the catalogue pins EVERY
/// tier to `ScaleReference` on import, so it has zero free tiers until the user
/// adopts at least one tier's real meet constraint or authors one by hand. That's
/// correct, expected behaviour for a design nobody has started editing, not a bug --
/// the hint text says so explicitly rather than leaving a disabled button to read as
/// broken.
///
/// `max_evaluations` is the CONFIGURED coordinate-stage budget
/// (`callbacks::solve_actions::configured_optimize_max_evaluations`, read from
/// `EditorModel.optimize_budget_text` -- `200` when unset/unparseable, matching
/// [`indicatrix_cut_core::OptimizeConfig::default`]), quoted here instead of a
/// literal `200` so the hint reflects what a run would actually use rather than a
/// hard-coded figure divorced from it.
///
/// Also names the fixed canonical light pose every Optimize
/// evaluation scores tilt brilliance under
/// ([`indicatrix_cut_core::optimize::CANONICAL_LIGHT_YAW`]/`CANONICAL_LIGHT_PITCH`)
/// -- NOT the light the trace/HUD/tilt dialog show after the cutter drags it. The
/// tilt dialog's own caption already declares this half of the discrepancy
/// (`performance_graph_dialog.slint`); this doc comment states the Optimize-panel
/// half of it.
fn optimize_hint(state: &EditorState, max_evaluations: usize) -> (bool, String) {
    let free = free_tier_indices(&state.design);
    if free.is_empty() {
        (
            false,
            "Every tier is currently pinned as a scale reference -- a freshly \
             imported design starts this way, and that is correct, not broken. \
             Optimize has nothing free to move until you adopt at least one tier's \
             real meet constraint (the tier list's Adopt action) or author one by \
             hand."
                .to_string(),
        )
    } else {
        (
            true,
            format!(
                "Coordinate search over {} free tier angle(s), scored on windowing, \
                 extinction, and tilt brilliance under a FIXED canonical light pose \
                 (not necessarily the light you see in the viewport right now -- drag \
                 the light and these figures can disagree with the trace/HUD until you \
                 re-run Optimize). Roughly 7 ms per evaluation on a small design, but \
                 up to several seconds each on a large, heavily meet-derived one (a \
                 {max_evaluations}-evaluation budget can then take minutes) -- plus two \
                 fixed full-fidelity scorings (one before, one after the search) that \
                 can each take over a second on their own, so even a fast run has some \
                 up-front and trailing wait beyond the quoted per-evaluation cost. Runs \
                 off the UI thread and can be cancelled.",
                free.len()
            ),
        )
    }
}

/// Formats one objective component's "after" cell as the raw value plus a signed
/// delta and a plain-English verdict -- "9.25% (-3.25%,
/// better)" rather than a bare number the cutter has to subtract by hand and
/// remember the polarity of. `higher_is_better` distinguishes tilt brilliance
/// (higher is better) from every other component/the blended score (lower is
/// better, see [`ObjectiveWeights::score`]'s own doc comment).
///
/// # Handoff
/// `OptimizeResultRow` (`ui/types.slint`) has only
/// `label`/`before`/`after` -- the delta/verdict below is folded into `after`'s
/// own string rather than added as new `delta`/`improved: bool` fields (and
/// coloured emerald/ruby per row, `ui/components/editor_inspector.slint`).
/// Adding those two fields plus the row colouring is still open.
#[must_use]
fn after_with_delta(before: f32, after: f32, higher_is_better: bool, unit: &str) -> (String, i32) {
    let delta = after - before;
    // `delta.abs() < f32::EPSILON` rather than `delta == 0.0` -- clippy's
    // `float_cmp` lint (pedantic) flags exact float equality even here, where
    // `delta` is a plain subtraction of two already-rounded measurements.
    let direction = if delta.abs() < f32::EPSILON {
        0
    } else if (higher_is_better && delta > 0.0) || (!higher_is_better && delta < 0.0) {
        1
    } else {
        -1
    };
    let verdict = match direction {
        1 => "better",
        -1 => "worse",
        _ => "unchanged",
    };
    (
        format!("{after:.2}{unit} ({delta:+.2}{unit}, {verdict})"),
        direction,
    )
}

/// Builds the rows `EditorView`'s Optimize result table needs from a completed
/// or cancelled run's [`OptimizeOutcome`] -- windowing, extinction, tilt
/// brilliance, and yield loss each get their OWN row, and the blended score
/// comes last as a clearly-separate row: an optimizer that improved windowing by
/// wrecking extinction must be visibly doing that, never collapsed into a single
/// figure. Each row's `after` cell also names its own signed delta and direction
/// via [`after_with_delta`], so a cutter reads which metric
/// moved and by how much without doing the subtraction (or remembering which way
/// is good) themselves.
///
/// The "Yield loss" row is shown
/// UNCONDITIONALLY, even at the Optimize tab's default `yield_weight == 0.0` --
/// `before_yield_loss_pct`/`after_yield_loss_pct` are real measurements of the
/// starting/final design either way (see [`OptimizeOutcome::before_yield_loss_pct`]'s
/// own doc comment), and a cutter who left the slider at its default still
/// benefits from seeing whether Optimize's angle changes happened to help or hurt
/// yield, even though the search itself never weighed it.
pub(super) fn optimize_result_rows(outcome: &OptimizeOutcome) -> Vec<OptimizeResultRow> {
    vec![
        metric_row(
            "Windowing",
            outcome.before.windowing_pct,
            outcome.after.windowing_pct,
            false,
            "%",
        ),
        metric_row(
            "Extinction",
            outcome.before.extinction_pct,
            outcome.after.extinction_pct,
            false,
            "%",
        ),
        metric_row(
            "Tilt brilliance",
            outcome.before.tilt_brilliance_pct,
            outcome.after.tilt_brilliance_pct,
            true,
            "%",
        ),
        metric_row(
            "Yield loss",
            outcome.before_yield_loss_pct,
            outcome.after_yield_loss_pct,
            false,
            "%",
        ),
        metric_row(
            "Blended score",
            outcome.before_score,
            outcome.after_score,
            false,
            "",
        ),
    ]
}

/// One row of [`optimize_result_rows`] -- the before figure, the after figure with
/// its own delta and verdict, and the direction the row is coloured by.
///
/// `higher_is_better` is the metric's own polarity, not a property of the numbers:
/// windowing and extinction going DOWN is an improvement, tilt brilliance going up
/// is. Getting that backwards would colour a real improvement red, which is why it
/// is stated per call rather than inferred.
fn metric_row(
    label: &str,
    before: f32,
    after: f32,
    higher_is_better: bool,
    unit: &str,
) -> OptimizeResultRow {
    let (after_text, direction) = after_with_delta(before, after, higher_is_better, unit);
    OptimizeResultRow {
        label: label.into(),
        before: format!("{before:.2}{unit}").into(),
        after: after_text.into(),
        direction,
    }
}

/// The one-line summary shown above [`optimize_result_rows`]'s table -- how many
/// tiers changed and how many candidate evaluations it took, plus (only when true)
/// the cancellation note and (only when the polish stage actually ran) how much of
/// the final score it is responsible for -- without this note,
/// `polish_evaluations`/`polish_improvement` would have no reader anywhere in
/// this crate. The per-component before/after numbers themselves live only in the
/// rows table, never duplicated here.
pub(super) fn optimize_status_text(outcome: &OptimizeOutcome) -> String {
    let cancelled_note = if outcome.cancelled {
        " (cancelled -- showing the best partial result found before the checkpoint \
         fired)"
    } else {
        ""
    };
    let polish_note = if outcome.polish_evaluations > 0 {
        format!(
            " (polish: +{:.2} in {} evaluation(s))",
            outcome.polish_improvement, outcome.polish_evaluations
        )
    } else {
        String::new()
    };
    if outcome.changes.is_empty() {
        format!(
            "Optimize found no improving move in {} evaluation(s) -- this design's \
             free tiers were already at (or very near) a local optimum for these \
             weights.{cancelled_note}{polish_note}",
            outcome.evaluations
        )
    } else {
        format!(
            "Optimize changed {} tier(s) in {} evaluation(s).{cancelled_note}{polish_note}",
            outcome.changes.len(),
            outcome.evaluations
        )
    }
}

/// A clone of `design` with every one of
/// `outcome`'s [`indicatrix_cut_core::AngleChange`]s already applied -- the
/// candidate a "Preview" toggle shows in the viewport BEFORE the cutter commits to
/// Apply. Never touches `History`/`Edit` at all: `ConstraintTier::angle_deg` is a
/// plain public field, and this is a display-only candidate, never something an
/// Undo could need to unwind.
#[must_use]
pub(super) fn build_optimize_preview_design(design: &Design, outcome: &OptimizeOutcome) -> Design {
    let mut preview = design.clone();
    for change in &outcome.changes {
        if let Some(tier) = preview.tiers.get_mut(change.index) {
            tier.angle_deg = change.to_deg;
        }
    }
    preview
}

/// Solves `design` and, on success, redraws the shared solid-preview viewport with
/// its planes at the CURRENT camera pose -- a raw, generation-independent reproject
/// (`SolidPreviewState::request_redraw_with_gear`, the same call
/// `gui::render::camera_lighting::resubmit_at_current_pose` uses for a camera
/// drag), deliberately NOT [`submit_preview_replan`]'s worker-queued
/// [`ReplanRequest`] path: a ghost preview must never stamp `solid_last_solved`/the
/// design-generation stash with a CANDIDATE design's own solved masts, which would
/// corrupt the next real edit's `resolve_dirty` baseline and the next landed
/// worker frame's tier-table push (`push_solved_preview`) into showing the ghost's
/// numbers instead of the live design's.
///
/// Returns whether `design` actually solved (and so was shown) -- `false` leaves
/// the viewport showing whatever it already had, since there is no honest
/// candidate geometry to draw for a design that does not close.
///
/// Used by both Optimize's own "Preview" toggle (via
/// [`build_optimize_preview_design`]) and Retarget's live ghost overlay
/// (`callbacks::retarget_actions`), so the two features share one implementation of
/// "show this candidate, without disturbing anything the REAL design's next edit
/// depends on."
#[must_use]
pub(super) fn submit_design_ghost_preview(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    design: &Design,
) -> bool {
    let Ok(solved) = design.solve() else {
        return false;
    };
    let planes_gpu = auto_solve::design_to_gpu_planes_from_solved(design, &solved);
    let planes: Vec<(glam::Vec3, f32)> = planes_gpu
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let camera = CameraPose {
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
    };
    let design_gear = ctx.design_gear;
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    let size = contained_request_size(view_mode, scaled_viewport_size(ui), (ctx.width, ctx.height));
    drop(ctx);
    preview_state.request_redraw_with_gear(planes, camera, size, view_mode, design_gear);
    true
}

/// Parses the Optimize weight form's three text fields (plus the yield slider's
/// own already-numeric `0..1` value) into an [`ObjectiveWeights`] -- the three
/// text fields must parse and be finite, but also reject negative values: only
/// the RATIOS between them matter, so a negative one would silently invert that
/// component's polarity (rewarding more windowing, say) rather than merely
/// weighting it oddly. `yield_weight` needs no such validation: it comes
/// straight from `EditorModel.optimize_weight_yield`
/// (`ui/components/editor_inspector.slint`'s `Slider`, `minimum: 0.0, maximum:
/// 1.0`), which cannot produce a non-finite or out-of-range value in the first
/// place.
pub(super) fn parse_optimize_weights(
    windowing: &str,
    extinction: &str,
    tilt_brilliance: &str,
    yield_weight: f32,
) -> Result<ObjectiveWeights, String> {
    fn parse_weight(label: &str, text: &str) -> Result<f32, String> {
        let value: f32 = text
            .trim()
            .parse()
            .map_err(|_| format!("{label} weight must be a number."))?;
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "{label} weight must be a non-negative, finite number."
            ));
        }
        Ok(value)
    }
    Ok(ObjectiveWeights {
        windowing: parse_weight("Windowing", windowing)?,
        extinction: parse_weight("Extinction", extinction)?,
        tilt_brilliance: parse_weight("Tilt brilliance", tilt_brilliance)?,
        yield_weight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::stone_metrics::ExternalProportions;
    use indicatrix_cut_core::{AngleChange, ConstraintTier, ObjectiveComponents};

    fn some_proportions() -> ExternalProportions {
        ExternalProportions {
            vol_w3: Some(1.2),
            lw: Some(1.0),
            cw: Some(0.2),
            pw: Some(0.4),
            hw: Some(0.6),
        }
    }

    fn scale_reference_tier(value: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg: 0.0,
            name: "T".to_string(),
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(value),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    // --- The `_from_solved` mirrors must dash out
    // a tierless design too, not just their plain (internally-solving)
    // counterparts -- see `proportions_texts_from_solved`'s own doc comment for
    // why a guard on `girdle_and_ratio_texts` alone would miss the hot
    // path `refresh_editor_panel_from_solve` actually takes once a design solves. ---

    #[test]
    fn proportions_texts_from_solved_dashes_out_a_design_with_no_tiers() {
        // A brand-new design (`Design::fresh`) has no tiers, but it still SOLVES
        // (an empty mast list is a valid, closed, zero-plane solve) -- so this
        // must be exercised with a REAL `solved` list from that solve, exactly
        // like `refresh_editor_panel_from_solve` hands one to this function,
        // not skipped because "the design doesn't solve."
        let design = EditorState::fresh().design;
        let solved = design.solve().expect("a tierless design still solves");
        assert_eq!(
            proportions_texts_from_solved(&design, &solved),
            (
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
            ),
            "a tierless design's proportions must never show the bare preform \
             block's own numbers (table 100%, total depth = preform depth) as \
             if they were the stone's"
        );
    }

    #[test]
    fn girdle_and_ratio_texts_from_solved_dashes_out_a_design_with_no_tiers() {
        let design = EditorState::fresh().design;
        let solved = design.solve().expect("a tierless design still solves");
        assert_eq!(
            girdle_and_ratio_texts_from_solved(&design, &solved),
            (
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
            )
        );
    }

    #[test]
    fn deep_solve_hint_is_unavailable_for_a_fresh_design_with_no_printed_proportions() {
        // A brand-new design has `printed_proportions: None` -- nothing to verify
        // against, so this must read as unavailable, not "nothing to repair yet".
        let state = EditorState::fresh();
        let (available, hint) = deep_solve_hint(&state);
        assert!(!available);
        assert!(hint.contains("printed proportions"));
    }

    #[test]
    fn deep_solve_hint_is_unavailable_when_every_tier_is_pinned_with_nothing_to_repair() {
        // Printed proportions exist, but every tier is pinned to a `ScaleReference`
        // -- correct and expected for an untouched import, but a run cannot change
        // the verdict either way, so the button must be DISABLED, with an
        // explanatory hint rather than reading as broken or missing.
        let mut state = EditorState::fresh();
        state.printed_proportions = Some(some_proportions());
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(scale_reference_tier(0.8));

        let (available, hint) = deep_solve_hint(&state);
        assert!(!available);
        assert!(hint.contains("pinned"));
        assert!(hint.contains("Adopt"));
    }

    #[test]
    fn deep_solve_hint_is_available_with_the_cost_caveat_when_a_tier_is_meet_derived() {
        // At least one tier is not pinned to a recorded mast -- Deep Solve has
        // something to search over, so the hint should be the cost/cancellability
        // caveat, not "nothing to repair".
        let mut state = EditorState::fresh();
        state.printed_proportions = Some(some_proportions());
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (available, hint) = deep_solve_hint(&state);
        assert!(available);
        assert!(!hint.contains("pinned"));
        assert!(hint.to_lowercase().contains("cancel"));
    }

    #[test]
    fn optimize_hint_is_unavailable_when_every_tier_is_pinned() {
        // A design with only `ScaleReference` tiers has zero free tiers. Must read
        // as "nothing to optimize yet, and that's expected," never as broken.
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(scale_reference_tier(0.8));

        let (available, hint) = optimize_hint(&state, 200);
        assert!(!available);
        assert!(hint.contains("pinned"));
        assert!(hint.contains("Adopt"));
    }

    #[test]
    fn optimize_hint_is_available_once_a_tier_is_free_to_move() {
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (available, hint) = optimize_hint(&state, 200);
        assert!(available);
        assert!(
            hint.contains('1'),
            "expected the free-tier count in: {hint}"
        );
        assert!(hint.to_lowercase().contains("cancel"));
    }

    #[test]
    fn optimize_hint_quotes_the_configured_budget_not_a_hardcoded_number() {
        // The hint must reflect whatever budget the caller
        // passes in, not a literal `200` baked into the format string.
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (_, hint) = optimize_hint(&state, 750);
        assert!(hint.contains("750"), "expected the budget in: {hint}");
        assert!(!hint.contains("200-evaluation"));
    }

    #[test]
    fn optimize_hint_names_the_canonical_light_pose() {
        // The Optimize panel must say its score is measured
        // under a fixed pose, not the user's own light.
        let mut state = EditorState::fresh();
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 24.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let (_, hint) = optimize_hint(&state, 200);
        assert!(hint.to_lowercase().contains("canonical light pose"));
    }

    /// A hand-built [`OptimizeOutcome`] whose `after` deliberately makes extinction
    /// WORSE while windowing and tilt brilliance improve -- proving
    /// [`optimize_result_rows`] reports every component's real number rather than
    /// only the still-improved blended score.
    fn sample_outcome(changed: bool, cancelled: bool) -> OptimizeOutcome {
        OptimizeOutcome {
            before: ObjectiveComponents {
                windowing_pct: 12.5,
                extinction_pct: 8.0,
                tilt_brilliance_pct: 60.0,
            },
            before_score: 20.0,
            before_yield_loss_pct: 30.0,
            after: ObjectiveComponents {
                windowing_pct: 9.25,
                extinction_pct: 11.0,
                tilt_brilliance_pct: 65.0,
            },
            after_score: 15.0,
            after_yield_loss_pct: 25.0,
            evaluations: 42,
            changes: if changed {
                vec![AngleChange {
                    index: 3,
                    from_deg: -40.0,
                    to_deg: -41.5,
                }]
            } else {
                Vec::new()
            },
            cancelled,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        }
    }

    #[test]
    fn optimize_result_rows_reports_each_component_separately_never_collapsed() {
        let outcome = sample_outcome(true, false);
        let rows = optimize_result_rows(&outcome);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].label.as_str(), "Windowing");
        assert_eq!(rows[0].before.as_str(), "12.50%");
        // Lower windowing is better -- a negative delta reads as "better".
        assert_eq!(rows[0].after.as_str(), "9.25% (-3.25%, better)");
        // Extinction got WORSE -- shown honestly, not hidden by the improved score.
        assert_eq!(rows[1].label.as_str(), "Extinction");
        assert_eq!(rows[1].before.as_str(), "8.00%");
        assert_eq!(rows[1].after.as_str(), "11.00% (+3.00%, worse)");
        assert_eq!(rows[2].label.as_str(), "Tilt brilliance");
        assert_eq!(rows[2].before.as_str(), "60.00%");
        // Higher tilt brilliance is better -- a positive delta reads as "better".
        assert_eq!(rows[2].after.as_str(), "65.00% (+5.00%, better)");
        // Yield loss went DOWN (less preform thrown away) -- reads as "better",
        // same polarity as windowing/extinction, even though this fixture's
        // `ObjectiveWeights` (implicit -- `OptimizeOutcome` carries no weights of
        // its own) never actually weighed it into the blended score.
        assert_eq!(rows[3].label.as_str(), "Yield loss");
        assert_eq!(rows[3].before.as_str(), "30.00%");
        assert_eq!(rows[3].after.as_str(), "25.00% (-5.00%, better)");
        assert_eq!(rows[4].label.as_str(), "Blended score");
        assert_eq!(rows[4].before.as_str(), "20.00");
        assert_eq!(rows[4].after.as_str(), "15.00 (-5.00, better)");
    }

    #[test]
    fn build_optimize_preview_design_moves_only_the_changed_tiers_angle() {
        // The ghost-preview candidate must apply every
        // `AngleChange` to the right tier and leave every other tier's angle (and
        // every other field) untouched.
        let mut design = Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            indicatrix_cut_core::ScheduleMeta::default(),
            vec![
                scale_reference_tier(0.5),
                scale_reference_tier(0.6),
                scale_reference_tier(0.7),
                ConstraintTier {
                    angle_deg: -40.0,
                    name: "P1".to_string(),
                    indices: vec![0.0, 24.0],
                    constraint: MeetConstraint::ScaleReference(0.8),
                    imported_meet: None,
                    original_notes: None,
                    detached: Vec::new(),
                },
            ],
        );
        design.tiers[3].angle_deg = -40.0;
        let outcome = sample_outcome(true, false); // changes tier index 3 to -41.5
        let preview = build_optimize_preview_design(&design, &outcome);
        assert!((preview.tiers[3].angle_deg - (-41.5)).abs() < 1e-9);
        // Every other tier is untouched.
        for i in 0..3 {
            assert!((preview.tiers[i].angle_deg - design.tiers[i].angle_deg).abs() < 1e-9);
        }
        // The original design is never mutated.
        assert!((design.tiers[3].angle_deg - (-40.0)).abs() < 1e-9);
    }

    #[test]
    fn build_optimize_preview_design_ignores_an_out_of_range_change_index() {
        // A design edited between when Optimize ran and when the preview toggle is
        // flipped can shrink the tier list out from under a stale outcome -- this
        // must degrade gracefully (skip that change), never panic.
        let design = Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            indicatrix_cut_core::ScheduleMeta::default(),
            vec![scale_reference_tier(0.5)],
        );
        let outcome = sample_outcome(true, false); // names tier index 3, out of range
        let preview = build_optimize_preview_design(&design, &outcome);
        assert_eq!(preview.tiers.len(), 1);
    }

    #[test]
    fn after_with_delta_reports_unchanged_when_the_value_did_not_move() {
        assert_eq!(
            after_with_delta(5.0, 5.0, false, "%"),
            ("5.00% (+0.00%, unchanged)".to_string(), 0)
        );
    }

    #[test]
    fn optimize_status_text_reports_the_change_and_evaluation_count() {
        let text = optimize_status_text(&sample_outcome(true, false));
        assert!(text.contains("1 tier(s)"));
        assert!(text.contains("42 evaluation(s)"));
        assert!(!text.contains("cancelled"));
    }

    #[test]
    fn optimize_status_text_reports_no_improving_move_when_nothing_changed() {
        let text = optimize_status_text(&sample_outcome(false, false));
        assert!(text.contains("no improving move"));
    }

    #[test]
    fn optimize_status_text_notes_cancellation_without_hiding_the_partial_result() {
        // `after`/`changes` still reflect the best REAL partial result found, never
        // discarded -- the cancellation note must be additive, not replace the summary.
        let text = optimize_status_text(&sample_outcome(true, true));
        assert!(text.contains("cancelled"));
        assert!(text.contains("1 tier(s)"));
    }

    #[test]
    fn optimize_status_text_names_the_polish_stages_own_contribution_when_it_ran() {
        // Without this reader, `polish_evaluations`/`polish_improvement` would
        // have no consumer anywhere in this crate -- whether the ridge-following
        // polish stage did anything at all would be invisible to a cutter.
        let mut outcome = sample_outcome(true, false);
        outcome.polish_evaluations = 31;
        outcome.polish_improvement = 0.42;
        let text = optimize_status_text(&outcome);
        assert!(text.contains("polish: +0.42 in 31 evaluation(s)"));
    }

    #[test]
    fn optimize_status_text_omits_the_polish_note_when_the_stage_never_ran() {
        let text = optimize_status_text(&sample_outcome(true, false));
        assert!(!text.contains("polish"));
    }

    #[test]
    fn parse_optimize_weights_accepts_well_formed_input() {
        let weights = parse_optimize_weights("1.0", "2.5", "0", 0.0).unwrap();
        assert_eq!(weights.windowing, 1.0);
        assert_eq!(weights.extinction, 2.5);
        assert_eq!(weights.tilt_brilliance, 0.0);
        assert_eq!(weights.yield_weight, 0.0);
    }

    #[test]
    fn parse_optimize_weights_rejects_a_non_numeric_field() {
        let err = parse_optimize_weights("not-a-number", "1.0", "1.0", 0.0).unwrap_err();
        assert!(err.contains("Windowing"));
    }

    #[test]
    fn parse_optimize_weights_rejects_a_negative_weight() {
        // A negative weight is not merely out of range -- it would invert that
        // component's polarity -- so this is checked separately from finiteness.
        let err = parse_optimize_weights("1.0", "-0.5", "1.0", 0.0).unwrap_err();
        assert!(err.contains("Extinction"));
    }

    #[test]
    fn parse_optimize_weights_rejects_non_finite_values() {
        assert!(parse_optimize_weights("NaN", "1.0", "1.0", 0.0).is_err());
        assert!(parse_optimize_weights("1.0", "inf", "1.0", 0.0).is_err());
    }

    /// `yield_weight` comes
    /// straight from the Optimize tab's `0..1` slider, not a parsed text field --
    /// it passes through into `ObjectiveWeights` untouched, whatever value it is
    /// (the slider itself is what keeps it in range).
    #[test]
    fn parse_optimize_weights_carries_the_yield_slider_value_through_untouched() {
        let weights = parse_optimize_weights("1.0", "1.0", "1.0", 0.4).unwrap();
        assert_eq!(weights.yield_weight, 0.4);
    }

    fn design_with_named_tiers(names: &[&str]) -> indicatrix_cut_core::Design {
        let mut state = EditorState::fresh();
        for &name in names {
            state.design.tiers.push(ConstraintTier {
                angle_deg: -40.0,
                name: name.to_string(),
                indices: vec![0.0],
                constraint: MeetConstraint::MeetExisting,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            });
        }
        state.design
    }

    // --- selected_tier_chips ---

    #[test]
    fn selected_tier_chips_is_empty_when_nothing_is_selected() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(selected_tier_chips(&design, -1).len(), 0);
    }

    #[test]
    fn selected_tier_chips_is_empty_for_an_out_of_range_index() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(selected_tier_chips(&design, 5).len(), 0);
    }

    #[test]
    fn selected_tier_chips_reads_the_selected_tiers_own_indices_and_detached_set() {
        let mut design = design_with_named_tiers(&["G1", "P1"]);
        design.tiers[1].indices = vec![0.0, 24.0];
        design.tiers[1].detached = vec![24.0];
        let chips = selected_tier_chips(&design, 1);
        assert_eq!(chips.len(), 2);
        assert!(!chips[0].detached);
        assert!(chips[1].detached);
    }

    // --- deep_solve_tier_rows ---

    #[test]
    fn deep_solve_tier_rows_formats_one_row_per_delta_with_the_tiers_own_name() {
        let design = design_with_named_tiers(&["G1", "P1", "P2"]);
        let deltas = [
            TierMastDelta {
                tier_index: 1,
                before_mast: 1.0,
                after_mast: 1.25,
            },
            TierMastDelta {
                tier_index: 2,
                before_mast: 2.0,
                after_mast: 1.9,
            },
        ];
        let rows = deep_solve_tier_rows(&deltas, &design);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tier_number.as_str(), "#2");
        assert_eq!(rows[0].name.as_str(), "P1");
        assert_eq!(rows[0].before_mast.as_str(), "1.0000");
        assert_eq!(rows[0].after_mast.as_str(), "1.2500");
        assert_eq!(rows[0].delta.as_str(), "+0.2500");
        // A negative movement stays signed, not just "smaller".
        assert_eq!(rows[1].delta.as_str(), "-0.1000");
    }

    #[test]
    fn deep_solve_tier_rows_is_empty_for_no_deltas() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(deep_solve_tier_rows(&[], &design).len(), 0);
    }

    #[test]
    fn deep_solve_tier_rows_names_an_out_of_range_tier_blank_rather_than_panicking() {
        // A design edited between Deep Solve's dispatch and its completion can
        // shrink the tier list out from under a stale delta -- this must degrade
        // gracefully, not panic or fabricate a name.
        let design = design_with_named_tiers(&["G1"]);
        let deltas = [TierMastDelta {
            tier_index: 5,
            before_mast: 1.0,
            after_mast: 1.1,
        }];
        let rows = deep_solve_tier_rows(&deltas, &design);
        assert_eq!(rows[0].name.as_str(), "");
    }

    // --- optimize_change_rows ---

    #[test]
    fn optimize_change_rows_formats_one_row_per_angle_change_with_the_tiers_own_name() {
        let design = design_with_named_tiers(&["G1", "P1"]);
        let outcome = OptimizeOutcome {
            before: ObjectiveComponents {
                windowing_pct: 0.0,
                extinction_pct: 0.0,
                tilt_brilliance_pct: 0.0,
            },
            before_score: 0.0,
            before_yield_loss_pct: 0.0,
            after: ObjectiveComponents {
                windowing_pct: 0.0,
                extinction_pct: 0.0,
                tilt_brilliance_pct: 0.0,
            },
            after_score: 0.0,
            after_yield_loss_pct: 0.0,
            evaluations: 1,
            changes: vec![AngleChange {
                index: 1,
                from_deg: -40.0,
                to_deg: -41.5,
            }],
            cancelled: false,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        };
        let rows = optimize_change_rows(&outcome, &design);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tier_number.as_str(), "#2");
        assert_eq!(rows[0].name.as_str(), "P1");
        assert_eq!(rows[0].from_angle.as_str(), "-40.00\u{b0}");
        assert_eq!(rows[0].to_angle.as_str(), "-41.50\u{b0}");
        assert_eq!(rows[0].delta.as_str(), "-1.50\u{b0}");
    }

    // --- facet_count_from_solved ---

    #[test]
    fn facet_count_from_solved_is_the_length_of_the_designs_expanded_planes() {
        let mut design = design_with_named_tiers(&["G1"]);
        design.tiers[0].constraint = MeetConstraint::ScaleReference(1.0);
        let solved = design
            .solve()
            .expect("a single scale-reference tier always solves");
        // A thin wrapper, so this mostly guards against the wrapper drifting from
        // `Design::planes_from_solved`'s own count rather than testing geometry.
        assert_eq!(
            facet_count_from_solved(&design, &solved),
            design.planes_from_solved(&solved).len()
        );
        assert!(
            facet_count_from_solved(&design, &solved) > 0,
            "a solved design always has at least one facet plane"
        );
    }

    // --- traced_material_for (the missing-name case must
    // refuse, never silently fall through to `resolve_material`'s own
    // materials[0]/Diamond fallback) ---

    #[test]
    fn traced_material_for_names_a_resolvable_built_in_selection() {
        let mut design = EditorState::fresh().design;
        design.material.name = Some("Sapphire".to_string());
        let (name, unresolved) = traced_material_for(&design, &[]);
        assert_eq!(name, "Sapphire");
        assert_eq!(unresolved, None);
    }

    #[test]
    fn traced_material_for_names_a_resolvable_custom_selection() {
        let mut design = EditorState::fresh().design;
        design.material.name = Some("My Garnet".to_string());
        let mut custom = indicatrix::optics::materials::GemMaterial::diamond();
        custom.name = "My Garnet".to_string();
        let (name, unresolved) = traced_material_for(&design, &[custom]);
        assert_eq!(name, "My Garnet");
        assert_eq!(unresolved, None);
    }

    /// The actual bug case this guards against: a design naming a material that
    /// resolves through NEITHER the built-in table NOR the live custom-material
    /// list (a deleted custom material, or a typo'd/hand-edited name) must refuse
    /// -- not report itself resolved and leave `sync_viewport_material_link` to
    /// fall through to `resolve_material`'s silent `materials[0]` (Diamond)
    /// fallback.
    #[test]
    fn traced_material_for_refuses_a_name_no_catalogue_resolves() {
        let mut design = EditorState::fresh().design;
        design.material.name = Some("Deleted Custom Garnet".to_string());
        let (name, unresolved) = traced_material_for(&design, &[]);
        assert_eq!(name, "");
        let reason = unresolved.expect("an unresolvable name must refuse, not substitute");
        assert!(reason.contains("Deleted Custom Garnet"));
    }
}
