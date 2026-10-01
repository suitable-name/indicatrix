//! Solving the current design, cached per generation.
//!
//! # Where a solve runs
//!
//! - **Tiny designs on the page**, like the desktop: `solve_policy::
//!   should_solve_synchronously_for` (few planes, few meet-derived tiers, no tier
//!   targets, and a last solve under 500 ms if there is one) solves right here, one
//!   event-loop turn after "Solving..." is shown.
//! - **Everything else in the solve Worker** (`crate::workers::pool`'s `SolveClient`),
//!   so the page stays responsive. A newer request supersedes an older one (the client
//!   resolves the old future with an error, which is ignored here); the status strip
//!   ticks the desktop's `solving_banner` meanwhile.
//!
//! Both paths run the same `indicatrix_web_core::solve::solve_design` (the Worker
//! through `handle_solve`), and [`install`] turns its `SolveOutcome` into
//! [`SolveState`] exactly as the first main-thread path did: the same status line and
//! problem flag, plus the viewport planes the renderer traces and the tier-tagged
//! manufacturability warnings the tier table tints its rows with.
//!
//! Callers waiting in [`with_solved`] for an older generation are carried over to the
//! newer job, so they always get the CURRENT design's answer.
//!
//! # A solve Worker that keeps failing
//!
//! A timeout, a crash or a missing Worker is not the design's answer, so it is not cached,
//! and the Render tab starts a solve whenever the design has none. To keep that from
//! repeating forever, such failures are counted per design generation: the renderer is told
//! to retry after a growing pause ([`RETRY_DELAYS`]), and after [`MAX_SOLVE_ATTEMPTS`]
//! failures the failure becomes the generation's answer ([`gave_up`]) until the design
//! changes or the Solve button ([`forget_failures`]) asks again.

use super::{
    Ctx, coalesce_now,
    push::{MessageKind, push_solve_status, show_message},
    state::SolveState,
};
use crate::AppModel;
use indicatrix::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use indicatrix_editor::solve_policy::{
    AUTO_SOLVE_DEBOUNCE, SOLVING_TICK_INTERVAL, SolveCostEstimate, auto_solve_off_note,
    should_schedule_auto_solve, should_solve_synchronously_for, solving_banner,
};
use indicatrix_web_core::{
    solve::{
        SolveOutcome, SolveRequest, SolveResponse, design_to_toml, solve_design, to_solved_tiers,
    },
    solve_error::SolveError,
};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::RefCell, sync::atomic::AtomicBool, time::Duration};

/// Called with the current design's solved tiers (or why there are none).
type Waiter = Box<dyn FnOnce(&Ctx, Result<Vec<SolvedTier>, String>)>;

/// How many times a design's solve may fail for want of a working solve Worker.
///
/// A timeout, a crash or no Worker at all; after this many the failure is kept as the
/// design's answer until it changes. Without the limit the Render tab, which starts a
/// solve whenever it has none, would start one forever.
const MAX_SOLVE_ATTEMPTS: u32 = 3;

/// The pause before the Render tab may start the solve again after its first and second
/// failures.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

/// The solve in progress.
struct InFlight {
    generation: u64,
    started: Duration,
    tier_count: usize,
    waiters: Vec<Waiter>,
}

#[derive(Default)]
struct Runtime {
    in_flight: Option<InFlight>,
    /// Ticks the "Solving..." banner while a Worker solve runs.
    ticker: Timer,
    /// Debounces [`auto_solve`] (`AUTO_SOLVE_DEBOUNCE`).
    debounce: Timer,
    /// Waits out the pause before the renderer is told a failed solve may be retried.
    retry: Timer,
    /// `(generation, failed attempts)` of the design generation whose solve keeps failing.
    failures: Option<(u64, u32)>,
    /// `(generation, why)` once [`MAX_SOLVE_ATTEMPTS`] failed for that generation: its
    /// answer until the design changes or [`forget_failures`] (the Solve button).
    gave_up: Option<(u64, String)>,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
}

/// What a dispatch needs from the state, read in one short borrow.
struct Snapshot {
    generation: u64,
    tier_count: usize,
    sync: bool,
}

/// The cached answer for the current generation, if there is one.
fn cached_result(ctx: &Ctx) -> Option<Result<Vec<SolvedTier>, String>> {
    let app = ctx.state.borrow();
    match &app.design {
        None => Some(Err("No design loaded.".to_string())),
        Some(design) if design.session.design.tiers.is_empty() => Some(Ok(Vec::new())),
        Some(design) => {
            let current = design.session.current_generation();
            if let Some(message) = gave_up_message(current) {
                return Some(Err(message));
            }
            match &app.solve {
                SolveState::Solved {
                    generation, tiers, ..
                } if *generation == current => Some(Ok(tiers.clone())),
                SolveState::Failed {
                    generation,
                    message,
                } if *generation == current => Some(Err(message.clone())),
                _ => None,
            }
        }
    }
}

/// Why the solve of `generation` was given up on after [`MAX_SOLVE_ATTEMPTS`] failures, if
/// it was.
fn gave_up_message(generation: u64) -> Option<String> {
    RUNTIME.with(|rt| {
        rt.borrow()
            .gave_up
            .as_ref()
            .filter(|(gave_up, _)| *gave_up == generation)
            .map(|(_, message)| format!("{message} (tried {MAX_SOLVE_ATTEMPTS} times)"))
    })
}

/// Why the current design's solve was given up on, if it was: the renderer shows this
/// instead of waiting for a solve nothing will start.
#[must_use]
pub fn gave_up(ctx: &Ctx) -> Option<String> {
    let generation = ctx.state.borrow().generation()?;
    gave_up_message(generation)
}

/// Forgets the failed attempts (the Solve button): the next request tries again from the
/// first attempt.
pub fn forget_failures() {
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.failures = None;
        rt.gave_up = None;
        rt.retry.stop();
    });
}

/// Counts a solve that failed for want of a working Worker; the pause before the renderer
/// may try again, or `None` when it has now failed [`MAX_SOLVE_ATTEMPTS`] times.
fn count_failure(generation: u64, message: &str) -> Option<Duration> {
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        let attempts = match rt.failures {
            Some((failed, count)) if failed == generation => count + 1,
            _ => 1,
        };
        rt.failures = Some((generation, attempts));
        if attempts >= MAX_SOLVE_ATTEMPTS {
            rt.gave_up = Some((generation, message.to_string()));
            None
        } else {
            RETRY_DELAYS.get(attempts as usize - 1).copied()
        }
    })
}

/// The last measured solve time, whatever generation it was for (the desktop's
/// "last real measurement").
fn last_solve_time(ctx: &Ctx) -> Option<Duration> {
    match &ctx.state.borrow().solve {
        SolveState::Solved { took, .. } => Some(*took),
        _ => None,
    }
}

fn snapshot(ctx: &Ctx) -> Option<Snapshot> {
    let last = last_solve_time(ctx);
    let app = ctx.state.borrow();
    let design = app.design.as_ref()?;
    let tiers = &design.session.design.tiers;
    let cost = SolveCostEstimate::of(&design.session.design);
    Some(Snapshot {
        generation: design.session.current_generation(),
        tier_count: tiers.len(),
        sync: should_solve_synchronously_for(cost, last),
    })
}

/// Runs `then` with the current design's solved tiers: at once when the cached solve
/// matches the current generation (or the design has no tiers, which needs no
/// solve), otherwise when the solve it starts (or joins) finishes -- see the module
/// doc comment.
pub fn with_solved(ctx: &Ctx, then: impl FnOnce(&Ctx, Result<Vec<SolvedTier>, String>) + 'static) {
    if let Some(result) = cached_result(ctx) {
        then(ctx, result);
        return;
    }
    dispatch(ctx, Some(Box::new(then)));
}

/// Whether a solve for the current generation is already running.
#[must_use]
pub fn is_solving(ctx: &Ctx) -> bool {
    let current = ctx.state.borrow().generation();
    RUNTIME.with(|rt| {
        rt.borrow()
            .in_flight
            .as_ref()
            .is_some_and(|f| Some(f.generation) == current)
    })
}

/// Starts a solve of the current generation unless one is running; `waiter` (if any)
/// joins it. Waiters of an older generation's job move to the new one.
fn dispatch(ctx: &Ctx, waiter: Option<Waiter>) {
    let Some(snap) = snapshot(ctx) else {
        if let Some(waiter) = waiter {
            waiter(ctx, Err("No design loaded.".to_string()));
        }
        return;
    };
    let joined = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        let mut waiter = waiter;
        if let Some(in_flight) = rt.in_flight.as_mut()
            && in_flight.generation == snap.generation
        {
            if let Some(waiter) = waiter.take() {
                in_flight.waiters.push(waiter);
            }
            return true;
        }
        let mut waiters = rt
            .in_flight
            .take()
            .map(|old| old.waiters)
            .unwrap_or_default();
        waiters.extend(waiter);
        rt.in_flight = Some(InFlight {
            generation: snap.generation,
            started: coalesce_now(),
            tier_count: snap.tier_count,
            waiters,
        });
        false
    });
    if joined {
        return;
    }
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<AppModel>()
            .set_solve_status(solving_banner(snap.tier_count, Duration::ZERO).into());
        ui.global::<AppModel>().set_solve_problem(false);
    }
    if snap.sync {
        let ctx = ctx.clone();
        // One event-loop turn, so "Solving..." paints before the page is busy.
        Timer::single_shot(Duration::from_millis(30), move || {
            solve_here(&ctx, snap.generation);
        });
    } else {
        solve_in_worker(ctx, snap.generation);
    }
}

/// The tiny-design path: `solve_design` on the page.
fn solve_here(ctx: &Ctx, generation: u64) {
    let started = coalesce_now();
    let response = {
        let app = ctx.state.borrow();
        app.design
            .as_ref()
            .filter(|design| design.session.current_generation() == generation)
            .map(|design| solve_design(&design.session.design, &AtomicBool::new(false)))
    };
    let Some(response) = response else {
        // Edited meanwhile: the newer generation is solved instead.
        redispatch_stale(ctx, generation);
        return;
    };
    let took = coalesce_now().saturating_sub(started);
    finish(ctx, generation, Ok(response), took);
}

/// The Worker path: the design as native TOML to the solve Worker.
fn solve_in_worker(ctx: &Ctx, generation: u64) {
    let toml = {
        let app = ctx.state.borrow();
        app.design.as_ref().map_or_else(
            || Err("No design loaded.".to_string()),
            |d| design_to_toml(&d.session.design),
        )
    };
    let started = toml.and_then(|toml| crate::workers::pool().map(|pool| (toml, pool)));
    let (toml, pool) = match started {
        Ok(pair) => pair,
        Err(error) => {
            finish(
                ctx,
                generation,
                Err(SolveError::Failed(error)),
                Duration::ZERO,
            );
            return;
        }
    };
    let future = pool.solve().solve(toml, SolveRequest::Solve);
    start_ticker(ctx);
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await;
        let started = RUNTIME.with(|rt| {
            rt.borrow()
                .in_flight
                .as_ref()
                .filter(|f| f.generation == generation)
                .map(|f| f.started)
        });
        // Superseded by a newer job: that job owns the waiters now.
        let Some(started) = started else {
            return;
        };
        finish(
            &ctx,
            generation,
            result,
            coalesce_now().saturating_sub(started),
        );
    });
}

/// Updates the "Solving..." banner every `SOLVING_TICK_INTERVAL` while a solve runs.
fn start_ticker(ctx: &Ctx) {
    let weak = ctx.ui.clone();
    RUNTIME.with(|rt| {
        rt.borrow()
            .ticker
            .start(TimerMode::Repeated, SOLVING_TICK_INTERVAL, move || {
                let banner = RUNTIME.with(|rt| {
                    rt.borrow().in_flight.as_ref().map(|f| {
                        solving_banner(f.tier_count, coalesce_now().saturating_sub(f.started))
                    })
                });
                if let (Some(ui), Some(banner)) = (weak.upgrade(), banner) {
                    ui.global::<AppModel>().set_solve_status(banner.into());
                }
            });
    });
}

/// The job for `generation` ended because the design changed first: its waiters move
/// to a solve of the current generation.
fn redispatch_stale(ctx: &Ctx, generation: u64) {
    let waiters = take_in_flight(generation);
    if waiters.is_empty() {
        push_status(ctx);
        return;
    }
    for waiter in waiters {
        with_solved(ctx, waiter);
    }
}

/// Removes the in-flight job for `generation`, returning its waiters.
fn take_in_flight(generation: u64) -> Vec<Waiter> {
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        if rt
            .in_flight
            .as_ref()
            .is_some_and(|f| f.generation == generation)
        {
            rt.ticker.stop();
            rt.in_flight.take().map(|f| f.waiters).unwrap_or_default()
        } else {
            Vec::new()
        }
    })
}

fn push_status(ctx: &Ctx) {
    if let Some(ui) = ctx.ui.upgrade() {
        push_solve_status(&ui, &ctx.state.borrow());
    }
}

/// A job ended: install its outcome, tell the waiters and the renderer.
fn finish(ctx: &Ctx, generation: u64, result: Result<SolveResponse, SolveError>, took: Duration) {
    let waiters = take_in_flight(generation);
    if result.is_ok() {
        RUNTIME.with(|rt| rt.borrow_mut().failures = None);
    }
    // The renderer hears of the end at once, unless the Worker failed and the retry
    // waits out a pause (see `count_failure`).
    let mut pause = Duration::ZERO;
    let answer: Result<Vec<SolvedTier>, String> = match result {
        Ok(SolveResponse::Solved(outcome)) => install(ctx, generation, outcome, took),
        Ok(SolveResponse::InvalidDesign { message }) => {
            let message = format!("The solve worker could not read the design: {message}");
            ctx.state.borrow_mut().solve = SolveState::Failed {
                generation,
                message: message.clone(),
            };
            Err(message)
        }
        Ok(SolveResponse::Cancelled) => Err("The solve was cancelled.".to_string()),
        // Answers to requests that are not sent from here (Optimize, Retarget; the
        // metrics and tilt jobs go to the analysis Worker).
        Ok(
            SolveResponse::Optimized(_)
            | SolveResponse::AnalysisFailed { .. }
            | SolveResponse::Retargeted(_)
            | SolveResponse::Metrics(_)
            | SolveResponse::TiltCurves(_),
        ) => Err("The solve worker answered a different request.".to_string()),
        // Replaced by a newer request, or cancelled with the Worker that ran it: not a
        // failure, and whoever cancelled it starts what they want next.
        Err(error @ (SolveError::Superseded | SolveError::Cancelled)) => Err(error.to_string()),
        // A timeout, a crashed Worker or no Worker at all: say so, but do not cache it as
        // the design's answer -- the next request tries again, after a pause, and after
        // `MAX_SOLVE_ATTEMPTS` failures the failure becomes the answer.
        Err(SolveError::Failed(message)) => {
            let delay = count_failure(generation, &message);
            pause = delay.unwrap_or(Duration::ZERO);
            let note = if delay.is_some() {
                format!("Solve failed: {message}")
            } else {
                format!(
                    "Solve failed {MAX_SOLVE_ATTEMPTS} times; giving up until the design \
                     changes: {message}"
                )
            };
            show_message(ctx, MessageKind::Error, &note);
            Err(message)
        }
    };
    push_status(ctx);
    let current = ctx.state.borrow().generation() == Some(generation);
    if current {
        for waiter in waiters {
            waiter(ctx, answer.clone());
        }
    } else {
        // Edited while solving: the waiters want the current design.
        for waiter in waiters {
            with_solved(ctx, waiter);
        }
    }
    if pause.is_zero() {
        crate::render::solve_finished(ctx);
    } else {
        let c = ctx.clone();
        RUNTIME.with(|rt| {
            rt.borrow()
                .retry
                .start(TimerMode::SingleShot, pause, move || {
                    crate::render::solve_finished(&c);
                });
        });
    }
    // The "Solve and check" step's goal reads this result (the desktop's auto-solve
    // completion does the same).
    crate::editor::guide::check_progress(ctx);
}

/// Stores a finished solve in [`SolveState`], stamped with `generation`.
fn install(
    ctx: &Ctx,
    generation: u64,
    outcome: SolveOutcome,
    took: Duration,
) -> Result<Vec<SolvedTier>, String> {
    let SolveOutcome {
        solved,
        error,
        status_text,
        status_is_problem,
        planes,
        too_many_planes,
        warnings,
        ..
    } = outcome;
    let answer = {
        let mut app = ctx.state.borrow_mut();
        if let Some(solved) = solved {
            let tiers = to_solved_tiers(&solved);
            app.solve = SolveState::Solved {
                generation,
                tiers: tiers.clone(),
                status: status_text,
                problem: status_is_problem,
                took,
                planes: planes.into_iter().map(GpuFacetPlane::from).collect(),
                warnings,
            };
            Ok(tiers)
        } else {
            let message = error.unwrap_or(status_text);
            app.solve = SolveState::Failed {
                generation,
                message: message.clone(),
            };
            Err(message)
        }
    };
    if too_many_planes {
        // The Worker already confirmed the cap (`too_many_planes_message` is `Some`);
        // this re-check fails at the plane count, before any solving.
        let text = ctx.state.borrow().design.as_ref().and_then(|d| {
            indicatrix_editor::solve_policy::too_many_planes_message(&d.session.design)
        });
        if let Some(text) = text {
            show_message(ctx, MessageKind::Warning, &text);
        }
    }
    answer
}

/// After a load, New, edit or undo/redo: schedules a solve after the desktop's
/// `AUTO_SOLVE_DEBOUNCE`, when the desktop would (`should_schedule_auto_solve` with the
/// dock's budget, [`crate::app::state::WebApp::auto_solve_budget_ms`]; a design that must
/// solve on the page is always small enough). Otherwise the status strip says auto-solve
/// is off for this design.
///
/// A budget of zero (the dock's "Auto-solve: Off") turns all of it off: an edited design
/// then stays "stale" until Solve is pressed.
pub fn auto_solve(ctx: &Ctx) {
    let (needs, budget_ms) = {
        let app = ctx.state.borrow();
        (
            app.design
                .as_ref()
                .is_some_and(|d| !d.session.design.tiers.is_empty())
                && app.current_solved().is_none(),
            app.auto_solve_budget_ms,
        )
    };
    let enabled = budget_ms > 0;
    if !needs {
        push_status(ctx);
        return;
    }
    if !enabled {
        RUNTIME.with(|rt| rt.borrow().debounce.stop());
        push_status(ctx);
        if let Some(ui) = ctx.ui.upgrade() {
            ui.global::<AppModel>()
                .set_solve_status("Auto-solve is off -- press Solve to update.".into());
        }
        return;
    }
    let last = last_solve_time(ctx);
    let sync = snapshot(ctx).is_some_and(|s| s.sync);
    if !sync && !should_schedule_auto_solve(last, Duration::from_millis(u64::from(budget_ms))) {
        push_status(ctx);
        if let (Some(ui), Some(last)) = (ctx.ui.upgrade(), last) {
            ui.global::<AppModel>()
                .set_solve_status(auto_solve_off_note(last).into());
        }
        return;
    }
    push_status(ctx);
    let c = ctx.clone();
    RUNTIME.with(|rt| {
        rt.borrow()
            .debounce
            .start(TimerMode::SingleShot, AUTO_SOLVE_DEBOUNCE, move || {
                if cached_result(&c).is_none() {
                    dispatch(&c, None);
                }
            });
    });
}
