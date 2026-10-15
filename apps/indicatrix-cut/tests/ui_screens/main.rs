//! Screenshot harness for `indicatrix-cut` (custom `main`, `harness = false`): rewrites the UI
//! screenshots in the workspace's `assets` folder from the real application.
//!
//! It drives the real `MainWindow` with all its Rust wiring on Slint's testing backend with the
//! software renderer, inside a throw-away profile (the same isolation as the `ui_harness` target:
//! the settings file and the design library are in the profile, never the owner's), and saves one
//! PNG per scenario. One scenario is one screenshot and is named after the file stem; the app is
//! relaunched on a fresh copy of the library before each one.
//!
//! Run (the library is required: the shots photograph the owner's real catalogue, which is
//! copied into the profile and never opened by the harness itself):
//!
//! ```text
//! INDICATRIX_HARNESS_LIBRARY=C:\path\to\facet_diagrams.sqlite cargo ui-screens
//! INDICATRIX_HARNESS_LIBRARY=... cargo ui-screens -- main cad   (only these shots)
//! INDICATRIX_HARNESS_LIBRARY=... cargo ui-screens --features gpu (faster live renders)
//! ```
//!
//! Arguments that do not start with `-` select shots by name substring. Without the library the
//! run aborts. The run needs Slint debug info (the `cargo ui-screens` alias sets it).
//!
//! Environment variables:
//!
//! - `INDICATRIX_HARNESS_LIBRARY`: the `facet_diagrams.sqlite` to photograph (required).
//! - `INDICATRIX_SCREENS_OUT`: the output folder (default `<manifest>/../../assets`, resolved
//!   before the profile moves the working directory).
//! - `INDICATRIX_SCREENS_SPP`: the sample count a live render must reach (default 1024, a power
//!   of two from 8 to 1024).
//! - `INDICATRIX_SCREENS_RENDER_TIMEOUT`: seconds a shot waits for a render (default 120). A
//!   render that has not settled by then is photographed anyway and noted in the report; it is
//!   not a finding.
//!
//! Only the PNG files named below are written, and only when their shot succeeded: a shot whose
//! steps fail leaves the old file in place and reports a finding `shot` with the reason. No file
//! is ever deleted or renamed. Every written file is announced as
//! `ui-screens: wrote assets/<name>.png (<w> x <h>)`. The report is in
//! `target/ui-screens/report.md`.
//!
//! Shots: main, `solid_rendering`, `cutting_instructions`, `tilt_performance`, `tilt_performance_export`,
//! `tilt_performance_cad`, `emerald_export_settings_4k`, cad, `cad_solved_diagram`, `diagram_view`,
//! `diagram_view_detail`, `retarget_dialog`, `retarget_dialog_1`, `retarget_gear_2`, `retarget_merge`,
//! `optimizer_1`, `optimizer_2`, `optimizer__compare_1`, `optimizer__compare_2`, `tutorial_from_design`,
//! `tutorial_from_design_2`, `tutorial_from_design_3`, `rough_planner`, `rough_planner_result`,
//! `parameters_stone`, `parameters_lighting`, `parameters_render`, and the shots of the user
//! manual's chapters: `new_design_dialog`, `material_editor`, `material_zones`, `preferences`,
//! `command_palette`, `remote_coordinator`, `tier_form`, `concave_tier_form`, `history_steps`,
//! `history_variants`, `cutting_mode`, `angle_sweep`, `render_jobs`.
//!
//! Not reproducible by the harness (left alone): `web_ui.png`, `web_ui_mobile.png` (the web app),
//! `hybrid_remote_rendering.png` (several machines), the Rough Color wizard (it needs a scanned
//! mesh rough and eight rig photos), `settings_1.png` and `settings_2.png` (the
//! Settings dialog is gone; the `parameters_*` shots replace them), and the unused
//! `retarget_dialog_2.png`, `retarget_dialog_3.png`, `retarget_gear.png`, `retarget_gear_3.png`.
//!
//! Privacy: no shot may show a path with the user's name. The profile lives under the user's
//! temp folder, so the Export dialog's Output Location is set to a neutral text before its shot,
//! and every shot's visible labels, values and placeholders are scanned for the user's name and
//! for profile path fragments; a hit fails the shot instead of writing it.
//!
//! Look: the software renderer draws with the system fonts and draws no window frame, so a shot
//! shows the window content only. Windows are 1920 x 1152 logical pixels (the Compare window and
//! the Rough Planner use their preferred sizes) at scale 2.0.

mod config;
mod dialog_shots;
mod editor_shots;
mod kit;
mod library_shots;
mod manual_shots;
mod nav;
#[path = "../ui_harness/support/mod.rs"]
mod support;
mod window_shots;

use config::{Config, MISSING_LIBRARY};
use kit::Sc;
use slint_harness::{
    Harness, Jobs,
    backend::{Renderer, require_debug_info},
    profile::{Profile as HarnessProfile, request_abort},
    report::{Finding, Format},
    screenshot::Shot,
    take_panics,
};
use std::{
    cell::{Cell, RefCell},
    process::ExitCode,
    rc::Rc,
};
use support::{Ctx, HarnessApp, SettingsSeed, profile::print_shutdown_summary};

/// The app, shared by the scenario closures and the teardown (one UI thread).
type SharedApp = Rc<RefCell<HarnessApp>>;

/// A shot: drives the app (already on the library copy) to its state and captures it.
type ShotFn = fn(&mut Sc<'_, '_>) -> Result<Shot, String>;

/// Every shot, in the order it is taken: the file stem and the function that makes it.
const SHOTS: [(&str, ShotFn); 40] = [
    ("main", library_shots::main_window),
    ("solid_rendering", library_shots::solid_rendering),
    ("cutting_instructions", library_shots::cutting_instructions),
    ("tilt_performance", library_shots::tilt_performance),
    (
        "tilt_performance_export",
        library_shots::tilt_performance_export,
    ),
    ("tilt_performance_cad", library_shots::tilt_performance_cad),
    (
        "emerald_export_settings_4k",
        library_shots::emerald_export_settings_4k,
    ),
    ("cad", editor_shots::cad),
    ("cad_solved_diagram", editor_shots::cad_solved_diagram),
    ("diagram_view", editor_shots::diagram_view),
    ("diagram_view_detail", editor_shots::diagram_view_detail),
    ("retarget_dialog", editor_shots::retarget_dialog),
    ("retarget_dialog_1", editor_shots::retarget_dialog_1),
    ("retarget_gear_2", editor_shots::retarget_gear_2),
    ("retarget_merge", editor_shots::retarget_merge),
    ("optimizer_1", editor_shots::optimizer_1),
    ("optimizer_2", editor_shots::optimizer_2),
    ("optimizer__compare_1", window_shots::compare_1),
    ("optimizer__compare_2", window_shots::compare_2),
    ("tutorial_from_design", library_shots::tutorial_from_design),
    (
        "tutorial_from_design_2",
        library_shots::tutorial_from_design_2,
    ),
    (
        "tutorial_from_design_3",
        library_shots::tutorial_from_design_3,
    ),
    ("rough_planner", window_shots::rough_planner),
    ("rough_planner_result", window_shots::rough_planner_result),
    ("parameters_stone", library_shots::parameters_stone),
    ("parameters_lighting", library_shots::parameters_lighting),
    ("parameters_render", library_shots::parameters_render),
    ("new_design_dialog", dialog_shots::new_design_dialog),
    ("material_editor", dialog_shots::material_editor),
    ("material_zones", dialog_shots::material_zones),
    ("preferences", dialog_shots::preferences),
    ("command_palette", dialog_shots::command_palette),
    ("remote_coordinator", dialog_shots::remote_coordinator),
    ("tier_form", manual_shots::tier_form),
    ("concave_tier_form", manual_shots::concave_tier_form),
    ("history_steps", manual_shots::history_steps),
    ("history_variants", manual_shots::history_variants),
    ("cutting_mode", manual_shots::cutting_mode),
    ("angle_sweep", manual_shots::angle_sweep),
    ("render_jobs", manual_shots::render_jobs),
];

fn main() -> ExitCode {
    // Read before the profile moves the working directory and redirects the user variables.
    let config = Rc::new(Config::from_env());
    let harness = Harness::new()
        .profile(
            HarnessProfile::isolated()
                .name_prefix("indicatrix-ui-screens")
                .guard_paths(|| {
                    vec![
                        indicatrix_cut::harness_support::settings_path(),
                        indicatrix_cut::harness_support::database_path(),
                    ]
                }),
        )
        .renderer(Renderer::Software)
        // Worker processes while cores are idle (each its own profile and library copy); the
        // shots are independent. `SLINT_HARNESS_JOBS=serial` runs them one after another.
        .jobs(Jobs::Auto)
        .report_dir(slint_harness::target_dir!().join("ui-screens"))
        .formats([Format::Markdown]);

    let app: SharedApp = Rc::new(RefCell::new(HarnessApp::empty()));
    let checked = Rc::new(Cell::new(false));
    let mut harness = harness;
    for (name, shot) in SHOTS {
        harness = register(harness, &app, &checked, &config, name, shot);
    }
    let teardown_app = Rc::clone(&app);
    harness
        .teardown(move || {
            teardown_app.borrow_mut().shutdown();
            print_shutdown_summary();
        })
        .run()
}

/// Saves the picture as `<out>/<name>.png` and announces it.
fn write_shot(config: &Config, name: &str, shot: &Shot) -> Result<(), String> {
    let file = config.out_dir.join(format!("{name}.png"));
    shot.save_png(&file)?;
    println!(
        "ui-screens: wrote assets/{name}.png ({} x {})",
        shot.width(),
        shot.height()
    );
    Ok(())
}

/// Launches the app on the library copy, takes the shot and writes it.
fn take_shot(
    ctx: &mut Ctx<'_>,
    config: &Config,
    name: &str,
    shot: ShotFn,
    checked: &Cell<bool>,
) -> Result<(), String> {
    let mut sc = Sc { ctx, cfg: config };
    sc.prepare()?;
    if !checked.get() {
        checked.set(true);
        // The element API needs Slint debug info; without it every query returns nothing.
        if let Err(message) = require_debug_info(&sc.ctx.app.main_view().root) {
            request_abort(message.clone());
            return Err(message);
        }
    }
    let picture = shot(&mut sc)?;
    write_shot(config, name, &picture)
}

/// Registers one shot as a scenario.
fn register(
    harness: Harness,
    app: &SharedApp,
    checked: &Rc<Cell<bool>>,
    config: &Rc<Config>,
    name: &'static str,
    shot: ShotFn,
) -> Harness {
    let app = Rc::clone(app);
    let checked = Rc::clone(checked);
    let config = Rc::clone(config);
    harness.scenario(name, move |cx| {
        if config.library.is_none() {
            let note = config.library_note.as_deref().unwrap_or_default();
            let message = format!("{MISSING_LIBRARY} ({note})");
            request_abort(message.clone());
            return Err(Finding::simple("launch", message));
        }
        let seed = SettingsSeed::ExistingInstall;
        let mut guard = app.borrow_mut();
        let _ = take_panics();
        // The toast readers serve the crate's own end-of-scenario `check_health`.
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
        let outcome = take_shot(&mut ctx, &config, name, shot, &checked);
        ctx.check_health(name);
        guard.clear_notice();
        outcome.map_err(|reason| Finding::simple("shot", format!("{name}: {reason}")))
    })
}
