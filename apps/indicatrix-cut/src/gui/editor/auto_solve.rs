//! Background solving for the "Edit" sub-tab: moves `Design::solve` off the UI thread
//! for the explicit "Solve" action (button and F5, via [`super::view::refresh_all`]'s
//! large-design path) and, when the design is cheap enough, schedules the SAME
//! machinery automatically after an edit ("auto-solve") so small/medium designs get a
//! fresh path-traced view and masts without a click. See `mod.rs`'s "Never block the
//! UI thread with a solve" section for the rule this exists to uphold.
//!
//! # Why a `thread_local!`, not a new `EditorState` field
//!
//! `deep_solve`/`optimize`'s in-flight handles live directly on `EditorState`
//! (`state/mod.rs`), which is NOT this group's file to edit. Everything this module
//! needs beyond `EditorState::design`/`generation` (both already readable through a
//! plain `&EditorState`) -- the debounce timer, the running request counter, the last
//! measured solve time, and the shared solid-preview handles a completed background
//! solve pushes into -- lives instead in [`Runtime`], a `thread_local!` singleton.
//! That is sound for the identical reason `EditorState` itself gets away with a plain
//! `Rc<RefCell<..>>` rather than `Arc<Mutex<..>>` (see that struct's own doc comment):
//! every function in this module that touches `Runtime` runs on the UI/event-loop
//! thread, either directly from a Slint callback or from inside a
//! `Weak::upgrade_in_event_loop` closure. The worker threads this module spawns never
//! touch `Runtime` themselves -- they only compute pure `Design` -> data conversions
//! and hand the result back across the event-loop boundary.
//!
//! # Two kinds of "stale" a completed background solve must detect
//!
//! A background solve is dispatched against a snapshot: a cloned [`Design`] plus the
//! [`EditorState::generation`] counter's value at that moment. By the time it
//! completes, either or both of the following can have happened, and
//! [`apply_background_solve_result`] must not let either corrupt the display:
//!
//! 1. **A newer background solve was dispatched** (the user clicked Solve again, or
//!    another auto-solve fired) before this one returned. [`Runtime::current_seq`] is
//!    bumped on every dispatch; a completion only touches the UI at all while its own
//!    sequence number still matches -- an older one simply has nothing left to do,
//!    since the newer dispatch already owns the "Solving..." banner and will apply its
//!    own result in turn.
//! 2. **The design changed without a new dispatch** -- possible when auto-solve is
//!    disabled (or this design's own measured cost exceeds the budget) and an edit
//!    lands while an explicit Solve from BEFORE that edit is still running. Here this
//!    result's own sequence number is still current (nothing superseded it), but
//!    `generation` has moved past what was captured at dispatch time. The edit's own
//!    [`super::view::refresh_editor_panel_stale`] call already repainted the banner
//!    correctly at edit time, so this arm only needs to clear `editor_solve_running`
//!    (nothing else will) and otherwise touch nothing.
//!
//! Both checks are cheap `u64` comparisons; nothing here ever tries to reinterpret a
//! stale result's tier indices against a design it no longer describes.

use super::{
    state::{
        EditorState, apply_multi_selection, design_to_gpu_planes, manufacturability_warning_lines,
        status_text_and_is_problem, tier_items, yield_report_texts,
    },
    view::SolidLastSolved,
};
use crate::{
    EditorModel, EditorTierItem, MainWindow, SolidPreviewModel,
    bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::{CameraPose, SolidPreviewState},
};
use indicatrix::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use indicatrix_cut_core::Design;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Tier-count ceiling under which [`super::view::refresh_all`] still solves
/// synchronously on the UI thread (New/Load/the explicit Solve action alike -- see
/// this module's own doc comment and `mod.rs`'s "Never block the UI thread" section
/// for why the explicit action is not special-cased back to always-synchronous). Well
/// below the corpus's largest measured design (103 tiers, 5.9s) and comfortably above
/// the tier counts a fresh or lightly-edited design starts with, so this exception is
/// rarely even visible: it exists for the "New" dialog's promise of an immediately
/// solved, unstale design, not to dodge the async path for anything that matters.
pub(super) const SYNC_SOLVE_TIER_LIMIT: usize = 16;

/// Debounce delay between an edit landing and an eligible auto-solve actually
/// dispatching -- short enough that a burst of keystrokes (typing an angle, say) only
/// ever triggers the LAST one, long enough that it reads as "just happened," not
/// "laggy."
const AUTO_SOLVE_DEBOUNCE: Duration = Duration::from_millis(150);

/// How often a running background solve's ticker updates the "Solving... (N tiers)"
/// banner with fresh elapsed time -- matches `deep_solve::TICK_INTERVAL`/
/// `optimize_solve::TICK_INTERVAL`'s own value and rationale.
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// This module's UI-thread-confined state -- see the module doc comment for why this
/// is a `thread_local!` rather than a new `EditorState` field.
struct Runtime {
    /// Set once by [`init`], from the same `Arc`s `setup_editor_callbacks` already
    /// holds -- lets a completed background solve push into the shared solid preview
    /// exactly like [`super::view::refresh_viewport`] does, without this module
    /// needing `EditorState` (or a wider parameter list on
    /// [`super::view::refresh_editor_panel_stale`], which many callbacks this group
    /// does not own already call with a fixed signature).
    preview_state: Option<Arc<SolidPreviewState>>,
    solid_last_solved: Option<SolidLastSolved>,
    /// The most recent REAL measured solve wall time (background or synchronous) for
    /// whichever design is currently loaded -- `None` until the first solve since the
    /// last New/Load. Drives [`should_schedule_auto_solve`].
    last_solve: Option<Duration>,
    /// Bumped by every dispatch (explicit-Solve or auto-solve alike); see the module
    /// doc comment's "newer background solve" case.
    current_seq: u64,
    /// The debounce timer auto-solve restarts on every eligible edit. Replacing this
    /// with a fresh `Timer` cancels whatever the previous one had pending -- Slint
    /// stops a `Timer`'s callback from firing once the `Timer` itself is dropped.
    debounce: Option<slint::Timer>,
}

impl Runtime {
    const fn new() -> Self {
        Self {
            preview_state: None,
            solid_last_solved: None,
            last_solve: None,
            current_seq: 0,
            debounce: None,
        }
    }
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = const { RefCell::new(Runtime::new()) };
}

/// Stashes the shared solid-preview handles this module needs -- called once from
/// [`super::setup_editor_callbacks`], before any callback (and therefore any possible
/// dispatch) is wired up.
pub(super) fn init(preview_state: &Arc<SolidPreviewState>, solid_last_solved: &SolidLastSolved) {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.preview_state = Some(Arc::clone(preview_state));
        rt.solid_last_solved = Some(Arc::clone(solid_last_solved));
    });
}

/// Whether an edit that just landed should schedule a debounced auto-solve --
/// `budget` `0` disables auto-solve outright (today's stale-marker-only behaviour);
/// otherwise, a design with no measurement YET (`last_solve: None` -- nothing has
/// solved since the last New/Load) is scheduled optimistically: a fresh design's
/// schedule starts empty and solves near-instantly, so refusing to try would only
/// delay that design's very first auto-solve for no reason. Once a real measurement
/// exists, it alone decides.
///
/// Pure and unit tested directly -- the one genuinely decision-shaped piece of logic
/// in this module.
pub(super) const fn should_schedule_auto_solve(
    last_solve: Option<Duration>,
    budget: Duration,
) -> bool {
    if budget.is_zero() {
        return false;
    }
    match last_solve {
        Some(last) => last.as_millis() < budget.as_millis(),
        None => true,
    }
}

/// The banner text shown while auto-solve is disabled FOR THIS DESIGN specifically
/// (as opposed to the user having set the budget to `0` outright) -- this design's own
/// last measured solve already exceeds the configured budget.
pub(super) fn auto_solve_off_note(last_solve: Duration) -> String {
    format!(
        "Auto-solve off for this design: last solve took {:.1}s.",
        last_solve.as_secs_f32()
    )
}

/// The "Solving..." banner text a running background solve shows, ticked forward by
/// [`dispatch_background_solve`]'s ticker thread.
fn solving_banner(tier_count: usize, elapsed: Duration) -> String {
    format!(
        "Solving... ({tier_count} tier{}) -- {:.1}s elapsed",
        if tier_count == 1 { "" } else { "s" },
        elapsed.as_secs_f32()
    )
}

/// Records a just-completed solve's wall time (background or synchronous) as this
/// design's new estimate for [`should_schedule_auto_solve`]'s next decision.
pub(super) fn record_solve_duration(elapsed: Duration) {
    RUNTIME.with(|cell| cell.borrow_mut().last_solve = Some(elapsed));
}

/// Clears the running estimate -- called whenever `EditorState` itself is replaced
/// wholesale (New/Load Selected), since a different design's solve cost has nothing
/// to do with the last one's.
pub(super) fn reset_for_new_design() {
    RUNTIME.with(|cell| cell.borrow_mut().last_solve = None);
}

fn is_current(seq: u64) -> bool {
    RUNTIME.with(|cell| cell.borrow().current_seq == seq)
}

/// Called at the end of [`super::view::refresh_editor_panel_stale`] -- every edit
/// callback's stale-refresh path calls that function with a fixed, already-established
/// signature, so this hook reads everything it needs (the budget property, `state`'s
/// design/generation) rather than asking for new parameters. See
/// [`should_schedule_auto_solve`] for the eligibility rule.
pub(super) fn on_edit(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) {
    let budget_ms =
        u64::try_from(ui.global::<EditorModel>().get_auto_solve_budget_ms()).unwrap_or(0);
    let budget = Duration::from_millis(budget_ms);
    let last_solve = RUNTIME.with(|cell| cell.borrow().last_solve);

    if !should_schedule_auto_solve(last_solve, budget) {
        // Only worth a dedicated note when THIS design is the reason (its own last
        // solve exceeded a real, nonzero budget) -- a `0` budget is a deliberate,
        // global "auto-solve off" choice that the ordinary stale banner already
        // covers without needing an extra explanation.
        if !budget.is_zero()
            && let Some(last) = last_solve
        {
            ui.global::<EditorModel>()
                .set_status_text(auto_solve_off_note(last).into());
            ui.global::<EditorModel>().set_status_is_problem(true);
        }
        return;
    }

    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let design = state.design.clone();
    let generation = Arc::clone(&state.generation);
    let multi_selected = state.multi_selected.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        AUTO_SOLVE_DEBOUNCE,
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                dispatch_background_solve(
                    &ui,
                    &render_ctx,
                    design.clone(),
                    &generation,
                    multi_selected.clone(),
                );
            }
        },
    );
    RUNTIME.with(|cell| cell.borrow_mut().debounce = Some(timer));
}

/// Bundles a completed background solve's results -- kept as one struct (rather than
/// threading each value through as its own parameter) purely to keep
/// [`apply_background_solve_result`] under clippy's argument-count lint without a new
/// `#[allow]`.
struct BackgroundSolveResult {
    elapsed: Duration,
    tiers: Vec<EditorTierItem>,
    status_text: String,
    status_is_problem: bool,
    warnings: Vec<String>,
    yield_texts: (String, String, String, String),
    planes: Vec<GpuFacetPlane>,
    solved: Option<Vec<SolvedTier>>,
    /// Index-wheel tooth count and reference angle, for the diagram view's wheel.
    gear: (u32, f32),
}

/// Spawns a background solve of `design` (a snapshot taken the moment this is called)
/// plus a ticker that keeps the "Solving..." banner's elapsed time fresh, following
/// `deep_solve::spawn_deep_solve`'s own `thread::spawn` + `Weak::upgrade_in_event_loop`
/// shape. Shared by [`super::view::refresh_all`]'s large-design path and this module's
/// own debounced auto-solve dispatch from [`on_edit`] -- both just need a `Design`
/// snapshot and the `generation` counter to check it against on arrival.
///
/// See the module doc comment for exactly what the completion handler does and does
/// not apply.
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
pub(super) fn dispatch_background_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: Design,
    generation: &Arc<AtomicU64>,
    multi_selected: BTreeSet<usize>,
) {
    let tier_count = design.tiers.len();
    let started_generation = generation.load(Ordering::Relaxed);
    let seq = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.current_seq
    });

    ui.global::<EditorModel>().set_solve_running(true);
    ui.global::<EditorModel>()
        .set_status_text(solving_banner(tier_count, Duration::ZERO).into());
    ui.global::<EditorModel>().set_status_is_problem(false);

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

    let ui_weak = ui.as_weak();
    let render_ctx_for_apply = Arc::clone(render_ctx);
    let generation = Arc::clone(generation);
    thread::spawn(move || {
        let start = Instant::now();
        // The same pure `&Design` -> data conversions `refresh_editor_panel`/
        // `refresh_viewport` already call synchronously today -- moved here
        // unchanged, just off the UI thread. Each one solves `design` independently
        // (no caching added here, and none removed), matching this crate's existing
        // total work per Solve, not adding to it.
        let n_d = design.effective_refractive_index();
        let mut tiers = tier_items(&design, n_d);
        apply_multi_selection(&mut tiers, &multi_selected);
        let (status_text, status_is_problem) = status_text_and_is_problem(&design);
        let warnings = manufacturability_warning_lines(&design);
        let yield_texts = yield_report_texts(&design);
        let planes = design_to_gpu_planes(&design);
        let solved = design.solve().ok();
        let gear = (
            design.meta.gear_teeth_abs(),
            design.meta.gear_reference_angle as f32,
        );
        let result = BackgroundSolveResult {
            elapsed: start.elapsed(),
            tiers,
            status_text,
            status_is_problem,
            warnings,
            yield_texts,
            planes,
            solved,
            gear,
        };
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

/// Applies a completed background solve's result -- see the module doc comment for
/// the two staleness checks this performs before touching anything, and `mod.rs`'s
/// "Feeding the viewport" section for the `GpuFacetPlane` sign convention
/// `refresh_viewport` already documents (this mirrors it exactly, just deferred to a
/// worker-computed `Vec<GpuFacetPlane>` instead of computing it inline).
fn apply_background_solve_result(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    seq: u64,
    started_generation: u64,
    generation: &Arc<AtomicU64>,
    result: BackgroundSolveResult,
) {
    record_solve_duration(result.elapsed);
    if !is_current(seq) {
        // A newer dispatch has already taken over the banner and will apply its own
        // result in turn -- nothing here is still current enough to show.
        return;
    }
    ui.global::<EditorModel>().set_solve_running(false);
    if generation.load(Ordering::Relaxed) != started_generation {
        // The design changed after this solve was dispatched, without a newer
        // dispatch superseding it (auto-solve disabled, or this design's own budget
        // was already exceeded) -- the edit that changed it already repainted the
        // banner correctly via `refresh_editor_panel_stale`. Only the running flag
        // above needed clearing.
        return;
    }

    ui.global::<EditorModel>()
        .set_tiers(ModelRc::new(VecModel::from(result.tiers)));
    ui.global::<EditorModel>()
        .set_status_text(result.status_text.into());
    ui.global::<EditorModel>()
        .set_status_is_problem(result.status_is_problem);
    ui.global::<EditorModel>()
        .set_manufacturability_warnings(ModelRc::new(VecModel::from(
            result
                .warnings
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
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

    let planes = Arc::new(result.planes);
    let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
    ctx.active_planes = Arc::clone(&planes);
    ctx.dirty = true;
    ctx.design_gear = Some(result.gear);
    let design_gear = ctx.design_gear;
    let camera = CameraPose {
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
    };
    drop(ctx);
    let redraw_planes: Vec<(glam::Vec3, f32)> = planes
        .iter()
        .map(|p| (glam::Vec3::from(p.normal), -p.d))
        .collect();
    let size = (
        ui.global::<SolidPreviewModel>().get_viewport_width() as u32,
        ui.global::<SolidPreviewModel>().get_viewport_height() as u32,
    );
    let view_mode = ui.global::<SolidPreviewModel>().get_view_mode() as u8;
    let (preview_state, solid_last_solved) = RUNTIME.with(|cell| {
        let rt = cell.borrow();
        (rt.preview_state.clone(), rt.solid_last_solved.clone())
    });
    if let Some(preview_state) = preview_state {
        preview_state.request_redraw_with_gear(redraw_planes, camera, size, view_mode, design_gear);
    }
    if let (Some(solved), Some(cache)) = (result.solved, solid_last_solved) {
        *cache.lock().unwrap_or_else(PoisonError::into_inner) = Some(solved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- should_schedule_auto_solve ---

    #[test]
    fn zero_budget_disables_auto_solve_regardless_of_measurement() {
        assert!(!should_schedule_auto_solve(None, Duration::ZERO));
        assert!(!should_schedule_auto_solve(
            Some(Duration::from_millis(1)),
            Duration::ZERO
        ));
    }

    #[test]
    fn no_measurement_yet_is_scheduled_optimistically() {
        // A fresh design (or one just New/Loaded) has no measurement -- see this
        // function's own doc comment for why that is treated as "try it," not
        // "refuse until proven cheap."
        assert!(should_schedule_auto_solve(None, Duration::from_millis(300)));
    }

    #[test]
    fn a_measurement_under_budget_is_scheduled() {
        assert!(should_schedule_auto_solve(
            Some(Duration::from_millis(120)),
            Duration::from_millis(300)
        ));
    }

    #[test]
    fn a_measurement_at_or_over_budget_is_not_scheduled() {
        assert!(!should_schedule_auto_solve(
            Some(Duration::from_millis(300)),
            Duration::from_millis(300)
        ));
        assert!(!should_schedule_auto_solve(
            Some(Duration::from_secs(6)),
            Duration::from_millis(300)
        ));
    }

    // --- is_current / the sequence-number epoch ---

    #[test]
    fn a_freshly_recorded_sequence_number_is_current() {
        let seq = RUNTIME.with(|cell| {
            let mut rt = cell.borrow_mut();
            rt.current_seq += 1;
            rt.current_seq
        });
        assert!(is_current(seq));
    }

    #[test]
    fn an_older_sequence_number_is_superseded_by_a_newer_dispatch() {
        let old_seq = RUNTIME.with(|cell| {
            let mut rt = cell.borrow_mut();
            rt.current_seq += 1;
            rt.current_seq
        });
        // A second dispatch bumps the counter again, exactly like
        // `dispatch_background_solve` does on every call.
        RUNTIME.with(|cell| cell.borrow_mut().current_seq += 1);
        assert!(!is_current(old_seq));
    }

    // --- record_solve_duration / reset_for_new_design ---

    #[test]
    fn record_solve_duration_is_visible_to_the_next_schedule_decision() {
        record_solve_duration(Duration::from_millis(42));
        let last = RUNTIME.with(|cell| cell.borrow().last_solve);
        assert_eq!(last, Some(Duration::from_millis(42)));
        reset_for_new_design();
        let last = RUNTIME.with(|cell| cell.borrow().last_solve);
        assert_eq!(last, None);
    }

    // --- banner text ---

    #[test]
    fn solving_banner_pluralizes_the_tier_count() {
        assert!(solving_banner(1, Duration::ZERO).contains("1 tier)"));
        assert!(solving_banner(2, Duration::ZERO).contains("2 tiers)"));
    }

    #[test]
    fn auto_solve_off_note_reports_the_measured_seconds() {
        let text = auto_solve_off_note(Duration::from_millis(5900));
        assert!(text.contains("5.9s"));
    }
}
