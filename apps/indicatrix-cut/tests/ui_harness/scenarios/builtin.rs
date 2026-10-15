//! The minimal scenario set that proves the harness machinery end to end: a state sweep at two
//! sizes, a click-everything pass, secondary windows that need no data, and the first-run state.

use super::{Scenario, existing_install, first_run};
use crate::support::{Ctx, Finding, HarnessApp, app::pump};
use indicatrix_cut::{HelpModel, PreferencesModel, RoughPlanModel};
use slint::ComponentHandle;

/// The built-in scenarios.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "startup_state_sweep",
            seed: existing_install,
            run: startup_state_sweep,
        },
        Scenario {
            name: "startup_click_everything",
            seed: existing_install,
            run: startup_click_everything,
        },
        Scenario {
            name: "secondary_windows_open",
            seed: existing_install,
            run: secondary_windows_open,
        },
        Scenario {
            name: "first_run_welcome",
            seed: first_run,
            run: first_run_welcome,
        },
    ]
}

/// The freshly started main window, swept at 1366x768 and at its 720x480 minimum.
fn startup_state_sweep(ctx: &mut Ctx<'_>) {
    ctx.sweep_main_at_sizes("startup");
    ctx.check_health("startup sweep");
}

/// Every enabled control of the freshly started main window, one at a time.
fn startup_click_everything(ctx: &mut Ctx<'_>) {
    let _stats = ctx.smoke_main("startup", &mut |_app: &mut HarnessApp| {});
    ctx.check_health("startup click-everything");
}

/// Opens the windows that need no data (Help, Rough Planner) and checks nothing breaks. The
/// planner is closed with the main window at teardown.
fn secondary_windows_open(ctx: &mut Ctx<'_>) {
    ctx.app
        .ui()
        .global::<HelpModel>()
        .invoke_open_context_help();
    pump(300);
    ctx.check_health("open Help window");
    indicatrix_cut::gui::help::close_help_window();
    pump(100);
    ctx.check_health("close Help window");

    ctx.app.ui().global::<RoughPlanModel>().invoke_open();
    pump(500);
    ctx.check_health("open Rough Planner window");
}

/// The first-run state: the Welcome dialog is pending and the screen holds together.
fn first_run_welcome(ctx: &mut Ctx<'_>) {
    let pending = ctx
        .app
        .ui()
        .global::<PreferencesModel>()
        .get_first_run_tour_pending();
    if !pending {
        ctx.report.add(Finding::new(
            "MainWindow",
            "PreferencesModel.first_run_tour_pending",
            "first-run",
            "no settings file, but the first-run tour is not pending",
        ));
    }
    ctx.sweep_main_at_sizes("first run (Welcome)");
    ctx.check_health("first run");
}
