//! The replan-on-edit request path of the shared viewport: [`submit_preview_replan`] and its
//! variants hand an edit to the solid preview's worker thread, and [`push_solved_preview`]
//! pushes the worker's frame back into the tier table, the banner and the yield figures.

use super::scaled_viewport_size;
use crate::{
    EditorModel, MainWindow, SolidPreviewModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            manipulate::{frame_updates_mast_cache, note_committed_replan_submitted},
            state::{
                EditorState, apply_multi_selection, manufacturability_warnings_tagged_with,
                push_multi_selected_count, push_tiers, status_text_and_is_problem_from_solved,
                tier_items_from_solved_with_warnings, yield_report_texts_from_solved,
            },
        },
        render::camera_lighting::contained_request_size,
        solid_preview::{
            cut_slider,
            preview_state::{CameraPose, ReplanRequest, SolidLastSolved, SolidPreviewState},
        },
    },
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, ManufacturabilityWarning};
use slint::{ComponentHandle, SharedString};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
};

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
pub(in crate::gui::editor) fn submit_preview_replan(
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
pub(in crate::gui::editor) struct ReplanSource<'a> {
    pub(in crate::gui::editor) design: &'a Design,
    pub(in crate::gui::editor) generation: u64,
    pub(in crate::gui::editor) multi_selected: &'a BTreeSet<usize>,
}

/// [`submit_preview_replan`]'s own body, taking a [`ReplanSource`] SNAPSHOT
/// rather than a live `&EditorState` -- split out so
/// `auto_solve::schedule_idle_replan_if_stale` can resubmit a follow-up replan
/// once the solid-preview worker goes idle after a partial (subgraph) resolve,
/// without needing the live `Rc<RefCell<EditorState>>` that module deliberately
/// never holds (see its own doc comment, "Why a `thread_local!`, not a new
/// `EditorState` field"). [`submit_preview_replan`] itself is the thin,
/// `EditorState`-shaped wrapper every other caller keeps using.
pub(in crate::gui::editor) fn submit_preview_replan_for(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    source: ReplanSource<'_>,
    dirty: BTreeSet<usize>,
    force_full_solve: bool,
) {
    let generation = source.generation;
    let last_solved = if force_full_solve {
        None
    } else {
        solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|(cached_generation, masts)| {
                // only chain the cached masts forward when they
                // describe THIS generation or the immediately preceding one --
                // a cache entry more than one edit stale is more likely to be
                // misaligned with `dirty`'s own tier indices (this edit's own
                // `resolve_dirty` subgraph) than to still match by
                // coincidence. `live_update::plan_preview` already falls back
                // to a full solve whenever the lengths disagree regardless.
                (*cached_generation == generation || *cached_generation + 1 == generation)
                    .then(|| masts.clone())
            })
    };
    submit_preview_replan_chained(ui, render_ctx, preview_state, source, dirty, last_solved);
}

/// [`submit_preview_replan_for`]'s body with the `last_solved` masts handed in
/// explicitly instead of read from the shared `solid_last_solved` cache -- for a
/// replan whose design is NOT the committed one, so the shared cache (which only ever
/// holds the committed design's masts) is the wrong chain. The Slice tool's
/// provisional replans pass the masts of their own previous frame with
/// `dirty = {provisional tier}`, which `live_update::plan_preview` re-solves as a
/// subgraph instead of a full solve. `None` means a full solve.
pub(in crate::gui::editor) fn submit_preview_replan_chained(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    source: ReplanSource<'_>,
    dirty: BTreeSet<usize>,
    last_solved: Option<Vec<SolvedTier>>,
) {
    // A ghost still being solved would land over this replan's frame.
    super::super::optimize_apply::cancel_ghost_preview();
    let ReplanSource {
        design,
        generation,
        multi_selected,
    } = source;
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
    // A selected CONCAVE row has a table position past the flat tiers
    // (`design.tiers.len() + concave index`); `FacetMap::overlay_flags` tints that
    // concave tier's tool facets from it. Anything past the last concave row is stale.
    let selected_tier = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index())
        .ok()
        .filter(|&index| index < design.tiers.len() + design.concave_tiers.len());
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
    // The Slice tool's provisional design (its reserved generation) is NOT the live
    // design: stashing it would overwrite the committed snapshot the next real frame
    // consumes (and arm the idle-replan check on the wrong design). The sink never
    // asks for it either -- see `frame_updates_mast_cache`.
    if frame_updates_mast_cache(generation) {
        // This committed job takes the plan gate's single slot: a provisional replan
        // still queued there is gone, so the Slice tool must not wait for its frame.
        note_committed_replan_submitted();
        auto_solve::stash_current_design(
            generation,
            Arc::clone(&design_snapshot),
            multi_selected.clone(),
        );
    }
    // Read here rather than cached on `SolidPreviewState` alone, so the slider and the
    // redraw can never disagree about how much of the cut is being shown: `None` is the
    // finished design, `Some(0)` the rough. A replan of the committed design also
    // refreshes the slider's own fields (step count, handle, label) from this design --
    // the Slice tool's provisional replans do not, their design has one tier too many.
    // The Diagram always shows the finished design.
    let position_steps = if frame_updates_mast_cache(generation) {
        cut_slider::sync_model(ui, design)
    } else {
        cut_slider::current_steps(ui, design)
    };
    preview_state.set_cut_steps(position_steps.filter(|_| view_mode != 3));
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
/// Deliberately narrower than [`super::super::panel::refresh_editor_panel`]/
/// [`super::super::panel_stale::push_stale_content`]: proportions, the cutting
/// schedule, preform/material scratch fields and Deep Solve/Optimize
/// availability are all either solve-independent (already current, pushed
/// synchronously by [`super::super::panel_stale::refresh_editor_panel_stale`] at edit
/// time) or not pushed by this mechanism -- only the four fields that
/// [`auto_solve::dispatch_background_solve`]'s OWN completion would otherwise
/// have been the sole source of.
///
/// Called from `gui::SlintSolidSink::apply` -- a solid-preview WORKER-thread
/// callback hopped onto the UI thread via `slint::Weak::upgrade_in_event_loop` --
/// through `editor::apply_matching_preview_frame`'s thin forwarding wrapper, the
/// one bridge this group exposes beyond [`super::super::super::setup_editor_callbacks`]
/// itself (see that module's own doc comment, "Module split").
///
/// `custom_materials`: resolves `design`'s
/// effective refractive index the SAME custom-catalogue-aware way
/// [`super::super::inspector::refresh_design_settings`]/`auto_solve::panel_inputs`
/// already do -- the bare accessor would silently fall back to 1.5442 for a
/// design named after a custom catalogue material, leaving the tier table's
/// critical-angle margin column disagreeing with the Design Settings panel's
/// own effective-RI readout for exactly that design.
///
/// `custom_sg`: the catalogue's custom-material specific-
/// gravity table, handed to [`yield_report_texts_from_solved`] for the same
/// reason `auto_solve::panel_inputs`/`super::super::panel::push_yield_and_proportions`
/// do -- see that function's own doc comment.
///
/// `precomputed_warnings`: the frame's manufacturability pass, run by the plan worker
/// (`PreviewFrame::warnings`). The rows and the banner only DISPLAY it, so this
/// UI-thread function never builds the stone's solid for the concave-tool check; `None`
/// (no pass for this frame's generation) is resolved by the row builders.
pub(in crate::gui::editor) fn push_solved_preview(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[indicatrix::optics::materials::GemMaterial],
    custom_sg: &[(String, f64)],
    precomputed_warnings: Option<&[ManufacturabilityWarning]>,
) {
    let n_d = design.effective_refractive_index_with(custom_materials);
    push_tier_rows(
        ui,
        design,
        solved,
        multi_selected,
        n_d,
        precomputed_warnings,
    );

    let (status_text, is_problem) = status_text_and_is_problem_from_solved(design, solved);
    ui.global::<EditorModel>()
        .set_status_text(status_text.into());
    ui.global::<EditorModel>().set_status_is_problem(is_problem);
    ui.global::<EditorModel>()
        .set_solve_state(if is_problem { "failed" } else { "solved" }.into());
    // A third route to "solved" (after `refresh_all` and the background solve's
    // own completion): let the guide's "Solve and check" step see it, AFTER the
    // verdict above is in place.
    crate::gui::editor::guide::check_design_progress(ui, design);

    push_warning_list(ui, design, solved, precomputed_warnings);

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

    // see `panel::push_proportion_verdicts_from_solved`'s own doc
    // comment -- this solid-preview completion is the other path that used to
    // leave the chips describing whatever design the last synchronous "Solve"
    // click left, right next to the fresh numbers this frame just pushed.
    super::super::panel::push_proportion_verdicts_from_solved(ui, design, Some(solved), n_d);
}

/// Refreshes the tier table's rows and the warning list with the manufacturability
/// `findings` of a frame that has already landed. The plan worker submits its frame BEFORE it
/// runs the costly concave-tool check (check 6), so the first push of the rows used only the
/// checks that need no solid; this second push brings the full pass in. Everything else the
/// frame pushed (status, yield figures, verdict chips) does not read the findings and is left
/// alone.
///
/// The caller has already checked that `design` is still the editor's design.
pub(in crate::gui::editor) fn push_late_findings(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[indicatrix::optics::materials::GemMaterial],
    findings: &[ManufacturabilityWarning],
) {
    let n_d = design.effective_refractive_index_with(custom_materials);
    push_tier_rows(ui, design, solved, multi_selected, n_d, Some(findings));
    push_warning_list(ui, design, solved, Some(findings));
}

/// The tier table's rows for `design` from its masts, with the multi-selection marked.
/// `precomputed_warnings` is the plan worker's manufacturability pass, when it has one.
fn push_tier_rows(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    multi_selected: &BTreeSet<usize>,
    n_d: f64,
    precomputed_warnings: Option<&[ManufacturabilityWarning]>,
) {
    let mut tiers = tier_items_from_solved_with_warnings(design, solved, n_d, precomputed_warnings);
    apply_multi_selection(&mut tiers, multi_selected);
    push_tiers(ui, tiers);
    push_multi_selected_count(ui, multi_selected.len());
}

/// The manufacturability warning list and the tier index each one is about.
fn push_warning_list(
    ui: &MainWindow,
    design: &Design,
    solved: &[SolvedTier],
    precomputed_warnings: Option<&[ManufacturabilityWarning]>,
) {
    // The tagged pairs, not `manufacturability_warning_lines_
    // from_solved`'s flattened text-only list, so the tier index survives to
    // `manufacturability_warning_tiers` -- `editor_tier_table.slint`'s own row
    // markers read this to flag the specific row a warning is about.
    let tagged_warnings =
        manufacturability_warnings_tagged_with(design, solved, precomputed_warnings);
    let warning_tiers: Vec<i32> = tagged_warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    let warnings: Vec<SharedString> = tagged_warnings
        .into_iter()
        .map(|(_, text)| SharedString::from(text))
        .collect();
    super::super::state::push_rows(
        &ui.global::<EditorModel>().get_manufacturability_warnings(),
        warnings,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warnings(model);
        },
    );
    super::super::state::push_rows(
        &ui.global::<EditorModel>()
            .get_manufacturability_warning_tiers(),
        warning_tiers,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warning_tiers(model);
        },
    );
}
