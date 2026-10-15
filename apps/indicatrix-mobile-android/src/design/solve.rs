//! Solving from the UI thread: when a solve runs, where, and how its answer comes back.
//!
//! - A tiny design (the shared policy, `should_solve_synchronously`) is solved right on
//!   the UI thread, one event-loop turn after "Solving..." has been shown.
//! - Anything else runs on a dedicated `solve` thread (`BackgroundQueue`) with a cancel
//!   flag. A newer request supersedes an older one: the older job's flag is set, its
//!   answer is dropped by `install` (it is for a generation the design left), and the
//!   status line ticks the shared "Solving..." banner meanwhile.
//! - After an edit, [`schedule_auto`] restarts a single-shot timer (the shared debounce);
//!   it fires a solve only when the auto-solve rules allow one.
//!
//! The state lives in an `Rc<RefCell<AppState>>` on the UI thread, so the worker never
//! touches it: the job carries a copy of the design, and the answer hops back through
//! `slint::Weak::upgrade_in_event_loop`.

use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use indicatrix_editor::solve_policy::{SOLVING_TICK_INTERVAL, solving_banner};
use indicatrix_mobile_core::{
    app_state::AppState,
    design_state::{
        Installed, SolveResponse, StatusKind, begin_solve, install, install_failure, run_solve,
        should_auto_solve, should_solve_synchronously, status_line,
    },
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, Timer, TimerMode};

use super::Shared;
use crate::{AppWindow, TiersModel, push::toast};

/// How long "Solving..." gets to paint before a synchronous solve occupies the UI thread.
const PAINT_DELAY: Duration = Duration::from_millis(30);

/// The job that is running, for the status banner.
struct Running {
    generation: u64,
    started: Instant,
    tier_count: usize,
}

struct Runtime {
    /// The app state, for answers that arrive through the event loop.
    shared: Option<Shared>,
    /// The solve thread; `None` after [`shutdown`].
    queue: Option<BackgroundQueue>,
    /// The cancel flag of the newest job.
    cancel: Arc<AtomicBool>,
    running: Option<Running>,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime {
        shared: None,
        queue: Some(BackgroundQueue::new("solve")),
        cancel: Arc::new(AtomicBool::new(false)),
        running: None,
    });
    /// Debounces the automatic solve after an edit.
    static AUTO: Timer = Timer::default();
    /// Ticks the "Solving..." banner.
    static TICK: Timer = Timer::default();
}

/// Cancels a running solve and ends the solve thread. Call on the UI thread when the
/// event loop has ended: a thread-local queue left for the process's own teardown
/// would join a thread Windows has already ended, which aborts.
pub fn shutdown() {
    let queue = RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        runtime.cancel.store(true, Ordering::Relaxed);
        runtime.shared = None;
        runtime.running = None;
        runtime.queue.take()
    });
    drop(queue);
}

/// A monotonic clock for the history's coalescing windows: the time since the first call.
pub fn clock() -> Duration {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed()
}

/// Connects the Solve button and starts the first automatic solve.
pub fn wire(window: &AppWindow, state: &Shared) {
    RUNTIME.with(|runtime| runtime.borrow_mut().shared = Some(Rc::clone(state)));
    window.global::<TiersModel>().on_solve({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let solved = {
                let app = state.borrow();
                app.solve.is_current(app.session.current_generation())
            };
            if solved {
                toast::show(&window, "The design is already solved.");
            } else {
                request_solve(&window, &state);
            }
        }
    });
    schedule_auto(window, state);
}

/// Pushes the status line and the solving flag.
pub fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<TiersModel>();
    let line = status_line(state);
    let text = if line.kind == StatusKind::Solving {
        banner(state).unwrap_or(line.text)
    } else {
        line.text
    };
    model.set_status_kind(match line.kind {
        StatusKind::None => 0,
        StatusKind::Solving => 1,
        StatusKind::Solved => 2,
        StatusKind::Problem | StatusKind::Failed => 3,
        StatusKind::Stale => 4,
    });
    model.set_status_problem(matches!(
        line.kind,
        StatusKind::Problem | StatusKind::Failed
    ));
    model.set_status_text(text.into());
    model.set_solving(state.solve.is_solving());
}

/// The "Solving... (8 tiers) -- 1.3s elapsed" banner of the running job, if it is for the
/// design as it is now.
fn banner(state: &AppState) -> Option<String> {
    RUNTIME.with(|runtime| {
        let runtime = runtime.borrow();
        let running = runtime.running.as_ref()?;
        (running.generation == state.session.current_generation())
            .then(|| solving_banner(running.tier_count, running.started.elapsed()))
    })
}

/// After an edit (or a new design): cancels a job that has become outdated and
/// (re)starts the debounce for an automatic solve, if the rules allow one.
pub fn schedule_auto(window: &AppWindow, state: &Shared) {
    cancel_outdated(&state.borrow());
    let delay = should_auto_solve(&state.borrow());
    AUTO.with(|timer| match delay {
        Some(delay) => {
            let (weak, state) = (window.as_weak(), Rc::clone(state));
            timer.start(TimerMode::SingleShot, delay, move || {
                if let Some(window) = weak.upgrade() {
                    request_solve(&window, &state);
                }
            });
        }
        None => timer.stop(),
    });
}

/// Sets the cancel flag of the running job when the design has moved past it, so the
/// worker stops spending time on an answer nobody wants.
fn cancel_outdated(app: &AppState) {
    let outdated = app
        .solve
        .in_flight_generation()
        .is_some_and(|generation| generation != app.session.current_generation());
    if outdated {
        RUNTIME.with(|runtime| runtime.borrow().cancel.store(true, Ordering::Relaxed));
    }
}

/// Solves the current design now (the Solve button, or the debounce firing): inline for a
/// tiny design, otherwise on the solve thread. Does nothing when the design already has an
/// answer or a job for it is running.
pub fn request_solve(window: &AppWindow, state: &Shared) {
    let (job, synchronous, tier_count) = {
        let mut app = state.borrow_mut();
        let synchronous = should_solve_synchronously(&app);
        let tier_count = app.session.design.tiers.len();
        (begin_solve(&mut app), synchronous, tier_count)
    };
    let Some(job) = job else {
        return;
    };
    let cancel = Arc::new(AtomicBool::new(false));
    RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        // The job before this one is superseded.
        runtime.cancel.store(true, Ordering::Relaxed);
        runtime.cancel = Arc::clone(&cancel);
        runtime.running = Some(Running {
            generation: job.generation,
            started: Instant::now(),
            tier_count,
        });
    });
    start_ticker(window, state);
    super::refresh(window, state);

    let generation = job.generation;
    let weak = window.as_weak();
    if synchronous {
        let state = Rc::clone(state);
        Timer::single_shot(PAINT_DELAY, move || {
            let started = Instant::now();
            let response = run_solve(&job, &cancel);
            if let Some(window) = weak.upgrade() {
                finish(
                    &window,
                    &state,
                    generation,
                    Some(response),
                    started.elapsed(),
                );
            }
        });
    } else {
        RUNTIME.with(|runtime| {
            let runtime = runtime.borrow();
            let Some(queue) = runtime.queue.as_ref() else {
                tracing::debug!("solve not started: the app is shutting down");
                return;
            };
            queue.submit(move || {
                let started = Instant::now();
                let response = catch_unwind(AssertUnwindSafe(|| run_solve(&job, &cancel))).ok();
                let took = started.elapsed();
                let delivered = weak.upgrade_in_event_loop(move |window| {
                    if let Some(state) = shared() {
                        finish(&window, &state, generation, response, took);
                    }
                });
                if let Err(error) = delivered {
                    tracing::warn!(%error, "a solve answer was dropped: the UI thread is gone");
                }
            });
        });
    }
}

/// The app state, from the UI thread.
fn shared() -> Option<Shared> {
    RUNTIME.with(|runtime| runtime.borrow().shared.clone())
}

/// A job ended (`None`: its thread panicked): install the answer and re-push the design.
fn finish(
    window: &AppWindow,
    state: &Shared,
    generation: u64,
    response: Option<SolveResponse>,
    took: Duration,
) {
    let installed = {
        let mut app = state.borrow_mut();
        match response {
            Some(response) => install(&mut app, generation, response, took),
            None => install_failure(
                &mut app,
                generation,
                "The solver stopped unexpectedly. Try editing the design or solving again."
                    .to_owned(),
                took,
            ),
        }
    };
    if !state.borrow().solve.is_solving() {
        TICK.with(Timer::stop);
        RUNTIME.with(|runtime| runtime.borrow_mut().running = None);
    }
    if installed == Installed::Stale {
        // The design moved on while the job ran: make sure its newer state gets solved.
        schedule_auto(window, state);
    }
    super::refresh(window, state);
}

/// Repaints the status line every `SOLVING_TICK_INTERVAL` while a job runs.
fn start_ticker(window: &AppWindow, state: &Shared) {
    let (weak, state) = (window.as_weak(), Rc::clone(state));
    TICK.with(|timer| {
        timer.start(TimerMode::Repeated, SOLVING_TICK_INTERVAL, move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let app = state.borrow();
            if app.solve.is_solving() {
                push(&window, &app);
            }
        });
    });
}
