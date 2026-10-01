//! Applying a completed background solve's result back onto the UI: the two
//! staleness checks, the panel/proportions/schedule pushes, the plane-cap toast, and
//! re-dispatching whatever request queued up behind the just-finished solve.

use super::{
    dispatch::is_current,
    runtime::{PendingDispatch, RUNTIME},
};
use crate::{
    AngleItem, EditorModel, EditorTierItem, MainWindow, SolidPreviewModel, TiltModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor::{
            state::{
                cutting_instructions_rows, girdle_and_ratio_texts, preform_mm_texts,
                preform_y_offset_mm_text, proportions_texts, push_multi_selected_count, push_tiers,
            },
            view::{
                facet_count_from_solved, girdle_and_ratio_texts_from_solved,
                preform_mm_texts_from_solved, proportions_texts_from_solved,
                push_proportion_verdicts_from_solved, scaled_viewport_size,
            },
        },
        render::camera_lighting::contained_request_size,
        show_toast,
        solid_preview::preview_state::CameraPose,
    },
};
use indicatrix::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use indicatrix_cut_core::Design;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Bundles a completed background solve's results -- kept as one struct (rather than
/// threading each value through as its own parameter) purely to keep
/// [`apply_background_solve_result`] under clippy's argument-count lint without a new
/// `#[allow]`.
pub(super) struct BackgroundSolveResult {
    pub(super) elapsed: Duration,
    pub(super) tiers: Vec<EditorTierItem>,
    pub(super) status_text: String,
    pub(super) status_is_problem: bool,
    /// `(tier index, warning text)` pairs, not a flattened
    /// text-only list -- see `super::dispatch::PanelInputs::warnings`'s own doc
    /// comment.
    pub(super) warnings: Vec<(usize, String)>,
    pub(super) yield_texts: (String, String, String, String),
    pub(super) planes: Vec<GpuFacetPlane>,
    pub(super) solved: Option<Vec<SolvedTier>>,
    /// `true` iff `status_text`
    /// above is `super::dispatch::too_many_planes_message`'s plane-cap sentence --
    /// read by [`apply_background_solve_result`] to fire the once-per-design warning
    /// toast. See `super::runtime::Runtime::too_many_planes_toasted`'s own doc
    /// comment.
    pub(super) too_many_planes: bool,
    /// Index-wheel tooth count and reference angle, for the diagram view's wheel.
    pub(super) gear: (u32, f32),
    /// `multi_selected.len()` at dispatch time, for the tier table's "N selected"
    /// header indicator -- read on the UI thread by [`apply_background_solve_result`],
    /// never recomputed from `EditorState` there (a solve completion has no access
    /// to it beyond this snapshot).
    pub(super) multi_selected_count: usize,
    /// The design's effective refractive index, the SAME value `panel_inputs`
    /// computed for the tier table's own critical-angle margin column -- kept
    /// here so [`push_solve_dependent_background_fields`] can judge the
    /// proportion-verdict chips against it too, without a second
    /// `RenderContext` lock just to recompute it.
    pub(super) n_d: f64,
    /// `true` iff this result is [`BackgroundSolveResult::panicked`]'s minimal
    /// placeholder -- every OTHER field above is an empty/zero
    /// placeholder in that case, not a real solve, so
    /// [`apply_background_solve_result`] must report the failure and stop
    /// rather than push any of them over whatever the panel already showed.
    pub(super) panicked: bool,
    /// The SAME `Design` snapshot this worker
    /// solved, moved in here rather than dropped once `panel_inputs`/`solved`
    /// are built -- [`apply_background_solve_result`] needs it to build
    /// proportions/girdle-ratio/preform-mm/facet-count/cutting-instructions the same
    /// way `super::super::view::refresh_editor_panel_from_solve` does, so the
    /// background-solve path builds these too instead of leaving them stale
    /// until the next foreground refresh (see that function's own doc comment).
    /// Free: this `Design` was already cloned into the
    /// worker closure for `Design::solve`; nothing else in this struct needs
    /// it again after construction, so moving it costs nothing further.
    pub(super) design: Design,
}

impl BackgroundSolveResult {
    /// A minimal placeholder for a dispatch `dispatch::solve_and_build_result`
    /// cancelled before finishing `dispatch::panel_inputs` -- built
    /// without paying for any of that function's five-plus solve-derived
    /// helpers. In practice this never reaches the screen at all:
    /// `dispatch::cancel_in_flight_solve` ("Abandon Solve") bumps
    /// `Runtime::current_seq` on the very same click that sets the cancel flag
    /// this result answers, so `apply_background_solve_result`'s own
    /// `is_current(seq)` check discards it before any field below is read for
    /// real -- see [`dispatch::solve_cancellably`]'s own doc comment. These
    /// values exist only so the struct can be built at all on that path.
    pub(super) const fn cancelled(
        design: Design,
        elapsed: Duration,
        multi_selected_count: usize,
    ) -> Self {
        Self {
            elapsed,
            tiers: Vec::new(),
            status_text: String::new(),
            status_is_problem: false,
            warnings: Vec::new(),
            yield_texts: (String::new(), String::new(), String::new(), String::new()),
            planes: Vec::new(),
            solved: None,
            too_many_planes: false,
            gear: (
                design.meta.gear_teeth_abs(),
                design.meta.gear_reference_angle as f32,
            ),
            multi_selected_count,
            n_d: 0.0,
            panicked: false,
            design,
        }
    }

    /// a minimal, TOASTED placeholder for a worker that panicked inside
    /// `dispatch::solve_and_build_result` -- unlike [`Self::cancelled`], this
    /// case is not guaranteed stale (a panic can happen on an otherwise still-
    /// current dispatch), so `apply_background_solve_result` reports it via
    /// `panicked` instead of silently discarding it, while leaving whatever the
    /// panel already showed untouched (every other field here is an empty
    /// placeholder, not a real solve, and must never overwrite good data).
    pub(super) fn panicked(design: Design, elapsed: Duration, multi_selected_count: usize) -> Self {
        Self {
            status_text: "Solve failed unexpectedly (internal error) -- try again or simplify \
                          the design."
                .to_string(),
            status_is_problem: true,
            panicked: true,
            ..Self::cancelled(design, elapsed, multi_selected_count)
        }
    }
}

/// Frees `super::runtime::Runtime::solve_in_flight`/`super::runtime::Runtime::current_cancel`
/// and takes whatever [`PendingDispatch`] queued up behind this completion --
/// pulled out of [`apply_background_solve_result`] purely to keep that function
/// under clippy's function-length lint. Must run BEFORE any staleness check, so a
/// pending request still gets dispatched even when this particular completion
/// turns out to be stale (see [`PendingDispatch`]'s own doc comment for
/// why it must not be dropped in that case).
fn free_in_flight_slot() -> Option<PendingDispatch> {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.solve_in_flight = false;
        // This worker's own cancel flag (if any -- see `Runtime::current_cancel`'s
        // own doc comment) has nothing left to stop; clear it so a later, unrelated
        // "Abandon Solve" click cannot reach back and flip a flag no thread is
        // reading anymore.
        rt.current_cancel = None;
        // Removes this dispatch from
        // the activity list on EVERY completion path (current or stale, cancelled or
        // not) -- see `Runtime::activity_id`'s own doc comment. A click on the
        // activity list's own Cancel button already reset `EditorModel` via
        // `cancel_in_flight_solve_from_activity`; this is what removes the row
        // itself, once the (now near-instantly cancelled, per `solve_cancellably`)
        // worker actually returns.
        if let (Some(activity), Some(id)) = (rt.activity.clone(), rt.activity_id.take()) {
            activity.finish(id);
        }
        rt.pending_dispatch.take()
    })
}

/// Exactly one warning toast
/// for a design stuck over `MAX_PLANES` -- see
/// `super::runtime::Runtime::too_many_planes_toasted`'s own doc comment for why
/// this is a one-shot-until-cleared flag rather than a toast per background solve
/// (the status-strip sentence itself, `result.status_text`, already re-states the
/// problem on every refresh regardless). Split out of
/// [`apply_background_solve_result`] purely to keep that function under
/// clippy's function-length lint.
fn maybe_toast_too_many_planes(ui: &MainWindow, result: &BackgroundSolveResult) {
    let already_toasted = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        let already = rt.too_many_planes_toasted;
        rt.too_many_planes_toasted = result.too_many_planes;
        already
    });
    if result.too_many_planes && !already_toasted {
        show_toast(ui, &result.status_text, "warning");
    }
}

/// Applies a completed background solve's result -- see the parent module doc
/// comment for the two staleness checks this performs before touching anything, and
/// `mod.rs`'s "Feeding the viewport" section for the `GpuFacetPlane` sign convention
/// `refresh_viewport` already documents (this mirrors it exactly, just deferred to a
/// worker-computed `Vec<GpuFacetPlane>` instead of computing it inline).
pub(super) fn apply_background_solve_result(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    seq: u64,
    started_generation: u64,
    generation: &Arc<AtomicU64>,
    result: BackgroundSolveResult,
) {
    let pending = free_in_flight_slot();

    if !is_current(seq) {
        // A newer dispatch has already taken over the banner and will apply its own
        // result in turn -- nothing here is still current enough to show.
        // `record_solve_duration` must not run above this check -- a
        // superseded result's elapsed time is not a measurement of the design that
        // is actually current, and must never set `should_schedule_auto_solve`'s
        // budget baseline.
        dispatch_pending(ui, render_ctx, pending);
        return;
    }
    super::scheduling::record_solve_duration(result.elapsed);
    ui.global::<EditorModel>().set_solve_running(false);
    if generation.load(Ordering::Relaxed) != started_generation {
        // The design changed after this solve was dispatched, without a newer
        // dispatch superseding it (auto-solve disabled, or this design's own budget
        // was already exceeded) -- the edit that changed it already repainted the
        // banner correctly via `refresh_editor_panel_stale`. Only the running flag
        // above needed clearing.
        dispatch_pending(ui, render_ctx, pending);
        return;
    }

    if result.panicked {
        // `result`'s own tiers/planes/etc. are empty placeholders, not a
        // real solve -- pushing them would blank out whatever the panel
        // already showed. Report the failure and stop; `solve_running` above
        // is already cleared, which is all "Abandon Solve"'s own reset does
        // for this case too.
        ui.global::<EditorModel>()
            .set_status_text(result.status_text.clone().into());
        ui.global::<EditorModel>().set_status_is_problem(true);
        ui.global::<EditorModel>().set_solve_state("failed".into());
        show_toast(ui, &result.status_text, "error");
        dispatch_pending(ui, render_ctx, pending);
        return;
    }

    maybe_toast_too_many_planes(ui, &result);

    // Pushed first, before ANY field of `result` is
    // moved out below (a shared `&result` borrow needs every field still in place) --
    // see `push_solve_dependent_background_fields`'s own doc comment for what this
    // closes.
    push_solve_dependent_background_fields(ui, &result);
    push_tiers(ui, result.tiers);
    push_multi_selected_count(ui, result.multi_selected_count);
    ui.global::<EditorModel>()
        .set_status_text(result.status_text.into());
    ui.global::<EditorModel>()
        .set_status_is_problem(result.status_is_problem);
    // Below the staleness early returns above, same as the line it follows: a
    // superseded run must not relabel the strip for a design it no longer describes.
    ui.global::<EditorModel>().set_solve_state(
        if result.status_is_problem {
            "failed"
        } else {
            "solved"
        }
        .into(),
    );
    ui.global::<EditorModel>()
        .set_last_solve_duration_ms(i32::try_from(result.elapsed.as_millis()).unwrap_or(i32::MAX));
    // The solve verdict just changed: the worked-example guide's "Solve and check"
    // step can complete here, with no click (auto-solve got there first).
    // `result.design` is the design this solve ran against, and the generation
    // check above confirms it is still the one on screen.
    crate::gui::editor::guide::check_design_progress(ui, &result.design);
    // `result.warnings` is `(tier index, text)` pairs -- see
    // `PanelInputs::warnings`'s own doc comment.
    let warning_tiers: Vec<i32> = result
        .warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(
            result
                .warnings
                .into_iter()
                .map(|(_, text)| SharedString::from(text))
                .collect::<Vec<_>>(),
        )));
    ui.global::<EditorModel>()
        .set_manufacturability_warning_tiers(ModelRc::new(VecModel::from(warning_tiers)));
    let (vol_yield_text, carat_text, sg_used_text, fit_text) = result.yield_texts;
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());
    // `editor_deep_solve_available`/`editor_optimize_available` (and their hints)
    // depend only on `state.printed_proportions`/free-tier membership, neither of
    // which a solve can change -- and `generation` matching above already confirms
    // no edit touched them since `refresh_editor_panel_stale` last pushed them at
    // edit time. Nothing to refresh here.

    if !push_viewport_after_background_solve(
        ui,
        render_ctx,
        started_generation,
        result.planes,
        result.gear,
        result.solved,
    ) {
        dispatch_pending(ui, render_ctx, pending);
        return;
    }

    dispatch_pending(ui, render_ctx, pending);
}

/// [`apply_background_solve_result`]'s viewport/solid-preview-redraw/tilt-
/// staleness tail -- split out purely to keep that function under clippy's
/// function-length lint. Returns `false` when `started_generation` lost the
/// race to claim `render_ctx`'s shared plane slot (a newer editor state already
/// owns it -- the `is_current(seq)` guard `apply_background_solve_result`
/// already ran should have caught this, so this is only the belt to that
/// braces), in which case the caller must treat this result as stale and skip
/// the rest of the pipeline rather than push a redraw/cache write for it.
fn push_viewport_after_background_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    started_generation: u64,
    planes: Vec<GpuFacetPlane>,
    gear: (u32, f32),
    solved: Option<Vec<SolvedTier>>,
) -> bool {
    let planes = Arc::new(planes);
    let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
    // `started_generation` rather than the live counter: a
    // background solve that finished against an older design must not out-rank the
    // claim a newer edit already made, which is exactly what `may_claim_active_planes`
    // compares.
    if !ctx.claim_active_planes(
        Arc::clone(&planes),
        Some(gear),
        PlanesOwner::Editor {
            generation: started_generation,
        },
    ) {
        return false;
    }
    ctx.dirty = true;
    let design_gear = ctx.design_gear;
    let camera = CameraPose {
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
    };
    let render_size = (ctx.width, ctx.height);
    drop(ctx);
    let redraw_planes: Vec<(glam::Vec3, f32)> = planes
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    // This is the "immediately after the next edit or Solve" call site that
    // `camera_lighting::contained_request_size`'s own doc comment names as still
    // reproducing the picking misregistration if it were skipped: without it, a
    // completed background solve would request the redraw at the viewport's own
    // raw size instead of the Path-traced/Both letterboxed rectangle
    // `resubmit_at_current_pose`/`refresh_viewport` already correct for, so the
    // mismatch a camera drag/zoom/view-mode switch already corrects for would
    // reappear the moment a background Solve completes -- the common path, not
    // an edge case.
    let size = contained_request_size(view_mode, scaled_viewport_size(ui), render_size);
    let (preview_state, solid_last_solved) = RUNTIME.with(|cell| {
        let rt = cell.borrow();
        (rt.preview_state.clone(), rt.solid_last_solved.clone())
    });
    if let Some(preview_state) = preview_state {
        preview_state.request_redraw_with_gear(redraw_planes, camera, size, view_mode, design_gear);
    }
    if let (Some(solved), Some(cache)) = (solved, solid_last_solved) {
        // stamped with `started_generation`, not the live counter --
        // this completion already confirmed `generation.load(..) ==
        // started_generation` above, so `started_generation` names exactly
        // the design this solve ran against.
        *cache.lock().unwrap_or_else(PoisonError::into_inner) = Some((started_generation, solved));
    }

    // This background solve just replaced the shared plane
    // slot's contents for THIS design -- see `view::refresh_viewport`'s identical
    // comment for why any material name a PREVIOUS occupant left in
    // `cached_curve_material` must stop being compared against
    // `ctx.material_name` for tilt-dialog staleness the moment that happens.
    ui.global::<TiltModel>()
        .set_cached_curve_material("".into());
    // Geometry just changed under the tilt dialog -- if it's
    // open, its four curves and summary badges are about to describe the
    // pre-solve stone as settled results unless a fresh sweep is requested.
    // `AxesCacheKey` (tilt_profile.rs) already hashes the planes, so this is a
    // no-op resweep whenever nothing about the planes actually moved.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }
    true
}

/// The proportions/girdle-ratio/preform-mm/facet-count/cutting-instructions push half
/// of [`apply_background_solve_result`] -- split out purely to keep that function
/// under clippy's function-length lint, the same reasoning `view::
/// push_solve_dependent_panel_fields` documents for itself.
///
/// These are the fields every OTHER
/// solve-completion path pushes -- proportions, girdle/ratio texts, preform mm
/// readouts, facet count, the cutting instructions. The standard round brilliant template alone
/// sums ~72 plane indices, over the 32-plane limit of
/// `indicatrix_editor::solve_policy::should_solve_synchronously_for` (which also
/// requires few meet-derived tiers, no tier targets and a fast last solve), so
/// essentially every real design takes THIS path (background solve) rather than
/// `refresh_editor_panel_from_solve`'s synchronous one; without this function, the
/// Proportions section would read "-", the Cutting Instructions tab would stay empty, preform mm
/// readouts would stay blank, and the status strip's facet count would freeze at whatever
/// the last SYNCHRONOUSLY solved design had, for every one of those designs.
/// `result.solved` is the SAME single `Design::solve()` this dispatch already
/// paid for -- every helper below is the `_from_solved`
/// shape that reuses it, exactly like `refresh_editor_panel_from_solve`'s own
/// `push_yield_and_proportions`/`push_manufacturability_and_preform_scratch`
/// (`view.rs`) do, just called directly here since this module has no
/// `EditorState`/`ScratchDelta` of its own to route through those two functions
/// themselves (see the parent module doc comment, "Why a `thread_local!`, not a new
/// `EditorState` field").
fn push_solve_dependent_background_fields(ui: &MainWindow, result: &BackgroundSolveResult) {
    let solved_slice = result.solved.as_deref();
    let (table_pct, crown_height, pavilion_depth, total_depth, length_to_width) = solved_slice
        .map_or_else(
            || proportions_texts(&result.design),
            |solved| proportions_texts_from_solved(&result.design, solved),
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

    let (girdle_thickness, crown_to_width, pavilion_to_width, girdle_to_width) = solved_slice
        .map_or_else(
            || girdle_and_ratio_texts(&result.design),
            |solved| girdle_and_ratio_texts_from_solved(&result.design, solved),
        );
    ui.global::<EditorModel>()
        .set_girdle_thickness_text(girdle_thickness.into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text(crown_to_width.into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text(pavilion_to_width.into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text(girdle_to_width.into());

    let (preform_half_width_mm, preform_depth_mm) = solved_slice.map_or_else(
        || preform_mm_texts(&result.design),
        |solved| preform_mm_texts_from_solved(&result.design, solved),
    );
    ui.global::<EditorModel>()
        .set_preform_half_width_mm_text(preform_half_width_mm.into());
    ui.global::<EditorModel>()
        .set_preform_depth_mm_text(preform_depth_mm.into());
    let mm_per_unit =
        solved_slice.and_then(|solved| result.design.yield_report(solved).mm_per_unit);
    ui.global::<EditorModel>().set_preform_y_offset_mm(
        preform_y_offset_mm_text(result.design.preform_y_offset, mm_per_unit).into(),
    );

    let facet_count = solved_slice.map_or(0, |solved| {
        i32::try_from(facet_count_from_solved(&result.design, solved)).unwrap_or(i32::MAX)
    });
    ui.global::<EditorModel>().set_facet_count(facet_count);

    if let Some(solved) = solved_slice {
        let rows: Vec<AngleItem> = cutting_instructions_rows(&result.design, solved);
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(rows)));
    } else {
        ui.global::<EditorModel>()
            .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));
    }

    // see `view::panel::push_proportion_verdicts_from_solved`'s own doc
    // comment for why a background-solve completion is one of the paths that
    // used to leave this chip stale.
    push_proportion_verdicts_from_solved(ui, &result.design, solved_slice, result.n_d);
}

/// Re-dispatches whatever `super::runtime::Runtime::pending_dispatch`
/// [`apply_background_solve_result`] took, if any -- the "dispatch once on
/// completion" half of the queue-instead-of- spawn-a-second-worker mechanism. A
/// no-op when nothing queued up while the just-finished solve was running.
fn dispatch_pending(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    pending: Option<PendingDispatch>,
) {
    if let Some(pending) = pending {
        super::dispatch::dispatch_background_solve(
            ui,
            render_ctx,
            pending.design,
            &pending.generation,
            pending.multi_selected,
            pending.started_generation,
        );
    }
}
