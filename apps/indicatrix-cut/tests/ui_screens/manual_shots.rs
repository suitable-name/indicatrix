//! The shots of the editor chapters of the user manual: the Tier form, the concave tier form,
//! the History and Variants views, Cutting Mode, the Angle Sweep dialog and the Render Jobs
//! window. Each drives the real app through the model calls its controls make.

use crate::{
    kit::{Sc, card_rect, rect_of},
    nav::{CROWN_TIER, FVS},
    support::app::{pump, wait_until},
};
use indicatrix_cut::{
    CuttingModeModel, EditorModel, HistoryModel, RenderJobsModel, SweepModel, VariantsModel,
    harness_support,
};
use slint::{ComponentHandle, Model};
use slint_harness::screenshot::Shot;
use std::time::Duration;

/// How many tiers the History shot changes (each its own step).
const HISTORY_EDITS: usize = 3;
/// The names of the two variants the Variants shot saves.
const VARIANT_NAMES: [&str; 2] = ["Shorter Crown", "Steeper Pavilion"];
/// How long the angle sweep may take, in seconds.
const SWEEP_SECS: u64 = 300;

/// The inspector of the Edit view as a picture.
fn inspector_shot(sc: &Sc<'_, '_>) -> Result<Shot, String> {
    sc.expand_inspector();
    let shot = sc.snap_main()?;
    Ok(rect_of(&sc.view(), "EditorInspector")?.crop(&shot))
}

/// `tier_form.png`: the inspector's Tier tab with a crown tier selected.
///
/// # Errors
///
/// The design does not open, or has no crown tier.
pub fn tier_form(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_view_mode(0);
    sc.set_inspector_tab(0);
    sc.select_tier(CROWN_TIER)?;
    pump(500);
    inspector_shot(sc)
}

/// `concave_tier_form.png`: the Tier tab on the blank form that "+ Add Concave Tier" opens.
///
/// # Errors
///
/// The design does not open.
pub fn concave_tier_form(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_view_mode(0);
    let ui = sc.ui();
    ui.global::<EditorModel>().invoke_add_concave_tier();
    pump(800);
    inspector_shot(sc)
}

/// Nudges the angle of `HISTORY_EDITS` different crown or pavilion tiers, one after the other,
/// so the History tab lists one step for each (nudges of the same tier would merge into one).
fn make_history(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let editor = ui.global::<EditorModel>();
    let positions: Vec<i32> = editor
        .get_tiers()
        .iter()
        .enumerate()
        .filter(|(_, tier)| tier.name.starts_with('C') || tier.name.starts_with('P'))
        .filter_map(|(position, _)| i32::try_from(position).ok())
        .take(HISTORY_EDITS)
        .collect();
    if positions.len() < HISTORY_EDITS {
        return Err("the design has too few crown and pavilion tiers for a history".to_owned());
    }
    for position in positions {
        editor.invoke_nudge_angle(position, 0.5);
        pump(1200);
        let solved = Sc::wait(180, || {
            !editor.get_solve_running() && editor.get_solve_state().as_str() == "solved"
        });
        if !solved {
            return Err("the design did not solve after an angle change".to_owned());
        }
    }
    sc.ctx.app.clear_notice();
    Ok(())
}

/// `history_steps.png`: the History tab's Steps view after three angle changes.
///
/// # Errors
///
/// The design does not open or solve.
pub fn history_steps(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    make_history(sc)?;
    sc.set_inspector_tab(4);
    let ui = sc.ui();
    ui.global::<VariantsModel>().set_view(0);
    let history = ui.global::<HistoryModel>();
    let listed = Sc::wait(30, || history.get_rows().row_count() > HISTORY_EDITS);
    if !listed {
        sc.note("the History tab lists fewer steps than changes were made; photographed anyway");
    }
    // Each row's picture is drawn in the background; state 0 is "still being drawn".
    let drawn = Sc::wait(60, || {
        let version = history.get_thumb_version();
        history
            .get_rows()
            .iter()
            .all(|row| history.invoke_thumbnail(row.position, version).state != 0)
    });
    if !drawn {
        sc.note("not every History picture was drawn in 60 s; photographed anyway");
    }
    pump(1500);
    inspector_shot(sc)
}

/// `history_variants.png`: the History tab's Variants view with two saved variants.
///
/// # Errors
///
/// The design does not open, or a variant is not saved.
pub fn history_variants(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_inspector_tab(4);
    let ui = sc.ui();
    let variants = ui.global::<VariantsModel>();
    variants.set_view(1);
    pump(300);
    for (index, name) in VARIANT_NAMES.into_iter().enumerate() {
        variants.invoke_begin_save(-1);
        pump(150);
        variants.set_form_name(name.into());
        variants.invoke_submit_form();
        let wanted = index + 1;
        if !Sc::wait(20, || variants.get_rows().row_count() >= wanted) {
            return Err(format!("the variant {name:?} was not saved"));
        }
        pump(400);
        if index == 0 {
            // A change between the two saves, so the variants differ.
            make_one_change(sc)?;
            sc.set_inspector_tab(4);
        }
    }
    pump(1000);
    inspector_shot(sc)
}

/// One angle change on the first crown or pavilion tier, and the solve after it.
fn make_one_change(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let editor = ui.global::<EditorModel>();
    let position = editor
        .get_tiers()
        .iter()
        .position(|tier| tier.name.starts_with('P'))
        .and_then(|position| i32::try_from(position).ok())
        .ok_or_else(|| "the design has no pavilion tier".to_owned())?;
    editor.invoke_nudge_angle(position, 0.5);
    pump(1200);
    let solved = Sc::wait(180, || {
        !editor.get_solve_running() && editor.get_solve_state().as_str() == "solved"
    });
    if solved {
        Ok(())
    } else {
        Err("the design did not solve after an angle change".to_owned())
    }
}

/// `cutting_mode.png`: Cutting Mode on the third step of the design.
///
/// # Errors
///
/// The design does not open, or cutting mode has nothing to step through.
pub fn cutting_mode(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    let ui = sc.ui();
    let cutting = ui.global::<CuttingModeModel>();
    cutting.invoke_open_cutting_mode();
    let ready = Sc::wait(30, || {
        !cutting.get_busy() && (cutting.get_step_count() > 0 || !cutting.get_error().is_empty())
    });
    if !ready || !cutting.get_error().is_empty() {
        return Err(format!(
            "cutting mode did not open: {}",
            cutting.get_error()
        ));
    }
    for _ in 0..2 {
        cutting.invoke_next();
        pump(700);
    }
    pump(800);
    sc.snap_main()
}

/// `angle_sweep.png`: the Angle Sweep dialog after a sweep of the selected crown tier over its
/// default range.
///
/// # Errors
///
/// The design does not open, or the sweep does not finish with results.
pub fn angle_sweep(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.select_tier(CROWN_TIER)?;
    let ui = sc.ui();
    let sweep = ui.global::<SweepModel>();
    sweep.invoke_open_dialog();
    if !Sc::wait(30, || sweep.get_dialog_open()) {
        return Err("the Angle Sweep dialog did not open".to_owned());
    }
    pump(500);
    if !Sc::wait(30, || sweep.get_form_ok()) {
        return Err(format!(
            "the sweep form is not usable: {}",
            sweep.get_summary_text()
        ));
    }
    sweep.invoke_run();
    let done = Sc::wait(SWEEP_SECS, || {
        sweep.get_has_results() && !sweep.get_running()
    });
    if !done {
        return Err(format!("the sweep did not finish within {SWEEP_SECS} s"));
    }
    let usable = sweep
        .get_rows()
        .iter()
        .position(|row| row.is_valid && !row.is_current);
    if let Some(row) = usable.and_then(|row| i32::try_from(row).ok()) {
        sweep.invoke_select_row(row);
    }
    pump(1000);
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), "SweepDialog")?.crop(&shot))
}

/// `render_jobs.png`: the Render Jobs window with two queued pictures.
///
/// # Errors
///
/// The design does not open, or the jobs are not queued.
pub fn render_jobs(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    let ui = sc.ui();
    let jobs = ui.global::<RenderJobsModel>();
    // Queuing asks for the output folder: answer with a folder of the profile's working
    // directory (it is not shown).
    if let Ok(dir) = std::env::current_dir().map(|d| d.join("screens_queue")) {
        let _ = std::fs::create_dir_all(&dir);
        harness_support::queue_picker_path(dir.clone());
        harness_support::queue_picker_path(dir);
    }
    jobs.invoke_add_still_jobs(1920, 1080, 64, 0, 0, 2);
    let first = wait_until(Duration::from_secs(30), || jobs.get_rows().row_count() >= 1);
    jobs.invoke_add_still_jobs(1280, 720, 32, 0, 0, 2);
    let second = wait_until(Duration::from_secs(30), || jobs.get_rows().row_count() >= 2);
    harness_support::clear_picker_answers();
    if !(first && second) {
        return Err("the render jobs were not queued".to_owned());
    }
    jobs.set_panel_open(true);
    pump(1000);
    sc.ctx.app.clear_notice();
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), "RenderJobsPanel")?.crop(&shot))
}
