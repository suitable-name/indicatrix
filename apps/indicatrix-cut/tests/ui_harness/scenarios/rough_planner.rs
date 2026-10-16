//! Scenarios: the Rough Planner window, against the sample library (four designs).
//!
//! The planner is driven through its own `RoughPlanModel` (the window's instance, reached with
//! `harness_support::rough_planner_window()`): typed fields, `plan`, `begin_save`, `open_saved`,
//! `export_saved`, `import_saved` -- the callbacks its buttons call -- and the results are read
//! back from the same model. File pickers get their answers queued first.
//!
//! The export/import suffixes are the app's own (`gui::rough_plan::saved::naming`), re-exported
//! by `harness_support`.
//! Saving under a name a saved plan already has asks "Replace Saved Plan?"
//! (`confirm_overwrite_open`); `planner_save_and_reopen` answers Cancel and then Replace. The other
//! replace question is "Plan again over results that were not saved" (`confirm_replace_open`).

use super::{Piece, Scenario, dialogs, existing_install};
use crate::support::{
    Ctx, HarnessApp, Profile, SecondaryWindow,
    app::{pump, wait_until},
    coverage, sample_designs, seed_library,
};
use indicatrix_cut::{
    EditorModel, LibraryModel, MainWindow, RoughPlanModel, RoughPlannerWindow, harness_support,
};
use slint::{ComponentHandle, Model};
use slint_harness::backend::wait_until_labeled;
use std::{fs, time::Duration};

use harness_support::{
    ROUGH_PLAN_EXPORT_SUFFIX as EXPORT_SUFFIX,
    ROUGH_PLAN_LEGACY_EXPORT_SUFFIX as LEGACY_EXPORT_SUFFIX,
};
/// The first line of every plan file's header.
const PLAN_FORMAT_LINE: &str = "format = \"indicatrix-rough-plan\"";

/// How many pieces the planner click-everything pass (207 s in one piece, mostly relaunches with a
/// plan) is cut into: each piece presses the controls whose key hashes into its part
/// (`support::part`).
const CLICK_PARTS: usize = 3;

/// The pieces of this file.
pub fn pieces() -> Vec<Piece> {
    let mut all: Vec<Piece> = vec![
        Scenario {
            name: "planner_empty_state",
            seed: existing_install,
            run: planner_empty_state,
        }
        .into(),
        Scenario {
            name: "planner_block_and_plan",
            seed: existing_install,
            run: planner_block_and_plan,
        }
        .into(),
        Scenario {
            name: "planner_save_and_reopen",
            seed: existing_install,
            run: planner_save_and_reopen,
        }
        .into(),
        Scenario {
            name: "planner_export_import_isrd",
            seed: existing_install,
            run: planner_export_import_isrd,
        }
        .into(),
        Scenario {
            name: "planner_open_in_editor",
            seed: existing_install,
            run: planner_open_in_editor,
        }
        .into(),
    ];
    for index in 0..CLICK_PARTS {
        all.push(
            Piece::new(
                format!("planner_click_everything:{}of{CLICK_PARTS}", index + 1),
                existing_install,
                planner_click_everything,
            )
            .part(index, CLICK_PARTS),
        );
    }
    all
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Opens the planner from the main window's menu route and waits until it is showing with its
/// material list; `None` when it did not open in time.
fn open_planner_raw(ui: &MainWindow) -> Option<RoughPlannerWindow> {
    ui.global::<RoughPlanModel>().invoke_open();
    let ready = wait_until(Duration::from_secs(10), || {
        harness_support::is_window_open(SecondaryWindow::RoughPlanner)
            && harness_support::rough_planner_window()
                .is_some_and(|w| w.global::<RoughPlanModel>().get_materials().row_count() > 0)
    });
    if ready {
        harness_support::rough_planner_window()
    } else {
        None
    }
}

/// [`open_planner_raw`] with a finding when it fails.
fn open_planner(ctx: &mut Ctx<'_>) -> Option<RoughPlannerWindow> {
    let ui = ctx.app.ui().clone_strong();
    let window = open_planner_raw(&ui);
    ctx.expect(
        window.is_some(),
        "planner-opens",
        "RoughPlanModel.open did not show the Rough Planner with its materials within 10 s",
    );
    window
}

/// Types a 40 x 30 x 25 mm block, asks all of the library, plans, and waits (real time, up to
/// 60 s) for the layouts, each of at most `max_designs` different designs (the stones are
/// not capped). `Err` carries what the window said.
fn plan_block_raw(window: &RoughPlannerWindow, max_designs: i32) -> Result<(), String> {
    let model = window.global::<RoughPlanModel>();
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.set_max_designs(max_designs);
    model.set_use_filter(false);
    model.invoke_base_field_committed();
    pump(250);
    model.invoke_plan();
    wait_until(Duration::from_secs(60), || {
        !model.get_running()
            && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
    });
    if model.get_running() {
        return Err(format!("still running after 60 s: {}", model.get_stage()));
    }
    let error = model.get_error_text();
    if !error.is_empty() {
        return Err(error.to_string());
    }
    if model.get_results().row_count() == 0 {
        return Err(format!("no layout: {}", model.get_summary()));
    }
    Ok(())
}

/// [`plan_block_raw`] with a finding when it fails.
fn plan_block(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) -> bool {
    match plan_block_raw(window, 3) {
        Ok(()) => true,
        Err(message) => {
            ctx.expect(
                false,
                "plan-runs",
                format!("planning a block failed: {message}"),
            );
            false
        }
    }
}

/// Stones in the best layout.
fn first_stone_count(window: &RoughPlannerWindow) -> i32 {
    window
        .global::<RoughPlanModel>()
        .get_results()
        .row_data(0)
        .map_or(0, |layout| layout.stone_count)
}

/// Saves every result under the suggested name and waits until the saved list grows.
fn save_all(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) -> bool {
    let model = window.global::<RoughPlanModel>();
    let before = model.get_saved_plans().row_count();
    model.invoke_begin_save(false);
    pump(150);
    let named = model.get_save_name_open() && !model.get_save_name().is_empty();
    ctx.expect(
        named,
        "save-name-row",
        "Save All did not open the name row with a suggested name",
    );
    if !named {
        return false;
    }
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "save name row");
    model.invoke_confirm_save();
    // The suggested name may already be taken by a plan an earlier scenario saved in this profile
    // ("Replace Saved Plan?" then opens, after a name lookup on a worker): answer Replace, which
    // leaves the list the same length.
    let _ = wait_until(Duration::from_secs(15), || {
        model.get_saved_plans().row_count() > before || model.get_confirm_overwrite_open()
    });
    let replaced = model.get_confirm_overwrite_open();
    if replaced {
        model.invoke_confirm_overwrite(true);
        coverage::record_callback("RoughPlanModel.confirm_overwrite");
    }
    let listed = ctx.wait_for("the saved plan is listed", Duration::from_secs(15), || {
        model.get_saved_plans().row_count() > before
            || (replaced
                && !model.get_confirm_overwrite_open()
                && !model.get_save_name_open()
                && model.get_saved_plans().row_count() > 0)
    });
    if listed {
        coverage::record_callback("RoughPlanModel.confirm_save");
    } else {
        ctx.report.note(format!(
            "planner save: not listed; error line {:?}, name row open {}, replace question open {}",
            model.get_error_text().to_string(),
            model.get_save_name_open(),
            model.get_confirm_overwrite_open()
        ));
    }
    listed
}

/// The id of the first saved plan.
fn first_saved_id(window: &RoughPlannerWindow) -> Option<i32> {
    window
        .global::<RoughPlanModel>()
        .get_saved_plans()
        .row_data(0)
        .map(|row| row.id)
}

/// Closes the app, writes the sample library into the profile, starts again and waits for the
/// designs: what `Ctx::relaunch_with_library` does, for a `prepare` step that has no `Ctx`.
fn reload_with_library(app: &mut HarnessApp, profile: &Profile) -> bool {
    app.shutdown();
    if seed_library(profile, &sample_designs()).is_err() {
        return false;
    }
    match HarnessApp::launch() {
        Ok(fresh) => *app = fresh,
        Err(_) => return false,
    }
    let ui = app.ui().clone_strong();
    let wanted = sample_designs().len();
    wait_until_labeled("library-load", Duration::from_secs(10), || {
        ui.global::<LibraryModel>().get_diagram_list().row_count() >= wanted
    })
}

// ---------------------------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------------------------

/// The planner opens empty: no layouts, no typed size, not running, no error; sweeps clean at its
/// default and minimum size.
fn planner_empty_state(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    ctx.expect(
        model.get_results().row_count() == 0
            && model.get_rough_x().is_empty()
            && !model.get_running()
            && model.get_error_text().is_empty(),
        "planner-empty-state",
        "a fresh planner shows layouts, a size, a running plan or an error",
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "empty state");
    ctx.check_health("planner empty state");
}

/// A block rough: shape cuts can be added and removed, then Plan over the whole sample library
/// finishes with at least one layout holding at least one placed stone and a summary line; the
/// result state sweeps clean.
fn planner_block_and_plan(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.invoke_base_field_committed();
    pump(250);
    model.invoke_add_cut(0);
    pump(250);
    ctx.expect(
        model.get_cuts().row_count() == 1,
        "planner-add-cut",
        format!(
            "Add edge cut left {} cut row(s), expected 1",
            model.get_cuts().row_count()
        ),
    );
    model.invoke_remove_cut(0);
    pump(250);
    ctx.expect(
        model.get_cuts().row_count() == 0,
        "planner-remove-cut",
        "Remove did not take the cut away",
    );
    if !plan_block(ctx, &window) {
        return;
    }
    coverage::record_callback("RoughPlanModel.plan");
    ctx.expect(
        first_stone_count(&window) >= 1,
        "planner-stones",
        "the best layout holds no placed stone",
    );
    ctx.expect(
        !model.get_summary().is_empty(),
        "planner-summary",
        "the finished plan shows no summary line",
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "plan result");
    ctx.check_health("planner block and plan");
}

/// Plan again over results that were not saved asks to replace them (Keep leaves them
/// untouched); Save All lists the plan; a second save under the same name asks to replace it (Cancel keeps the row, Replace leaves one plan);
/// opening the saved plan brings back the same layouts and stone count.
fn planner_save_and_reopen(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_block(ctx, &window) {
        return;
    }
    let model = window.global::<RoughPlanModel>();
    let rows = model.get_results().row_count();
    let stones = first_stone_count(&window);
    replace_prompt(ctx, &window, rows);
    if !save_all(ctx, &window) {
        return;
    }
    if !overwrite_question(ctx, &window) {
        return;
    }
    let Some(id) = first_saved_id(&window) else {
        return;
    };
    model.invoke_open_saved(id);
    let opened = ctx.wait_for("the saved plan opens", Duration::from_secs(15), || {
        !model.get_loaded_banner().is_empty()
    });
    if opened {
        coverage::record_callback("RoughPlanModel.open_saved");
    }
    ctx.expect(
        model.get_results().row_count() == rows && first_stone_count(&window) == stones,
        "reopen-same-plan",
        format!(
            "reopened plan shows {} layout(s) / {} stone(s), saved {rows} / {stones}",
            model.get_results().row_count(),
            first_stone_count(&window)
        ),
    );
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "saved plan reopened");
    ctx.check_health("planner save and reopen");
}

/// Saving under a name a saved plan already has asks "Replace Saved Plan?". Cancel keeps the
/// name row open and the saved list as it was; saving again and answering Replace leaves exactly
/// ONE saved plan with that name. Returns whether the flow could be driven to its end.
fn overwrite_question(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow) -> bool {
    let model = window.global::<RoughPlanModel>();
    let saved = model.get_saved_plans();
    let Some(taken) = saved.row_data(0).map(|row| row.name.to_string()) else {
        ctx.expect(false, "overwrite-setup", "no saved plan to save over");
        return false;
    };
    let before = saved.row_count();
    model.invoke_begin_save(false);
    pump(150);
    model.set_save_name(taken.as_str().into());
    model.invoke_confirm_save();
    // The name lookup runs on a worker thread (real time); the question follows its answer.
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_confirm_overwrite_open()
    });
    let asked = model.get_confirm_overwrite_open();
    ctx.expect(
        asked && model.get_confirm_overwrite_name().as_str() == taken,
        "overwrite-question",
        format!("saving under the taken name {taken:?} did not ask to replace it"),
    );
    if !asked {
        return false;
    }
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "replace saved plan question");
    // Cancel: the question closes, the name row stays open, nothing was saved.
    model.invoke_confirm_overwrite(false);
    pump(300);
    ctx.expect(
        !model.get_confirm_overwrite_open()
            && model.get_save_name_open()
            && model.get_saved_plans().row_count() == before,
        "overwrite-cancel",
        "Cancel on the replace question did not keep the name row open with the list unchanged",
    );
    // Save again under the same name and Replace it.
    model.invoke_confirm_save();
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_confirm_overwrite_open()
    });
    if !ctx.expect(
        model.get_confirm_overwrite_open(),
        "overwrite-question-again",
        "the second save under the taken name did not ask again",
    ) {
        return false;
    }
    model.invoke_confirm_overwrite(true);
    let closed = ctx.wait_for("the replace is done", Duration::from_secs(15), || {
        !model.get_confirm_overwrite_open() && !model.get_save_name_open()
    });
    if closed {
        coverage::record_callback("RoughPlanModel.confirm_overwrite");
    }
    pump(500);
    let same_name = (0..model.get_saved_plans().row_count())
        .filter_map(|at| model.get_saved_plans().row_data(at))
        .filter(|row| row.name.as_str() == taken)
        .count();
    ctx.expect(
        same_name == 1 && model.get_saved_plans().row_count() == before,
        "overwrite-one-plan",
        format!(
            "after Replace {same_name} saved plan(s) are named {taken:?} and the list has {}              (was {before})",
            model.get_saved_plans().row_count()
        ),
    );
    ctx.expect(
        model.get_error_text().is_empty(),
        "save-same-name",
        format!(
            "replacing the saved plan raised: {}",
            model.get_error_text()
        ),
    );
    true
}

/// Plan over unsaved results asks "replace?"; Keep answers no and leaves the results as they
/// were.
fn replace_prompt(ctx: &mut Ctx<'_>, window: &RoughPlannerWindow, rows: usize) {
    let model = window.global::<RoughPlanModel>();
    model.invoke_plan();
    pump(250);
    let asked = model.get_confirm_replace_open() && model.get_confirm_replace_count() > 0;
    ctx.expect(
        asked,
        "replace-prompt",
        "Plan over results that were not saved did not ask whether to replace them",
    );
    if !asked {
        return;
    }
    ctx.sweep_secondary(SecondaryWindow::RoughPlanner, "replace question");
    model.invoke_confirm_replace(false);
    pump(250);
    ctx.expect(
        !model.get_confirm_replace_open()
            && !model.get_running()
            && model.get_results().row_count() == rows,
        "replace-keep",
        "Keep on the replace question did not leave the results alone",
    );
}

/// Export writes an `.isrd` file with the plan header; importing it adds a plan that opens with
/// the same stone count; a copy under the legacy suffix imports too.
fn planner_export_import_isrd(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_block(ctx, &window) || !save_all(ctx, &window) {
        return;
    }
    let model = window.global::<RoughPlanModel>();
    let stones = first_stone_count(&window);
    let Some(id) = first_saved_id(&window) else {
        return;
    };
    let path = ctx.scratch_file(&format!("p{EXPORT_SUFFIX}"));
    harness_support::queue_picker_path(&path);
    model.invoke_export_saved(id);
    let written = ctx.wait_for("the plan file is written", Duration::from_secs(15), || {
        path.exists()
    });
    if !written {
        return;
    }
    let text = fs::read_to_string(&path).unwrap_or_default();
    ctx.expect(
        text.lines()
            .take(8)
            .any(|line| line.trim() == PLAN_FORMAT_LINE),
        "export-header",
        format!("the exported file has no {PLAN_FORMAT_LINE} header line"),
    );
    coverage::record_callback("RoughPlanModel.export_saved");
    // Counted from what is stored now: another scenario's plan may already be in the profile.
    let stored = model.get_saved_plans().row_count();
    import_and_check(
        ctx,
        &window,
        &path,
        stored + 1,
        stones,
        "import of the .isrd file",
    );
    let legacy = ctx.scratch_file(&format!("p{LEGACY_EXPORT_SUFFIX}"));
    if fs::write(&legacy, &text).is_ok() {
        import_and_check(
            ctx,
            &window,
            &legacy,
            stored + 2,
            stones,
            "import of the legacy file",
        );
    }
    ctx.expect(
        harness_support::pending_picker_answers() == 0,
        "picker-queue-drained",
        "a queued picker answer was never consumed",
    );
    ctx.check_health("planner export and import");
}

/// Queues `path`, imports it, expects `plans` stored plans and the same stone count on screen.
fn import_and_check(
    ctx: &mut Ctx<'_>,
    window: &RoughPlannerWindow,
    path: &std::path::Path,
    plans: usize,
    stones: i32,
    what: &str,
) {
    let model = window.global::<RoughPlanModel>();
    harness_support::queue_picker_path(path);
    model.invoke_import_saved();
    let listed = ctx.wait_for(what, Duration::from_secs(15), || {
        model.get_saved_plans().row_count() >= plans
    });
    if !listed {
        return;
    }
    coverage::record_callback("RoughPlanModel.import_saved");
    pump(1500);
    ctx.expect(
        first_stone_count(window) == stones,
        "import-same-plan",
        format!(
            "{what}: the plan on screen has {} stone(s) in its best layout, expected {stones}; error: {}",
            first_stone_count(window),
            model.get_error_text()
        ),
    );
}

/// "Open in Editor" on a result hands a piece of rough to the main window's editor: it ends up
/// with a design (tier count above zero).
fn planner_open_in_editor(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    let Some(window) = open_planner(ctx) else {
        return;
    };
    if !plan_block(ctx, &window) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    ctx.expect(
        dialogs::tier_count(&ui) == 0,
        "editor-starts-empty",
        "the editor already holds a design before Open in Editor",
    );
    window
        .global::<RoughPlanModel>()
        .invoke_open_in_editor(0, 0);
    let opened = ctx.wait_for(
        "the editor receives the design",
        Duration::from_secs(20),
        || dialogs::tier_count(&ui) > 0,
    );
    if opened {
        coverage::record_callback("RoughPlanModel.open_in_editor");
        ctx.expect(
            ui.global::<EditorModel>().get_has_design(),
            "editor-has-design",
            "tiers arrived but the editor reports no design",
        );
    }
    ctx.check_health("planner open in editor");
}

/// Click-everything over the planner in its result state (block rough, a finished plan). The
/// engine denies delete, remove and replace controls.
fn planner_click_everything(ctx: &mut Ctx<'_>) {
    let profile: &Profile = ctx.profile;
    let mut prepare = |app: &mut HarnessApp| {
        if !reload_with_library(app, profile) {
            return;
        }
        let ui = app.ui().clone_strong();
        if let Some(window) = open_planner_raw(&ui) {
            let _ = plan_block_raw(&window, 2);
        }
    };
    // `reload_with_library` shuts down, seeds and launches by itself.
    let _stats = ctx.smoke_secondary_launching(
        SecondaryWindow::RoughPlanner,
        "block and plan result",
        &mut prepare,
    );
    ctx.check_health("planner click-everything");
}
