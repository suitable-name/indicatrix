//! Scenarios: the secondary windows (Help, Compare, Locate, Rig, Rough Color) and what closing
//! the main window does to them. The Rough Planner has its own file.
//!
//! GAP: the Help window has no route to the glossary of its own (`open_glossary` is only handled
//! on the main window), so `help_window` opens the glossary through the main window's `HelpModel`
//! while the Help window is up. GAP: Locate is opened without a mesh rough (the button that
//! normally gates it is bypassed through its callback), so the photo and alignment states are not
//! reached; they need a mesh file the harness does not build.

use super::{Scenario, dialogs, existing_install};
use crate::support::{
    Ctx, HarnessApp, SecondaryWindow,
    app::{pump, wait_until},
    coverage,
};
use indicatrix_cut::{
    CompareModel, EditorModel, HelpModel, MainWindow, RoughPlanModel, harness_support,
};
use slint::{ComponentHandle, Model, platform::WindowEvent};
use std::time::Duration;

/// Every secondary window, for "is anything still open" checks.
const ALL_WINDOWS: [SecondaryWindow; 6] = [
    SecondaryWindow::Compare,
    SecondaryWindow::RoughPlanner,
    SecondaryWindow::Locate,
    SecondaryWindow::Rig,
    SecondaryWindow::RoughColor,
    SecondaryWindow::Help,
];

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "help_window",
            seed: existing_install,
            run: help_window,
        },
        Scenario {
            name: "compare_window",
            seed: existing_install,
            run: compare_window,
        },
        Scenario {
            name: "locate_and_rig",
            seed: existing_install,
            run: locate_and_rig,
        },
        Scenario {
            name: "closing_main_closes_all",
            seed: existing_install,
            run: closing_main_closes_all,
        },
        Scenario {
            name: "rough_color_window",
            seed: existing_install,
            run: rough_color_window,
        },
    ]
}

/// Closes a secondary window the way its title-bar button does.
fn close_with_title_bar(window: &slint::Window) {
    window.dispatch_event(WindowEvent::CloseRequested);
    pump(200);
}

/// Waits until `which` is open or closed.
fn wait_open(which: SecondaryWindow, open: bool, seconds: u64) -> bool {
    wait_until(Duration::from_secs(seconds), || {
        harness_support::is_window_open(which) == open
    })
}

/// Opens the Help window with F1 and with `HelpModel.open_topic`; searches the manual and gets
/// hits; opens the glossary; closes the window with its own close callback and with the title
/// bar. The window is open exactly when expected each time.
fn help_window(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ctx.press("F1");
    let opened = wait_open(SecondaryWindow::Help, true, 10);
    ctx.expect(opened, "help-opens-f1", "F1 did not open the Help window");
    if !opened {
        return;
    }
    ctx.sweep_secondary(SecondaryWindow::Help, "opened with F1");
    search_manual(ctx);
    ui.global::<HelpModel>().invoke_open_glossary("".into());
    pump(300);
    ctx.expect(
        ui.global::<HelpModel>().get_glossary_open(),
        "help-glossary",
        "the glossary did not open while the Help window was up",
    );
    ui.global::<HelpModel>().set_glossary_open(false);
    pump(150);
    if let Some(window) = harness_support::help_window() {
        window.global::<HelpModel>().invoke_close_window();
    }
    pump(300);
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Help),
        "help-closes",
        "the Help window is still open after its close callback",
    );
    topic_then_title_bar(ctx, &ui);
    ctx.check_health("Help window");
}

/// Searches the open Help window for a term the manual certainly holds and sweeps the results.
fn search_manual(ctx: &mut Ctx<'_>) {
    let Some(window) = harness_support::help_window() else {
        return;
    };
    let model = window.global::<HelpModel>();
    model.set_search_text("facet".into());
    model.invoke_search("facet".into());
    let found = wait_until(Duration::from_secs(10), || {
        model.get_searching() && model.get_hits().row_count() > 0
    });
    ctx.expect(
        found,
        "help-search",
        format!(
            "searching the manual for \"facet\" gave no hits (status: {})",
            model.get_status()
        ),
    );
    if found {
        coverage::record_callback("HelpModel.search");
        ctx.sweep_secondary(SecondaryWindow::Help, "search results");
    }
    model.set_search_text("".into());
    model.invoke_search("".into());
    pump(200);
}

/// `HelpModel.open_topic` shows the window again with a chapter; the title-bar close hides it.
fn topic_then_title_bar(ctx: &mut Ctx<'_>, ui: &MainWindow) {
    ui.global::<HelpModel>()
        .invoke_open_topic("05-solving#what-solve-does".into());
    let opened = wait_open(SecondaryWindow::Help, true, 10);
    ctx.expect(
        opened,
        "help-opens-topic",
        "HelpModel.open_topic did not open the Help window",
    );
    if !opened {
        return;
    }
    coverage::record_callback("HelpModel.open_topic");
    let chapter = harness_support::help_window()
        .map_or(0, |w| w.global::<HelpModel>().get_blocks().row_count());
    ctx.expect(
        chapter > 0,
        "help-chapter",
        "the opened topic shows no content",
    );
    ctx.sweep_secondary(SecondaryWindow::Help, "opened on a topic");
    if let Some(window) = harness_support::help_window() {
        close_with_title_bar(window.window());
    }
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Help),
        "help-closes-title-bar",
        "the Help window is still open after the title-bar close",
    );
}

/// Brings a design and a snapshot of it up, edits the design, and opens the snapshot compare
/// window (what "Compare visually" does); returns once the window is open and its session built.
fn open_snapshot_compare(app: &HarnessApp) -> bool {
    let ui = app.ui().clone_strong();
    if !dialogs::make_template_design(&ui) {
        return false;
    }
    ui.global::<EditorModel>().invoke_snapshot_design();
    let _ = wait_until(Duration::from_secs(15), || {
        ui.global::<EditorModel>().get_has_snapshot()
    });
    if !dialogs::nudge_first_tier(&ui) {
        return false;
    }
    ui.global::<CompareModel>().invoke_open_snapshot();
    if !wait_open(SecondaryWindow::Compare, true, 10) {
        return false;
    }
    wait_until(Duration::from_secs(40), || {
        harness_support::compare_window()
            .is_some_and(|w| !w.global::<CompareModel>().get_status().contains("Solving"))
    })
}

/// A design, a snapshot, an edit; the snapshot compare window opens, solves both sides, sweeps
/// clean, survives a click-everything pass, and closes with its own Close callback.
///
/// No strong clone of the main window is held in this function: the smoke pass relaunches the
/// app, and a clone kept across that keeps the old main window, and every database handle its
/// callbacks hold, alive after its shutdown.
fn compare_window(ctx: &mut Ctx<'_>) {
    if !dialogs::create_template_design(ctx) {
        return;
    }
    {
        let editor = ctx.app.ui().global::<EditorModel>();
        editor.invoke_snapshot_design();
        let _ = wait_until(Duration::from_secs(15), || editor.get_has_snapshot());
    }
    if !dialogs::edit_design(ctx) {
        return;
    }
    ctx.app.ui().global::<CompareModel>().invoke_open_snapshot();
    let opened = wait_open(SecondaryWindow::Compare, true, 10);
    ctx.expect(
        opened,
        "compare-opens",
        "CompareModel.open_snapshot did not open the compare window (snapshot taken, design edited)",
    );
    if !opened {
        return;
    }
    coverage::record_callback("CompareModel.open_snapshot");
    ctx.wait_for(
        "the compare sides are solved",
        Duration::from_secs(40),
        || {
            harness_support::compare_window()
                .is_some_and(|w| !w.global::<CompareModel>().get_status().contains("Solving"))
        },
    );
    ctx.sweep_secondary(SecondaryWindow::Compare, "snapshot compare");
    ctx.check_health("compare window");
    let _stats = ctx.smoke_secondary(
        SecondaryWindow::Compare,
        "snapshot compare",
        &mut |app: &mut HarnessApp| {
            let _ = open_snapshot_compare(app);
        },
    );
    // The smoke pass relaunched the app; bring the window back for the close check.
    if !harness_support::is_window_open(SecondaryWindow::Compare) && !open_snapshot_compare(ctx.app)
    {
        ctx.expect(
            false,
            "compare-reopens",
            "the compare window could not be reopened",
        );
        return;
    }
    if let Some(window) = harness_support::compare_window() {
        window.global::<CompareModel>().invoke_close();
    }
    pump(400);
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Compare),
        "compare-closes",
        "the compare window is still open after its Close callback",
    );
    coverage::record_callback("CompareModel.close");
    ctx.check_health("compare window closed");
}

/// Locate opens from the planner, Rig from Locate; both sweep; closing Rig then Locate leaves
/// both closed.
fn locate_and_rig(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<RoughPlanModel>().invoke_open();
    let planner = wait_open(SecondaryWindow::RoughPlanner, true, 10);
    ctx.expect(planner, "planner-opens", "the Rough Planner did not open");
    let Some(planner_window) = harness_support::rough_planner_window().filter(|_| planner) else {
        return;
    };
    pump(300);
    planner_window
        .global::<RoughPlanModel>()
        .invoke_locate_inclusion();
    let locate = wait_open(SecondaryWindow::Locate, true, 10);
    ctx.expect(
        locate,
        "locate-opens",
        "locate_inclusion did not open the Locate window",
    );
    if !locate {
        return;
    }
    coverage::record_callback("RoughPlanModel.locate_inclusion");
    ctx.sweep_secondary(SecondaryWindow::Locate, "Locate window");
    if let Some(window) = harness_support::locate_window() {
        window.invoke_edit_rigs();
    }
    let rig = wait_open(SecondaryWindow::Rig, true, 10);
    ctx.expect(rig, "rig-opens", "Edit Rigs did not open the Rig window");
    if rig {
        ctx.sweep_secondary(SecondaryWindow::Rig, "Rig window");
        if let Some(window) = harness_support::rig_window() {
            close_with_title_bar(window.window());
        }
        ctx.expect(
            !harness_support::is_window_open(SecondaryWindow::Rig),
            "rig-closes",
            "the Rig window is still open after the title-bar close",
        );
    }
    if let Some(window) = harness_support::locate_window() {
        close_with_title_bar(window.window());
    }
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::Locate),
        "locate-closes",
        "the Locate window is still open after the title-bar close",
    );
    ctx.check_health("Locate and Rig windows");
}

/// The Rough colour wizard opens from the planner, sweeps clean, and closes with the title bar.
fn rough_color_window(ctx: &mut Ctx<'_>) {
    use indicatrix_cut::Zoning;
    let ui = ctx.app.ui().clone_strong();
    ui.global::<RoughPlanModel>().invoke_open();
    let planner = wait_open(SecondaryWindow::RoughPlanner, true, 10);
    ctx.expect(planner, "planner-opens", "the Rough Planner did not open");
    let Some(planner_window) = harness_support::rough_planner_window().filter(|_| planner) else {
        return;
    };
    pump(300);
    planner_window.global::<Zoning>().invoke_open_rough_colour();
    let opened = wait_open(SecondaryWindow::RoughColor, true, 10);
    ctx.expect(
        opened,
        "colour-opens",
        "Zoning.open_rough_colour did not open the wizard",
    );
    if !opened {
        return;
    }
    coverage::record_callback("Zoning.open_rough_colour");
    ctx.sweep_secondary(SecondaryWindow::RoughColor, "Rough colour wizard");
    if let Some(window) = harness_support::rough_color_window() {
        close_with_title_bar(window.window());
    }
    ctx.expect(
        !harness_support::is_window_open(SecondaryWindow::RoughColor),
        "colour-closes",
        "the Rough colour wizard is still open after the title-bar close",
    );
    ctx.check_health("Rough colour wizard");
}

/// Help and the Rough Planner are open; closing the main window (the title-bar request, with the
/// close guard answered Discard if it appears) leaves no secondary window open.
fn closing_main_closes_all(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<HelpModel>().invoke_open_context_help();
    ui.global::<RoughPlanModel>().invoke_open();
    let both = wait_until(Duration::from_secs(10), || {
        harness_support::is_window_open(SecondaryWindow::Help)
            && harness_support::is_window_open(SecondaryWindow::RoughPlanner)
    });
    ctx.expect(
        both,
        "windows-open",
        "Help and the Rough Planner did not both open",
    );
    dialogs::request_main_close(&ui);
    if ui.get_close_confirm_open() {
        ui.invoke_close_confirm_discard();
    }
    pump(400);
    ctx.expect(
        !ui.window().is_visible(),
        "main-closes",
        "the main window is still visible after the close request",
    );
    let open: Vec<String> = ALL_WINDOWS
        .iter()
        .filter(|w| harness_support::is_window_open(**w))
        .map(|w| format!("{w:?}"))
        .collect();
    ctx.expect(
        open.is_empty(),
        "secondary-windows-closed",
        format!("secondary windows still open after the main window closed: {open:?}"),
    );
    ctx.check_health("closing the main window");
}
