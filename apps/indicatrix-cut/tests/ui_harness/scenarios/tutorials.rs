//! Scenarios: tutorials finished end to end.
//!
//! * `tutorial:<id>` -- a built-in tutorial is started through the browser's own callback
//!   (`TutorialsModel.start`), every step is completed by its goal (see `drive`), and the
//!   guide closes, is recorded as done in the browser and leaves no error toast, panic or file
//!   dialog behind. Five are chosen per run by a seeded shuffle (the Welcome tour and the worked
//!   example always among them); `INDICATRIX_HARNESS_TUTORIALS=all` runs every built-in one.
//! * `build:<design>` -- up to five library designs are turned into a "Build this design"
//!   lesson (`BuildDesignModel.start_selected`) and the lesson is finished the same way; the
//!   built design must match the original (tier rows, closed solid). Designs come from the
//!   fixture library, or from a copy of the database `INDICATRIX_HARNESS_LIBRARY` names.
//!
//! Environment: `INDICATRIX_HARNESS_SEED` (any text; default fixed), `INDICATRIX_HARNESS_TUTORIALS`
//! (`all`), `INDICATRIX_HARNESS_TUTORIAL_COUNT` (default 5), `INDICATRIX_HARNESS_EXCLUDE`
//! (comma-separated tutorial ids left out, also the two always-included ones),
//! `INDICATRIX_HARNESS_LIBRARY` (path of a `facet_diagrams.sqlite`),
//! `INDICATRIX_HARNESS_BUILD_COUNT` (default 5, at most 8).
//!
//! One scenario per tutorial and per design (a fresh app each), so one failure hides no other.
//! A scenario is a plain `fn`, so the per-tutorial functions are monomorphised from a const
//! slot number that indexes the selection made at registration.

use super::{Scenario, existing_install};
use crate::support::{Ctx, app::pump, coverage};
use indicatrix_cut::{EditorModel, MainWindow, TutorialsModel};
use indicatrix_editor::{
    guide::{
        Goal, Guide, GuideCatalog, StartingState, WELCOME_TOUR_ID, WORKED_EXAMPLE_ID,
        lesson_start_spec,
    },
    material::gear_index_from_teeth,
};
use slint::{ComponentHandle, Model};
use std::{sync::OnceLock, time::Duration};

mod build;
mod drive;
mod show_me_where;

use crate::support::library_source;
use drive::{RunStats, snap, wait_real};
use library_source::{LibrarySource, configured_source, harness_seed, shuffled};

/// How many built-in tutorials a default run finishes.
const DEFAULT_TUTORIAL_COUNT: usize = 5;
/// How many library designs a default run builds.
const DEFAULT_BUILD_COUNT: usize = 5;
/// The most library lessons one run can have (the number of `build` slots).
const MAX_BUILD_SLOTS: usize = 8;
/// The gallery card the tutorials that need an open design get one from (the Standard Round
/// Brilliant).
const OPEN_DESIGN_CARD: i32 = 1;
/// Salt of the tutorial shuffle.
const SALT_TUTORIALS: u64 = 1;
/// Salt of the library design shuffle.
pub const SALT_DESIGNS: u64 = 2;
/// How long a started tutorial may take to open its first step.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// One library lesson of the run.
pub struct BuildPlan {
    /// The scenario name.
    pub name: &'static str,
    /// The fixture design to build (fixture library), or `None` for "the n-th eligible design
    /// of the connected database".
    pub fixture_title: Option<&'static str>,
}

/// What a run tests, decided once at registration.
pub struct Selection {
    pub seed: u64,
    pub tutorials: Vec<String>,
    pub source: LibrarySource,
    pub builds: Vec<BuildPlan>,
    pub notes: Vec<String>,
}

static SELECTION: OnceLock<Selection> = OnceLock::new();

/// The selection of this run.
pub fn selection() -> &'static Selection {
    SELECTION.get_or_init(make_selection)
}

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    let selection = selection();
    println!(
        "ui-harness: tutorials: seed {}; built-in {:?}; library source {:?}",
        selection.seed, selection.tutorials, selection.source
    );
    let mut all = vec![Scenario {
        name: "tutorial:show_me_where",
        seed: existing_install,
        run: show_me_where::run,
    }];
    for (slot, id) in selection.tutorials.iter().enumerate() {
        if let Some(run) = TUTORIAL_SLOTS.get(slot) {
            all.push(Scenario {
                name: leak(format!("tutorial:{id}")),
                seed: existing_install,
                run: *run,
            });
        }
    }
    for (slot, plan) in selection.builds.iter().enumerate() {
        if let Some(run) = BUILD_SLOTS.get(slot) {
            all.push(Scenario {
                name: plan.name,
                seed: existing_install,
                run: *run,
            });
        }
    }
    all
}

/// A name that lives for the whole run (this is a test binary).
fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Picks the tutorials and library designs of this run.
fn make_selection() -> Selection {
    let seed = harness_seed();
    let mut notes = Vec::new();
    let (tutorials, truncated) = pick_tutorials(seed);
    if truncated > 0 {
        notes.push(format!(
            "{truncated} built-in tutorial(s) left out: the harness has {} scenario slots",
            TUTORIAL_SLOTS.len()
        ));
    }
    let (source, source_note) = configured_source();
    notes.extend(source_note);
    let builds = pick_builds(seed, &source);
    Selection {
        seed,
        tutorials,
        source,
        builds,
        notes,
    }
}

/// The built-in tutorials to finish, in catalogue order, and how many did not fit the slots.
fn pick_tutorials(seed: u64) -> (Vec<String>, usize) {
    let catalog = GuideCatalog::new();
    let excluded: Vec<String> = std::env::var("INDICATRIX_HARNESS_EXCLUDE")
        .unwrap_or_default()
        .split(',')
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect();
    let ids: Vec<String> = catalog
        .all()
        .iter()
        .map(|guide| guide.id.clone())
        .filter(|id| !excluded.contains(id))
        .collect();
    let all = std::env::var("INDICATRIX_HARNESS_TUTORIALS")
        .is_ok_and(|v| v.trim().eq_ignore_ascii_case("all"));
    let mut chosen: Vec<String> = if all {
        ids.clone()
    } else {
        let want = env_count("INDICATRIX_HARNESS_TUTORIAL_COUNT", DEFAULT_TUTORIAL_COUNT);
        let mut picked: Vec<String> = [WELCOME_TOUR_ID, WORKED_EXAMPLE_ID]
            .into_iter()
            .filter(|id| ids.iter().any(|known| known == id))
            .map(str::to_owned)
            .collect();
        let rest: Vec<String> = ids
            .iter()
            .filter(|id| !picked.contains(id))
            .cloned()
            .collect();
        let missing = want.saturating_sub(picked.len());
        picked.extend(
            shuffled(rest, seed, SALT_TUTORIALS)
                .into_iter()
                .take(missing),
        );
        picked
    };
    // Run in catalogue order whatever the shuffle did.
    chosen.sort_by_key(|id| ids.iter().position(|known| known == id));
    let truncated = chosen.len().saturating_sub(TUTORIAL_SLOTS.len());
    chosen.truncate(TUTORIAL_SLOTS.len());
    (chosen, truncated)
}

/// The library lessons of this run.
fn pick_builds(seed: u64, source: &LibrarySource) -> Vec<BuildPlan> {
    let want =
        env_count("INDICATRIX_HARNESS_BUILD_COUNT", DEFAULT_BUILD_COUNT).min(MAX_BUILD_SLOTS);
    match source {
        LibrarySource::Fixtures => {
            let titles: Vec<&'static str> = crate::support::sample_designs()
                .iter()
                .map(|design| design.title)
                .collect();
            shuffled(titles, seed, SALT_DESIGNS)
                .into_iter()
                .take(want)
                .map(|title| BuildPlan {
                    name: leak(format!("build:{title}")),
                    fixture_title: Some(title),
                })
                .collect()
        }
        LibrarySource::Database(_) => (0..want)
            .map(|n| BuildPlan {
                name: leak(format!("build:db#{}", n + 1)),
                fixture_title: None,
            })
            .collect(),
    }
}

/// Notes the run's selection in the report (once; notes are de-duplicated).
pub fn announce(ctx: &mut Ctx<'_>) {
    let selection = selection();
    ctx.report.note(format!(
        "tutorials: seed {} (INDICATRIX_HARNESS_SEED); built-in tutorials {:?}; library source {:?}",
        selection.seed, selection.tutorials, selection.source
    ));
    for note in &selection.notes {
        ctx.report.note(format!("tutorials: {note}"));
    }
}

/// A built-in tutorial: started from the browser, finished step by step, recorded as done.
fn run_tutorial<const SLOT: usize>(ctx: &mut Ctx<'_>) {
    announce(ctx);
    if let Some(id) = selection().tutorials.get(SLOT) {
        run_builtin(ctx, id);
    }
}

/// A library design turned into a lesson and finished (see `build`).
fn run_build<const SLOT: usize>(ctx: &mut Ctx<'_>) {
    announce(ctx);
    if let Some(plan) = selection().builds.get(SLOT) {
        build::run(ctx, SLOT, plan);
    }
}

macro_rules! slot_table {
    ($name:ident, $function:ident, $($slot:literal),+ $(,)?) => {
        const $name: &[fn(&mut Ctx<'_>)] = &[$($function::<$slot>),+];
    };
}

slot_table!(
    TUTORIAL_SLOTS,
    run_tutorial,
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    9,
    10,
    11,
    12,
    13,
    14,
    15,
    16,
    17,
    18,
    19,
    20,
    21,
    22,
    23,
    24,
    25,
    26,
    27,
    28,
    29,
    30,
    31,
    32,
    33,
    34,
    35,
    36,
    37,
    38,
    39,
    40,
    41,
    42,
    43,
    44,
    45,
    46,
    47,
    48,
    49,
    50,
    51,
    52,
    53,
    54,
    55,
    56,
    57,
    58,
    59,
    60,
    61,
    62,
    63,
    64,
    65,
    66,
    67,
    68,
    69,
    70,
    71,
    72,
    73,
    74,
    75,
    76,
    77,
    78,
    79,
    80,
    81,
    82,
    83,
    84,
    85,
    86,
    87,
    88,
    89,
    90,
    91,
    92,
    93,
    94,
    95,
    96,
    97,
    98,
    99,
    100,
    101,
    102,
    103,
    104,
    105,
    106,
    107,
    108,
    109,
    110,
    111,
    112,
    113,
    114,
    115,
    116,
    117,
    118,
    119,
    120,
    121,
    122,
    123,
    124,
    125,
    126,
    127,
);
slot_table!(BUILD_SLOTS, run_build, 0, 1, 2, 3, 4, 5, 6, 7);

/// Finishes the built-in tutorial `id` and checks it was recorded.
fn run_builtin(ctx: &mut Ctx<'_>, id: &str) {
    let scope = format!("tutorial:{id}");
    ctx.report.set_context(scope.clone());
    let catalog = GuideCatalog::new();
    let Some(guide) = catalog.get(id) else {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-unknown",
            format!("{scope}: the catalogue has no guide with this id"),
        );
        return;
    };
    if !start_builtin(ctx, &scope, guide) {
        return;
    }
    let goals: Vec<Goal> = guide.steps.iter().map(|step| step.goal.clone()).collect();
    let shown = snap(ctx.app.ui()).total;
    if usize::try_from(shown).ok() != Some(guide.steps.len()) {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-step-count",
            format!(
                "{scope}: the panel has {shown} step(s), the guide {}",
                guide.steps.len()
            ),
        );
    }
    let stats = drive::drive_guide(ctx, &scope, id, Some(&goals));
    verify_finished(ctx, &scope, id, &stats);
}

/// Opens the browser, checks the guide's row and presses its Start (`TutorialsModel.start`).
fn start_builtin(ctx: &mut Ctx<'_>, scope: &str, guide: &Guide) -> bool {
    if matches!(guide.starting_state, StartingState::RequiresOpenDesign) {
        // A tutorial that works on "the design you have open": open one first, as a learner would.
        create_template_design(ctx.app.ui(), OPEN_DESIGN_CARD);
        let made = wait_real(Duration::from_secs(10), || {
            ctx.app.ui().global::<EditorModel>().get_has_design()
        });
        if !made {
            ctx.report.add_simple(
                "MainWindow",
                "tutorial-prepare",
                format!("{scope}: the design the tutorial needs could not be created"),
            );
            return false;
        }
    }
    ctx.app
        .ui()
        .global::<TutorialsModel>()
        .invoke_open_browser();
    pump(150);
    if browser_row(ctx.app.ui(), &guide.id).is_some() {
        coverage::record_callback("TutorialsModel.open_browser");
    }
    match browser_row(ctx.app.ui(), &guide.id) {
        None => ctx.report.add_simple(
            "MainWindow",
            "tutorial-not-listed",
            format!("{scope}: the tutorial browser has no row for {}", guide.id),
        ),
        Some(row) => {
            if row.done {
                ctx.report.add_simple(
                    "MainWindow",
                    "tutorial-state",
                    format!("{scope}: the row is marked done in a fresh app"),
                );
            }
            if !row.blocked_reason.is_empty() {
                ctx.report.add_simple(
                    "MainWindow",
                    "tutorial-blocked",
                    format!(
                        "{scope}: the row says it cannot start: {}",
                        row.blocked_reason
                    ),
                );
            }
        }
    }
    ctx.check_health(&format!("{scope}: tutorial browser"));
    ctx.app
        .ui()
        .global::<TutorialsModel>()
        .invoke_start(guide.id.as_str().into());
    let opened = wait_real(START_TIMEOUT, || {
        let s = snap(ctx.app.ui());
        s.open && s.id == guide.id
    });
    if opened {
        coverage::record_callback("TutorialsModel.start");
    } else {
        let ui = ctx.app.ui();
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-not-started",
            format!(
                "{scope}: the guide did not open its first step within {} s (toast: {})",
                START_TIMEOUT.as_secs(),
                ui.get_toast_message()
            ),
        );
    }
    opened
}

/// The browser row for guide `id`, if the browser lists it.
pub fn browser_row(ui: &MainWindow, id: &str) -> Option<indicatrix_cut::TutorialRowData> {
    let rows = ui.global::<TutorialsModel>().get_rows();
    (0..rows.row_count())
        .filter_map(|at| rows.row_data(at))
        .find(|row| row.id.as_str() == id)
}

/// After the last step: the guide is closed, the browser shows the guide as done, and nothing
/// went wrong on the way.
pub fn verify_finished(ctx: &mut Ctx<'_>, scope: &str, id: &str, stats: &RunStats) {
    ctx.report.set_context(scope.to_owned());
    println!(
        "ui-harness:   {scope}: {} step(s): {} by goal, {} read, {} event(s) raised directly, \
         {} GAP, {} skipped{}",
        stats.steps,
        stats.by_goal,
        stats.read,
        stats.injected,
        stats.gaps,
        stats.skipped,
        if stats.aborted { ", ABORTED" } else { "" }
    );
    ctx.report.note(format!(
        "{scope}: {} step(s): {} by goal, {} read, {} event(s) raised directly, {} GAP, {} skipped",
        stats.steps, stats.by_goal, stats.read, stats.injected, stats.gaps, stats.skipped
    ));
    if stats.aborted {
        return;
    }
    pump(100);
    if snap(ctx.app.ui()).open {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-not-closed",
            format!("{scope}: the guide panel is still open after its last step"),
        );
    }
    ctx.app
        .ui()
        .global::<TutorialsModel>()
        .invoke_open_browser();
    pump(150);
    if browser_row(ctx.app.ui(), id).is_some() {
        coverage::record_callback("TutorialsModel.open_browser");
    }
    match browser_row(ctx.app.ui(), id) {
        Some(row) if row.done => {}
        Some(_) => ctx.report.add_simple(
            "MainWindow",
            "tutorial-not-recorded",
            format!("{scope}: the browser does not show the finished guide as done"),
        ),
        None => ctx.report.add_simple(
            "MainWindow",
            "tutorial-not-listed",
            format!("{scope}: the finished guide is not listed in the browser"),
        ),
    }
    ctx.app
        .ui()
        .global::<TutorialsModel>()
        .invoke_close_browser();
    pump(50);
    // No assertion precedes this: the browser's closed state is not read back.
    coverage::record_callback("TutorialsModel.close_browser");
    ctx.check_health(&format!("{scope}: finished"));
}

/// New Design with the gallery card `card`, through the dialog's own Create callback.
fn create_template_design(ui: &MainWindow, card: i32) {
    use indicatrix_cut_core::PreformShape;
    let spec = lesson_start_spec(card);
    let shape_index = match spec.preform.shape {
        PreformShape::Cylinder { .. } => 1,
        _ => 0,
    };
    ui.global::<EditorModel>().invoke_new_design_create(
        shape_index,
        spec.preform.half_width.to_string().into(),
        spec.preform.length_over_width.to_string().into(),
        spec.preform.depth.to_string().into(),
        gear_index_from_teeth(spec.gear_teeth),
        spec.gear_teeth.to_string().into(),
        spec.symmetry_order.to_string().into(),
        spec.mirror,
        0,
        card,
    );
}
