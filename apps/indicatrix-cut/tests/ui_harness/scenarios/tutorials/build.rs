//! A library design turned into a "Build this design" lesson and finished.
//!
//! The design is selected in the library list (`LibraryModel.select_diagram`, what a click on
//! the row does), the lesson is started with `BuildDesignModel.start_selected` (the callback of
//! the detail header's button and the list's context menu), the app generates it on a worker
//! thread, and the lesson is then walked like any tutorial. At the end the built design must
//! match the original: the lesson's own last step is a `DesignRebuilt` goal (assert it passed
//! like every other step), and in addition the tier rows and the closed solid are compared with
//! the original read from the library.

use super::{
    BuildPlan, SALT_DESIGNS, drive,
    drive::{snap, wait_real},
    library_source::{
        LibrarySource, relaunch_with_database, shuffled, tier_rows_in_database, tier_rows_of_asc,
    },
    selection, verify_finished,
};
use crate::support::{Ctx, coverage, sample_designs};
use indicatrix_cut::{BuildDesignModel, EditorModel, LibraryModel, MainWindow};
use slint::{ComponentHandle, Model};
use std::{path::PathBuf, time::Duration};

/// How long preparing the library may take (a connected database can be large).
const LIBRARY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long generating the lesson may take.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the lesson may take to open after it was generated.
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a library row may take to show its detail after it was selected.
const SELECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the built design may take to report a closed solid after the last step.
const SOLVE_TIMEOUT: Duration = Duration::from_secs(20);
/// At most this many database rows are tried while looking for designs with an `.asc` file.
const MAX_SCAN: usize = 120;

/// The design chosen for a lesson.
struct Chosen {
    entry_id: i32,
    title: String,
    /// Tier rows of the original, when its `.asc` could be read.
    original_rows: Option<usize>,
}

/// Runs the lesson of `plan` (slot `slot`) end to end.
pub fn run(ctx: &mut Ctx<'_>, slot: usize, plan: &BuildPlan) {
    ctx.report.set_context(plan.name.to_owned());
    let Some(library) = prepare_library(ctx, plan) else {
        return;
    };
    let Some(chosen) = choose_design(ctx, slot, plan, library.copy_path()) else {
        return;
    };
    let scope = if plan.fixture_title.is_some() {
        plan.name.to_owned()
    } else {
        format!("{} [{}]", plan.name, chosen.title)
    };
    ctx.report.set_context(scope.clone());
    ctx.report.note(format!(
        "{scope}: library entry {} \"{}\"",
        chosen.entry_id, chosen.title
    ));
    ctx.check_health(&format!("{scope}: design selected"));
    let Some(guide_id) = start_lesson(ctx, &scope, &chosen) else {
        return;
    };
    let stats = drive::drive_guide(ctx, &scope, &guide_id, None);
    verify_finished(ctx, &scope, &guide_id, &stats);
    if !stats.aborted {
        compare_with_original(ctx, &scope, &chosen);
    }
}

/// The library the app was launched on for a lesson.
enum PreparedLibrary {
    /// The fixture library: no database file.
    Fixtures,
    /// The profile's copy of the connected database.
    Copy(PathBuf),
}

impl PreparedLibrary {
    /// The database copy's path; `None` for the fixture library.
    fn copy_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Fixtures => None,
            Self::Copy(path) => Some(path),
        }
    }
}

/// Launches the app on the library of `plan`'s source. Returns the library that was prepared
/// (with the path of the profile's copy of the connected database), or `None` when the library
/// could not be prepared (a finding was recorded).
fn prepare_library(ctx: &mut Ctx<'_>, plan: &BuildPlan) -> Option<PreparedLibrary> {
    match &selection().source {
        LibrarySource::Fixtures => ctx
            .relaunch_with_library(&sample_designs())
            .then_some(PreparedLibrary::Fixtures),
        LibrarySource::Database(original) => {
            // The original is only ever read by the file copy; the app and the harness use the copy.
            let copy = match relaunch_with_database(ctx, original) {
                Ok(copy) => copy,
                Err(error) => {
                    ctx.report.add_simple(
                        "MainWindow",
                        "build-library",
                        format!("{}: cannot connect the library: {error}", plan.name),
                    );
                    return None;
                }
            };
            let listed = wait_real(LIBRARY_TIMEOUT, || list_rows(ctx.app.ui()) > 0);
            if !listed {
                ctx.report.add_simple(
                    "MainWindow",
                    "build-library",
                    format!("{}: the connected library lists no design", plan.name),
                );
                return None;
            }
            Some(PreparedLibrary::Copy(copy))
        }
    }
}

fn list_rows(ui: &MainWindow) -> usize {
    ui.global::<LibraryModel>().get_diagram_list().row_count()
}

/// The library rows as (entry id, title).
fn list_items(ui: &MainWindow) -> Vec<(i32, String)> {
    let list = ui.global::<LibraryModel>().get_diagram_list();
    (0..list.row_count())
        .filter_map(|at| list.row_data(at))
        .map(|item| (item.id, item.title.to_string()))
        .collect()
}

/// Selects library entry `id` and waits for its detail. Returns whether it shows an `.asc` file.
fn select_and_check_asc(ctx: &Ctx<'_>, id: i32) -> bool {
    ctx.app
        .ui()
        .global::<LibraryModel>()
        .invoke_select_diagram(id);
    let selected = wait_real(SELECT_TIMEOUT, || {
        ctx.app
            .ui()
            .global::<LibraryModel>()
            .get_selected_entry_id()
            == id
    });
    if selected {
        coverage::record_callback("LibraryModel.select_diagram");
    }
    let files = ctx.app.ui().global::<LibraryModel>().get_current_files();
    (0..files.row_count())
        .filter_map(|at| files.row_data(at))
        .any(|file| file.name.to_ascii_lowercase().ends_with(".asc"))
}

/// Picks and selects the design of `plan`.
fn choose_design(
    ctx: &mut Ctx<'_>,
    slot: usize,
    plan: &BuildPlan,
    copy: Option<&std::path::Path>,
) -> Option<Chosen> {
    if let Some(title) = plan.fixture_title {
        return choose_fixture(ctx, plan, title);
    }
    let copy = copy?;
    let items = shuffled(list_items(ctx.app.ui()), selection().seed, SALT_DESIGNS);
    let mut eligible = 0;
    for (id, title) in items.into_iter().take(MAX_SCAN) {
        if !select_and_check_asc(ctx, id) {
            continue;
        }
        if eligible == slot {
            return Some(Chosen {
                entry_id: id,
                title,
                original_rows: tier_rows_in_database(copy, i64::from(id)),
            });
        }
        eligible += 1;
    }
    ctx.report.note(format!(
        "{}: the connected library has only {eligible} design(s) with an .asc file among the \
         first {MAX_SCAN} tried; nothing to build for this slot",
        plan.name
    ));
    None
}

/// The fixture design called `title`, found in the list and selected.
fn choose_fixture(ctx: &mut Ctx<'_>, plan: &BuildPlan, title: &str) -> Option<Chosen> {
    let designs = sample_designs();
    let fixture = designs.iter().find(|design| design.title == title)?;
    let wanted = title.to_lowercase();
    let items = list_items(ctx.app.ui());
    let found = items
        .iter()
        .find(|(_, listed)| listed.to_lowercase() == wanted)
        .or_else(|| {
            items
                .iter()
                .find(|(_, listed)| listed.to_lowercase().contains(&wanted))
        });
    let Some((id, listed)) = found else {
        ctx.report.add_simple(
            "MainWindow",
            "build-library",
            format!("{}: no library row is called \"{title}\"", plan.name),
        );
        return None;
    };
    if !select_and_check_asc(ctx, *id) {
        ctx.report.add_simple(
            "MainWindow",
            "build-library",
            format!(
                "{}: the selected design shows no .asc attachment",
                plan.name
            ),
        );
    }
    Some(Chosen {
        entry_id: *id,
        title: listed.clone(),
        original_rows: tier_rows_of_asc(fixture.file_name, &fixture.asc, None),
    })
}

/// Whether the main window shows an error toast.
fn error_toast(ui: &MainWindow) -> Option<String> {
    (ui.get_toast_visible() && ui.get_toast_type().as_str() == "error")
        .then(|| ui.get_toast_message().to_string())
}

/// Presses "Build this design" and waits for the lesson to open. Returns its guide id.
fn start_lesson(ctx: &mut Ctx<'_>, scope: &str, chosen: &Chosen) -> Option<String> {
    let selected = ctx
        .app
        .ui()
        .global::<LibraryModel>()
        .get_selected_entry_id();
    if selected != chosen.entry_id {
        ctx.report.add_simple(
            "MainWindow",
            "build-library",
            format!(
                "{scope}: the selected entry is {selected}, expected {}",
                chosen.entry_id
            ),
        );
        return None;
    }
    ctx.app
        .ui()
        .global::<BuildDesignModel>()
        .invoke_start_selected();
    let ui = ctx.app.ui();
    let generated = wait_real(GENERATE_TIMEOUT, || {
        !ui.global::<BuildDesignModel>().get_busy() || error_toast(ui).is_some()
    });
    let opened = generated
        && wait_real(OPEN_TIMEOUT, || {
            let s = snap(ui);
            (s.open && s.id.starts_with("build:")) || error_toast(ui).is_some()
        });
    let state = snap(ui);
    if opened && state.open && error_toast(ui).is_none() {
        coverage::record_callback("BuildDesignModel.start_selected");
    }
    if let Some(message) = error_toast(ui) {
        ctx.report.add_simple(
            "MainWindow",
            "build-lesson-error",
            format!(
                "{scope}: no lesson for \"{}\" (entry {}): {message}",
                chosen.title, chosen.entry_id
            ),
        );
        return None;
    }
    if !generated || !opened || !state.open {
        ctx.report.add_simple(
            "MainWindow",
            "build-lesson-not-started",
            format!(
                "{scope}: the lesson for \"{}\" did not open (generation finished: {generated}, \
                 guide open: {})",
                chosen.title, state.open
            ),
        );
        return None;
    }
    Some(state.id)
}

/// The built design against the original: same tier rows, a closed solid.
fn compare_with_original(ctx: &mut Ctx<'_>, scope: &str, chosen: &Chosen) {
    let ui = ctx.app.ui();
    let solved = wait_real(SOLVE_TIMEOUT, || {
        let editor = ui.global::<EditorModel>();
        editor.get_solve_state().as_str() == "solved" && !editor.get_status_is_problem()
    });
    let rows = ui.global::<EditorModel>().get_tiers().row_count();
    if !solved {
        ctx.report.add_simple(
            "MainWindow",
            "build-not-solved",
            format!("{scope}: the built design is not a solved, closed stone after the lesson"),
        );
    }
    match chosen.original_rows {
        Some(original) if original != rows => ctx.report.add_simple(
            "MainWindow",
            "build-tier-count",
            format!("{scope}: the built design has {rows} tier row(s), the original {original}"),
        ),
        Some(_) => {}
        None => ctx.report.note(format!(
            "{scope}: the original's tier count could not be read; compared the lesson's own \
             rebuilt goal only"
        )),
    }
    ctx.check_health(&format!("{scope}: built design compared"));
}
