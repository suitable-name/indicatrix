//! Feeding the shared (GPU path-traced + solid-preview) viewport: the paint-first
//! [`refresh_all_now`]/[`refresh_all`] entry points, the trace-staleness marker,
//! and the replan-on-edit request path ([`submit_preview_replan`]/
//! [`submit_preview_replan_for`]/[`push_solved_preview`]) the solid-preview
//! worker thread and its completion callback use.

use super::{
    SolidLastSolved,
    panel::refresh_editor_panel_from_solve,
    panel_stale::push_stale_content,
    state::{
        EditorState, apply_multi_selection, design_to_gpu_planes,
        manufacturability_warnings_tagged, push_multi_selected_count, push_tiers,
        status_text_and_is_problem_from_solved, tier_items_from_solved,
        yield_report_texts_from_solved,
    },
};
use crate::{
    EditorModel, MainWindow, SolidPreviewModel, TiltModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor::{
            auto_solve,
            manipulate::{frame_updates_mast_cache, note_committed_replan_submitted},
        },
        render::camera_lighting::contained_request_size,
        solid_preview::preview_state::{CameraPose, ReplanRequest, SolidPreviewState},
    },
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_editor::solve_policy::{SolveCostEstimate, should_solve_synchronously_for};
use slint::{ComponentHandle, SharedString};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
    time::{Duration, Instant},
};

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
pub(in crate::gui::editor) fn scaled_viewport_size(ui: &MainWindow) -> (u32, u32) {
    let scale = ui.window().scale_factor();
    (
        (ui.global::<SolidPreviewModel>().get_viewport_width() * scale) as u32,
        (ui.global::<SolidPreviewModel>().get_viewport_height() * scale) as u32,
    )
}

/// Writes `design`'s current plane arrangement into the shared (GPU path-traced)
/// viewport and marks it dirty, then re-issues those same planes to the solid
/// preview -- see this group's `mod.rs` doc comment ("Feeding the viewport") for why
/// this is split out from [`super::panel::refresh_editor_panel`].
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
///
/// `pub(super)`, not private: [`super::panel_stale::push_stale_content`] (a
/// sibling file) shares this exact push.
pub(super) fn push_trace_staleness(
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
    ui.global::<crate::ViewportModel>().set_trace_stale(stale);
}

/// `solved` is the SAME solve [`super::panel::refresh_editor_panel`] already
/// computed for this same "Solve" click -- `None` only for a design that does
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
    // The real design is about to be drawn: a ghost still being solved must not land
    // over it.
    super::optimize_apply::cancel_ghost_preview();
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
            .unwrap_or_else(PoisonError::into_inner) = Some((
            state.generation.load(AtomicOrdering::Relaxed),
            solved.to_vec(),
        ));
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
    super::optimize_apply::cancel_ghost_preview();
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
/// Deliberately narrower than [`super::panel::refresh_editor_panel`]/
/// [`super::panel_stale::push_stale_content`]: proportions, the cutting
/// schedule, preform/material scratch fields and Deep Solve/Optimize
/// availability are all either solve-independent (already current, pushed
/// synchronously by [`super::panel_stale::refresh_editor_panel_stale`] at edit
/// time) or not pushed by this mechanism -- only the four fields that
/// [`auto_solve::dispatch_background_solve`]'s OWN completion would otherwise
/// have been the sole source of.
///
/// Called from `gui::SlintSolidSink::apply` -- a solid-preview WORKER-thread
/// callback hopped onto the UI thread via `slint::Weak::upgrade_in_event_loop` --
/// through `editor::apply_matching_preview_frame`'s thin forwarding wrapper, the
/// one bridge this group exposes beyond [`super::super::setup_editor_callbacks`]
/// itself (see that module's own doc comment, "Module split").
///
/// `custom_materials`: resolves `design`'s
/// effective refractive index the SAME custom-catalogue-aware way
/// [`super::inspector::refresh_design_settings`]/`auto_solve::panel_inputs`
/// already do -- the bare accessor would silently fall back to 1.5442 for a
/// design named after a custom catalogue material, leaving the tier table's
/// critical-angle margin column disagreeing with the Design Settings panel's
/// own effective-RI readout for exactly that design.
///
/// `custom_sg`: the catalogue's custom-material specific-
/// gravity table, handed to [`yield_report_texts_from_solved`] for the same
/// reason `auto_solve::panel_inputs`/`super::panel::push_yield_and_proportions`
/// do -- see that function's own doc comment.
pub(in crate::gui::editor) fn push_solved_preview(
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
    // A third route to "solved" (after `refresh_all` and the background solve's
    // own completion): let the guide's "Solve and check" step see it, AFTER the
    // verdict above is in place.
    crate::gui::editor::guide::check_design_progress(ui, design);

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
    super::state::push_rows(
        &ui.global::<EditorModel>().get_manufacturability_warnings(),
        warnings,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warnings(model);
        },
    );
    super::state::push_rows(
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

    // see `panel::push_proportion_verdicts_from_solved`'s own doc
    // comment -- this solid-preview completion is the other path that used to
    // leave the chips describing whatever design the last synchronous "Solve"
    // click left, right next to the fresh numbers this frame just pushed.
    super::panel::push_proportion_verdicts_from_solved(ui, design, Some(solved), n_d);
}

/// [`super::panel::refresh_editor_panel`] + [`refresh_viewport`] together --
/// pushes a real solve's result into both the panel and the shared viewport.
/// Only called by `New`, `Load Selected`, "Adopt", and the explicit "Solve"
/// action; every other edit callback calls
/// [`super::panel_stale::refresh_editor_panel_stale`] instead and leaves the
/// viewport untouched.
///
/// # Synchronous only for a cheap-enough design
///
/// `Design::solve`'s refinement sweep is cubic in plane count (a real 103-tier/210-
/// plane design: 5.9s -- see `mod.rs`'s "Never block the UI thread with a solve"
/// section), so this only solves inline on the UI thread for a design that
/// [`should_solve_synchronously_for`] admits: few planes, few meet-derived tiers, no
/// tier targets, and no slow last measurement. That keeps the "New" dialog's promise
/// of an immediately solved, unstale design without a visible "Solving..." flash for
/// a small design. Every other design instead gets the SAME stale content
/// [`super::panel_stale::refresh_editor_panel_stale`] pushes after any other edit,
/// plus an immediately (not debounced) dispatched background solve -- see
/// [`auto_solve::dispatch_background_solve`].
///
/// `wholesale`: `true` only for a caller that just
/// replaced `EditorState` wholesale (New/Load Selected/Open) -- see
/// [`auto_solve::reset_for_new_design`]'s own doc comment for why exactly those
/// three (and only those three) need the reset it performs. Every OTHER caller
/// (the explicit "Solve" button, Adopt/Adopt All/Adopt Selected/Pin to
/// Mast/Optimize Apply/Retarget Apply) passes `false`: it is still solving the
/// SAME design the auto-solve budget has already been measuring, so
/// `auto_solve::last_solve()` is read instead and may veto an inline solve.
///
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
/// [`refresh_all`]'s own sync branch (a design [`solves_inline`] admits) sets
/// `solve_running`/`solve_state` to "solving" and then immediately runs the blocking
/// `Design::solve()` in the same call, with no yield back to the event loop in
/// between -- the toolkit only ever paints the FINAL state once the whole
/// callback returns, so "Solving..." is never actually visible for a fast-sync
/// design (see [`refresh_all`]'s own doc comment). This
/// function peeks the SAME [`solves_inline`] decision [`refresh_all`]
/// makes internally; when it would take the sync branch, it sets
/// `solve_running`/`solve_state` HERE, then defers the actual (still fully
/// synchronous) [`refresh_all`] call behind a `Timer::single_shot(Duration::ZERO,
/// ..)` so the event loop gets to paint this frame first. The background branch
/// is unaffected -- it already returns without blocking, so it runs immediately.
pub(in crate::gui::editor) fn refresh_all_now(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &Rc<RefCell<EditorState>>,
    wholesale: bool,
) {
    // `refresh_all` makes the same decision; its `auto_solve::reset_for_new_design`
    // side effect must run exactly once, so it is left to the real call below.
    let inline = solves_inline(&state.borrow().design, wholesale);
    if !inline {
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

/// Gives the shared viewport back to the editor's design when the Edit sub-tab is
/// entered while a Library selection owns it (see
/// [`RenderContext::may_claim_active_planes`]). Does nothing when the editor already
/// owns the slot or holds no real design, so switching tabs without browsing costs
/// nothing. Otherwise it is the same refresh a "Solve" click performs, which also
/// restores the editor's material and panel.
pub(in crate::gui::editor) fn reclaim_viewport_for_editor(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
) {
    let catalogue_owns = matches!(
        render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .planes_owner,
        PlanesOwner::Catalogue { .. }
    );
    if catalogue_owns && state.has_design {
        refresh_all(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            state,
            false,
        );
    }
}

/// Mirrors [`EditorState::has_design`] into `EditorModel.has_design` -- the one
/// setter path for that flag. Called by [`refresh_all`] (so every replacement that
/// refreshes through it stays in sync) and directly by the install paths
/// (`do_new_design_create`, `apply_loaded_design`, `native_io::finish_state_replace`),
/// whose [`refresh_all_now`] may defer the real refresh by one event-loop tick.
pub(in crate::gui::editor) fn push_has_design(ui: &MainWindow, state: &EditorState) {
    ui.global::<EditorModel>().set_has_design(state.has_design);
}

/// Whether `design` may be solved inline on the UI thread.
///
/// [`should_solve_synchronously_for`] over the design's solve-free cost estimate.
/// A `wholesale` refresh (the design was just replaced) passes no measurement: the
/// previous design's solve time says nothing about this one, and
/// `auto_solve::reset_for_new_design` has cleared it anyway. Otherwise the design is
/// the one the auto-solve budget has been measuring, so its last solve time may veto
/// an inline solve.
fn solves_inline(design: &Design, wholesale: bool) -> bool {
    let last_solve = if wholesale {
        None
    } else {
        auto_solve::last_solve()
    };
    should_solve_synchronously_for(SolveCostEstimate::of(design), last_solve)
}

/// Refreshes the panel and the viewport from `state` -- solving synchronously when
/// that is cheap, otherwise pushing stale content and dispatching a background solve
/// -- then lets the worked-example guide check whether its current step's goal was
/// just reached (`guide::check_progress`).
pub(in crate::gui::editor) fn refresh_all(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<crate::bridge::render_thread::RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
    wholesale: bool,
) {
    if wholesale {
        // A fresh call replacing `EditorState` wholesale means a real solve is
        // about to happen (inline below, or via the background dispatch) against a
        // DIFFERENT design than whatever `Runtime::last_solve` currently holds, so
        // that measurement is cleared unconditionally. The inline branch overwrites
        // it with a real measurement; the background branch leaves it `None` until
        // that dispatch completes, which `auto_solve::should_schedule_auto_solve`
        // treats as "try auto-solve".
        auto_solve::reset_for_new_design(state.generation.load(AtomicOrdering::Relaxed));
    }
    if solves_inline(&state.design, wholesale) {
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
            state.generation.load(AtomicOrdering::Relaxed),
        );
    }
    push_has_design(ui, state);
    crate::gui::editor::guide::check_progress(ui, state);
}
