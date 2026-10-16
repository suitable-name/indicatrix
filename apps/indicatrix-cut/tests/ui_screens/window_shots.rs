//! The shots of the secondary windows: the Compare window opened from Optimize and the Rough
//! Planner before and after a plan.

use crate::{
    editor_shots::{busy_status, pick_best_candidate, run_optimize},
    kit::{Sc, click_first, size_window},
    support::{SecondaryWindow, WindowView, app::pump},
};
use indicatrix_cut::{
    CompareModel, CompareWindow, LibraryModel, RoughPlanModel, RoughPlannerWindow, harness_support,
};
use slint::{ComponentHandle, Model};
use slint_harness::screenshot::Shot;

/// The Compare window's preferred size, logical pixels.
const COMPARE_SIZE: (u32, u32) = (1100, 780);
/// The Rough Planner's preferred size, logical pixels.
const PLANNER_SIZE: (u32, u32) = (1280, 820);
/// How long the tilt average may take, in seconds.
const TILT_SECS: u64 = 600;
/// How long a plan may take, in seconds.
const PLAN_SECS: u64 = 900;
/// The edges of the five cuts (indices into the planner's 12 edge names).
const CUT_EDGES: [i32; 5] = [1, 3, 5, 7, 9];
/// The library shape filter the planner's Current Filter candidates come from.
const PLANNER_SHAPE: &str = "oval";

/// Runs Optimize, picks the best candidate and opens the Compare window from the tab.
fn open_compare(sc: &mut Sc<'_, '_>) -> Result<CompareWindow, String> {
    run_optimize(sc)?;
    pick_best_candidate(sc);
    let ui = sc.ui();
    ui.global::<CompareModel>().invoke_open_optimize();
    let opened = Sc::wait(60, || {
        harness_support::is_window_open(SecondaryWindow::Compare)
    });
    let window = harness_support::compare_window()
        .filter(|_| opened)
        .ok_or_else(|| "the Compare window did not open from the Optimize tab".to_owned())?;
    size_window(&window, COMPARE_SIZE);
    Ok(window)
}

/// Waits until the window shows a finished frame; a timeout is a note.
fn settle_compare(sc: &mut Sc<'_, '_>, window: &CompareWindow) {
    let model = window.global::<CompareModel>();
    let seconds = sc.cfg.render_timeout.as_secs();
    let settled = Sc::wait(seconds, || {
        model.get_after_image().size().width > 0 && !busy_status(model.get_status().as_str())
    });
    if !settled {
        sc.note(format!(
            "the Compare window did not finish within {seconds} s; photographed anyway"
        ));
    }
    pump(800);
}

/// Presses the pill called `label` in the window.
fn press_pill(window: &CompareWindow, label: &str) -> Result<(), String> {
    let view = WindowView::of("CompareWindow", window);
    if click_first(&view, label) {
        pump(400);
        Ok(())
    } else {
        Err(format!("the Compare window has no {label:?} button"))
    }
}

/// `optimizer__compare_1.png`: the window against the current design, Overlay and Solid.
///
/// # Errors
///
/// Optimize does not deliver or the window does not open.
pub fn compare_1(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let window = open_compare(sc)?;
    press_pill(&window, "Overlay")?;
    press_pill(&window, "Solid")?;
    settle_compare(sc, &window);
    sc.snap_window("CompareWindow", &window)
}

/// `optimizer__compare_2.png`: Side by Side and Path Traced after the Tilt Average.
///
/// # Errors
///
/// As [`compare_1`].
pub fn compare_2(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let window = open_compare(sc)?;
    press_pill(&window, "Side by Side")?;
    press_pill(&window, "Path Traced")?;
    settle_compare(sc, &window);
    let model = window.global::<CompareModel>();
    if model.get_tilt_available() {
        press_pill(&window, "Tilt Average")?;
        let done = Sc::wait(TILT_SECS, || model.get_tilt_done());
        if !done {
            sc.note(format!(
                "the Tilt Average did not finish within {TILT_SECS} s; photographed anyway"
            ));
        }
        pump(800);
    } else {
        sc.note("the Compare window offers no Tilt Average for this pair");
    }
    sc.snap_window("CompareWindow", &window)
}

/// Narrows the library to one shape, as the Shape drop-down in the header does.
fn filter_library_shape(sc: &Sc<'_, '_>, shape: &str) -> Result<(), String> {
    let ui = sc.ui();
    let library = ui.global::<LibraryModel>();
    let options: Vec<String> = library
        .get_shape_options()
        .iter()
        .map(String::from)
        .collect();
    let index = options
        .iter()
        .position(|option| option.eq_ignore_ascii_case(shape))
        .or_else(|| {
            options
                .iter()
                .position(|option| option.to_lowercase().contains(shape))
        })
        .ok_or_else(|| format!("the library offers no {shape} shape among {options:?}"))?;
    let before = library.get_diagram_list().row_count();
    library.set_selected_shape_index(i32::try_from(index).unwrap_or(0));
    library.invoke_filter_shape_changed(options[index].as_str().into());
    let _ = Sc::wait(30, || library.get_diagram_list().row_count() != before);
    pump(1500);
    Ok(())
}

/// Filters the library to oval designs, opens the Rough Planner and fills in the sapphire
/// block with five edge cuts.
fn planner_with_block(sc: &Sc<'_, '_>) -> Result<RoughPlannerWindow, String> {
    filter_library_shape(sc, PLANNER_SHAPE)?;
    let ui = sc.ui();
    ui.global::<RoughPlanModel>().invoke_open();
    let ready = Sc::wait(30, || {
        harness_support::rough_planner_window()
            .is_some_and(|w| w.global::<RoughPlanModel>().get_materials().row_count() > 0)
    });
    let window = harness_support::rough_planner_window()
        .filter(|_| ready)
        .ok_or_else(|| "the Rough Planner did not open".to_owned())?;
    size_window(&window, PLANNER_SIZE);
    let model = window.global::<RoughPlanModel>();
    let names: Vec<String> = model.get_materials().iter().map(String::from).collect();
    let wanted = |text: &str| names.iter().position(|n| n.to_lowercase().contains(text));
    let index = wanted("sapphire")
        .or_else(|| wanted("corundum"))
        .ok_or_else(|| format!("the planner lists no sapphire among {names:?}"))?;
    model.set_material_index(i32::try_from(index).unwrap_or(0));
    model.invoke_set_base(0);
    model.set_rough_x("14".into());
    model.set_rough_y("38".into());
    model.set_rough_z("9".into());
    model.invoke_base_field_committed();
    pump(500);
    for (row, edge) in CUT_EDGES.into_iter().enumerate() {
        model.invoke_add_cut(0);
        pump(400);
        model.invoke_cut_faces_changed(i32::try_from(row).unwrap_or(0), edge);
        pump(400);
    }
    pump(1500);
    if model.get_cuts().row_count() != CUT_EDGES.len() {
        return Err(format!(
            "the planner holds {} cut(s), expected {}: {}",
            model.get_cuts().row_count(),
            CUT_EDGES.len(),
            model.get_model_error()
        ));
    }
    if !model.get_model_error().is_empty() {
        return Err(format!(
            "the rough is not valid: {}",
            model.get_model_error()
        ));
    }
    Ok(window)
}

/// `rough_planner.png`: the planner before planning.
///
/// # Errors
///
/// The planner does not open or rejects the rough.
pub fn rough_planner(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let window = planner_with_block(sc)?;
    let model = window.global::<RoughPlanModel>();
    let _ = Sc::wait(60, || {
        !model.get_view_busy() && model.get_filter_count() > 0 && model.get_library_count() > 0
    });
    pump(2000);
    sc.snap_window("RoughPlannerWindow", &window)
}

/// `rough_planner_result.png`: the planner after Plan (at most two designs per layout; the
/// stones are not capped) with the best layout selected.
///
/// # Errors
///
/// As [`rough_planner`], or the plan fails.
pub fn rough_planner_result(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let window = planner_with_block(sc)?;
    let model = window.global::<RoughPlanModel>();
    model.set_max_designs(2);
    pump(300);
    model.invoke_plan();
    pump(500);
    let done = Sc::wait(PLAN_SECS, || {
        !model.get_running()
            && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
    });
    if !done {
        return Err(format!(
            "the plan did not finish within {PLAN_SECS} s: {}",
            model.get_stage()
        ));
    }
    if !model.get_error_text().is_empty() {
        return Err(format!("the plan failed: {}", model.get_error_text()));
    }
    model.invoke_select_result(0);
    pump(500);
    let _ = Sc::wait(120, || {
        !model.get_thumbnails_pending() && !model.get_view_busy()
    });
    pump(1500);
    sc.snap_window("RoughPlannerWindow", &window)
}
