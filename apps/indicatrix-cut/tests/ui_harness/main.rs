//! Desktop UI harness for `indicatrix-cut` (custom `main`, `harness = false`), on the published
//! `slint-harness` crate.
//!
//! Drives the real `MainWindow` with all its Rust wiring on Slint's testing backend, inside a
//! throw-away profile, and collects every problem into `target/ui-harness/report.md` (plus
//! `report.json` next to it). The process exits non-zero when anything was found.
//!
//! Run: `cargo ui-harness` (see `.cargo/config.toml`), or
//! `cargo test -p indicatrix-cut --features ui-harness --test ui_harness` with the environment
//! variable `SLINT_EMIT_DEBUG_INFO=1` set (the element query API needs Slint debug info).
//! Extra arguments that do not start with `-` filter scenarios by name substring.
//!
//! Run order (the crate runner): profile (env redirect, working directory, guarded paths) ->
//! testing backend -> scenarios (each on a freshly relaunched app) -> coverage gate -> teardown
//! -> report. The app lives across scenarios in `SharedApp`; every scenario relaunches it with
//! its own settings seed, so there is no separate pre-scenario launch: a first launch that fails
//! is the first scenario's `launch` finding (`<scenario>: cannot start: ...`).

mod coverage_allow;
mod scenarios;
mod support;

use slint_harness::{
    Harness, Jobs,
    backend::require_debug_info,
    coverage::{Coverage, GateRule},
    profile::{Profile as HarnessProfile, request_abort},
    report::{Finding, Format},
    take_panics,
};
use std::{
    cell::{Cell, RefCell},
    process::ExitCode,
    rc::Rc,
};
use support::{
    Ctx, HarnessApp,
    app::{DEFAULT_SIZE, MINIMUM_SIZE},
    profile::print_shutdown_summary,
    sweep_options,
};

/// The app, shared by the scenario closures and the teardown (one UI thread).
type SharedApp = Rc<RefCell<HarnessApp>>;

/// A scenario body as the crate runs it, spelled out so a closure is inferred for every lifetime
/// of the crate context.
type ScenarioBody = dyn FnMut(&mut slint_harness::Ctx<'_>) -> Result<(), Finding>;

fn main() -> ExitCode {
    // Every process (the parent and each worker runs this `main`): the live viewport traces
    // nothing, see `disable_viewport_rendering`.
    indicatrix_cut::harness_support::disable_viewport_rendering();
    let harness = Harness::new()
        .profile(
            HarnessProfile::isolated()
                .name_prefix("indicatrix-ui-harness")
                .guard_paths(|| {
                    vec![
                        indicatrix_cut::harness_support::settings_path(),
                        indicatrix_cut::harness_support::database_path(),
                    ]
                }),
        )
        .sizes([DEFAULT_SIZE, MINIMUM_SIZE])
        // Most of a run is waiting on the app's threads and on timeouts, not CPU: worker
        // processes (each its own profile) run scenarios side by side while cores are idle.
        // `SLINT_HARNESS_JOBS=serial` restores the one-process run (e.g. to compare against a
        // serial baseline); `SLINT_HARNESS_JOBS_MAX` caps the workers.
        .jobs(Jobs::Auto)
        .sweep_options(sweep_options())
        .coverage(
            Coverage::scan("ui")
                .allow(&allow_list())
                .gate_when(GateRule::Armed)
                .label_props(&["reader_label", "slider_label"])
                .reader_props(&["reader_label", "slider_label"])
                .callback_hint(
                    "add a table row in scenarios/coverage/ or an allow-list entry with a reason",
                ),
        )
        .report_dir(slint_harness::target_dir!().join("ui-harness"))
        .formats([Format::Markdown]);

    let app: SharedApp = Rc::new(RefCell::new(HarnessApp::empty()));
    let launched = Rc::new(Cell::new(false));
    let mut harness = harness;
    for piece in scenarios::all() {
        harness = register(harness, &app, &launched, piece);
    }
    let teardown_app = Rc::clone(&app);
    harness
        .teardown(move || {
            // Shuts the app down (render thread, settings writer, design library handle), then
            // prints what `Profile::finish` used to: shutdowns that left handles alive and moves
            // to a fresh working directory because the database was still locked.
            teardown_app.borrow_mut().shutdown();
            print_shutdown_summary();
        })
        .run()
}

/// The coverage allow list: `coverage_allow::ALLOW`.
fn allow_list() -> Vec<(&'static str, &'static str)> {
    coverage_allow::ALLOW.to_vec()
}

/// Registers one piece: relaunch the app with the piece's settings seed, run it, check the
/// health once at its end. The crate does begin/end, `catch_unwind` and the `scenario-panic`
/// finding. A `last` piece is registered with `Harness::scenario_last` (it runs after every
/// ordinary one and, in a parallel run, sees the coverage of all workers).
fn register(
    harness: Harness,
    app: &SharedApp,
    launched: &Rc<Cell<bool>>,
    piece: scenarios::Piece,
) -> Harness {
    let scenarios::Piece {
        name,
        seed: seed_of,
        run,
        last,
        part,
        needed,
    } = piece;
    let registered = name.clone();
    let app = Rc::clone(app);
    let launched = Rc::clone(launched);
    // Boxed with the scenario signature spelled out, so the closure is inferred for every lifetime
    // of the crate context (it is registered through one of two methods below).
    let body: Box<ScenarioBody> = Box::new(move |cx| {
        // A piece that has nothing to do (a coverage chunk whose callbacks were all driven by
        // other pieces) costs no relaunch.
        if needed.as_ref().is_some_and(|needed| !needed()) {
            return Ok(());
        }
        let seed = seed_of();
        let mut guard = app.borrow_mut();
        if let Err(e) = guard.relaunch(cx.profile(), &seed) {
            return Err(Finding::simple(
                "launch",
                format!("{name}: cannot start: {e}"),
            ));
        }
        if !launched.get() {
            launched.set(true);
            // The element API needs Slint debug info; without it every query would silently
            // return nothing. Ends the run after this scenario, profile deleted.
            if let Err(message) = require_debug_info(&guard.main_view().root) {
                request_abort(message);
            }
        }
        let _ = take_panics();
        // The toast readers serve the crate's own end-of-scenario `check_health`; they see
        // nothing while a scenario holds the app (try_borrow), so the in-scenario checks stay
        // `Ctx::check_health`'s.
        let reader = Rc::clone(&app);
        cx.on_error_notice("MainWindow", move || {
            reader.try_borrow().ok().and_then(|a| a.error_notice())
        });
        let clearer = Rc::clone(&app);
        cx.clear_notice_with(move || {
            if let Ok(a) = clearer.try_borrow() {
                a.clear_notice();
            }
        });
        let (report, profile) = cx.parts();
        let mut ctx = Ctx {
            app: &mut guard,
            profile,
            report,
            seed,
        };
        support::part::set(part);
        run(&mut ctx);
        support::part::set(None);
        ctx.check_health("end of scenario");
        // Reported once above; hide the toast so the crate's own end-of-scenario check (which
        // reads it after this closure) does not report the same finding a second time.
        guard.clear_notice();
        Ok(())
    });
    if last {
        harness.scenario_last(registered, body)
    } else {
        harness.scenario(registered, body)
    }
}
