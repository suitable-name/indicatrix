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
        EditorMaterialLookup, MATERIAL_MATCH_TOLERANCE, nearest_built_in_material,
        traced_gem_material,
    },
    state::{
        EditorState, ScratchDelta, apply_multi_selection, cutting_schedule_rows, design_label_text,
        design_material_index_from_name, design_material_options, design_to_gpu_planes,
        gear_index_from_teeth, index_chip_items, manufacturability_warning_lines,
        manufacturability_warning_lines_from_solved, manufacturability_warnings_tagged,
        material_index_from_name, preform_mm_texts, proportions_texts, push_multi_selected_count,
        push_tiers, ri_source_text, status_text_and_is_problem,
        status_text_and_is_problem_from_solved, tier_items, tier_items_from_solved,
        tier_items_stale, yield_report_texts, yield_report_texts_from_solved,
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
    Design, MissingAnchor, ObjectiveWeights, OptimizeOutcome, PreformShape, critical_angle_deg,
    free_tier_indices,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
    time::Instant,
};

/// Refreshes everything `EditorView` itself renders (tier list, undo/redo
/// availability, validation banner, preform fields) -- but NOT the shared viewport,
/// see [`refresh_all`] for why those are kept separate.
///
/// Calls [`Design::solve`] exactly ONCE (CAD audit item 111) and derives every
/// other panel field from that SAME `solved_result` via its `_from_solved`
/// counterpart or a small local mirror of it (`proportions_texts_from_solved`/
/// `preform_mm_texts_from_solved` below -- `state/mod.rs` is not this lane's file
/// to add a new function to, same reasoning `auto_solve::design_to_gpu_planes_from_solved`
/// already documents on itself). This used to call `tier_items`,
/// `status_text_and_is_problem` (itself two solves, via `status()`/`measure()`),
/// `manufacturability_warning_lines`, `yield_report_texts`, `proportions_texts`,
/// `preform_mm_texts`, and a final explicit `state.design.solve()` for
/// `cutting_rows` -- six or more independent solves for one "Solve" click.
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
/// otherwise pay for a second, redundant one (CAD audit item 111, the timing
/// half: `refresh_all` used to solve once purely to measure elapsed time, then
/// call this function, which solved AGAIN internally to build the panel fields --
/// two real solves for one "Solve" click, not one).
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
    solved_result: Result<Vec<SolvedTier>, MissingAnchor>,
) -> Option<Vec<SolvedTier>> {
    // Computed exactly once per refresh -- see `ScratchDelta`'s own doc
    // comment for why each group below is gated on its OWN flag rather than
    // "anything changed" (CAD audit items 50/52).
    let delta = state.record_scratch_push();
    let n_d = refresh_design_settings(ui, render_ctx, state, &delta);
    // Item 151: derived from the same solve everything else on this refresh uses,
    // never a second one. Zero when the design does not currently solve, which
    // reads as "nothing to count yet" rather than as a stale figure.
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

    // Manufacturability warnings against this same "Solve" click's design state.
    let warnings: Vec<SharedString> = solved
        .map_or_else(
            || manufacturability_warning_lines(&state.design),
            |solved| manufacturability_warning_lines_from_solved(&state.design, solved),
        )
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(warnings)));

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
        // CAD audit item 136: the mm equivalent shown ALONGSIDE the model-unit
        // fields above -- see `preform_mm_texts`'s own doc comment.
        let (preform_half_width_mm, preform_depth_mm) = solved.map_or_else(
            || preform_mm_texts(&state.design),
            |solved| preform_mm_texts_from_solved(&state.design, solved),
        );
        ui.global::<EditorModel>()
            .set_preform_half_width_mm_text(preform_half_width_mm.into());
        ui.global::<EditorModel>()
            .set_preform_depth_mm_text(preform_depth_mm.into());
    }

    // Seed the Yield form's scratch buffers from the design's current state
    // (only when it actually changed -- see `delta`'s own doc comment), then
    // push the read-only figures this same "Solve" click's state produces
    // (always, since those are never user-editable).
    push_yield_material_scratch(ui, state, &delta);
    let (vol_yield_text, carat_text, sg_used_text, fit_text) = solved.map_or_else(
        || yield_report_texts(&state.design),
        |solved| yield_report_texts_from_solved(&state.design, solved),
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

    ui.global::<EditorModel>()
        .set_design_label(design_label_text(state.asc_filename.as_deref()).into());
    if let Some(solved) = solved {
        let rows: Vec<AngleItem> = cutting_schedule_rows(&state.design, solved);
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(rows)));
    } else {
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));
    }

    refresh_deep_solve_availability(ui, state);
    refresh_optimize_availability(ui, state);
    solved_result.ok()
}

/// [`proportions_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`refresh_editor_panel`]'s own doc
/// comment for why this small mirror lives here rather than as a new function on
/// `state/mod.rs` (not this lane's file to add one to). Mirrors that function's
/// body exactly, minus the internal `design.solve()` it exists to avoid repeating.
#[must_use]
fn proportions_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
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

/// [`preform_mm_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`proportions_texts_from_solved`]'s
/// own doc comment for why this lives here.
#[must_use]
fn preform_mm_texts_from_solved(design: &Design, solved: &[SolvedTier]) -> (String, String) {
    let Some(mm_per_unit) = design.yield_report(solved).mm_per_unit else {
        return (String::new(), String::new());
    };
    let preform = &design.preform;
    (
        format!("\u{2248} {:.3} mm", preform.half_width * mm_per_unit),
        format!("\u{2248} {:.3} mm", preform.depth * mm_per_unit),
    )
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
/// CAD audit item 148: this used to also require `render_view_tab == 1` (the
/// Edit tab itself being shown), so switching to Live Render and back never
/// re-synced anything until the next edit happened to run this function again --
/// the render's material depended on tab history rather than on the design.
/// `viewport_material_linked` alone is the real gate a display override needs.
///
/// Returns this design's effective refractive index so [`tier_items`]/
/// [`tier_items_stale`] can reuse the identical value for their per-tier
/// margin/risk column rather than re-deriving it.
///
/// `delta` (from [`EditorState::record_scratch_push`], computed once by the
/// caller) gates the material/gear/symmetry scratch pushes independently, so
/// an edit that only changed one of the three never re-seeds -- and so
/// silently discards any in-progress typing/selection in -- the other two's
/// fields (CAD audit items 50/52).
/// The material name the tracer should use for `design`, and -- when there is no
/// honest answer -- the sentence saying why it will not trace at all.
///
/// CAD audit item 57. A design built from an `.asc`, or a brand-new one, carries
/// `MaterialSelection::none()`: the schedule records a refractive index but never a
/// species. This used to substitute `"Diamond"`, so a quartz design was traced,
/// tilt-swept and HUD-scored at n=2.417 while MARGIN and the critical angle beside
/// them used its real n=1.5442 -- two numbers on screen contradicting each other
/// with no hint why, and angles that window badly in quartz looking fine in the
/// render.
///
/// The rule, chosen by the owner over silently substituting anything: use the
/// design's own named material when it has one; otherwise the nearest built-in
/// within [`MATERIAL_MATCH_TOLERANCE`] of its actual refractive index; and when
/// nothing is that close, refuse. A `Some(reason)` suspends both tracing and metrics
/// (see `bridge::render_thread::frame_helpers::SuspensionFlags`), and the reason is
/// shown in place of a simulation nobody should trust.
fn traced_material_for(design: &Design) -> (String, Option<String>) {
    if let Some(name) = &design.material.name {
        return (name.clone(), None);
    }
    let n_d = design.effective_refractive_index();
    if let Some((name, _)) = nearest_built_in_material(n_d, MATERIAL_MATCH_TOLERANCE) {
        return (name.to_string(), None);
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

fn refresh_design_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
) -> f64 {
    let design = &state.design;

    let mut ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Custom-catalogue-aware (CAD audit item 53): unlike `effective_refractive_index`,
    // this also resolves a custom material by name before falling back to a built-in
    // or the design's legacy `I` line -- see `ri_source_text` below for the matching
    // "where did this number come from" explanation shown in the inspector.
    let n_d = design.effective_refractive_index_with(&ctx.custom_materials);
    ui.global::<EditorModel>()
        .set_ri_source_text(ri_source_text(&design.material, &ctx.custom_materials).into());
    // Custom materials can change any time, independently of `design` -- this
    // option LIST is always refreshed; only the in-out `*_index`/`*_text`
    // selections below are gated on `delta`.
    let options = design_material_options(&ctx.custom_materials);
    ui.global::<EditorModel>()
        .set_material_combo_options(ModelRc::new(VecModel::from(
            options
                .iter()
                .cloned()
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
        // CAD audit item 135: whatever the last typed-but-not-yet-applied preview
        // said is no longer meaningful once the fields are freshly reseeded from
        // the real (just-applied, or freshly loaded) design -- a value that
        // matches the live design has nothing left to preview.
        ui.global::<EditorModel>()
            .set_symmetry_preview_text("".into());
    }

    // Viewport link -- see this function's own doc comment for why there is no
    // `render_view_tab` gate any more (CAD audit item 148).
    if ui.global::<ViewportModel>().get_viewport_material_linked() {
        let (name, unresolved) = traced_material_for(design);
        if ctx.material_unresolved != unresolved {
            ctx.material_unresolved.clone_from(&unresolved);
            ctx.dirty = true;
        }
        // The resolved material itself, not merely its name (CAD audit item 61):
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
        // CAD audit item 140: keep the Render Material dropdown's own displayed
        // selection in sync with the material actually being traced -- this used
        // to be written only once, at startup (`startup_settings::
        // apply_saved_settings`), so the dropdown could go on showing e.g.
        // "Diamond" long after `ctx.material_name` (and so the trace, and the
        // tilt dialog's staleness check, both of which read `ViewportModel.
        // selected_material_index`/`material_options`) had moved to something
        // else entirely.
        let options = ui.global::<ViewportModel>().get_material_options();
        if let Some(idx) = crate::gui::startup_settings::find_option_index(&options, &name) {
            ui.global::<ViewportModel>()
                .set_selected_material_index(idx);
        }
        // CAD audit item 146: the design's own real girdle diameter, under the
        // same link gate as the material above -- previously nothing under
        // `gui::editor` ever wrote `ctx.stone_width_mm` at all, so
        // `context::apply_material_overrides` always skipped absorption-path
        // scaling (treated every design as if it had no physical size), and the
        // Yield panel's millimetre carat estimate had no counterpart in the
        // render's own colour depth.
        let stone_width_mm = design.girdle_diameter_mm.unwrap_or(0.0) as f32;
        if (ctx.stone_width_mm - stone_width_mm).abs() > f32::EPSILON {
            ctx.stone_width_mm = stone_width_mm;
            ctx.dirty = true;
        }
    }
    n_d
}

/// Builds the chip row for whichever tier the inspector's Tier tab currently has
/// loaded (`EditorModel.selected_tier_index`; `None` for "Add Tier" or an
/// out-of-range value) -- CAD audit item 45's per-facet editing surface. Computed
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

/// Pushes [`selected_tier_chips`]'s result into `EditorModel.selected_tier_chips`
/// -- shared by [`refresh_editor_panel`]/[`push_stale_content`] so the chip row
/// stays current after both a real solve and a stale (no-solve) edit, exactly
/// like `tiers` itself.
///
/// # Handoff
/// These two callers cover every EDIT, but not a plain selection change with no
/// edit (a row click, or a viewport facet click) -- neither reaches this module
/// at all today. `callbacks::tier_actions::setup_solid_selected_tier_changed_callback`
/// (a different lane's file) already runs on exactly that event (see its own doc
/// comment: "whenever the tier list's selection changes"); it needs one more
/// call, `view::push_selected_tier_chips(&ui, &st)`, alongside its existing
/// `submit_preview_replan`.
pub(super) fn push_selected_tier_chips(ui: &MainWindow, state: &EditorState) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let chips = selected_tier_chips(&state.design, selected);
    ui.global::<EditorModel>()
        .set_selected_tier_chips(ModelRc::new(VecModel::from(chips)));
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
fn refresh_optimize_availability(ui: &MainWindow, state: &EditorState) {
    let (available, hint) = optimize_hint(state);
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
pub(super) fn refresh_editor_panel_stale(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    push_stale_content(ui, render_ctx, state);
    auto_solve::on_edit(ui, render_ctx, state);
}

/// The actual "no-solve" content push -- see [`refresh_editor_panel_stale`]'s doc
/// comment for why this is split out.
fn push_stale_content(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    push_trace_staleness(ui, render_ctx, state);
    let delta = state.record_scratch_push();
    let n_d = refresh_design_settings(ui, render_ctx, state, &delta);
    let tiers = tier_items_stale(&state.design, n_d);
    push_tier_list_and_undo_redo(ui, state, tiers);

    ui.global::<EditorModel>().set_status_text(
        "Not solved -- click Solve to compute masts and validate this design.".into(),
    );
    ui.global::<EditorModel>().set_status_is_problem(true);
    ui.global::<EditorModel>().set_solve_state("stale".into());

    // A stale solve's MESH-based manufacturability findings would no longer
    // describe the current (edited, unsolved) design, so those are not shown --
    // but the two mast-free checks (gear quantization, cut order) need no mast
    // at all and stay real and actionable even here, tagged "(pre-solve)" so
    // they are never mistaken for a completed pass (CAD audit item 64, fixing
    // the same drop item 75 fixes for the `MissingAnchor` case specifically).
    let warnings: Vec<SharedString> = manufacturability_warnings_tagged(&state.design, None)
        .into_iter()
        .map(|(_, text)| SharedString::from(text))
        .collect();
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(warnings)));

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
        // Same "cleared, not left showing a superseded result" reasoning as the
        // read-only figures below -- `preform_mm_texts` needs a real solve this
        // function deliberately never does.
        ui.global::<EditorModel>()
            .set_preform_half_width_mm_text("".into());
        ui.global::<EditorModel>()
            .set_preform_depth_mm_text("".into());
    }

    // The form fields still get seeded from the design's current state (an edit may
    // have just changed the girdle diameter/material), but only when that value
    // actually changed (`delta`), and the read-only figures are cleared, not left
    // showing a superseded result: `yield_report` needs a solve this function
    // deliberately never does.
    push_yield_material_scratch(ui, state, &delta);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text("".into());
    ui.global::<EditorModel>().set_carat_weight_text("".into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text("".into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning("".into());

    // Proportions/cutting-schedule both need a real solve -- cleared here, like
    // the yield figures above, and repopulated once `refresh_editor_panel` next
    // runs (the explicit "Solve" action, or New/Load).
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
        .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));

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
/// the bench -- CAD audit item 59.
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
/// same "Solve" click (CAD audit item 111) -- `None` only for a design that does
/// not currently solve at all (a `MissingAnchor`, most commonly), in which case
/// this falls back to the plain, internally-solving forms exactly like before.
/// Passing it through here removes what used to be a SECOND independent
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
    // CAD audit item 58: the editor always wins a claim, so this cannot fail --
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
    // #117 (remaining half): letterbox the request to the traced image's own
    // rectangle in Path-traced/Both, exactly like `camera_lighting::
    // resubmit_at_current_pose` already does for a camera drag/zoom/view-mode
    // switch -- see `contained_request_size`'s own doc comment's HANDOFF note
    // naming this call site.
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

    // CAD audit item 141: the editor just claimed the shared plane slot for
    // `state.design` -- whatever material name a PREVIOUS occupant (a catalogue
    // preview load, the only writer of this field) left in `cached_curve_material`
    // no longer describes anything, so it must stop being compared against
    // `ctx.material_name` for tilt-dialog staleness. Clearing it here (rather than
    // leaving it to whatever the next catalogue load happens to overwrite it with)
    // means "no cached artefact for this design" is representable instead of an
    // unrelated design's material silently standing in for it.
    ui.global::<TiltModel>()
        .set_cached_curve_material("".into());
    // CAD audit item 147: geometry just changed under the tilt dialog -- if it's
    // open, its four curves and summary badges are about to describe stale
    // geometry as settled results unless a fresh sweep is requested.
    // `AxesCacheKey` (tilt_profile.rs) already hashes the planes, so this is a
    // no-op resweep whenever nothing about the planes actually moved.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }

    // This is the SAME real design solve (`New`/`Load Selected`/the explicit
    // "Solve" action) `refresh_editor_panel` already computed (CAD audit item
    // 111 -- this used to independently re-solve a second time just to populate
    // this cache). Stashing it here is what lets the NEXT small edit's
    // `submit_preview_replan` call use a real `resolve_dirty` subgraph solve
    // rather than falling back to another full solve.
    if let Some(solved) = solved {
        *solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(solved.to_vec());
    }
    // After the guard above is dropped: the planes the tracer holds were just
    // re-stamped with this generation, so the item-59 marker clears here.
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
/// stash_current_design` (`cad_todo.md` #73) -- see [`push_solved_preview`]/
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
    let last_solved = if force_full_solve {
        None
    } else {
        solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    };
    let (camera, render_size) = {
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
        )
    };
    let selected_tier = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok();
    let n_d = state.design.effective_refractive_index();
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    // #117 (remaining half): see `refresh_viewport`'s identical call, just above,
    // for why -- this is the post-edit path `contained_request_size`'s own doc
    // comment names as still needing the same treatment.
    let size = contained_request_size(view_mode, scaled_viewport_size(ui), render_size);
    let show_preform = ui.global::<SolidPreviewModel>().get_show_preform_planes();
    let enlarged_panel = ui
        .global::<SolidPreviewModel>()
        .get_diagram_enlarged_panel();
    let generation = state.generation.load(AtomicOrdering::Relaxed);
    // `cad_todo.md` #73: stashes this SAME design (a second cheap clone,
    // alongside the one going into `ReplanRequest::design` below) keyed by
    // `generation`, so `auto_solve::take_matching_design` can hand it back to
    // `editor::apply_matching_preview_frame` once a solid-preview frame lands
    // claiming this exact generation -- see that function's own doc comment
    // for why this cannot instead be read straight off `state` from there
    // (`gui::SlintSolidSink::apply` runs off the UI thread it hops back onto,
    // `Send`-bound, and can never reach this `Rc<RefCell<EditorState>>`).
    auto_solve::stash_current_design(
        generation,
        state.design.clone(),
        state.multi_selected.clone(),
    );
    // Item 211: read here rather than cached on `SolidPreviewState` alone, so the
    // slider and the redraw can never disagree about how much of the schedule is
    // being shown. `-1` (the default) means the whole design.
    let cutoff = ui.global::<SolidPreviewModel>().get_tier_cutoff();
    preview_state.set_tier_cutoff(usize::try_from(cutoff).ok());
    preview_state.request_replan(ReplanRequest {
        design: state.design.clone(),
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

/// `cad_todo.md` #73: pushes the tier table's rows, the validation banner, the
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
/// mentioned by `cad_todo.md` #73's mechanism -- only the four fields that
/// [`auto_solve::dispatch_background_solve`]'s OWN completion would otherwise have
/// been the sole source of.
///
/// Called from `gui::SlintSolidSink::apply` -- a solid-preview WORKER-thread
/// callback hopped onto the UI thread via `slint::Weak::upgrade_in_event_loop` --
/// through `editor::apply_matching_preview_frame`'s thin forwarding wrapper, the
/// one bridge this group exposes beyond [`super::setup_editor_callbacks`] itself
/// (see that module's own doc comment, "Module split").
pub(super) fn push_solved_preview(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
) {
    let n_d = design.effective_refractive_index();
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

    let warnings: Vec<SharedString> = manufacturability_warning_lines_from_solved(design, solved)
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(warnings)));

    let (vol_yield_text, carat_text, sg_used_text, fit_text) =
        yield_report_texts_from_solved(design, solved);
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
pub(super) fn refresh_all(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
) {
    // A fresh call to `refresh_all` means a real solve is about to happen (either
    // synchronously below, or via the background dispatch) against whichever design
    // `state` currently holds -- any PREVIOUS design's measured solve time has
    // nothing to say about this one's cost, so this is cleared unconditionally. The
    // synchronous branch below overwrites it again immediately with a real
    // measurement; the background branch leaves it `None` until that dispatch
    // completes, which `auto_solve::should_schedule_auto_solve`'s doc comment
    // already treats as "try auto-solve," a reasonable default right after a solve
    // this function itself just triggered.
    auto_solve::reset_for_new_design();
    // CAD audit item 113: plane count, not tier count. Tier count does not drive
    // solve cost -- a wide-orbit tier emits many planes at once, so a small schedule
    // can still be an expensive, UI-blocking solve (the corpus's worst case is 103
    // tiers but 210 planes). The index count per tier is the cheap estimate of that,
    // available without solving.
    //
    // `None` for the measurement deliberately: `reset_for_new_design` immediately
    // above has just cleared it, and reaching back for the value it cleared would
    // judge a freshly loaded design by the previous one's solve time -- exactly the
    // case where the two have nothing to do with each other, and the one where
    // guessing wrong blocks the UI thread.
    let plane_estimate: usize = state.design.tiers.iter().map(|t| t.indices.len()).sum();
    if auto_solve::should_solve_synchronously(plane_estimate, None) {
        // CAD audit item 113 (remaining half): bracket the synchronous solve with
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
        // several call sites (outside this lane's files) would need auditing
        // against, left undone here. What this DOES fix: `solve_state` is
        // "solving" for the whole duration of this call rather than whatever it
        // was left at by the PREVIOUS refresh, so a reentrant read of it (were one
        // ever added) could not mistake this design for already-idle mid-solve.
        ui.global::<EditorModel>().set_solve_running(true);
        ui.global::<EditorModel>().set_solve_state("solving".into());
        // CAD audit item 238: timed around the real `Design::solve()` call alone,
        // not `refresh_editor_panel`'s UI-model pushes -- inflating the measured
        // duration with that work would make it incomparable with
        // `auto_solve::dispatch_background_solve`'s own timer (the async path's
        // baseline `should_schedule_auto_solve` compares this measurement
        // against), so a design near the budget threshold could flip auto-solve on
        // or off depending only on which path solved it, not on how expensive
        // solving actually is.
        //
        // CAD audit item 111 (the timing half): this used to be a second,
        // thrown-away `state.design.solve()` purely to measure elapsed time,
        // immediately followed by `refresh_editor_panel` solving the SAME design
        // again internally to build the panel fields -- two real solves for one
        // "Solve" click. `refresh_editor_panel_from_solve` below takes this exact
        // result instead of re-solving, so this is now the only solve on this
        // path.
        let start = Instant::now();
        let solved_result = state.design.solve();
        let elapsed = start.elapsed();
        auto_solve::record_solve_duration(elapsed);
        // Same measurement `record_solve_duration` already takes, now also shown
        // (CAD audit item 151) -- captured once rather than re-read, so the figure
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
        push_stale_content(ui, render_ctx, state);
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
///   **disabled** (CAD audit item 239 -- a run against an all-pinned design cannot
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
        // CAD audit item 239: an all-`ScaleReference` design cannot be repaired --
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
/// behind Deep Solve's aggregate verdict (CAD audit item 67's display half),
/// additional to (never replacing) [`format_deep_solve_report`]'s status-line
/// summary and [`super::deep_solve::format_tier_mast_deltas`]'s own one-line
/// suffix. `deltas` is expected to already be [`super::deep_solve::
/// tier_mast_deltas`]'s output -- already filtered to only the tiers whose mast
/// actually moved.
///
/// # Handoff
/// Nothing calls this yet -- see `EditorModel.deep_solve_tier_rows`'s own doc
/// comment (`ui/models/editor.slint`) for the exact one-line wiring
/// `callbacks::solve_actions::setup_deep_solve_callback` still needs.
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
/// [`optimize_result_rows`]'s four aggregate component rows (CAD audit item 68's
/// display half), so a cutter can see WHICH tiers move, and by how much, before
/// clicking Apply.
///
/// # Handoff
/// Nothing calls this yet -- see `EditorModel.optimize_change_rows`'s own doc
/// comment (`ui/models/editor.slint`) for the exact wiring
/// `callbacks::solve_actions` still needs, at each of its three
/// `set_optimize_result_rows` call sites.
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
/// still lacks (CAD audit item 151's one remaining piece: the state dot, tier
/// count and last-solve duration are already live there). One entry in
/// [`Design::planes_from_solved`]'s own output IS one facet, so this is just its
/// length -- no new geometry computation, only a name for a count that already
/// exists.
///
/// # Handoff
/// Nothing calls this yet: it needs an `EditorModel.facet_count: int` property
/// (`ui/models/editor.slint`, not this lane's file) pushed from
/// [`refresh_editor_panel_from_solve`] and [`auto_solve::dispatch_background_solve`]
/// alongside `tiers`/`last_solve_duration_ms`, and a segment in
/// `EditorStatusStrip` (this lane's file) reading it next to the existing tier
/// count once the property exists.
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
fn optimize_hint(state: &EditorState) -> (bool, String) {
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
                 extinction, and tilt brilliance. Roughly 7 ms per evaluation on a \
                 small design, but up to several seconds each on a large, heavily \
                 meet-derived one (a 200-evaluation budget can then take minutes) -- \
                 plus two fixed full-fidelity scorings (one before, one after the \
                 search) that can each take over a second on their own, so even a \
                 fast run has some up-front and trailing wait beyond the quoted \
                 per-evaluation cost. Runs off the UI thread and can be cancelled.",
                free.len()
            ),
        )
    }
}

/// Formats one objective component's "after" cell as the raw value plus a signed
/// delta and a plain-English verdict (CAD audit item 237) -- "9.25% (-3.25%,
/// better)" rather than a bare number the cutter has to subtract by hand and
/// remember the polarity of. `higher_is_better` distinguishes tilt brilliance
/// (higher is better) from every other component/the blended score (lower is
/// better, see [`ObjectiveWeights::score`]'s own doc comment).
///
/// # Handoff
/// `OptimizeResultRow` (`ui/types.slint`, not this pass's file) has only
/// `label`/`before`/`after` -- the delta/verdict below is folded into `after`'s
/// own string rather than added as new `delta`/`improved: bool` fields (and
/// coloured emerald/ruby per row, `ui/components/editor_inspector.slint`, also
/// not this pass's file) the way `cad_todo.md` item 237 originally asked. Adding
/// those two fields plus the row colouring is still open.
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

/// Builds the four rows `EditorView`'s Optimize result table needs from a completed
/// or cancelled run's [`OptimizeOutcome`] -- windowing, extinction, and tilt
/// brilliance each get their OWN row, and the blended score comes last as a fourth,
/// clearly-separate row: an optimizer that improved windowing by wrecking extinction
/// must be visibly doing that, never collapsed into a single figure. Each row's
/// `after` cell also names its own signed delta and direction (CAD audit item 237)
/// via [`after_with_delta`], so a cutter reads which metric moved and by how much
/// without doing the subtraction (or remembering which way is good) themselves.
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
/// the final score it is responsible for (CAD audit item 237 -- previously
/// invisible: `polish_evaluations`/`polish_improvement` had no reader anywhere in
/// this crate). The per-component before/after numbers themselves live only in the
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

/// Parses the Optimize weight form's three text fields into an [`ObjectiveWeights`]
/// -- must parse and be finite, but a weight also rejects negative values: only the
/// RATIOS between the three matter, so a negative one would silently invert that
/// component's polarity (rewarding more windowing, say) rather than merely
/// weighting it oddly.
pub(super) fn parse_optimize_weights(
    windowing: &str,
    extinction: &str,
    tilt_brilliance: &str,
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
        // the verdict either way, so the button must be DISABLED (CAD audit item
        // 239), with an explanatory hint rather than reading as broken or missing.
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

        let (available, hint) = optimize_hint(&state);
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

        let (available, hint) = optimize_hint(&state);
        assert!(available);
        assert!(
            hint.contains('1'),
            "expected the free-tier count in: {hint}"
        );
        assert!(hint.to_lowercase().contains("cancel"));
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
            after: ObjectiveComponents {
                windowing_pct: 9.25,
                extinction_pct: 11.0,
                tilt_brilliance_pct: 65.0,
            },
            after_score: 15.0,
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
        assert_eq!(rows.len(), 4);
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
        assert_eq!(rows[3].label.as_str(), "Blended score");
        assert_eq!(rows[3].before.as_str(), "20.00");
        assert_eq!(rows[3].after.as_str(), "15.00 (-5.00, better)");
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
        // CAD audit item 237: `polish_evaluations`/`polish_improvement` had no
        // reader anywhere in this crate before this -- whether the ridge-following
        // polish stage did anything at all was invisible to a cutter.
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
        let weights = parse_optimize_weights("1.0", "2.5", "0").unwrap();
        assert_eq!(weights.windowing, 1.0);
        assert_eq!(weights.extinction, 2.5);
        assert_eq!(weights.tilt_brilliance, 0.0);
    }

    #[test]
    fn parse_optimize_weights_rejects_a_non_numeric_field() {
        let err = parse_optimize_weights("not-a-number", "1.0", "1.0").unwrap_err();
        assert!(err.contains("Windowing"));
    }

    #[test]
    fn parse_optimize_weights_rejects_a_negative_weight() {
        // A negative weight is not merely out of range -- it would invert that
        // component's polarity -- so this is checked separately from finiteness.
        let err = parse_optimize_weights("1.0", "-0.5", "1.0").unwrap_err();
        assert!(err.contains("Extinction"));
    }

    #[test]
    fn parse_optimize_weights_rejects_non_finite_values() {
        assert!(parse_optimize_weights("NaN", "1.0", "1.0").is_err());
        assert!(parse_optimize_weights("1.0", "inf", "1.0").is_err());
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

    // --- selected_tier_chips (CAD audit item 45) ---

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

    // --- deep_solve_tier_rows (CAD audit item 67's display half) ---

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

    // --- optimize_change_rows (CAD audit item 68's display half) ---

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
            after: ObjectiveComponents {
                windowing_pct: 0.0,
                extinction_pct: 0.0,
                tilt_brilliance_pct: 0.0,
            },
            after_score: 0.0,
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

    // --- facet_count_from_solved (CAD audit item 151's remaining piece) ---

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
}
