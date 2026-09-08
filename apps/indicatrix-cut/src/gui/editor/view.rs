//! Pushing [`EditorState`](super::state::EditorState) into `EditorView`/the shared
//! viewport ([`refresh_editor_panel`]/[`refresh_viewport`]/[`refresh_all`]/
//! [`refresh_editor_panel_stale`]), and the Deep Solve/Optimize hint and result-
//! formatting helpers. See this group's own `mod.rs` doc comment for the "Solve on
//! explicit action, not on every edit" reasoning [`refresh_editor_panel_stale`] exists
//! to honour.

use super::{
    auto_solve,
    state::{
        EditorState, apply_multi_selection, design_material_index_from_name,
        design_material_options, design_to_gpu_planes, gear_index_from_teeth,
        manufacturability_warning_lines, material_index_from_name, status_text_and_is_problem,
        tier_items, tier_items_stale, yield_report_texts,
    },
};
use crate::{
    EditorModel, MainWindow, OptimizeResultRow, SolidPreviewModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::{CameraPose, ReplanRequest, SolidPreviewState},
};
// Re-exported so sibling `callbacks::*` modules can spell this `view::SolidLastSolved`
// (matching how they already reach every other `view::*` helper) rather than reaching
// past this module into `solid_preview::preview_state` directly.
pub(in crate::gui::editor) use crate::gui::solid_preview::preview_state::SolidLastSolved;
use indicatrix::geometry::meet_solver::{MeetConstraint, VerifiedSolveReport};
use indicatrix_cut_core::{
    ObjectiveWeights, OptimizeOutcome, PreformShape, critical_angle_deg, free_tier_indices,
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
/// Calls [`Design::solve`] (via [`tier_items`]/[`status_text_and_is_problem`]) --
/// only used by [`refresh_all`] (New/Load/the explicit "Solve" action). Every other
/// edit callback uses [`refresh_editor_panel_stale`] instead, which updates the same
/// fields except the ones that require a solve.
pub(super) fn refresh_editor_panel(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    let n_d = refresh_design_settings(ui, render_ctx, state);
    let mut tiers = tier_items(&state.design, n_d);
    apply_multi_selection(&mut tiers, &state.multi_selected);
    ui.global::<EditorModel>()
        .set_tiers(ModelRc::new(VecModel::from(tiers)));
    ui.global::<EditorModel>()
        .set_can_undo(state.history.can_undo());
    ui.global::<EditorModel>()
        .set_can_redo(state.history.can_redo());

    let (status_text, is_problem) = status_text_and_is_problem(&state.design);
    ui.global::<EditorModel>()
        .set_status_text(status_text.into());
    ui.global::<EditorModel>().set_status_is_problem(is_problem);

    // Manufacturability warnings against this same "Solve" click's design state.
    let warnings: Vec<SharedString> = manufacturability_warning_lines(&state.design)
        .into_iter()
        .map(SharedString::from)
        .collect();
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(warnings)));

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

    // Seed the Yield form's scratch buffers from the design's current state, then
    // push the read-only figures this same "Solve" click's state produces.
    ui.global::<EditorModel>().set_girdle_diameter_mm(
        state
            .design
            .girdle_diameter_mm
            .map_or_else(String::new, |mm| format!("{mm:.4}"))
            .into(),
    );
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
    let (vol_yield_text, carat_text, sg_used_text, fit_text) = yield_report_texts(&state.design);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());

    refresh_deep_solve_availability(ui, state);
    refresh_optimize_availability(ui, state);
}

/// Pushes the design settings panel's state (material combo options and index,
/// RI-override/effective-RI/critical-angle readouts, gear/symmetry/mirror) and,
/// while the Edit sub-tab is shown AND "linked to design" is on, syncs the shared
/// viewport's render material to match -- a display override left on some OTHER
/// sub-tab must never be silently overridden. Shared by [`refresh_editor_panel`]/
/// [`refresh_editor_panel_stale`].
///
/// Returns this design's effective refractive index so [`tier_items`]/
/// [`tier_items_stale`] can reuse the identical value for their per-tier
/// margin/risk column rather than re-deriving it.
fn refresh_design_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) -> f64 {
    let design = &state.design;
    let n_d = design.effective_refractive_index();

    let mut ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let options = design_material_options(&ctx.custom_materials);
    ui.global::<EditorModel>()
        .set_material_combo_index(design_material_index_from_name(
            design.material.name.as_deref(),
            &options,
        ));
    ui.global::<EditorModel>()
        .set_material_combo_options(ModelRc::new(VecModel::from(
            options
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<EditorModel>().set_ri_override_text(
        design
            .material
            .refractive_index_override
            .map_or_else(String::new, |v| format!("{v:.4}"))
            .into(),
    );
    ui.global::<EditorModel>()
        .set_effective_ri_text(format!("{n_d:.4}").into());
    ui.global::<EditorModel>()
        .set_critical_angle_text(format!("{:.2}\u{b0}", critical_angle_deg(n_d)).into());
    ui.global::<EditorModel>()
        .set_gear_index(gear_index_from_teeth(design.meta.gear_teeth));
    ui.global::<EditorModel>()
        .set_gear_custom_text(design.meta.gear_teeth.to_string().into());
    ui.global::<EditorModel>()
        .set_symmetry_order_text(design.meta.symmetry_order.to_string().into());
    ui.global::<EditorModel>().set_mirror(design.meta.mirror);

    // Viewport link -- see this function's doc comment for the `render_view_tab == 1` gate.
    if ui.get_render_view_tab() == 1 && ui.global::<ViewportModel>().get_viewport_material_linked()
    {
        let name = design
            .material
            .name
            .clone()
            .unwrap_or_else(|| "Diamond".to_string());
        if ctx.material_name != name {
            ctx.material_name = name;
            ctx.dirty = true;
        }
    }
    n_d
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
    let n_d = refresh_design_settings(ui, render_ctx, state);
    let mut tiers = tier_items_stale(&state.design, n_d);
    apply_multi_selection(&mut tiers, &state.multi_selected);
    ui.global::<EditorModel>()
        .set_tiers(ModelRc::new(VecModel::from(tiers)));
    ui.global::<EditorModel>()
        .set_can_undo(state.history.can_undo());
    ui.global::<EditorModel>()
        .set_can_redo(state.history.can_redo());

    ui.global::<EditorModel>().set_status_text(
        "Not solved -- click Solve to compute masts and validate this design.".into(),
    );
    ui.global::<EditorModel>().set_status_is_problem(true);

    // A stale solve's manufacturability findings would no longer describe the
    // current (edited, unsolved) design -- cleared, not left showing a superseded
    // result. Re-populated next time `refresh_editor_panel` runs.
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));

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

    // The form fields still get seeded from the design's current state (an edit may
    // have just changed the girdle diameter/material), but the read-only figures are
    // cleared, not left showing a superseded result: `yield_report` needs a solve
    // this function deliberately never does.
    ui.global::<EditorModel>().set_girdle_diameter_mm(
        state
            .design
            .girdle_diameter_mm
            .map_or_else(String::new, |mm| format!("{mm:.4}"))
            .into(),
    );
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
    ui.global::<EditorModel>()
        .set_volumetric_yield_text("".into());
    ui.global::<EditorModel>().set_carat_weight_text("".into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text("".into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning("".into());

    refresh_deep_solve_availability(ui, state);
    refresh_optimize_availability(ui, state);
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
fn refresh_viewport(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
) {
    let mut ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ctx.active_planes = std::sync::Arc::new(design_to_gpu_planes(&state.design));
    ctx.dirty = true;
    ctx.design_gear = Some((
        state.design.meta.gear_teeth_abs(),
        state.design.meta.gear_reference_angle as f32,
    ));
    let design_gear = ctx.design_gear;
    let planes: Vec<(glam::Vec3, f32)> = ctx
        .active_planes
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let size = (
        ui.global::<SolidPreviewModel>().get_viewport_width() as u32,
        ui.global::<SolidPreviewModel>().get_viewport_height() as u32,
    );
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
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
    // This is a real design solve (`New`/`Load Selected`/the explicit "Solve"
    // action), already computed multiple times over by `refresh_editor_panel`.
    // Stashing it here is what lets the NEXT small edit's `submit_preview_replan`
    // call use a real `resolve_dirty` subgraph solve rather than falling back to
    // another full solve.
    if let Ok(solved) = state.design.solve() {
        *solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(solved);
    }
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
    let camera = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        CameraPose {
            yaw: ctx.yaw,
            pitch: ctx.pitch,
            distance: ctx.distance,
        }
    };
    let size = (
        ui.global::<SolidPreviewModel>().get_viewport_width() as u32,
        ui.global::<SolidPreviewModel>().get_viewport_height() as u32,
    );
    let selected_tier = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok();
    let n_d = state.design.effective_refractive_index();
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    preview_state.request_replan(ReplanRequest {
        design: state.design.clone(),
        dirty,
        last_solved,
        camera,
        size,
        selected_tier,
        n_d,
        view_mode,
    });
}

/// [`refresh_editor_panel`] + [`refresh_viewport`] together -- pushes a real solve's
/// result into both the panel and the shared viewport. Only called by `New`, `Load
/// Selected`, "Adopt", and the explicit "Solve" action; every other edit callback
/// calls [`refresh_editor_panel_stale`] instead and leaves the viewport untouched.
///
/// # Synchronous only under [`auto_solve::SYNC_SOLVE_TIER_LIMIT`] tiers
///
/// `Design::solve`'s refinement sweep is cubic in plane count (a real 103-tier/210-
/// plane design: 5.9s -- see `mod.rs`'s "Never block the UI thread with a solve"
/// section), so this only solves inline on the UI thread for a design at or under
/// [`auto_solve::SYNC_SOLVE_TIER_LIMIT`] tiers -- comfortably fast in practice, and
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
    if state.design.tiers.len() <= auto_solve::SYNC_SOLVE_TIER_LIMIT {
        let start = Instant::now();
        refresh_editor_panel(ui, render_ctx, state);
        refresh_viewport(ui, render_ctx, preview_state, solid_last_solved, state);
        auto_solve::record_solve_duration(start.elapsed());
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
///   enabled, hinted that there's nothing to repair yet -- the user must first
///   convert a tier to a meet constraint (the tier list's "Adopt" action) before
///   Deep Solve has anything to search over.
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
        (
            true,
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
                 runs off the UI thread and can be cancelled.",
                free.len()
            ),
        )
    }
}

/// Builds the four rows `EditorView`'s Optimize result table needs from a completed
/// or cancelled run's [`OptimizeOutcome`] -- windowing, extinction, and tilt
/// brilliance each get their OWN row, and the blended score comes last as a fourth,
/// clearly-separate row: an optimizer that improved windowing by wrecking extinction
/// must be visibly doing that, never collapsed into a single figure.
pub(super) fn optimize_result_rows(outcome: &OptimizeOutcome) -> Vec<OptimizeResultRow> {
    vec![
        OptimizeResultRow {
            label: "Windowing".into(),
            before: format!("{:.2}%", outcome.before.windowing_pct).into(),
            after: format!("{:.2}%", outcome.after.windowing_pct).into(),
        },
        OptimizeResultRow {
            label: "Extinction".into(),
            before: format!("{:.2}%", outcome.before.extinction_pct).into(),
            after: format!("{:.2}%", outcome.after.extinction_pct).into(),
        },
        OptimizeResultRow {
            label: "Tilt brilliance".into(),
            before: format!("{:.2}%", outcome.before.tilt_brilliance_pct).into(),
            after: format!("{:.2}%", outcome.after.tilt_brilliance_pct).into(),
        },
        OptimizeResultRow {
            label: "Blended score".into(),
            before: format!("{:.2}", outcome.before_score).into(),
            after: format!("{:.2}", outcome.after_score).into(),
        },
    ]
}

/// The one-line summary shown above [`optimize_result_rows`]'s table -- how many
/// tiers changed and how many candidate evaluations it took, plus (only when true)
/// the cancellation note. The per-component before/after numbers themselves live
/// only in the rows table, never duplicated here.
pub(super) fn optimize_status_text(outcome: &OptimizeOutcome) -> String {
    let cancelled_note = if outcome.cancelled {
        " (cancelled -- showing the best partial result found before the checkpoint \
         fired)"
    } else {
        ""
    };
    if outcome.changes.is_empty() {
        format!(
            "Optimize found no improving move in {} evaluation(s) -- this design's \
             free tiers were already at (or very near) a local optimum for these \
             weights.{cancelled_note}",
            outcome.evaluations
        )
    } else {
        format!(
            "Optimize changed {} tier(s) in {} evaluation(s).{cancelled_note}",
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
    fn deep_solve_hint_is_available_but_says_nothing_to_repair_when_every_tier_is_pinned() {
        // Printed proportions exist, but every tier is pinned to a `ScaleReference`
        // -- correct, but the button must stay enabled with an explanatory hint
        // rather than reading as broken or missing.
        let mut state = EditorState::fresh();
        state.printed_proportions = Some(some_proportions());
        state.design.tiers.push(scale_reference_tier(0.5));
        state.design.tiers.push(scale_reference_tier(0.8));

        let (available, hint) = deep_solve_hint(&state);
        assert!(available);
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
        assert_eq!(rows[0].after.as_str(), "9.25%");
        // Extinction got WORSE -- shown honestly, not hidden by the improved score.
        assert_eq!(rows[1].label.as_str(), "Extinction");
        assert_eq!(rows[1].before.as_str(), "8.00%");
        assert_eq!(rows[1].after.as_str(), "11.00%");
        assert_eq!(rows[2].label.as_str(), "Tilt brilliance");
        assert_eq!(rows[2].before.as_str(), "60.00%");
        assert_eq!(rows[2].after.as_str(), "65.00%");
        assert_eq!(rows[3].label.as_str(), "Blended score");
        assert_eq!(rows[3].before.as_str(), "20.00");
        assert_eq!(rows[3].after.as_str(), "15.00");
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
}
