//! The shared states of the direct-invoke table (see [`Setup`]): each function relaunches the
//! app and navigates to the state, or says why it cannot.

use super::table::Setup;
use crate::support::{
    Ctx, FixtureDesign, HarnessApp, ProfileExt, app::wait_until, sample_designs, seed_library,
};
use indicatrix_cut::{
    CompareModel, EditorModel, HelpModel, LibraryModel, MainWindow, RoughPlanModel, harness_support,
};
use slint::{ComponentHandle, Model};
use slint_harness::{backend::wait_until_labeled, timing};
use std::time::Duration;

/// Relaunches the app into `setup`.
///
/// # Errors
///
/// A message naming the step that did not work.
pub fn prepare(ctx: &mut Ctx<'_>, setup: Setup) -> Result<(), String> {
    match setup {
        Setup::Startup => startup(ctx),
        Setup::Library => library(ctx),
        Setup::Editor => editor(ctx),
        Setup::Planner => planner(ctx),
        Setup::Compare => compare(ctx),
        Setup::Help => help(ctx),
    }
}

/// Whether the state's window (and the main window) are still there.
#[must_use]
pub fn still_usable(ctx: &Ctx<'_>, setup: Setup) -> bool {
    if !ctx.app.is_open() {
        return false;
    }
    match setup {
        Setup::Planner => harness_support::rough_planner_window().is_some(),
        Setup::Compare => harness_support::compare_window().is_some(),
        Setup::Help => harness_support::help_window().is_some(),
        _ => true,
    }
}

fn startup(ctx: &mut Ctx<'_>) -> Result<(), String> {
    ctx.app.relaunch(ctx.profile, &ctx.seed)
}

/// How often the library is seeded and the app relaunched before the setup gives up.
const LIBRARY_ATTEMPTS: u32 = 3;

/// Seeds the fixture designs, relaunches the app and waits (real time, up to 10 s) until the
/// library list MODEL holds them (`row_count` of `LibraryModel.diagram_list`, not drawn items).
/// Returns the row count the model reached.
fn launch_with_library(ctx: &mut Ctx<'_>, designs: &[FixtureDesign]) -> Result<usize, String> {
    timing::time("relaunch", || {
        ctx.app.shutdown();
        ctx.profile.wipe_app_data();
        ctx.profile.seed_settings(&ctx.seed);
        harness_support::clear_picker_answers();
        seed_library(ctx.profile, designs).map_err(|e| format!("cannot seed the library: {e}"))?;
        *ctx.app = HarnessApp::launch().map_err(|e| format!("cannot start with a library: {e}"))?;
        let wanted = designs.len();
        let app = &*ctx.app;
        let list_len = || {
            app.ui()
                .global::<LibraryModel>()
                .get_diagram_list()
                .row_count()
        };
        let _ = wait_until_labeled("library-load", Duration::from_secs(10), || {
            list_len() >= wanted
        });
        Ok(list_len())
    })
}

/// The fixture designs in the library, the first row selected. The relaunch is repeated (after a
/// short real-time pause) when the list model does not reach the seeded count: the finding names
/// the count the model held on the last attempt.
fn library(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let designs = sample_designs();
    let wanted = designs.len();
    let mut counts: Vec<usize> = Vec::new();
    for _ in 0..LIBRARY_ATTEMPTS {
        let shown = launch_with_library(ctx, &designs)?;
        counts.push(shown);
        if shown >= wanted {
            break;
        }
        // Was a fixed `settle(500)` before the next attempt: the relaunch already waited 10 s
        // for the rows, so this only gives a late list one more chance (returns at once when
        // the rows arrive).
        let late = {
            let ui = ctx.app.ui();
            wait_until(Duration::from_millis(500), || {
                ui.global::<LibraryModel>().get_diagram_list().row_count() >= wanted
            })
        };
        if late {
            counts.push(wanted);
            break;
        }
    }
    let shown = counts.last().copied().unwrap_or(0);
    if shown < wanted {
        return Err(format!(
            "the fixture designs did not show up in the library: LibraryModel.diagram_list held \
             {counts:?} row(s) of {wanted} seeded after {LIBRARY_ATTEMPTS} attempt(s)"
        ));
    }
    let ui = ctx.app.ui();
    let model = ui.global::<LibraryModel>();
    let id = model
        .get_diagram_list()
        .row_data(0)
        .map(|row| row.id)
        .ok_or("the library list is empty")?;
    model.invoke_select_diagram(id);
    if !wait_until(Duration::from_secs(5), || {
        ui.global::<LibraryModel>().get_selected_entry_id() == id
    }) {
        return Err(format!("selecting library entry {id} did not take"));
    }
    // Was a fixed `settle(100)`: the detail of the selected entry follows on a worker; wait for
    // its title (100 ms at most, as before).
    let _ = wait_until(Duration::from_millis(100), || {
        !ui.global::<LibraryModel>()
            .get_current_detail()
            .title
            .is_empty()
    });
    Ok(())
}

/// The selected fixture design loaded into the editor.
fn editor(ctx: &mut Ctx<'_>) -> Result<(), String> {
    library(ctx)?;
    let ui = ctx.app.ui();
    ui.global::<EditorModel>().invoke_load_selected();
    if wait_until(Duration::from_secs(10), || {
        ui.global::<EditorModel>().get_has_design()
    }) {
        // Was a fixed `settle(150)`: the load starts a solve; wait until it is not running.
        let _ = wait_until(Duration::from_millis(150), || {
            !ui.global::<EditorModel>().get_solve_running()
        });
        Ok(())
    } else {
        Err("Load Selected did not install a design in the editor".to_owned())
    }
}

fn planner(ctx: &mut Ctx<'_>) -> Result<(), String> {
    library(ctx)?;
    ctx.app.ui().global::<RoughPlanModel>().invoke_open();
    if wait_until(Duration::from_secs(5), || {
        harness_support::rough_planner_window().is_some()
    }) {
        // Was a fixed `settle(150)`: the planner lists its materials once it is ready.
        let _ = wait_until(Duration::from_millis(150), || {
            harness_support::rough_planner_window()
                .is_some_and(|w| w.global::<RoughPlanModel>().get_materials().row_count() > 0)
        });
        Ok(())
    } else {
        Err("RoughPlanModel.open did not open the Rough Planner window".to_owned())
    }
}

/// How long a snapshot may take to be held: the background solve of a design that was not
/// solved yet (the corpus' slowest design takes about 6 s).
const SNAPSHOT_WAIT: Duration = Duration::from_secs(30);

/// Takes a snapshot of the editor's design, waits until the app holds it, then opens the snapshot
/// compare window (what "Compare Visually…" does) and waits until it is shown.
///
/// The first wait matters: a design whose solve is not cached yet (one Load Selected has just
/// installed, its own solve still running) is snapshotted only once a background solve lands,
/// often well after 300 ms. `open_snapshot` before that finds no snapshot and only toasts "No
/// snapshot taken yet", so the window never opens.
///
/// # Errors
///
/// A message naming the step that did not happen.
pub(super) fn snapshot_and_open_compare(ui: &MainWindow) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    editor.invoke_snapshot_design();
    if !wait_until_labeled("snapshot-held", SNAPSHOT_WAIT, || editor.get_has_snapshot()) {
        return Err(format!(
            "Snapshot Design did not hold a snapshot within {} s",
            SNAPSHOT_WAIT.as_secs()
        ));
    }
    ui.global::<CompareModel>().invoke_open_snapshot();
    if wait_until_labeled("compare-open", Duration::from_secs(10), || {
        harness_support::compare_window().is_some()
    }) {
        Ok(())
    } else {
        Err(format!(
            "CompareModel.open_snapshot did not open the compare window (snapshot held; toast: \
             {:?})",
            ui.get_toast_message()
        ))
    }
}

/// A snapshot of the loaded design compared with itself in the pop-out window.
fn compare(ctx: &mut Ctx<'_>) -> Result<(), String> {
    editor(ctx)?;
    snapshot_and_open_compare(ctx.app.ui())
}

fn help(ctx: &mut Ctx<'_>) -> Result<(), String> {
    startup(ctx)?;
    ctx.app
        .ui()
        .global::<HelpModel>()
        .invoke_open_context_help();
    if wait_until(Duration::from_secs(5), || {
        harness_support::help_window().is_some()
    }) {
        // Was a fixed `settle(100)`: the window is shown.
        let _ = wait_until(Duration::from_millis(100), || {
            harness_support::help_window().is_some_and(|w| w.window().is_visible())
        });
        Ok(())
    } else {
        Err("HelpModel.open_context_help did not open the Help window".to_owned())
    }
}
