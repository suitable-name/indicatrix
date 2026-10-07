//! Feeding the shared (GPU path-traced + solid-preview) viewport: the paint-first
//! [`refresh_all_now`]/[`refresh_all`] entry points, the trace-staleness marker,
//! and the replan-on-edit request path ([`submit_preview_replan`]/
//! [`submit_preview_replan_for`]/[`push_solved_preview`]) the solid-preview
//! worker thread and its completion callback use.

mod replan;

use super::{
    SolidLastSolved, panel::refresh_editor_panel_from_solve, panel_stale::push_stale_content,
    state::EditorState,
};
use crate::{
    EditorModel, MainWindow, SolidPreviewModel, TiltModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor::auto_solve,
        render::camera_lighting::contained_request_size,
        solid_preview::{
            cut_slider,
            preview_state::{CameraPose, SolidPreviewState},
        },
        tutorial_events::raise,
    },
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    guide::viewing_events as events,
    solve_policy::{SolveCostEstimate, should_solve_synchronously_for},
};
use indicatrix_solid::preview::StoneGeometryBuf;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
    time::{Duration, Instant},
};

// The replan-on-edit request path lives in `replan`; these are its entry points.
pub(in crate::gui::editor) use replan::{
    ReplanSource, push_late_findings, push_solved_preview, submit_preview_replan,
    submit_preview_replan_chained, submit_preview_replan_for,
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
/// the stone is drawn without masts (nothing, except the rough under the Cut slider)
/// rather than retrying the failed solve here.
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
    // The stone the Cut slider is showing, not always the finished one: this full redraw
    // used to overwrite a cut with the whole design (and so did every other redraw path).
    // The planes and the concave tools of the same solve (the tools are empty, at no
    // cost, for a planar design) are claimed together so the tracer never sees one
    // without the other. Built before the render context is locked. It never solves: with
    // no `solved` the design did not solve a moment ago either, and a second attempt
    // would only fail again on the UI thread.
    let cut_steps = cut_slider::sync_model(ui, &state.design);
    let StoneGeometryBuf {
        planes: planes_for_claim,
        tools: tools_for_claim,
        placements: placements_for_claim,
    } = cut_slider::cut_geometry_no_solve(&state.design, solved, cut_steps);
    let mut ctx = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The editor always wins a claim, so this cannot fail --
    // but it goes through the same single write path as every other writer so the
    // ownership stamp stays truthful, which is what stops a later catalogue click
    // from silently replacing the design under the cutter's hands.
    ctx.claim_active_geometry(
        std::sync::Arc::new(planes_for_claim),
        std::sync::Arc::new(tools_for_claim),
        placements_for_claim,
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
    // Live Render shows a "Cut: ..." pill only while the picture is the editor's design.
    crate::gui::editor::push_viewport_owner(ui, ctx.planes_owner);
    let stone = StoneGeometryBuf {
        planes: ctx.active_planes.as_ref().clone(),
        tools: ctx.active_tools.as_ref().clone(),
        placements: ctx.active_placements.clone(),
    };
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    // Letterboxes the request to the traced image's own
    // rectangle in Path-traced/Both, exactly like `camera_lighting::
    // resubmit_at_current_pose` already does for a camera drag/zoom/view-mode
    // switch -- see `contained_request_size`'s own doc comment for why this
    // call site needs the identical treatment.
    let size = contained_request_size(view_mode, scaled_viewport_size(ui), (ctx.width, ctx.height));
    preview_state.request_redraw_geometry(
        stone,
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

    // This is the SAME real design solve (`New`/`Load Selected`/the explicit
    // "Solve" action) `refresh_editor_panel` already computed. Stashing it here,
    // rather than independently re-solving a second time just to populate this
    // cache, is what lets the NEXT small edit's `submit_preview_replan` call use
    // a real `resolve_dirty` subgraph solve rather than falling back to another
    // full solve. Stored before the tilt sweep is requested just below: that request
    // asks the cache for the finished stone (`gui::editor::finished_stone`), and would
    // otherwise find the previous generation's masts and wait for a solve of its own.
    if let Some(solved) = solved {
        *solid_last_solved
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((
            state.generation.load(AtomicOrdering::Relaxed),
            solved.to_vec(),
        ));
    }

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
    // After the guards above are dropped: the planes the tracer holds were just
    // re-stamped with this generation, so the trace-staleness marker clears here.
    push_trace_staleness(ui, render_ctx, state);
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
    if wholesale {
        // The inline branch defers `refresh_all` by an event-loop turn: reset the Cut
        // slider now so the old design's cut is not on screen for that turn.
        cut_slider::reset_to_finished(ui, &state.borrow().design);
    }
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
    let catalogue_owns = {
        let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        let owns = matches!(ctx.planes_owner, PlanesOwner::Catalogue { .. });
        if owns && state.has_design {
            // At once, not when the (possibly background) solve claims the planes: a
            // Library row that resolved no material must not keep tracing suspended.
            ctx.restore_editor_material();
        }
        owns
    };
    if catalogue_owns && state.has_design {
        refresh_all(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            state,
            false,
        );
        let name = render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .material_name
            .clone();
        let options = ui.global::<crate::ViewportModel>().get_material_options();
        if let Some(index) = crate::gui::startup_settings::find_option_index(&options, &name) {
            ui.global::<crate::ViewportModel>()
                .set_selected_material_index(index);
        }
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
///
/// A design with concave tools never solves inline: the cost estimate counts flat
/// planes only, while the manufacturability pass that follows every solve builds the
/// stone's solid with every tool carved out, which is far more than a UI thread may
/// spend. Such a design takes the background solve, whose worker runs that pass.
fn solves_inline(design: &Design, wholesale: bool) -> bool {
    if !design.concave_tiers.is_empty() {
        return false;
    }
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
        // A new, opened or loaded design starts uncut: the Cut slider of the design it
        // replaced must not keep its label (or truncate the view) for this one.
        cut_slider::reset_to_finished(ui, &state.design);
        // The lighting this design remembered (or the normal lighting, if it has none)
        // replaces the one the design before it left showing.
        crate::gui::render::design_lighting::design_opened(
            ui,
            state.design_uuid(),
            state.has_design,
        );
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
    if wholesale {
        // New Design, Load Selected and Open end here: a tutorial step may wait for a
        // different design to replace the open one.
        raise(ui, events::DESIGN_REPLACED);
    }
    crate::gui::editor::guide::check_progress(ui, state);
}
