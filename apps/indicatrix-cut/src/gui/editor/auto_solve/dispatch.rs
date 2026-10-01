//! Spawning a background solve of a `Design` snapshot, ticking its "Solving..."
//! banner, and the cancellable-solve/plane-cap helpers `dispatch::apply` and
//! `state::mod.rs` share.

use super::{
    apply::{BackgroundSolveResult, apply_background_solve_result},
    runtime::{PendingDispatch, RUNTIME},
};
use crate::{
    EditorModel, EditorTierItem, MainWindow,
    bridge::render_thread::RenderContext,
    gui::editor::state::{
        apply_multi_selection, design_to_gpu_planes, manufacturability_warnings_tagged,
        status_text_and_is_problem, status_text_and_is_problem_from_solved, tier_items,
        tier_items_from_solved, yield_report_texts, yield_report_texts_from_solved,
    },
};
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        meet_solver::{SolveError, SolvedTier},
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{Design, DesignSolveError};
use indicatrix_editor::solve_policy::SOLVING_TICK_INTERVAL as TICK_INTERVAL;
use slint::{ComponentHandle, Weak};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

// The cancellable solve, the plane-cap diagnosis and the "Solving..." banner text
// live in `indicatrix_editor::solve_policy`, shared with the web app; re-exported
// here at their old paths. `TICK_INTERVAL` (the banner's refresh period) matches
// `deep_solve::TICK_INTERVAL`/`optimize_solve::TICK_INTERVAL`'s own value.
pub(super) use indicatrix_editor::solve_policy::solving_banner;
pub(in crate::gui::editor) use indicatrix_editor::solve_policy::{
    likely_hit_plane_cap, solve_cancellably, too_many_planes_message,
};

/// Invalidates whatever background solve is currently in flight or queued behind
/// it, and stops the worker -- see `super::runtime::Runtime::current_cancel`'s own
/// doc comment.
///
/// Bumping `current_seq` makes the in-flight worker's eventual completion fail its
/// `is_current(seq)` check (the same mechanism `scheduling::reset_for_new_design`
/// already uses for a wholesale design replacement), so
/// [`super::apply::apply_background_solve_result`] still runs when that worker
/// finishes (freeing `super::runtime::Runtime::solve_in_flight` for the next
/// dispatch) but touches nothing else. Setting
/// `super::runtime::Runtime::current_cancel`
/// (when one is running -- `None` while nothing is in flight, e.g. a stray click
/// after the last worker already finished) is what makes the worker itself notice:
/// `dispatch_background_solve`'s worker threads `SolveControl::with_cancel` through
/// `Design::solve_with`, which checks the flag at every cancel point
/// `indicatrix::geometry::meet_solver` exposes (per-sweep, per-pipeline-run) --
/// typically single-digit milliseconds, rather than waiting the multiple seconds a
/// full run-to-completion could take. Also drops any [`PendingDispatch`] queued behind the cancelled
/// solve (it named the design as of the click that queued it; a fresh dispatch
/// from the CURRENT design will replace it via the ordinary edit path if one is
/// still needed) and the debounced auto-solve timer `scheduling::on_edit` may have
/// pending.
///
/// Also drops `super::runtime::Runtime::idle_replan` for the
/// same reason `scheduling::reset_for_new_design` does: a cancelled solve leaves
/// nothing guaranteeing the solid preview will settle on its own, but a stale
/// partial-frame idle-replan timer left armed from before the cancel would otherwise
/// fire later and resubmit a replan the cutter no longer asked for.
///
/// Deliberately narrower than `scheduling::reset_for_new_design`: `last_solve`/
/// `current_design` are left untouched, since cancelling a solve does not change
/// which design is loaded or invalidate that design's previous measurement.
///
/// The UI-visible half of "Abandon Solve" -- clearing `EditorModel.solve_running`/
/// setting `solve_state`/`status_text` back to an "abandoned" message on the SAME
/// click -- is `gui::editor::setup::setup_solve_cancel_callback`, which calls this
/// function first.
pub(in crate::gui::editor) fn cancel_in_flight_solve() {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if let Some(cancel) = rt.current_cancel.as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
        rt.current_seq += 1;
        rt.debounce = None;
        rt.pending_dispatch = None;
        rt.idle_replan = None;
    });
}

/// [`cancel_in_flight_solve`] plus the SAME `EditorModel` reset
/// `gui::editor::setup::setup_solve_cancel_callback` applies on the command bar's own "Abandon
/// Solve" button -- used as the background-solve activity's own
/// `ActivityRegistry` cancel closure (see [`dispatch_background_solve`]), so
/// cancelling from the status strip's activity list is indistinguishable from
/// clicking Abandon Solve itself. `mod.rs`'s own callback is left untouched (it is
/// not this module's file to edit beyond a registration line) and keeps doing the
/// same two things separately; this exists only so the SECOND cancel entry point
/// this module adds behaves identically rather than only stopping the worker without
/// resetting what the command bar shows.
fn cancel_in_flight_solve_from_activity(ui_weak: &Weak<MainWindow>) {
    cancel_in_flight_solve();
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let model = ui.global::<EditorModel>();
    model.set_solve_running(false);
    model.set_solve_state("stale".into());
    model.set_status_text("Solve abandoned -- click Solve when you are ready.".into());
    model.set_status_is_problem(true);
}

/// `true` iff `seq` is still `super::runtime::Runtime::current_seq` -- shared by
/// [`spawn_solving_ticker`] and `super::apply::apply_background_solve_result` to
/// decide whether a dispatch/completion is still the one currently owning the
/// "Solving..." banner.
pub(super) fn is_current(seq: u64) -> bool {
    RUNTIME.with(|cell| cell.borrow().current_seq == seq)
}

/// Everything [`dispatch_background_solve`]'s worker derives from one solve, so the
/// two branches live in one named place rather than inline in an already-long
/// function.
struct PanelInputs {
    tiers: Vec<EditorTierItem>,
    status_text: String,
    status_is_problem: bool,
    /// `(tier index, warning text)` pairs -- kept tagged
    /// (rather than a flattened `manufacturability_warning_lines`/`_from_solved`
    /// text-only list) so `apply::apply_background_solve_result` can
    /// push `EditorModel.manufacturability_warning_tiers` alongside the text,
    /// the same "which row is this about" attribution
    /// `view::push_stale_content`/`push_solved_preview` already give theirs.
    warnings: Vec<(usize, String)>,
    yield_texts: (String, String, String, String),
    planes: Vec<GpuFacetPlane>,
    /// See [`BackgroundSolveResult::too_many_planes`]'s own doc comment -- the
    /// same flag, computed here (on the worker thread, where a real `solve_with`
    /// re-check is safe) and carried straight through.
    too_many_planes: bool,
}

/// Builds every panel readout from a SINGLE `Design::solve`, avoiding the cost of
/// calling five separate helpers that would each solve internally -- with
/// `status_text_and_is_problem` solving twice on its own, via `status()` and
/// `measure()` -- for five or more solves per dispatch.
///
/// `solved` is `None` only when the design does not solve at all. That case falls
/// back to each helper's own solving form, which costs nothing extra: a design with
/// a block missing its scale-reference anchor fails `Design::solve` before any real
/// meet-solving work (see that method's early `MissingAnchor` check), so the
/// fallback re-pays only the cheap early-out, never the expensive case.
///
/// `custom_materials` is a snapshot of `RenderContext::custom_materials` taken at
/// dispatch time: this avoids calling the built-ins-only
/// `Design::effective_refractive_index`, which would give a design with a custom
/// catalogue material selected (e.g. "Garnet 1.74") every MARGIN/risk badge in
/// this background-solved tier table computed against the wrong (default) RI --
/// silently wrong numbers on the one readout a pavilion-angle decision rests on.
/// `Design::effective_refractive_index_with` resolves the SAME way
/// `EditorMaterialLookup`/the viewport's `resolve_material` already do.
///
/// `custom_sg` is likewise a snapshot of `RenderContext::
/// custom_material_specific_gravity`, taken at the same dispatch point as
/// `custom_materials` -- see [`dispatch_background_solve`]'s own snapshot comment
/// -- and handed straight through to `yield_report_texts`/
/// `yield_report_texts_from_solved`, which have no `RenderContext` access of
/// their own.
fn panel_inputs(
    design: &Design,
    solved: Option<&Vec<SolvedTier>>,
    custom_materials: &[GemMaterial],
    custom_sg: &[(String, f64)],
) -> PanelInputs {
    let n_d = design.effective_refractive_index_with(custom_materials);
    // The same cheap-then-
    // authoritative check `state::status_text_and_is_problem*` runs internally for
    // its OWN status text, run once more here purely to hand
    // `apply::apply_background_solve_result` a plain `bool` for the once-per-design
    // toast -- see `BackgroundSolveResult::too_many_planes`'s own doc comment.
    // Safe to call `too_many_planes_message` (a real `solve_with`) unconditionally
    // behind the cheap pre-filter here: this whole function runs on the
    // background worker thread (`solve_and_build_result`'s own caller), never the
    // UI thread.
    let too_many_planes = solved.is_some_and(|solved| likely_hit_plane_cap(solved))
        && too_many_planes_message(design).is_some();
    solved.map_or_else(
        || {
            let (status_text, status_is_problem) = status_text_and_is_problem(design);
            PanelInputs {
                tiers: tier_items(design, n_d),
                status_text,
                status_is_problem,
                warnings: manufacturability_warnings_tagged(design, None),
                yield_texts: yield_report_texts(design, custom_sg),
                planes: design_to_gpu_planes(design),
                too_many_planes,
            }
        },
        |solved| {
            let (status_text, status_is_problem) =
                status_text_and_is_problem_from_solved(design, solved);
            PanelInputs {
                tiers: tier_items_from_solved(design, solved, n_d),
                status_text,
                status_is_problem,
                warnings: manufacturability_warnings_tagged(design, Some(solved)),
                yield_texts: yield_report_texts_from_solved(design, solved, custom_sg),
                planes: super::replan::design_to_gpu_planes_from_solved(design, solved),
                too_many_planes,
            }
        },
    )
}

/// Spawns the ticker thread that keeps `dispatch_background_solve`'s
/// "Solving..." banner's elapsed time fresh -- split out purely to keep that
/// function under clippy's function-length lint. Returns the `done` flag the
/// caller must set once the real solve finishes, which stops this thread's loop
/// on its next `TICK_INTERVAL` wakeup.
fn spawn_solving_ticker(ui: &MainWindow, seq: u64, tier_count: usize) -> Arc<AtomicBool> {
    let done_flag = Arc::new(AtomicBool::new(false));
    let done_ticker = Arc::clone(&done_flag);
    let ticker_ui = ui.as_weak();
    thread::spawn(move || {
        let start = Instant::now();
        loop {
            thread::sleep(TICK_INTERVAL);
            if done_ticker.load(Ordering::Relaxed) {
                break;
            }
            let elapsed = start.elapsed();
            let _ = ticker_ui.upgrade_in_event_loop(move |ui| {
                // A superseded dispatch's ticker has nothing left worth touching --
                // the newer dispatch owns the banner now.
                if is_current(seq) {
                    ui.global::<EditorModel>()
                        .set_status_text(solving_banner(tier_count, elapsed).into());
                }
            });
        }
    });
    done_flag
}

/// [`dispatch_background_solve`]'s "a solve is already in flight" guard: queues
/// `design`/`generation`/`multi_selected` as a
/// [`PendingDispatch`] (overwriting any earlier one still waiting) rather than
/// spawning a second concurrent worker thread, when one is already running.
/// Otherwise claims the in-flight slot for `cancel` and returns `false`. Split
/// out purely to keep [`dispatch_background_solve`] under clippy's
/// function-length lint.
///
/// stamps the queued [`PendingDispatch`] with the CALLER's own
/// `started_generation` -- the value `generation` held at the moment `design`
/// was snapshotted, threaded through as a parameter rather than reloaded from
/// the live `Arc` here. `generation` is the SAME shared counter `EditorState`
/// keeps bumping, so reloading it at queue time would silently pick up every
/// edit that landed between the ORIGINAL dispatch (or an earlier replay of a
/// still-queued one) and this call, and replay a stale `design` snapshot AS IF
/// it were current -- exactly the bug a nested replay-of-a-replay could hit if
/// this read `generation.load(..)` itself instead of trusting the value its own
/// caller already captured. See [`PendingDispatch::started_generation`]'s own
/// doc comment for the completion-side half this pairs with.
pub(super) fn queue_or_claim_solve_slot(
    design: &Design,
    generation: &Arc<AtomicU64>,
    started_generation: u64,
    multi_selected: &BTreeSet<usize>,
    cancel: &Arc<AtomicBool>,
) -> bool {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if rt.solve_in_flight {
            rt.pending_dispatch = Some(PendingDispatch {
                design: design.clone(),
                started_generation,
                generation: Arc::clone(generation),
                multi_selected: multi_selected.clone(),
            });
            true
        } else {
            rt.solve_in_flight = true;
            rt.current_cancel = Some(Arc::clone(cancel));
            false
        }
    })
}

/// Registers this dispatch with
/// the shared `ActivityRegistry` (see `super::runtime::activity`'s own doc comment
/// for why no new parameter is needed here) so the status strip's activity list
/// shows it alongside Deep Solve/Optimize -- indeterminate (`meet_solver` reports no
/// completion fraction, only elapsed time, same as [`spawn_solving_ticker`]'s own
/// banner). The cancel closure goes through [`cancel_in_flight_solve_from_activity`]
/// (not the bare `Runtime`-only [`cancel_in_flight_solve`]) so cancelling from the
/// activity list also resets `EditorModel.solve_running`/`solve_state`/
/// `status_text` exactly like clicking "Abandon Solve" does. Split out of
/// [`dispatch_background_solve`] purely to keep that function under clippy's
/// function-length lint.
fn register_solve_activity(ui: &MainWindow, tier_count: usize) {
    let activity_id = RUNTIME
        .with(|cell| cell.borrow().activity.clone())
        .map(|a| {
            let ui_weak = ui.as_weak();
            a.start(
                "solve",
                solving_banner(tier_count, Duration::ZERO),
                Some(Box::new(move || {
                    cancel_in_flight_solve_from_activity(&ui_weak);
                })),
            )
        });
    RUNTIME.with(|cell| cell.borrow_mut().activity_id = activity_id);
}

/// The worker thread's own solve-and-build-panel-inputs body -- see the comment
/// inside this function's body for why `design` is solved exactly
/// once here. Split out of [`dispatch_background_solve`]'s worker closure purely
/// to keep that function under clippy's function-length lint.
fn solve_and_build_result(
    design: Design,
    cancel: &AtomicBool,
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[GemMaterial],
    custom_sg: &[(String, f64)],
    start: Instant,
) -> BackgroundSolveResult {
    // `design` is solved exactly ONCE here, and every other
    // conversion below is built from that same `solved` list via its
    // `_from_solved` counterpart -- the same helpers `view::push_solved_preview`
    // already proves out on the solid-preview worker's own replan-completion
    // path. This avoids `tier_items`/
    // `status_text_and_is_problem`/`manufacturability_warning_lines`/
    // `yield_report_texts`/`design_to_gpu_planes` each independently calling
    // `Design::solve` (`status_text_and_is_problem` would even call it twice, via
    // `status()` and `measure()`), which would add up to five or more total
    // solves per background dispatch. `result.solved` below
    // comes from this same single `solved_result`, not a second `design.solve()`.
    //
    // `solve_cancellably` (not a
    // plain `design.solve()`) threads `cancel` through, so `cancel_in_flight_solve`
    // ("Abandon Solve") can actually stop this thread's work within a sweep or
    // pipeline run, rather than only discarding whatever this eventually returns.
    let solved_result = solve_cancellably(&design, cancel);
    let multi_selected_count = multi_selected.len();
    // the SAME effective RI `panel_inputs` below computes for the tier
    // table's critical-angle margin column, kept on the result so
    // `apply::push_solve_dependent_background_fields` can judge the
    // proportion-verdict chips against the design's real effective RI instead
    // of a built-in-only fallback.
    let n_d = design.effective_refractive_index_with(custom_materials);
    // a cancelled solve must not go on to pay for `panel_inputs`'s five
    // (or more) further `Design::solve()`/`status()`/`measure()` calls, each as
    // expensive as the solve that was just cancelled -- that is the real reason
    // "Abandon Solve" used to take 25-30s to actually free `solve_in_flight` on a
    // large design, not the (already near-instant, per `solve_cancellably`'s own
    // 100ms-latency test) cancel itself. `cancel.load` is also checked directly
    // (not just the `Err` variant) so a cancel observed between
    // `solve_cancellably` returning and this check still short-circuits, rather
    // than only when `solve_cancellably` itself happened to observe it first.
    if matches!(
        solved_result,
        Err(DesignSolveError::Solve(SolveError::Cancelled))
    ) || cancel.load(Ordering::Relaxed)
    {
        return BackgroundSolveResult::cancelled(design, start.elapsed(), multi_selected_count);
    }
    let PanelInputs {
        mut tiers,
        status_text,
        status_is_problem,
        warnings,
        yield_texts,
        planes,
        too_many_planes,
    } = panel_inputs(
        &design,
        solved_result.as_ref().ok(),
        custom_materials,
        custom_sg,
    );
    apply_multi_selection(&mut tiers, multi_selected);
    let solved = solved_result.ok();
    let gear = (
        design.meta.gear_teeth_abs(),
        design.meta.gear_reference_angle as f32,
    );
    BackgroundSolveResult {
        elapsed: start.elapsed(),
        tiers,
        status_text,
        status_is_problem,
        warnings,
        yield_texts,
        planes,
        solved,
        too_many_planes,
        gear,
        multi_selected_count,
        n_d,
        panicked: false,
        // The same snapshot solved above, moved in last (after
        // every borrow of it -- `panel_inputs`/`gear` -- is done with it).
        design,
    }
}

/// Spawns a background solve of `design` (a snapshot taken the moment this is called)
/// plus a ticker that keeps the "Solving..." banner's elapsed time fresh, following
/// `deep_solve::spawn_deep_solve`'s own `thread::spawn` + `Weak::upgrade_in_event_loop`
/// shape. Shared by `super::super::view::refresh_all`'s large-design path and
/// `scheduling::on_edit`'s own debounced auto-solve dispatch -- both just need a
/// `Design` snapshot and the `generation` counter to check it against on arrival.
///
/// See the parent module doc comment for exactly what the completion handler does and
/// does not apply.
///
/// `multi_selected` is a snapshot of `EditorState::multi_selected` taken at dispatch
/// time -- cloned in (a plain `BTreeSet<usize>` is `Send`, unlike the `Rc<RefCell<..>>`
/// wrapping `EditorState` itself) so the worker thread can bake the live multi-select
/// highlight into `tier_items`' result via `apply_multi_selection`, matching every
/// other full tier-list rebuild (`view::refresh_editor_panel`/`push_stale_content`).
/// A toggle that lands after dispatch but before this solve completes is not
/// reflected in that particular frame -- no worse than the existing generation-based
/// staleness this module already accepts elsewhere, and the next refresh (of any
/// kind) reads the current selection fresh.
/// `started_generation` is the value `generation` held at the moment `design` was
/// SNAPSHOTTED (not necessarily "right now" -- see [`PendingDispatch::
/// started_generation`]'s own doc comment for why a replayed pending dispatch
/// must pass its OWN captured value here rather than let this function reload the
/// live counter, which may have moved on since). An ordinary fresh dispatch
/// (`scheduling::on_edit`/`view::refresh_all`) simply reads `generation` right
/// before calling this, which is the same moment `design` itself was snapshotted
/// on those paths, so the two callers share no special-casing beyond that.
pub(in crate::gui::editor) fn dispatch_background_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: Design,
    generation: &Arc<AtomicU64>,
    multi_selected: BTreeSet<usize>,
    started_generation: u64,
) {
    // A solve already in flight keeps this request queued
    // (overwriting any earlier one still waiting) rather than spawning a second
    // concurrent worker thread -- `apply_background_solve_result` replays exactly
    // this dispatch once the in-flight one completes. The existing "Solving..."
    // banner/ticker already belongs to that in-flight solve, so nothing else here
    // needs touching.
    // Created fresh on every call,
    // but only actually stored (and therefore only actually observed by anything)
    // when this call goes on to spawn a worker below -- a call that instead
    // queues as a `PendingDispatch` leaves this `Arc` unused and it is simply
    // dropped, harmlessly.
    let cancel = Arc::new(AtomicBool::new(false));
    if queue_or_claim_solve_slot(
        &design,
        generation,
        started_generation,
        &multi_selected,
        &cancel,
    ) {
        return;
    }

    let tier_count = design.tiers.len();
    let seq = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.current_seq
    });

    ui.global::<EditorModel>().set_solve_running(true);
    ui.global::<EditorModel>().set_solve_state("solving".into());
    ui.global::<EditorModel>()
        .set_status_text(solving_banner(tier_count, Duration::ZERO).into());
    ui.global::<EditorModel>().set_status_is_problem(false);
    register_solve_activity(ui, tier_count);

    let done_flag = spawn_solving_ticker(ui, seq, tier_count);

    // Snapshotted here (still on the UI thread) rather than
    // read from `render_ctx_for_apply` inside the worker below -- `RenderContext`'s
    // lock is meant for the render thread's frame cadence, not held across a
    // multi-second `Design::solve()`, and `Arc<Vec<GemMaterial>>` clones cheaply
    // (see `RenderContext::custom_materials`'s own doc comment on why it is an
    // `Arc` in the first place). `custom_sg` is snapshotted in
    // this SAME locked read for the identical reason -- see `panel_inputs`'s own
    // doc comment for where it ends up.
    let (custom_materials, custom_sg) = {
        let ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        (
            Arc::clone(&ctx.custom_materials),
            Arc::clone(&ctx.custom_material_specific_gravity),
        )
    };

    let ui_weak = ui.as_weak();
    let render_ctx_for_apply = Arc::clone(render_ctx);
    let generation = Arc::clone(generation);
    let multi_selected_count = multi_selected.len();
    thread::spawn(move || {
        let start = Instant::now();
        // a panic anywhere inside `solve_and_build_result` (a real meet-
        // solver bug, most plausibly) must not leave `Runtime::solve_in_flight`
        // stuck `true` for the rest of the session -- every dispatch after that
        // would then queue forever behind a slot nothing will ever free.
        // `design` is cloned BEFORE the `catch_unwind` boundary purely so a panic
        // still leaves something to build a minimal, toasted result from: the
        // ordinary (non-panicking) path never touches `design_for_panic` again,
        // so this costs nothing beyond the one clone on the rare panic path
        // itself mattering.
        let design_for_panic = design.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            solve_and_build_result(
                design,
                &cancel,
                &multi_selected,
                &custom_materials,
                &custom_sg,
                start,
            )
        }))
        .unwrap_or_else(|_| {
            BackgroundSolveResult::panicked(design_for_panic, start.elapsed(), multi_selected_count)
        });
        // Always set, and always followed by a posted completion -- a solve that
        // panicked must free `Runtime::solve_in_flight`/`current_cancel` exactly
        // like an ordinary or a cancelled one does, via
        // `apply_background_solve_result`'s own `free_in_flight_slot` preamble.
        done_flag.store(true, Ordering::Relaxed);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            apply_background_solve_result(
                &ui,
                &render_ctx_for_apply,
                seq,
                started_generation,
                &generation,
                result,
            );
        });
    });
}
