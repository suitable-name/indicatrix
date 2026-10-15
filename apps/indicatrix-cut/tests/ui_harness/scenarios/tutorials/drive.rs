//! The guide driver: walks an open guide to its end the way a learner would, one step at a time.
//!
//! Per step:
//!
//! * a reading step (`completion == "manual"`) is finished with `GuideModel.next`, the Next
//!   button's callback;
//! * a step the panel offers to do (`offers_perform`) is done with `GuideModel.go_on`, the very
//!   callback the panel's button calls (it sets `perform_busy` and runs `GuideModel.perform`).
//!   The harness then waits in REAL time, with mock time kept in step with it so the recipe's
//!   own tick budget behaves as in the app, until the step's goal is met;
//! * a step without a recipe is completed by what it asks for where the harness can do that
//!   (the palette chord, the tutorial browser), otherwise by raising the event it waits for
//!   through `GuideModel.event` (the callback every widget route ends in; reported once as a
//!   note), otherwise it is a GAP (reported, then skipped so the later steps are still tried).
//!
//! A step that only advances by skipping is always a finding or a GAP, never a pass.

use crate::support::{Ctx, app::pump, coverage};
use indicatrix_cut::{
    EditorModel, GuideModel, MainWindow, PreferencesModel, TutorialsModel, harness_support,
};
use indicatrix_editor::guide::Goal;
use slint::ComponentHandle;
use std::{
    cell::Cell,
    time::{Duration, Instant},
};

/// How long one step may take before it is a finding (a solve takes seconds).
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// The limit once an earlier step was skipped: the state may not be what the step expects.
const AFTER_SKIP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to keep waiting after the recipe gave up (a late solve result still completes the
/// step in the app).
const GIVE_UP_GRACE: Duration = Duration::from_secs(5);
/// Time for a freshly entered step to check its goal against the design.
const ENTRY_SETTLE: Duration = Duration::from_millis(120);
/// Time for a chord or a browser opening to reach the guide.
const ROUTE_SETTLE: Duration = Duration::from_millis(1500);
/// The event the palette reports when it opens.
const PALETTE_OPENED: &str = "palette_opened";
/// The event the tutorial browser reports when it opens.
const TUTORIALS_OPENED: &str = "tutorials_opened";
/// The event a written cutting sheet reports.
const SHEET_EXPORTED: &str = "sheet_exported";
/// The event the Interface Mode switch reports.
const INTERFACE_MODE_CHANGED: &str = "interface_mode_changed";
/// Time for a solve and the sheet write to reach the guide.
const SHEET_SETTLE: Duration = Duration::from_secs(30);

/// What `GuideModel` shows right now.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the GuideModel properties it reads; open, done, busy, failed, offers_perform and manual are independent flags"
)]
pub struct Snap {
    pub open: bool,
    pub id: String,
    pub index: i32,
    pub total: i32,
    pub done: bool,
    pub busy: bool,
    pub failed: bool,
    pub offers_perform: bool,
    pub manual: bool,
    pub title: String,
    pub waiting: String,
}

impl Snap {
    /// Whether step `n` (0-based) is the open guide's current step.
    fn at(&self, n: usize) -> bool {
        self.open && usize::try_from(self.index) == Ok(n)
    }
}

/// Reads `GuideModel`.
pub fn snap(ui: &MainWindow) -> Snap {
    let guide = ui.global::<GuideModel>();
    let current = guide.get_current();
    Snap {
        open: guide.get_open(),
        id: guide.get_guide_id().to_string(),
        index: guide.get_step_index(),
        total: guide.get_total_steps(),
        done: guide.get_step_done(),
        busy: guide.get_perform_busy(),
        failed: guide.get_perform_failed(),
        offers_perform: guide.get_offers_perform(),
        manual: guide.get_current_is_manual(),
        title: current.title.to_string(),
        waiting: current.waiting.to_string(),
    }
}

/// Pumps until `condition` holds or `timeout` of real time has passed. Unlike
/// `app::wait_until`, mock time advances by the real time that passed (not a fixed 10 ms per
/// round), so the guide's timers and the recipe's tick budget run at the speed they do in the
/// app while worker threads solve in real time.
pub fn wait_real(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let mut last = start;
    loop {
        let now = Instant::now();
        let ms = u64::try_from(now.duration_since(last).as_millis()).unwrap_or(10);
        last = now;
        pump(ms.clamp(5, 100));
        if condition() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What a run of [`drive_guide`] did.
#[derive(Debug, Default)]
pub struct RunStats {
    /// Steps the guide has.
    pub steps: usize,
    /// Steps finished by their goal (a recipe, or the thing the step asks for).
    pub by_goal: usize,
    /// Reading steps finished with Next.
    pub read: usize,
    /// Steps whose event was raised through `GuideModel.event` instead of its widget.
    pub injected: usize,
    /// Steps the harness could not drive (reported as GAP).
    pub gaps: usize,
    /// Steps passed with Skip (after a finding or a GAP).
    pub skipped: usize,
    /// The walk ended early because the guide was not where it should be.
    pub aborted: bool,
}

/// Walks the open guide from its current step to its end. `goals` are the guide's goals when
/// the guide is a built-in one (they tell the harness what a step without a recipe waits for);
/// `scenario` names the scenario for the findings.
pub fn drive_guide(
    ctx: &mut Ctx<'_>,
    scenario: &str,
    guide_id: &str,
    goals: Option<&[Goal]>,
) -> RunStats {
    let mut stats = RunStats::default();
    let first = snap(ctx.app.ui());
    let total = usize::try_from(first.total).unwrap_or(0);
    stats.steps = total;
    for n in 0..total {
        let before = snap(ctx.app.ui());
        let label = format!("{guide_id} step {}: {}", n + 1, before.title);
        ctx.report.set_context(format!("{scenario}: {label}"));
        if !before.at(n) {
            ctx.report.add_simple(
                "MainWindow",
                "tutorial-state",
                format!(
                    "the guide should be at step {} but shows open={} step_index={}",
                    n + 1,
                    before.open,
                    before.index
                ),
            );
            stats.aborted = true;
            break;
        }
        if n == 0 {
            ctx.sweep_main_at_sizes(&format!("{guide_id} guide panel open"));
        }
        let goal = goals.and_then(|all| all.get(n));
        drive_step(ctx, &label, n, total, &before, goal, &mut stats);
        ctx.check_health(&label);
        if stats.aborted {
            break;
        }
    }
    ctx.report.set_context(scenario.to_owned());
    stats
}

/// Finishes step `n` and checks that the guide moved on.
fn drive_step(
    ctx: &mut Ctx<'_>,
    label: &str,
    n: usize,
    total: usize,
    before: &Snap,
    goal: Option<&Goal>,
    stats: &mut RunStats,
) {
    // Was a fixed wait of `ENTRY_SETTLE`: a reading step and a step the goal already finished
    // (the guide moved on or reports it done) are decided at once, so the wait ends there; any
    // other step still gets the full time to check its goal.
    let _ = wait_real(ENTRY_SETTLE, || {
        let now = snap(ctx.app.ui());
        !now.at(n) || now.manual || now.done
    });
    let now = snap(ctx.app.ui());
    if !now.at(n) {
        // The goal held on entry and the "Done" moment already moved on.
        stats.by_goal += 1;
    } else if now.manual {
        stats.read += 1;
        press_next(ctx);
    } else if now.done {
        // The goal already held when the step was entered.
        stats.by_goal += 1;
        press_next(ctx);
    } else if now.offers_perform {
        ctx.app.ui().global::<GuideModel>().invoke_go_on();
        await_and_advance(ctx, label, n, before, stats);
        // The assertion of its effect is verify_advanced below; recorded here, after the call.
        coverage::record_callback("GuideModel.go_on");
    } else {
        drive_without_recipe(ctx, label, n, goal, before, stats);
    }
    verify_advanced(ctx, label, n, total, stats);
}

/// The Next button.
fn press_next(ctx: &Ctx<'_>) {
    ctx.app.ui().global::<GuideModel>().invoke_next();
    pump(30);
    // No assertion precedes this: verify_advanced checks the step moved on, after the call.
    coverage::record_callback("GuideModel.next");
}

/// Waits for step `n` to be done (the app advances by itself after the "Done" moment, which
/// also counts), then presses Next. A recipe that gave up, a timeout and an unsaved-changes
/// question are findings; the step is then skipped so the later steps are still tried.
fn await_and_advance(
    ctx: &mut Ctx<'_>,
    label: &str,
    n: usize,
    before: &Snap,
    stats: &mut RunStats,
) {
    let timeout = if stats.skipped > 0 {
        AFTER_SKIP_TIMEOUT
    } else {
        STEP_TIMEOUT
    };
    let started = Instant::now();
    let gave_up_at: Cell<Option<Instant>> = Cell::new(None);
    let question = Cell::new(false);
    let ui = ctx.app.ui();
    let _ = wait_real(timeout, || {
        let s = snap(ui);
        if !s.at(n) || s.done {
            return true;
        }
        if ui.global::<EditorModel>().get_unsaved_dialog_open() {
            question.set(true);
            return true;
        }
        if s.failed && !s.busy {
            let since = gave_up_at.get().unwrap_or_else(Instant::now);
            gave_up_at.set(Some(since));
            return since.elapsed() >= GIVE_UP_GRACE;
        }
        false
    });
    let end = snap(ui);
    let after_skip = if stats.skipped > 0 {
        " (an earlier step of this guide was skipped, so the state may not match)"
    } else {
        ""
    };
    let waiting = &before.waiting;
    if question.get() {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-unsaved-question",
            format!("{label}: the unsaved-changes question appeared; waiting for: {waiting}"),
        );
        ctx.app
            .ui()
            .global::<EditorModel>()
            .invoke_unsaved_dialog_discard();
        skip(ctx, stats);
    } else if !end.at(n) || end.done {
        stats.by_goal += 1;
        if gave_up_at.get().is_some() {
            ctx.report.add_simple(
                "MainWindow",
                "tutorial-perform-gave-up",
                format!(
                    "{label}: the recipe gave up (perform_failed) before its goal was met; \
                     the step was done after {} ms. Waiting for: {waiting}",
                    started.elapsed().as_millis()
                ),
            );
        }
        if end.at(n) {
            press_next(ctx);
        }
    } else {
        let why = if gave_up_at.get().is_some() {
            "the recipe ran, gave up (perform_failed) and the goal was never met"
        } else {
            "the goal was not met in time"
        };
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-step-not-completed",
            format!(
                "{label}: {why} after {} ms. Waiting for: {waiting}{after_skip}",
                started.elapsed().as_millis()
            ),
        );
        skip(ctx, stats);
    }
}

/// "Skip step": Next on an unfinished step.
fn skip(ctx: &Ctx<'_>, stats: &mut RunStats) {
    stats.skipped += 1;
    press_next(ctx);
}

/// A step with no recipe: do what it asks where the harness can, else raise its event, else GAP.
fn drive_without_recipe(
    ctx: &mut Ctx<'_>,
    label: &str,
    n: usize,
    goal: Option<&Goal>,
    before: &Snap,
    stats: &mut RunStats,
) {
    let Some(goal) = goal else {
        gap(
            ctx,
            label,
            "a step without a recipe in a generated lesson (no goal information)",
            stats,
        );
        return;
    };
    if matches!(goal, Goal::YieldApplied) {
        apply_yield_inputs(ctx);
        await_and_advance(ctx, label, n, before, stats);
        return;
    }
    if !event_only(goal) {
        let why = format!(
            "goal {} has no recipe and cannot be driven",
            goal_kind(goal)
        );
        gap(ctx, label, &why, stats);
        return;
    }
    let events: Vec<String> = goal.events().into_iter().map(str::to_owned).collect();
    for name in &events {
        drive_event(ctx, name, events.len() == 1, stats);
    }
    await_and_advance(ctx, label, n, before, stats);
}

/// The recipe of a `YieldApplied` step (the guide has none of its own: the learner types the
/// stone width into the Preform tab): sets the girdle diameter and presses Apply Yield Inputs
/// through the callback the button calls.
fn apply_yield_inputs(ctx: &Ctx<'_>) {
    const GIRDLE_DIAMETER_MM: &str = "6.5";
    let editor = ctx.app.ui().global::<EditorModel>();
    editor.set_girdle_diameter_mm(GIRDLE_DIAMETER_MM.into());
    editor.invoke_apply_yield_inputs(
        GIRDLE_DIAMETER_MM.into(),
        0,
        editor.get_specific_gravity_override(),
    );
    pump(100);
    if !editor.get_design_girdle_diameter_text().is_empty() {
        coverage::record_callback("EditorModel.apply_yield_inputs");
    }
}

/// Records a GAP and skips the step.
fn gap(ctx: &mut Ctx<'_>, label: &str, why: &str, stats: &mut RunStats) {
    stats.gaps += 1;
    ctx.report.note(format!("GAP: {label} -- {why}"));
    skip(ctx, stats);
}

/// Whether the goal is made of events only.
fn event_only(goal: &Goal) -> bool {
    match goal {
        Goal::Event(_) => true,
        Goal::All(goals) | Goal::Any(goals) => goals.iter().all(event_only),
        _ => false,
    }
}

/// A short name for a goal kind, for the report.
const fn goal_kind(goal: &Goal) -> &'static str {
    match goal {
        Goal::Manual => "Manual",
        Goal::Event(_) => "Event",
        Goal::TierMatches { .. } => "TierMatches",
        Goal::TierExists(_) => "TierExists",
        Goal::TierCountAtLeast(_) => "TierCountAtLeast",
        Goal::Material(_) => "Material",
        Goal::SolvedClosed => "SolvedClosed",
        Goal::YieldApplied => "YieldApplied",
        Goal::ViewMode(_) => "ViewMode",
        Goal::InspectorTab(_) => "InspectorTab",
        Goal::FreshDesign { .. } => "FreshDesign",
        Goal::ConcaveTiersAtLeast(_) => "ConcaveTiersAtLeast",
        Goal::DesignRebuilt { .. } => "DesignRebuilt",
        Goal::Check { label, .. } => label,
        Goal::All(_) => "All",
        Goal::Any(_) => "Any",
    }
}

/// Raises event `name` the way the UI does where the harness can drive that route (`single` is
/// whether it is the only event the step waits for, so the route can be judged), else through
/// `GuideModel.event`.
fn drive_event(ctx: &mut Ctx<'_>, name: &str, single: bool, stats: &mut RunStats) {
    match name {
        PALETTE_OPENED => {
            ctx.press("Ctrl+K");
            let done = wait_real(ROUTE_SETTLE, || snap(ctx.app.ui()).done);
            ctx.press("Escape");
            if !done && single {
                ctx.report.add_simple(
                    "MainWindow",
                    "tutorial-event-route",
                    "Ctrl+K did not report palette_opened to the guide",
                );
                inject(ctx, name, stats);
            }
        }
        TUTORIALS_OPENED => {
            ctx.app
                .ui()
                .global::<TutorialsModel>()
                .invoke_open_browser();
            let done = wait_real(ROUTE_SETTLE, || snap(ctx.app.ui()).done);
            if done {
                coverage::record_callback("TutorialsModel.open_browser");
            }
            ctx.app
                .ui()
                .global::<TutorialsModel>()
                .invoke_close_browser();
            pump(50);
            // No assertion precedes this: the browser's closed state is not read back.
            coverage::record_callback("TutorialsModel.close_browser");
            if !done && single {
                ctx.report.add_simple(
                    "MainWindow",
                    "tutorial-event-route",
                    "opening the tutorial browser did not report tutorials_opened",
                );
                inject(ctx, name, stats);
            }
        }
        SHEET_EXPORTED => {
            // The Export menu's cutting sheet: the picker answer first, then the callback the
            // menu item calls; the sheet is written on a worker and reported when it lands.
            let path = ctx.scratch_file("tutorial_cutting_sheet.html");
            harness_support::queue_picker_path(path.clone());
            ctx.app
                .ui()
                .global::<EditorModel>()
                .invoke_export_cutting_sheet();
            let done = wait_real(SHEET_SETTLE, || path.exists() && snap(ctx.app.ui()).done);
            if done {
                // The sheet exists and the guide saw the event.
                coverage::record_callback("EditorModel.export_cutting_sheet");
            } else {
                harness_support::clear_picker_answers();
                if single {
                    ctx.report.add_simple(
                        "MainWindow",
                        "tutorial-event-route",
                        "Export Cutting Sheet did not report sheet_exported to the guide",
                    );
                }
                inject(ctx, name, stats);
            }
        }
        INTERFACE_MODE_CHANGED => {
            // The Preferences switch: flips the mode and fires the callback the switch calls.
            let prefs = ctx.app.ui().global::<PreferencesModel>();
            let simple = !prefs.get_simple_mode();
            prefs.set_simple_mode(simple);
            prefs.invoke_simple_mode_changed(simple);
            let done = wait_real(ROUTE_SETTLE, || snap(ctx.app.ui()).done);
            if done {
                coverage::record_callback("PreferencesModel.simple_mode_changed");
            } else {
                if single {
                    ctx.report.add_simple(
                        "MainWindow",
                        "tutorial-event-route",
                        "switching the interface mode did not report interface_mode_changed",
                    );
                }
                inject(ctx, name, stats);
            }
        }
        _ => inject(ctx, name, stats),
    }
}

/// Raises `name` through `GuideModel.event`.
fn inject(ctx: &mut Ctx<'_>, name: &str, stats: &mut RunStats) {
    stats.injected += 1;
    ctx.report.note(format!(
        "tutorial event `{name}` is raised through GuideModel.event (the callback its widget \
         route ends in); the widget route itself is not driven"
    ));
    ctx.app
        .ui()
        .global::<GuideModel>()
        .invoke_event(name.into());
    pump(30);
    // No assertion precedes this: the step's advance is asserted by verify_advanced afterwards.
    coverage::record_callback("GuideModel.event");
}

/// After step `n` was finished: the guide is at step `n + 1`, or closed after the last step.
fn verify_advanced(ctx: &mut Ctx<'_>, label: &str, n: usize, total: usize, stats: &mut RunStats) {
    pump(30);
    let s = snap(ctx.app.ui());
    let advanced = if n + 1 < total { s.at(n + 1) } else { !s.open };
    if !advanced {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-advance",
            format!(
                "{label}: after finishing the step the guide shows open={} step_index={} \
                 (expected {})",
                s.open,
                s.index,
                if n + 1 < total {
                    format!("step {}", n + 2)
                } else {
                    "closed".to_owned()
                }
            ),
        );
        stats.aborted = true;
    }
}
